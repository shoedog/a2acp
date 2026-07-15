use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("spawned");
    let cwd_marker = dir.path().join("spawned.cwd");
    let adapter = dir.path().join("codex-acp");
    fs::write(
        &adapter,
        format!(
            "#!/bin/sh\ntouch {:?}\npwd -P > {:?}\nexit 99\n",
            marker, cwd_marker
        ),
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
    write_smoke_artifact(&source, &repo, &config, "container_runtime", false);
    (dir, marker, config, source)
}

fn write_smoke_artifact(path: &Path, repo: &Path, config: &Path, class: &str, accepted: bool) {
    let artifact = smoke_artifact(repo, config, class, accepted);
    write_json(path, &artifact);
}

fn sha256_hex(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn smoke_artifact(repo: &Path, config: &Path, class: &str, accepted: bool) -> serde_json::Value {
    let disposition = if accepted {
        "fatal"
    } else {
        "container_fallback_candidate"
    };
    let failure = serde_json::json!({
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
    });
    let canonical_config = fs::canonicalize(config).unwrap();
    let config_sha256 = sha256_hex(&fs::read(config).unwrap());
    serde_json::json!({
        "schema_version": 2,
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
            "requested_config_path": config,
            "canonical_config_path": canonical_config,
            "config_sha256": config_sha256,
            "session_cwd": repo
        },
        "target": {
            "execution_mode": "container_ro",
            "provenance": [
                {"check": "provenance:reader-container:execution", "status": "ok", "detail": "fixture execution", "remedy": ""},
                {"check": "provenance:reader-container:adapter", "status": "warn", "detail": "fixture adapter unknown", "remedy": "inspect image metadata"},
                {"check": "provenance:reader-container:agent-cli", "status": "warn", "detail": "fixture agent CLI unknown", "remedy": "inspect image metadata"},
                {"check": "provenance:reader-container:image", "status": "warn", "detail": "fixture image unknown", "remedy": "inspect image metadata"},
                {"check": "provenance:reader-container:auth", "status": "ok", "detail": "fixture", "remedy": ""},
                {"check": "provenance:reader-container:model", "status": "ok", "detail": "fixture", "remedy": ""}
            ],
            "authentication": {"path": "automatic"}
        },
        "session": {"id": "smoke-fixture-1", "configure_calls": 0, "effective_request": {}},
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
            "lifecycle": [
                {"transition": {"phase": "resolve", "status": "started", "at_ms": 11}},
                {"transition": {"phase": "spawn", "status": "started", "at_ms": 12}},
                {"transition": {"phase": "spawn", "status": "failed", "at_ms": 13}, "failure": failure.clone()},
                {"transition": {"phase": "resolve", "status": "failed", "at_ms": 14, "code": "backend.initialize_failed"}}
            ],
            "dropped_events": 0,
            "failure": failure,
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
    fallback_command_for_trusted(
        config,
        source,
        host_agent,
        &source.parent().unwrap().join("owned repo"),
    )
}

fn fallback_command_for_trusted(
    config: &Path,
    source: &Path,
    host_agent: &str,
    trusted_session_cwd: &Path,
) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"));
    command
        .arg("fallback-plan")
        .arg("--from")
        .arg(source)
        .arg("--host-agent")
        .arg(host_agent)
        .arg("--trusted-session-cwd")
        .arg(trusted_session_cwd)
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
    assert_eq!(plan["schema_version"], 2);
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
fn production_smoke_preflight_artifact_is_eligible_without_hand_built_lifecycle() {
    let (dir, marker, _fixture_config, _source) = fixture();
    let adapter = marker.with_file_name("codex-acp");
    let repo = dir.path().join("owned repo");
    let missing_credential = dir.path().join("missing-auth.json");
    let config = dir.path().join("production-preflight.toml");
    fs::write(
        &config,
        format!(
            "default = \"trusted-host\"\nallowed_cwd_root = {repo:?}\n\n\
             [registry]\nallowed_cmds = [{adapter:?}, \"/usr/bin/true\"]\n\n\
             [[agents]]\nid = \"trusted-host\"\ncmd = {adapter:?}\nhost_fallback_eligible = true\n\n\
             [[agents]]\nid = \"reader-container\"\ncmd = {adapter:?}\n\n\
             [agents.sandbox]\nruntime = \"/usr/bin/true\"\nimage = \"reader:latest\"\nmount = {repo:?}\naccess = \"ro\"\negress = \"open\"\nvolumes = [{volume:?}]\n\n\
             [server]\n",
            volume = format!(
                "{}:/root/.codex/auth.json:ro",
                missing_credential.display()
            )
        ),
    )
    .unwrap();
    let artifact_path = dir.path().join("production-failed-smoke.json");
    let smoke = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .arg("smoke")
        .arg("--agent")
        .arg("reader-container")
        .arg("--config")
        .arg(&config)
        .arg("--session-cwd")
        .arg(&repo)
        .arg("--acknowledge-billable")
        .arg("--out")
        .arg(&artifact_path)
        .output()
        .unwrap();
    assert!(!smoke.status.success());
    assert!(smoke.stdout.is_empty());
    let artifact: serde_json::Value =
        serde_json::from_slice(&fs::read(&artifact_path).unwrap()).unwrap();
    assert_eq!(
        artifact["diagnostics"]["failure"]["class"],
        "container_credentials"
    );
    assert_eq!(
        artifact["diagnostics"]["lifecycle"]
            .as_array()
            .unwrap()
            .len(),
        4
    );

    let planned = fallback_command(&config, &artifact_path)
        .arg("--confirm-trusted-own-repo-read-only")
        .output()
        .unwrap();
    let plan = read_plan(&planned);
    assert_eq!(plan["eligible"], true, "plan: {plan:#}");
    assert!(
        !marker.exists(),
        "neither smoke preflight nor planner may spawn an agent"
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
fn missing_or_out_of_scope_trusted_cwd_never_emits_a_command() {
    let (_dir, marker, config, source) = fixture();
    let missing = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .arg("fallback-plan")
        .arg("--from")
        .arg(&source)
        .arg("--host-agent")
        .arg("trusted-host")
        .arg("--config")
        .arg(&config)
        .arg("--confirm-trusted-own-repo-read-only")
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(missing.stdout.is_empty());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("--trusted-session-cwd"));

    let outside = fallback_command_for_trusted(&config, &source, "trusted-host", Path::new("/etc"))
        .arg("--confirm-trusted-own-repo-read-only")
        .output()
        .unwrap();
    assert_ineligible_without_command(
        &read_plan(&outside),
        "trusted_session_cwd_outside_source_mount",
    );
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
        write_smoke_artifact(&source, &repo, &config, class, false);
        let output = fallback_command(&config, &source)
            .arg("--confirm-trusted-own-repo-read-only")
            .output()
            .unwrap();
        let plan = read_plan(&output);
        assert_eq!(plan["eligible"], true, "class {class}: {plan:#}");
        assert_eq!(plan["source"]["failure_class"], class);
        assert_eq!(
            plan["rerun"]["attempt_semantics"],
            "new_distinct_verification_smoke"
        );
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
        let mut artifact = smoke_artifact(&repo, &config, class, false);
        artifact["diagnostics"]["failure"]["disposition"] = serde_json::json!("fatal");
        artifact["diagnostics"]["failure"]["summary"] =
            serde_json::json!("docker image network mount credential exit 125");
        artifact["diagnostics"]["lifecycle"][2]["failure"] =
            artifact["diagnostics"]["failure"].clone();
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

    let mut race = smoke_artifact(&repo, &config, "container_runtime", true);
    race["diagnostics"]["failure"]["failed_phase"] = serde_json::json!("prompt_start");
    race["diagnostics"]["failure"]["last_completed_phase"] = serde_json::json!("config_apply");
    race["diagnostics"]["lifecycle"] = serde_json::json!([
        {"transition": {"phase": "prompt_start", "status": "started", "at_ms": 11}},
        {
            "transition": {"phase": "prompt_start", "status": "failed", "at_ms": 12},
            "failure": race["diagnostics"]["failure"].clone()
        }
    ]);
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

    let mut missing = smoke_artifact(&repo, &config, "container_runtime", false);
    missing["attempt"]["prompt_may_have_been_accepted"] = serde_json::json!(true);
    missing["diagnostics"]
        .as_object_mut()
        .unwrap()
        .remove("failure");
    missing["diagnostics"]["lifecycle"] = serde_json::json!([
        {"transition": {"phase": "prompt_start", "status": "started", "at_ms": 11}},
        {"transition": {"phase": "prompt_start", "status": "completed", "at_ms": 12}}
    ]);
    missing["turn"]["prompt_calls"] = serde_json::json!(1);
    missing["turn"]["terminal_state"] = serde_json::json!("non_success_terminal");
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
        let mut artifact = smoke_artifact(&repo, &config, "container_runtime", false);
        artifact["request"]["agent"] = serde_json::json!(agent);
        artifact["target"]["execution_mode"] = serde_json::json!(mode);
        for row in artifact["target"]["provenance"].as_array_mut().unwrap() {
            let check = row["check"]
                .as_str()
                .unwrap()
                .replace("reader-container", agent);
            row["check"] = serde_json::json!(check);
        }
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
        let mut artifact = smoke_artifact(&repo, &config, "container_runtime", false);
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
                artifact["diagnostics"]["lifecycle"][1]["transition"]["phase"] =
                    serde_json::json!("initialize");
                artifact["diagnostics"]["lifecycle"][2]["transition"]["phase"] =
                    serde_json::json!("initialize");
                artifact["diagnostics"]["lifecycle"][2]["failure"] =
                    artifact["diagnostics"]["failure"].clone();
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
fn hand_assembled_task_diagnostic_is_rejected_without_a_plan() {
    let (_dir, marker, config, source) = fixture();
    let repo = source.parent().unwrap().join("owned repo");
    let smoke = smoke_artifact(&repo, &config, "container_mount", false);
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
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not trusted evidence"));
    assert!(!marker.exists());
}

#[test]
fn malformed_legacy_unsupported_and_oversized_sources_are_rejected() {
    let (dir, marker, config, source) = fixture();
    let repo = source.parent().unwrap().join("owned repo");
    let mut unsupported = smoke_artifact(&repo, &config, "container_runtime", false);
    unsupported["schema_version"] = serde_json::json!(3);
    let mut bogus_provenance = smoke_artifact(&repo, &config, "container_runtime", false);
    bogus_provenance["target"]["provenance"] = serde_json::json!([0]);
    let mut bogus_authentication = smoke_artifact(&repo, &config, "container_runtime", false);
    bogus_authentication["target"]["authentication"] = serde_json::json!(true);
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
        (
            "bogus-provenance",
            serde_json::to_vec_pretty(&bogus_provenance).unwrap(),
        ),
        (
            "bogus-authentication",
            serde_json::to_vec_pretty(&bogus_authentication).unwrap(),
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
fn production_auth_wire_matches_current_source_and_impossible_provenance_fails_closed() {
    let (_dir, marker, config, source) = fixture();
    let configured = fs::read_to_string(&config).unwrap().replacen(
        "[agents.sandbox]",
        "auth_method = \"chat-gpt\"\n\n[agents.sandbox]",
        1,
    );
    fs::write(&config, configured).unwrap();
    let mut artifact: serde_json::Value =
        serde_json::from_slice(&fs::read(&source).unwrap()).unwrap();
    artifact["request"]["config_sha256"] =
        serde_json::json!(sha256_hex(&fs::read(&config).unwrap()));
    artifact["target"]["authentication"] = serde_json::json!({
        "path": "configured_method",
        "method": {"state": "value", "value": "chat-gpt"}
    });
    write_json(&source, &artifact);
    let valid = fallback_command(&config, &source)
        .arg("--confirm-trusted-own-repo-read-only")
        .output()
        .unwrap();
    assert_eq!(read_plan(&valid)["eligible"], true);
    assert!(!marker.exists());

    let (_dir, _marker, config, source) = fixture();
    let pre_authenticated = fs::read_to_string(&config).unwrap().replacen(
        "[agents.sandbox]",
        "pre_authenticated = true\n\n[agents.sandbox]",
        1,
    );
    fs::write(&config, pre_authenticated).unwrap();
    let mut artifact: serde_json::Value =
        serde_json::from_slice(&fs::read(&source).unwrap()).unwrap();
    artifact["request"]["config_sha256"] =
        serde_json::json!(sha256_hex(&fs::read(&config).unwrap()));
    write_json(&source, &artifact);
    let mismatch = fallback_command(&config, &source)
        .arg("--confirm-trusted-own-repo-read-only")
        .output()
        .unwrap();
    assert_ineligible_without_command(&read_plan(&mismatch), "source_agent_configuration_mismatch");

    let (_dir, marker, config, source) = fixture();
    let mut artifact: serde_json::Value =
        serde_json::from_slice(&fs::read(&source).unwrap()).unwrap();
    artifact["target"]["provenance"][4]["status"] = serde_json::json!("fail");
    artifact["target"]["provenance"][4]["remedy"] = serde_json::json!("repair auth");
    write_json(&source, &artifact);
    let impossible = fallback_command(&config, &source)
        .arg("--confirm-trusted-own-repo-read-only")
        .output()
        .unwrap();
    assert_ineligible_without_command(&read_plan(&impossible), "source_diagnostics_incomplete");
    assert!(!marker.exists());
}

#[test]
fn redacted_configured_auth_matches_only_the_current_source_redactor() {
    let (_dir, marker, config, source) = fixture();
    let secret = "shared-auth-and-mcp-secret";
    let configured = fs::read_to_string(&config).unwrap().replacen(
        "[agents.sandbox]",
        &format!(
            "auth_method = {secret:?}\n\n\
             [[agents.mcp]]\nname = \"secret-source\"\ncommand = \"docker\"\n\n\
             [[agents.mcp.env]]\nname = \"TOKEN\"\nvalue = {secret:?}\n\n\
             [agents.sandbox]"
        ),
        1,
    );
    fs::write(&config, configured).unwrap();
    let mut artifact: serde_json::Value =
        serde_json::from_slice(&fs::read(&source).unwrap()).unwrap();
    artifact["request"]["config_sha256"] =
        serde_json::json!(sha256_hex(&fs::read(&config).unwrap()));
    artifact["target"]["authentication"] = serde_json::json!({
        "path": "configured_method",
        "method": {"state": "redacted"}
    });
    write_json(&source, &artifact);

    let genuine = fallback_command(&config, &source)
        .arg("--confirm-trusted-own-repo-read-only")
        .output()
        .unwrap();
    assert_eq!(read_plan(&genuine)["eligible"], true);
    assert!(!marker.exists());

    let (_dir, marker, config, source) = fixture();
    let configured = fs::read_to_string(&config).unwrap().replacen(
        "[agents.sandbox]",
        "auth_method = \"ordinary-method\"\n\n[agents.sandbox]",
        1,
    );
    fs::write(&config, configured).unwrap();
    let mut artifact: serde_json::Value =
        serde_json::from_slice(&fs::read(&source).unwrap()).unwrap();
    artifact["request"]["config_sha256"] =
        serde_json::json!(sha256_hex(&fs::read(&config).unwrap()));
    artifact["target"]["authentication"] = serde_json::json!({
        "path": "configured_method",
        "method": {"state": "redacted"}
    });
    write_json(&source, &artifact);

    let fabricated = fallback_command(&config, &source)
        .arg("--confirm-trusted-own-repo-read-only")
        .output()
        .unwrap();
    assert_ineligible_without_command(
        &read_plan(&fabricated),
        "source_agent_configuration_mismatch",
    );
    assert!(!marker.exists());
}

#[test]
fn exact_argv_uses_current_binary_config_digest_and_explicit_trusted_cwd() {
    let (_dir, marker, config, source) = fixture();
    let output = fallback_command(&config, &source)
        .arg("--confirm-trusted-own-repo-read-only")
        .output()
        .unwrap();
    let plan = read_plan(&output);
    let canonical_config = fs::canonicalize(&config).unwrap();
    let canonical_repo = fs::canonicalize(source.parent().unwrap().join("owned repo")).unwrap();
    let canonical_repo_text = canonical_repo.to_string_lossy().into_owned();
    let canonical_repo_metadata = fs::metadata(&canonical_repo).unwrap();
    use std::os::unix::fs::MetadataExt as _;
    let canonical_repo_device = canonical_repo_metadata.dev().to_string();
    let canonical_repo_inode = canonical_repo_metadata.ino().to_string();
    let config_sha256 = sha256_hex(&fs::read(&config).unwrap());
    let executable = fs::canonicalize(env!("CARGO_BIN_EXE_a2a-bridge")).unwrap();
    let executable_sha256 = sha256_hex(&fs::read(&executable).unwrap());
    let argv = plan["rerun"]["argv"].as_array().unwrap();
    let expected_argv = serde_json::json!([
        executable,
        "smoke",
        "--agent",
        "trusted-host",
        "--config",
        canonical_config,
        "--acknowledge-billable",
        "--session-cwd",
        canonical_repo_text,
        "--expected-session-cwd",
        canonical_repo_text,
        "--expected-session-cwd-device",
        canonical_repo_device,
        "--expected-session-cwd-inode",
        canonical_repo_inode,
        "--expected-config-sha256",
        config_sha256,
        "--expected-executable-sha256",
        executable_sha256,
        "--fallback-source-agent",
        "reader-container",
        "--require-host-fallback-eligible"
    ]);
    assert_eq!(argv, expected_argv.as_array().unwrap());
    assert_eq!(
        plan["source"]["reported_session_cwd"],
        source
            .parent()
            .unwrap()
            .join("owned repo")
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(plan["trust"]["trusted_session_cwd"], canonical_repo_text);
    assert_eq!(
        plan["trust"]["trusted_session_cwd_device"],
        canonical_repo_metadata.dev()
    );
    assert_eq!(
        plan["trust"]["trusted_session_cwd_inode"],
        canonical_repo_metadata.ino()
    );
    assert_eq!(plan["target"]["config_sha256"], config_sha256);
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
fn artifact_cwd_cannot_replace_the_explicit_trusted_repo() {
    let (_dir, marker, config, source) = fixture();
    let mut artifact: serde_json::Value =
        serde_json::from_slice(&fs::read(&source).unwrap()).unwrap();
    artifact["request"]["session_cwd"] = serde_json::json!("/etc");
    write_json(&source, &artifact);

    let output = fallback_command(&config, &source)
        .arg("--confirm-trusted-own-repo-read-only")
        .output()
        .unwrap();
    assert_ineligible_without_command(&read_plan(&output), "source_session_cwd_mismatch");
    assert!(!marker.exists());
}

#[test]
fn drifted_config_and_contradictory_lifecycle_fail_closed() {
    let (_dir, marker, config, source) = fixture();

    let mut drifted: serde_json::Value =
        serde_json::from_slice(&fs::read(&source).unwrap()).unwrap();
    drifted["request"]["canonical_config_path"] = serde_json::json!("/different/config.toml");
    write_json(&source, &drifted);
    let output = fallback_command(&config, &source)
        .arg("--confirm-trusted-own-repo-read-only")
        .output()
        .unwrap();
    assert_ineligible_without_command(&read_plan(&output), "source_config_provenance_mismatch");

    let repo = source.parent().unwrap().join("owned repo");
    let mut contradictory = smoke_artifact(&repo, &config, "container_runtime", false);
    contradictory["diagnostics"]["lifecycle"]
        .as_array_mut()
        .unwrap()
        .extend(
            serde_json::json!([
                {"transition": {"phase": "prompt_start", "status": "started", "at_ms": 13}},
                {"transition": {"phase": "prompt_start", "status": "completed", "at_ms": 14}}
            ])
            .as_array()
            .unwrap()
            .iter()
            .cloned(),
        );
    write_json(&source, &contradictory);
    let output = fallback_command(&config, &source)
        .arg("--confirm-trusted-own-repo-read-only")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());

    let mut retried = smoke_artifact(&repo, &config, "container_runtime", false);
    let failure = retried["diagnostics"]["failure"].clone();
    retried["diagnostics"]["lifecycle"] = serde_json::json!([
        {"transition": {"phase": "resolve", "status": "started", "at_ms": 11}},
        {"transition": {"phase": "spawn", "status": "started", "at_ms": 12}},
        {"transition": {"phase": "spawn", "status": "completed", "at_ms": 13}},
        {"transition": {"phase": "spawn", "status": "started", "at_ms": 14}},
        {"transition": {"phase": "spawn", "status": "failed", "at_ms": 15}, "failure": failure},
        {"transition": {"phase": "resolve", "status": "failed", "at_ms": 16, "code": "backend.initialize_failed"}}
    ]);
    write_json(&source, &retried);
    let output = fallback_command(&config, &source)
        .arg("--confirm-trusted-own-repo-read-only")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());

    let mut time_reversed = smoke_artifact(&repo, &config, "container_runtime", false);
    time_reversed["diagnostics"]["lifecycle"][1]["transition"]["at_ms"] = serde_json::json!(10);
    write_json(&source, &time_reversed);
    let output = fallback_command(&config, &source)
        .arg("--confirm-trusted-own-repo-read-only")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());

    let mut divergent_failure = smoke_artifact(&repo, &config, "container_runtime", false);
    divergent_failure["diagnostics"]["lifecycle"][2]["failure"]["summary"] =
        serde_json::json!("event and outer diagnostics disagree");
    write_json(&source, &divergent_failure);
    let output = fallback_command(&config, &source)
        .arg("--confirm-trusted-own-repo-read-only")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());

    let mut missing_lifecycle = smoke_artifact(&repo, &config, "container_runtime", false);
    missing_lifecycle["diagnostics"]["lifecycle"] = serde_json::json!([]);
    write_json(&source, &missing_lifecycle);
    let output = fallback_command(&config, &source)
        .arg("--confirm-trusted-own-repo-read-only")
        .output()
        .unwrap();
    assert_ineligible_without_command(&read_plan(&output), "source_diagnostics_incomplete");
    assert!(!marker.exists());
}

#[test]
fn generated_smoke_refuses_config_executable_and_cwd_drift_before_spawn() {
    for drift in ["config", "executable", "cwd", "target-marker"] {
        let (_dir, marker, config, source) = fixture();
        let plan_output = fallback_command(&config, &source)
            .arg("--confirm-trusted-own-repo-read-only")
            .output()
            .unwrap();
        let plan = read_plan(&plan_output);
        let mut argv: Vec<String> = plan["rerun"]["argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect();
        match drift {
            "config" => fs::write(
                &config,
                format!("{}\n# drift\n", fs::read_to_string(&config).unwrap()),
            )
            .unwrap(),
            "executable" => {
                let index = argv
                    .iter()
                    .position(|value| value == "--expected-executable-sha256")
                    .unwrap();
                argv[index + 1] = "0".repeat(64);
            }
            "cwd" => {
                let index = argv
                    .iter()
                    .position(|value| value == "--session-cwd")
                    .unwrap();
                argv[index + 1] = "/etc".into();
            }
            "target-marker" => {
                let changed = fs::read_to_string(&config).unwrap().replace(
                    "host_fallback_eligible = true",
                    "host_fallback_eligible = false",
                );
                fs::write(&config, changed).unwrap();
                let index = argv
                    .iter()
                    .position(|value| value == "--expected-config-sha256")
                    .unwrap();
                argv[index + 1] = sha256_hex(&fs::read(&config).unwrap());
            }
            _ => unreachable!(),
        }
        let output = Command::new(&argv[0]).args(&argv[1..]).output().unwrap();
        assert!(!output.status.success(), "{drift} drift unexpectedly ran");
        let artifact: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let expected_code = match drift {
            "config" => "smoke.fallback_config_drift",
            "executable" => "smoke.fallback_executable_drift",
            "cwd" => "smoke.fallback_cwd_drift",
            "target-marker" => "smoke.fallback_target_drift",
            _ => unreachable!(),
        };
        assert_eq!(artifact["diagnostics"]["failure"]["code"], expected_code);
        assert!(!marker.exists(), "{drift} drift spawned the target");
    }
}

#[test]
fn generated_smoke_refuses_trusted_cwd_symlink_swap_before_spawn() {
    let (dir, marker, config, source) = fixture();
    let plan = read_plan(
        &fallback_command(&config, &source)
            .arg("--confirm-trusted-own-repo-read-only")
            .output()
            .unwrap(),
    );
    let argv: Vec<String> = plan["rerun"]["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect();
    let planned = dir.path().join("owned repo");
    fs::rename(&planned, dir.path().join("planned-repo-moved")).unwrap();
    let sibling = dir.path().join("sibling repo");
    fs::create_dir(&sibling).unwrap();
    std::os::unix::fs::symlink(&sibling, &planned).unwrap();

    let output = Command::new(&argv[0]).args(&argv[1..]).output().unwrap();
    assert!(!output.status.success());
    let artifact: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        artifact["diagnostics"]["failure"]["code"],
        "smoke.fallback_cwd_drift"
    );
    assert!(!marker.exists(), "cwd identity drift spawned the target");
}

#[test]
fn generated_smoke_refuses_same_path_directory_replacement_before_spawn() {
    let (dir, marker, config, source) = fixture();
    let plan = read_plan(
        &fallback_command(&config, &source)
            .arg("--confirm-trusted-own-repo-read-only")
            .output()
            .unwrap(),
    );
    let mut argv: Vec<String> = plan["rerun"]["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect();
    argv.extend(["--timeout-secs".into(), "1".into()]);

    let planned = dir.path().join("owned repo");
    fs::rename(&planned, dir.path().join("planned-repo-moved")).unwrap();
    let replacement = dir.path().join("replacement-repo");
    fs::create_dir(&replacement).unwrap();
    fs::rename(&replacement, &planned).unwrap();

    let output = Command::new(&argv[0]).args(&argv[1..]).output().unwrap();
    assert!(!output.status.success());
    let artifact: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(!marker.exists(), "replacement repo spawned the target");
    assert_eq!(
        artifact["diagnostics"]["failure"]["code"],
        "smoke.fallback_cwd_drift"
    );
}

#[test]
fn guarded_host_smoke_never_invokes_the_degraded_container_runtime() {
    let (dir, target_marker, _fixture_config, _fixture_source) = fixture();
    let target_cwd_marker = target_marker.with_file_name("spawned.cwd");
    let adapter = target_marker.with_file_name("codex-acp");
    let repo = dir.path().join("owned repo");
    let source_root = dir.path();
    let runtime_marker = dir.path().join("runtime-invoked");
    let runtime = dir.path().join("runtime-probe");
    fs::write(
        &runtime,
        format!("#!/bin/sh\ntouch {:?}\nexit 0\n", runtime_marker),
    )
    .unwrap();
    let mut permissions = fs::metadata(&runtime).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&runtime, permissions).unwrap();
    let config = dir.path().join("guarded-host.toml");
    fs::write(
        &config,
        format!(
            "default = \"trusted-host\"\nallowed_cwd_root = {source_root:?}\n\n\
             [registry]\nallowed_cmds = [{adapter:?}, {runtime:?}]\n\n\
             [[agents]]\nid = \"trusted-host\"\ncmd = {adapter:?}\nhost_fallback_eligible = true\n\n\
             [[agents]]\nid = \"reader-container\"\ncmd = {adapter:?}\n\n\
             [agents.sandbox]\nruntime = {runtime:?}\nimage = \"reader:latest\"\nmount = {source_root:?}\naccess = \"ro\"\negress = \"open\"\n\n\
             [server]\n"
        ),
    )
    .unwrap();
    let source = dir.path().join("guarded-host-source.json");
    write_smoke_artifact(&source, &repo, &config, "container_runtime", false);
    let plan = read_plan(
        &fallback_command(&config, &source)
            .arg("--confirm-trusted-own-repo-read-only")
            .output()
            .unwrap(),
    );
    let argv: Vec<String> = plan["rerun"]["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect();
    let smoke = Command::new(&argv[0]).args(&argv[1..]).output().unwrap();
    assert!(
        !smoke.status.success(),
        "fixture target intentionally exits"
    );
    let _: serde_json::Value = serde_json::from_slice(&smoke.stdout).unwrap();
    assert!(
        target_marker.exists(),
        "guarded host target should be reached"
    );
    assert_eq!(
        fs::read_to_string(target_cwd_marker).unwrap().trim(),
        fs::canonicalize(&repo).unwrap().to_string_lossy(),
        "guarded host adapter must start inside the pinned trusted repo object"
    );
    assert!(
        !runtime_marker.exists(),
        "guarded host smoke must not recover or sweep through the degraded runtime"
    );
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
    let mut artifact = smoke_artifact(&repo, &config, "container_runtime", false);
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
