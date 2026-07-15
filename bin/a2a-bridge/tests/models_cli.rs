use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

fn write_config(path: &Path, body: &str) {
    fs::write(path, body).expect("write models test config");
}

fn bridge_models(config: &Path, extra: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"));
    command
        .arg("models")
        .arg("--config")
        .arg(config)
        .args(extra);
    command.output().expect("run a2a-bridge models")
}

#[test]
fn configured_agent_probe_failure_is_structured_and_nonzero() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing-acp-adapter");
    let config = dir.path().join("a2a-bridge.toml");
    write_config(
        &config,
        &format!(
            "default = \"broken\"\n\n\
             [server]\naddr = \"127.0.0.1:0\"\n\n\
             [registry]\nallowed_cmds = [{missing:?}]\n\n\
             [[agents]]\nid = \"broken\"\ncmd = {missing:?}\nversion = \"9.9.9\"\n",
        ),
    );

    let output = bridge_models(&config, &["--agent", "broken", "--json"]);
    assert!(
        !output.status.success(),
        "a configured agent whose probe failed must not exit successfully: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("failure stdout remains machine-readable JSON");
    assert_eq!(value["broken"]["available"], false);
    let failure = &value["broken"]["failure"];
    assert_eq!(failure["agent"], "broken");
    assert_eq!(failure["strategy"], "acp");
    assert_eq!(failure["phase"], "spawn");
    assert_eq!(failure["executable"], missing.to_string_lossy().as_ref());
    assert_eq!(failure["configured_version"], "9.9.9");
    assert!(
        failure["error"]
            .as_str()
            .is_some_and(|error| error.contains("spawn") && !error.is_empty()),
        "deepest bounded error was not retained: {failure}",
    );
    assert_eq!(failure["diagnostic"]["failed_phase"], "spawn");
    let deepest = failure["diagnostic"]["causes"]
        .as_array()
        .and_then(|causes| causes.last())
        .and_then(serde_json::Value::as_str)
        .expect("structured ACP failure retains its deepest safe cause");
    assert!(!deepest.is_empty());
    assert!(failure["error"].as_str().unwrap().contains(deepest));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("models: probe failed for agent")
            && String::from_utf8_lossy(&output.stderr).contains("'broken'")
            && String::from_utf8_lossy(&output.stderr).contains("during spawn"),
        "stderr must explain the nonzero exit: {}",
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn all_agent_json_keeps_healthy_caps_and_represents_probe_failure() {
    let dir = tempfile::tempdir().unwrap();
    let healthy = dir.path().join("kiro-cli");
    fs::write(
        &healthy,
        "#!/bin/sh\nprintf '%s\\n' '* auto 1.00x credits Models chosen by task'\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&healthy).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&healthy, permissions).unwrap();

    let missing = dir.path().join("missing-acp-adapter");
    let config = dir.path().join("a2a-bridge.toml");
    write_config(
        &config,
        &format!(
            "default = \"healthy\"\n\n\
             [server]\naddr = \"127.0.0.1:0\"\n\n\
             [registry]\nallowed_cmds = [{healthy:?}, {missing:?}]\n\n\
             [[agents]]\nid = \"healthy\"\ncmd = {healthy:?}\n\n\
             [[agents]]\nid = \"broken\"\ncmd = {missing:?}\n",
        ),
    );

    let output = bridge_models(&config, &["--json"]);
    assert!(
        output.status.success(),
        "all-agent discovery should preserve partial-success semantics: stderr={}",
        String::from_utf8_lossy(&output.stderr),
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["healthy"]["models"], serde_json::json!(["auto"]));
    assert_eq!(value["broken"]["available"], false);
    assert_eq!(value["broken"]["failure"]["agent"], "broken");
    assert_eq!(value.as_object().unwrap().len(), 2);
}
