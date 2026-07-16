use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

fn marker_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("spawned");
    let adapter = dir.path().join("marker-agent");
    fs::write(
        &adapter,
        format!("#!/bin/sh\ntouch {:?}\nexec /bin/cat\n", marker),
    )
    .unwrap();
    let mut permissions = fs::metadata(&adapter).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&adapter, permissions).unwrap();

    let config = dir.path().join("a2a-bridge.toml");
    fs::write(
        &config,
        format!(
            "default = \"marker\"\n\n\
             [registry]\nallowed_cmds = [{adapter:?}]\n\n\
             [[agents]]\nid = \"marker\"\ncmd = {adapter:?}\n\n\
             [server]\n"
        ),
    )
    .unwrap();
    (dir, marker, config)
}

fn smoke_command(config: &PathBuf) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"));
    command
        .arg("smoke")
        .arg("--agent")
        .arg("marker")
        .arg("--config")
        .arg(config);
    command
}

#[test]
fn missing_billable_acknowledgement_refuses_before_agent_spawn() {
    let (_dir, marker, config) = marker_fixture();

    let output = smoke_command(&config)
        .output()
        .expect("run smoke without acknowledgement");

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--acknowledge-billable"),
        "stderr must explain the explicit billing barrier: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "argument refusal must not emit an attempt artifact"
    );
    assert!(
        !marker.exists(),
        "missing acknowledgement must refuse before spawning the configured agent"
    );
}

#[test]
fn malformed_options_refuse_before_agent_spawn_or_artifact() {
    for bad in [
        ["--timeout-secs", "0"],
        ["--timeout-secs", "901"],
        ["--effort", "turbo"],
        ["--mode", " padded "],
        ["--out", "-"],
        ["--unknown", "value"],
        ["--agent", "duplicate"],
    ] {
        let (_dir, marker, config) = marker_fixture();
        let output = smoke_command(&config)
            .arg("--acknowledge-billable")
            .args(bad)
            .output()
            .unwrap();
        assert!(!output.status.success(), "accepted malformed args {bad:?}");
        assert!(
            output.stdout.is_empty(),
            "malformed args emitted an artifact"
        );
        assert!(!marker.exists(), "malformed args {bad:?} spawned an agent");
    }
}

#[test]
fn blocked_model_refuses_before_agent_spawn() {
    let (_dir, marker, config) = marker_fixture();
    let output = smoke_command(&config)
        .arg("--acknowledge-billable")
        .arg("--model")
        .arg("claude-fable-5[1m]")
        .env_remove("A2A_BRIDGE_ALLOW_FABLE")
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("blocked model"));
    assert!(!marker.exists());
}

#[test]
fn invalid_session_cwd_is_an_artifact_failure_before_agent_spawn() {
    let (dir, marker, config) = marker_fixture();
    let output = smoke_command(&config)
        .arg("--acknowledge-billable")
        .arg("--session-cwd")
        .arg(dir.path().join("missing-repo"))
        .output()
        .unwrap();

    assert!(!output.status.success());
    let artifact: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        artifact["diagnostics"]["failure"]["code"],
        "smoke.session_cwd"
    );
    assert!(!marker.exists());
}

#[test]
fn acknowledged_pre_spawn_failure_emits_artifact_before_nonzero_exit() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.toml");
    let output = smoke_command(&missing)
        .arg("--acknowledge-billable")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let artifact: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("failure must emit machine-readable JSON before returning nonzero");
    assert_eq!(artifact["schema_version"], 2);
    assert_eq!(artifact["success"], false);
    assert_eq!(
        artifact["diagnostics"]["failure"]["code"],
        "smoke.config_path"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("inspect the emitted artifact"),
        "stderr should carry only the human failure direction"
    );
}

#[test]
fn explicit_out_gets_failure_artifact_and_stdout_stays_empty() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.toml");
    let artifact_path = dir.path().join("artifact.json");
    let output = smoke_command(&missing)
        .arg("--acknowledge-billable")
        .arg("--out")
        .arg(&artifact_path)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let artifact: serde_json::Value =
        serde_json::from_slice(&fs::read(&artifact_path).unwrap()).unwrap();
    assert_eq!(artifact["schema_version"], 2);
    assert_eq!(artifact["success"], false);
    assert_eq!(
        fs::metadata(artifact_path).unwrap().permissions().mode() & 0o777,
        0o600,
        "a newly created artifact must be owner-only"
    );
}

