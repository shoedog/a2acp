use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("spawned");
    let adapter = dir.path().join("marker-agent");
    fs::write(
        &adapter,
        format!("#!/bin/sh\ntouch {:?}\nexit 99\n", marker),
    )
    .unwrap();
    let mut permissions = fs::metadata(&adapter).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&adapter, permissions).unwrap();

    let repo = dir.path().join("owned repo");
    fs::create_dir(&repo).unwrap();
    let config = dir.path().join("a2a bridge 'trusted'.toml");
    fs::write(
        &config,
        format!(
            "default = \"trusted-host\"\nallowed_cwd_root = {repo:?}\n\n\
             [registry]\nallowed_cmds = [{adapter:?}, \"docker\"]\n\n\
             [[agents]]\nid = \"trusted-host\"\ncmd = {adapter:?}\nhost_fallback_eligible = true\n\n\
             [[agents]]\nid = \"unmarked-host\"\ncmd = {adapter:?}\n\n\
             [[agents]]\nid = \"reader-container\"\ncmd = {adapter:?}\n\n\
             [agents.sandbox]\nimage = \"reader:latest\"\nmount = {repo:?}\naccess = \"ro\"\negress = \"open\"\n\n\
             [[agents]]\nid = \"writer-container\"\nkind = \"container_rw\"\ncmd = {adapter:?}\n\n\
             [agents.sandbox]\nimage = \"writer:latest\"\nmount = {repo:?}\naccess = \"rw\"\negress = \"open\"\n\n\
             [[agents]]\nid = \"api-target\"\nkind = \"api\"\nbase_url = \"http://127.0.0.1:9/v1\"\n\n\
             [server]\n"
        ),
    )
    .unwrap();

    let source = dir.path().join("failed-smoke.json");
    write_smoke_artifact(&source, &repo, "container_runtime", false);
    (dir, marker, config, source)
}

fn write_smoke_artifact(path: &Path, repo: &Path, class: &str, accepted: bool) {
    let artifact = smoke_artifact(repo, class, accepted);
    write_json(path, &artifact);
}

fn smoke_artifact(repo: &Path, class: &str, accepted: bool) -> serde_json::Value {
    let disposition = if accepted {
        "fatal"
    } else {
        "container_fallback_candidate"
    };
    serde_json::json!({
        "schema_version": 1,
        "success": false,
        "bridge": {"package_version": "0.2.1", "git_commit": "fixture"},
        "attempt": {
            "id": "smoke-fixture-1",
            "timeout_secs": 120,
            "started_at_ms": 10,
            "ended_at_ms": 20,
            "timed_out": false,
            "prompt_may_have_been_accepted": accepted
        },
        "request": {
            "agent": "reader-container",
            "requested_config_path": "/untrusted/from-artifact.toml",
            "canonical_config_path": "/untrusted/from-artifact.toml",
            "session_cwd": repo
        },
        "target": {
            "execution_mode": "container_ro",
            "provenance": [],
            "authentication": {"path": "automatic"}
        },
        "session": {"id": "smoke-fixture-1", "configure_calls": 0},
        "turn": {
            "prompt": "Reply exactly PONG. Do not use tools.",
            "prompt_calls": 0,
            "terminal_state": "not_started",
            "exact_pong": false,
            "text_bytes": 0,
            "tool_event_count": 0,
            "permission_update_count": 0
        },
        "diagnostics": {
            "lifecycle": [],
            "dropped_events": 0,
            "failure": {
                "schema_version": 1,
                "failed_phase": "spawn",
                "class": class,
                "disposition": disposition,
                "code": "container.fixture.failure",
                "summary": "fixture failure",
                "causes": [],
                "stderr_observed": false,
                "stderr_line_count": 0,
                "prompt_may_have_been_accepted": accepted
            },
            "stderr_text": "excluded"
        },
        "cleanup": {
            "grace_timeout_secs": 5,
            "cancel": "not_needed",
            "release": "not_needed",
            "retire": "not_needed",
            "run_scoped_backstop": "not_needed"
        }
    })
}