#[test]
fn existing_out_or_link_refuses_before_agent_spawn_or_mutation() {
    let (_dir, marker, config) = marker_fixture();
    let artifact_path = config.with_file_name("existing-artifact.json");
    fs::write(&artifact_path, b"stale artifact").unwrap();
    let mut permissions = fs::metadata(&artifact_path).unwrap().permissions();
    permissions.set_mode(0o644);
    fs::set_permissions(&artifact_path, permissions).unwrap();

    let output = smoke_command(&config)
        .arg("--acknowledge-billable")
        .arg("--out")
        .arg(&artifact_path)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!marker.exists());
    assert_eq!(fs::read(&artifact_path).unwrap(), b"stale artifact");
    assert_eq!(
        fs::metadata(&artifact_path).unwrap().permissions().mode() & 0o777,
        0o644
    );

    let (_dir, marker, config) = marker_fixture();
    let victim = config.with_file_name("symlink-victim");
    let artifact_path = config.with_file_name("symlink-output.json");
    fs::write(&victim, b"symlink victim").unwrap();
    std::os::unix::fs::symlink(&victim, &artifact_path).unwrap();

    let output = smoke_command(&config)
        .arg("--acknowledge-billable")
        .arg("--out")
        .arg(&artifact_path)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!marker.exists());
    assert_eq!(fs::read(&victim).unwrap(), b"symlink victim");

    let (_dir, marker, config) = marker_fixture();
    let victim = config.with_file_name("hard-link-victim");
    let artifact_path = config.with_file_name("hard-link-output.json");
    fs::write(&victim, b"hard-link victim").unwrap();
    fs::hard_link(&victim, &artifact_path).unwrap();

    let output = smoke_command(&config)
        .arg("--acknowledge-billable")
        .arg("--out")
        .arg(&artifact_path)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!marker.exists());
    assert_eq!(fs::read(&victim).unwrap(), b"hard-link victim");
}

#[test]
fn unwritable_out_refuses_before_agent_spawn() {
    let (dir, marker, config) = marker_fixture();
    let impossible = dir.path().join("missing-parent").join("artifact.json");
    let output = smoke_command(&config)
        .arg("--acknowledge-billable")
        .arg("--timeout-secs")
        .arg("1")
        .arg("--out")
        .arg(impossible)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        !marker.exists(),
        "artifact destination must be opened before a billable agent can spawn"
    );
}

#[test]
fn artifact_path_cannot_alias_or_truncate_config() {
    let (_dir, marker, config) = marker_fixture();
    let before = fs::read(&config).unwrap();
    let output = smoke_command(&config)
        .arg("--acknowledge-billable")
        .arg("--out")
        .arg(&config)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        fs::read(&config).unwrap(),
        before,
        "artifact preflight must never truncate the selected config"
    );
    assert!(!marker.exists());

    let (_dir, marker, config) = marker_fixture();
    let alias = config.with_file_name("symlink-artifact.json");
    std::os::unix::fs::symlink(&config, &alias).unwrap();
    let before = fs::read(&config).unwrap();
    let output = smoke_command(&config)
        .arg("--acknowledge-billable")
        .arg("--out")
        .arg(&alias)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read(&config).unwrap(), before);
    assert_eq!(fs::read(&alias).unwrap(), before);
    assert!(!marker.exists());

    let (_dir, marker, config) = marker_fixture();
    let alias = config.with_file_name("hard-link-artifact.json");
    fs::hard_link(&config, &alias).unwrap();
    let before = fs::read(&config).unwrap();
    let output = smoke_command(&config)
        .arg("--acknowledge-billable")
        .arg("--out")
        .arg(&alias)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read(&config).unwrap(), before);
    assert_eq!(fs::read(&alias).unwrap(), before);
    assert!(!marker.exists());

    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("same-missing-path.json");
    let output = smoke_command(&missing)
        .arg("--acknowledge-billable")
        .arg("--out")
        .arg(&missing)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        !missing.exists(),
        "a missing config/output alias must not be created"
    );
}

#[test]
fn stdout_remains_one_json_artifact_when_tracing_is_enabled() {
    let (_dir, _marker, config) = marker_fixture();
    let output = smoke_command(&config)
        .arg("--acknowledge-billable")
        .arg("--timeout-secs")
        .arg("1")
        .env("RUST_LOG", "trace")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let artifact: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("stdout must remain exactly one JSON artifact under tracing");
    assert_eq!(artifact["schema_version"], 2);
}

#[test]
fn unknown_agent_is_an_artifact_failure_without_spawning_configured_agent() {
    let (_dir, marker, config) = marker_fixture();
    let output = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .arg("smoke")
        .arg("--agent")
        .arg("absent")
        .arg("--config")
        .arg(&config)
        .arg("--acknowledge-billable")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let artifact: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        artifact["diagnostics"]["failure"]["code"],
        "smoke.unknown_agent"
    );
    assert!(!marker.exists());
}