fn write_json(path: &Path, value: &serde_json::Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn read_plan(output: &std::process::Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn assert_ineligible_without_command(plan: &serde_json::Value, reason: &str) {
    assert_eq!(plan["eligible"], false, "plan: {plan:#}");
    assert!(
        plan["reasons"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!(reason)),
        "missing reason {reason:?}: {plan:#}"
    );
    assert!(plan.get("rerun").is_none(), "plan: {plan:#}");
}

fn fallback_command(config: &Path, source: &Path) -> Command {
    fallback_command_for(config, source, "trusted-host")
}

fn fallback_command_for(config: &Path, source: &Path, host_agent: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"));
    command
        .arg("fallback-plan")
        .arg("--from")
        .arg(source)
        .arg("--host-agent")
        .arg(host_agent)
        .arg("--config")
        .arg(config);
    command
}

#[test]
fn eligible_plan_is_local_non_billable_and_emits_separate_rerun() {
    let (_dir, marker, config, source) = fixture();
    let output = fallback_command(&config, &source)
        .arg("--confirm-trusted-own-repo-read-only")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let plan: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(plan["schema_version"], 1);
    assert_eq!(plan["eligible"], true);
    assert_eq!(plan["reasons"], serde_json::json!([]));
    assert_eq!(plan["source"]["attempt_id"], "smoke-fixture-1");
    assert_eq!(plan["source"]["original_agent"], "reader-container");
    assert_eq!(plan["target"]["host_agent"], "trusted-host");
    assert_eq!(
        plan["rerun"]["argv"][1], "smoke",
        "a plan may describe only a separate fixed-prompt smoke"
    );
    assert!(plan["rerun"]["argv"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == "--acknowledge-billable"));
    assert!(
        !marker.exists(),
        "fallback-plan must never spawn its target"
    );
}

#[test]
fn missing_trust_confirmation_emits_no_runnable_command() {
    let (_dir, marker, config, source) = fixture();
    let output = fallback_command(&config, &source).output().unwrap();
    assert!(output.status.success());
    let plan: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(plan["eligible"], false);
    assert!(plan["reasons"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("trust_confirmation_missing")));
    assert!(plan.get("rerun").is_none());
    assert!(!marker.exists());
}

#[test]
fn every_typed_container_class_is_eligible_only_as_a_plan() {
    let (_dir, marker, config, source) = fixture();
    for class in [
        "container_runtime",
        "container_image",
        "container_network",
        "container_mount",
        "container_credentials",
    ] {
        let repo = source.parent().unwrap().join("owned repo");
        write_smoke_artifact(&source, &repo, class, false);
        let output = fallback_command(&config, &source)
            .arg("--confirm-trusted-own-repo-read-only")
            .output()
            .unwrap();
        let plan = read_plan(&output);
        assert_eq!(plan["eligible"], true, "class {class}: {plan:#}");
        assert_eq!(plan["source"]["failure_class"], class);
        assert_eq!(plan["rerun"]["attempt_semantics"], "new_distinct_attempt");
        assert!(!marker.exists(), "class {class} spawned the target");
    }
}

#[test]
fn every_non_container_class_is_ineligible_even_with_container_words() {
    let (_dir, marker, config, source) = fixture();
    let repo = source.parent().unwrap().join("owned repo");
    for class in [
        "config",
        "authentication",
        "model",
        "protocol",
        "transport",
        "agent_process",
        "timeout",
        "overloaded",
        "provider_limit",
        "persistence",
        "canceled",
        "unknown",
    ] {
        let mut artifact = smoke_artifact(&repo, class, false);
        artifact["diagnostics"]["failure"]["disposition"] = serde_json::json!("fatal");
        artifact["diagnostics"]["failure"]["summary"] =
            serde_json::json!("docker image network mount credential exit 125");
        write_json(&source, &artifact);
        let output = fallback_command(&config, &source)
            .arg("--confirm-trusted-own-repo-read-only")
            .output()
            .unwrap();
        let plan = read_plan(&output);
        assert_ineligible_without_command(&plan, "source_failure_not_container");
        assert_eq!(plan["source"]["failure_class"], class);
        assert!(!marker.exists(), "class {class} spawned the target");
    }
}

#[test]
fn prompt_start_race_and_missing_diagnostic_both_fail_closed() {
    let (_dir, marker, config, source) = fixture();
    let repo = source.parent().unwrap().join("owned repo");

    let mut race = smoke_artifact(&repo, "container_runtime", true);
    race["diagnostics"]["failure"]["failed_phase"] = serde_json::json!("prompt_start");
    race["diagnostics"]["failure"]["last_completed_phase"] = serde_json::json!("config_apply");
    race["turn"]["prompt_calls"] = serde_json::json!(1);
    race["turn"]["terminal_state"] = serde_json::json!("non_success_terminal");
    write_json(&source, &race);
    let race_output = fallback_command(&config, &source)
        .arg("--confirm-trusted-own-repo-read-only")
        .output()
        .unwrap();
    let race_plan = read_plan(&race_output);
    assert_ineligible_without_command(&race_plan, "source_prompt_may_have_been_accepted");
    assert_eq!(race_plan["source"]["prompt_may_have_been_accepted"], true);

    let mut missing = smoke_artifact(&repo, "container_runtime", false);
    missing["attempt"]["prompt_may_have_been_accepted"] = serde_json::json!(true);
    missing["diagnostics"]
        .as_object_mut()
        .unwrap()
        .remove("failure");
    write_json(&source, &missing);
    let missing_output = fallback_command(&config, &source)
        .arg("--confirm-trusted-own-repo-read-only")
        .output()
        .unwrap();
    let missing_plan = read_plan(&missing_output);
    assert_ineligible_without_command(&missing_plan, "source_diagnostic_missing");
    assert_ineligible_without_command(&missing_plan, "source_prompt_may_have_been_accepted");
    assert_eq!(
        missing_plan["source"]["prompt_may_have_been_accepted"],
        true
    );
    assert!(!marker.exists());
}

#[test]
fn source_must_be_current_known_container_read_only_agent() {
    let (_dir, marker, config, source) = fixture();
    let repo = source.parent().unwrap().join("owned repo");

    let cases = [
        ("missing-source", "container_ro", "source_agent_unknown"),
        (
            "trusted-host",
            "container_ro",
            "source_agent_configuration_mismatch",
        ),
        ("reader-container", "container_rw", "source_not_read_only"),
        ("reader-container", "host", "source_not_container_execution"),
    ];
    for (agent, mode, expected_reason) in cases {
        let mut artifact = smoke_artifact(&repo, "container_runtime", false);
        artifact["request"]["agent"] = serde_json::json!(agent);
        artifact["target"]["execution_mode"] = serde_json::json!(mode);
        write_json(&source, &artifact);
        let output = fallback_command(&config, &source)
            .arg("--confirm-trusted-own-repo-read-only")
            .output()
            .unwrap();
        let plan = read_plan(&output);
        assert_ineligible_without_command(&plan, expected_reason);
        assert!(!marker.exists(), "source case {agent}/{mode} spawned");
    }
}

#[test]
fn target_matrix_is_default_off_and_never_executes() {
    let (_dir, marker, config, source) = fixture();
    for (target, reason) in [
        ("missing-target", "target_agent_unknown"),
        ("unmarked-host", "target_agent_not_eligible"),
        ("reader-container", "target_agent_not_eligible"),
        ("writer-container", "target_agent_not_eligible"),
        ("api-target", "target_agent_not_eligible"),
    ] {
        let output = fallback_command_for(&config, &source, target)
            .arg("--confirm-trusted-own-repo-read-only")
            .output()
            .unwrap();
        let plan = read_plan(&output);
        assert_ineligible_without_command(&plan, reason);
        assert!(!marker.exists(), "target {target} executed");
    }
}

#[test]
fn failed_success_timeout_and_dropped_event_invariants_are_closed() {
    let (_dir, marker, config, source) = fixture();
    let repo = source.parent().unwrap().join("owned repo");
    let cases = [
        ("success", "source_not_failed"),
        ("timeout", "source_timed_out"),
        ("dropped", "source_diagnostics_incomplete"),
        ("provenance", "source_config_provenance_missing"),
        ("phase", "source_failure_phase_invalid"),
    ];
    for (case, reason) in cases {
        let mut artifact = smoke_artifact(&repo, "container_runtime", false);
        match case {
            "success" => artifact["success"] = serde_json::json!(true),
            "timeout" => artifact["attempt"]["timed_out"] = serde_json::json!(true),
            "dropped" => artifact["diagnostics"]["dropped_events"] = serde_json::json!(1),
            "provenance" => {
                artifact["request"]
                    .as_object_mut()
                    .unwrap()
                    .remove("canonical_config_path");
            }
            "phase" => {
                artifact["diagnostics"]["failure"]["failed_phase"] =
                    serde_json::json!("initialize");
            }
            _ => unreachable!(),
        }
        write_json(&source, &artifact);
        let output = fallback_command(&config, &source)
            .arg("--confirm-trusted-own-repo-read-only")
            .output()
            .unwrap();
        assert_ineligible_without_command(&read_plan(&output), reason);
        assert!(!marker.exists());
    }
}

#[test]
fn task_diagnostic_v1_uses_the_same_closed_gate() {
    let (_dir, marker, config, source) = fixture();
    let repo = source.parent().unwrap().join("owned repo");
    let smoke = smoke_artifact(&repo, "container_mount", false);
    let task = serde_json::json!({
        "artifact_type": "task_diagnostic",
        "schema_version": 1,
        "task_id": "task-fixture-1",
        "attempt_id": "attempt-fixture-1",
        "agent": "reader-container",
        "execution_mode": "container_ro",
        "prompt_may_have_been_accepted": false,
        "session_cwd": repo,
        "diagnostic": smoke["diagnostics"]["failure"].clone()
    });
    write_json(&source, &task);
    let output = fallback_command(&config, &source)
        .arg("--confirm-trusted-own-repo-read-only")
        .output()
        .unwrap();
    let plan = read_plan(&output);
    assert_eq!(plan["eligible"], true, "plan: {plan:#}");
    assert_eq!(plan["source"]["artifact_schema"], "task_diagnostic_v1");
    assert_eq!(plan["source"]["task_id"], "task-fixture-1");
    assert_eq!(plan["source"]["attempt_id"], "attempt-fixture-1");
    assert_eq!(plan["source"]["failure_class"], "container_mount");
    assert!(!marker.exists());
}

#[test]
fn malformed_legacy_unsupported_and_oversized_sources_are_rejected() {
    let (dir, marker, config, source) = fixture();
    let repo = source.parent().unwrap().join("owned repo");
    let mut unsupported = smoke_artifact(&repo, "container_runtime", false);
    unsupported["schema_version"] = serde_json::json!(2);
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("malformed", b"{".to_vec()),
        (
            "legacy",
            br#"{"error":"AgentCrashed","reason":"docker image exit 125"}"#.to_vec(),
        ),
        (
            "unsupported",
            serde_json::to_vec_pretty(&unsupported).unwrap(),
        ),
        ("oversized", vec![b' '; 1024 * 1024 + 1]),
    ];
    for (name, bytes) in cases {
        let candidate = dir.path().join(format!("{name}.json"));
        fs::write(&candidate, bytes).unwrap();
        let output = fallback_command(&config, &candidate)
            .arg("--confirm-trusted-own-repo-read-only")
            .output()
            .unwrap();
        assert!(!output.status.success(), "{name} unexpectedly succeeded");
        assert!(output.stdout.is_empty(), "{name} emitted a plan");
        assert!(
            !String::from_utf8_lossy(&output.stderr).contains("rerun"),
            "{name} emitted a runnable command"
        );
        assert!(!marker.exists(), "{name} spawned the target");
    }

    let missing = dir.path().join("missing.json");
    let output = fallback_command(&config, &missing)
        .arg("--confirm-trusted-own-repo-read-only")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!source.as_os_str().is_empty());
    assert!(!marker.exists());
}

#[test]
fn exact_argv_uses_cli_config_not_artifact_config_and_escapes_quotes() {
    let (_dir, marker, config, source) = fixture();
    let output = fallback_command(&config, &source)
        .arg("--confirm-trusted-own-repo-read-only")
        .output()
        .unwrap();
    let plan = read_plan(&output);
    let canonical_config = fs::canonicalize(&config).unwrap();
    let argv = plan["rerun"]["argv"].as_array().unwrap();
    let expected_argv = serde_json::json!([
        "a2a-bridge",
        "smoke",
        "--agent",
        "trusted-host",
        "--config",
        canonical_config,
        "--acknowledge-billable",
        "--session-cwd",
        source.parent().unwrap().join("owned repo")
    ]);
    assert_eq!(argv, expected_argv.as_array().unwrap());
    assert_ne!(
        plan["target"]["config_canonical_path"],
        "/untrusted/from-artifact.toml"
    );
    assert!(
        plan["rerun"]["shell_command"]
            .as_str()
            .unwrap()
            .contains("'\"'\"'"),
        "single quote in config path must use POSIX-safe quoting: {plan:#}"
    );
    assert!(!marker.exists());
}

#[test]
fn control_character_injection_is_rejected_without_a_plan() {
    let (dir, marker, config, source) = fixture();
    let bad_agent = fallback_command_for(&config, &source, "trusted-host\nmalicious")
        .arg("--confirm-trusted-own-repo-read-only")
        .output()
        .unwrap();
    assert!(!bad_agent.status.success());
    assert!(bad_agent.stdout.is_empty());

    let newline_source = dir.path().join("failed\nsource.json");
    fs::copy(&source, &newline_source).unwrap();
    let bad_path = fallback_command(&config, &newline_source)
        .arg("--confirm-trusted-own-repo-read-only")
        .output()
        .unwrap();
    assert!(!bad_path.status.success());
    assert!(bad_path.stdout.is_empty());

    let repo = source.parent().unwrap().join("owned repo");
    let mut artifact = smoke_artifact(&repo, "container_runtime", false);
    artifact["request"]["session_cwd"] = serde_json::json!("/trusted\nrepo");
    write_json(&source, &artifact);
    let bad_cwd = fallback_command(&config, &source)
        .arg("--confirm-trusted-own-repo-read-only")
        .output()
        .unwrap();
    assert!(!bad_cwd.status.success());
    assert!(bad_cwd.stdout.is_empty());
    assert!(!marker.exists());
}
