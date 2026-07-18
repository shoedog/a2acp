use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn compatibility_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../compatibility")
}

fn compatibility_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"));
    command.arg("compatibility");
    command
}

fn digest(ch: char) -> String {
    ch.to_string().repeat(64)
}

fn copy_foundation(destination: &Path) {
    let source = compatibility_root();
    for name in [
        "scheduling-policy.toml",
        "characterization-profiles.toml",
        "scheduled-cases.toml",
        "manifest.toml",
        "floating-current.toml",
    ] {
        fs::copy(source.join(name), destination.join(name)).unwrap();
    }
    fs::create_dir(destination.join("configs")).unwrap();
    for entry in fs::read_dir(source.join("configs")).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            fs::copy(
                entry.path(),
                destination.join("configs").join(entry.file_name()),
            )
            .unwrap();
        }
    }
}

fn validate_foundation(root: &Path) -> std::process::Output {
    compatibility_command()
        .arg("validate")
        .arg("--schedule-foundation")
        .arg(root)
        .output()
        .unwrap()
}

fn bundle_sha256(output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .split("profile-policy-bundle sha256 ")
        .nth(1)
        .and_then(|tail| tail.strip_suffix(")\n"))
        .unwrap_or_else(|| panic!("missing bundle hash in stdout: {stdout}"))
        .to_owned()
}

fn caps_json() -> serde_json::Value {
    serde_json::json!({
        "timeout_secs": 180,
        "max_tokens": 1000,
        "max_cost_microusd": 1000,
        "attempts": 1,
        "retry_cap": 0,
        "fallback_cap": 0
    })
}

fn fingerprint_json(ch: char) -> serde_json::Value {
    serde_json::json!({"schema_version": 1, "sha256": digest(ch)})
}

fn case_execution_record_json(fingerprint: &str) -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "input": {
            "schema_version": 1,
            "characterization_profile": fingerprint_json('a'),
            "target": {
                "kind": "repository_snapshot",
                "repository": "shoedog/a2acp",
                "head_sha256": digest('b'),
                "tree_sha256": digest('c'),
                "range_start_exclusive": {"kind": "absent"}
            },
            "candidate": {
                "sha256": digest('d'),
                "length_bytes": 1,
                "build_provenance_sha256": digest('e')
            },
            "bindings": {
                "source_sha256": digest('f'),
                "row_sha256": digest('1'),
                "run_manifest_sha256": digest('2'),
                "generated_config_sha256": digest('3'),
                "pin_set_sha256": digest('4'),
                "resolution_bundle": {"kind": "absent"},
                "package_integrity_sha256": digest('5'),
                "image_digest": {"kind": "absent"},
                "base_image_digest": {"kind": "absent"},
                "environment_sha256": digest('6'),
                "prerequisites_sha256": digest('7')
            },
            "requested_identity": {"model": "gpt-5.6-luna", "effort": "low"},
            "expected_effective_identity": {"model": "gpt-5.6-luna", "effort": "low"},
            "actual_caps": caps_json()
        },
        "fingerprint": {"schema_version": 1, "sha256": fingerprint}
    })
}

fn admission_attempt_record_json(fingerprint: &str) -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "input": {
            "schema_version": 1,
            "characterization_profile": fingerprint_json('a'),
            "case_execution": fingerprint_json('b'),
            "authority": {
                "kind": "standing_grant",
                "grant_id": "grant-1",
                "generation": 1,
                "grant_sha256": digest('c'),
                "characterization_id": "characterization-1",
                "characterization_sha256": digest('d')
            },
            "trigger": {
                "source": "daily_launchd",
                "kind": "daily",
                "request_id": "request-1",
                "window_id": "window-1",
                "attempt_id": "attempt-1",
                "repeat_nonce": {"kind": "absent"}
            }
        },
        "fingerprint": {"schema_version": 1, "sha256": fingerprint}
    })
}

#[test]
fn r3d0_checked_in_schedule_foundation_validates_without_effects() {
    let output = compatibility_command()
        .arg("validate")
        .arg("--schedule-foundation")
        .arg(compatibility_root())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("schedule foundation valid"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("scheduled advisory profiles"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("claimed-support profiles"),
        "stdout: {stdout}"
    );
}

#[test]
fn r3d0_schedule_foundation_rejects_unknown_schema_fields() {
    let temp = tempfile::tempdir().unwrap();
    copy_foundation(temp.path());

    let cases_path = temp.path().join("scheduled-cases.toml");
    let mut cases = fs::read_to_string(&cases_path).unwrap();
    cases.push_str("\ncaller_prompt = \"must never be accepted\"\n");
    fs::write(&cases_path, cases).unwrap();

    let output = compatibility_command()
        .arg("validate")
        .arg("--schedule-foundation")
        .arg(temp.path())
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unknown field"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn r3d0_schedule_foundation_rejects_hidden_config_behavior() {
    let temp = tempfile::tempdir().unwrap();
    copy_foundation(temp.path());
    let path = temp.path().join("configs/codex-luna-host.toml");
    let config = fs::read_to_string(&path).unwrap();
    fs::write(&path, format!("caller_prompt = \"not allowed\"\n{config}")).unwrap();

    let output = validate_foundation(temp.path());
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unknown fields"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn r3d0_schedule_foundation_rejects_parent_symlink_escape() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    copy_foundation(temp.path());
    let configs = temp.path().join("configs");
    fs::remove_dir_all(&configs).unwrap();
    symlink(compatibility_root().join("configs"), &configs).unwrap();

    let output = validate_foundation(temp.path());
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("escapes the foundation root"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn r3d0_execution_only_pins_do_not_change_profile_policy_bundle() {
    let baseline = validate_foundation(&compatibility_root());
    assert!(baseline.status.success());
    let baseline_bundle = bundle_sha256(&baseline);

    let package_temp = tempfile::tempdir().unwrap();
    copy_foundation(package_temp.path());
    let manifest_path = package_temp.path().join("manifest.toml");
    let manifest = fs::read_to_string(&manifest_path)
        .unwrap()
        .replace("@openai/codex=0.144.1", "@openai/codex=0.144.2");
    fs::write(&manifest_path, manifest).unwrap();
    let package_result = validate_foundation(package_temp.path());
    assert!(
        package_result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&package_result.stderr)
    );
    assert_eq!(bundle_sha256(&package_result), baseline_bundle);

    let image_temp = tempfile::tempdir().unwrap();
    copy_foundation(image_temp.path());
    let config_path = image_temp.path().join("configs/codex-luna-reader.toml");
    let original = fs::read_to_string(&config_path).unwrap();
    let config = original.replace(
        "sha256:b154aefda301a59a11857700debe826a282dc6e07b76a0ebb46dd6a8e55a03f1",
        &format!("sha256:{}", digest('c')),
    );
    assert_ne!(config, original);
    fs::write(&config_path, config).unwrap();
    let image_result = validate_foundation(image_temp.path());
    assert!(
        image_result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&image_result.stderr)
    );
    assert_eq!(bundle_sha256(&image_result), baseline_bundle);
}

#[test]
fn r3d0_profile_field_change_invalidates_the_checked_in_inventory() {
    let temp = tempfile::tempdir().unwrap();
    copy_foundation(temp.path());
    let registry_path = temp.path().join("scheduled-cases.toml");
    let registry = fs::read_to_string(&registry_path)
        .unwrap()
        .replacen(
            "model = \"gpt-5.6-luna\"",
            "model = \"gpt-5.6-luna-new\"",
            1,
        )
        .replacen(
            "expected_effective_model = \"gpt-5.6-luna\"",
            "expected_effective_model = \"gpt-5.6-luna-new\"",
            1,
        );
    fs::write(&registry_path, registry).unwrap();
    let config_path = temp.path().join("configs/codex-luna-host.toml");
    let config = fs::read_to_string(&config_path)
        .unwrap()
        .replace("model = \"gpt-5.6-luna\"", "model = \"gpt-5.6-luna-new\"");
    fs::write(config_path, config).unwrap();

    let output = validate_foundation(temp.path());
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("characterization fingerprint mismatch"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn r3d0_characterization_inventory_rejects_omitted_and_duplicate_profiles() {
    let omitted_temp = tempfile::tempdir().unwrap();
    copy_foundation(omitted_temp.path());
    let omitted_path = omitted_temp.path().join("characterization-profiles.toml");
    let inventory = fs::read_to_string(&omitted_path).unwrap();
    let first = inventory.find("[[profiles]]").unwrap();
    let second = inventory[first + 1..].find("[[profiles]]").unwrap() + first + 1;
    let mut omitted = inventory.clone();
    omitted.replace_range(first..second, "");
    fs::write(&omitted_path, omitted).unwrap();

    let output = validate_foundation(omitted_temp.path());
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("characterization inventory mismatch"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let duplicate_temp = tempfile::tempdir().unwrap();
    copy_foundation(duplicate_temp.path());
    let duplicate_path = duplicate_temp.path().join("characterization-profiles.toml");
    let mut duplicate = inventory.clone();
    duplicate.push_str(&inventory[first..second]);
    fs::write(&duplicate_path, duplicate).unwrap();

    let output = validate_foundation(duplicate_temp.path());
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("duplicate inventory id"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn r3d0_case_and_admission_fingerprint_records_are_supported() {
    let temp = tempfile::tempdir().unwrap();
    for (kind, value) in [
        (
            "case-execution-fingerprint",
            case_execution_record_json(
                "8c3aecf5677b69493a24db7693ddffb4fe7485fb8569b72dfd5ad8b7c8fcb379",
            ),
        ),
        (
            "admission-attempt-fingerprint",
            admission_attempt_record_json(
                "f3b351b9ee199f754755a65efe8562d075cc942f638dd37c975eba473c3cc127",
            ),
        ),
    ] {
        let path = temp.path().join(format!("{kind}.json"));
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let output = compatibility_command()
            .arg("validate")
            .arg("--schedule-record")
            .arg(kind)
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{kind} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn r3d0_fingerprint_layers_reject_drift_and_cross_layer_fields() {
    let temp = tempfile::tempdir().unwrap();
    let case_hash = "8c3aecf5677b69493a24db7693ddffb4fe7485fb8569b72dfd5ad8b7c8fcb379";

    let mut trigger_leak = case_execution_record_json(case_hash);
    trigger_leak["input"]["trigger"] = serde_json::json!({"kind": "daily"});
    let trigger_leak_path = temp.path().join("trigger-leak.json");
    fs::write(
        &trigger_leak_path,
        serde_json::to_vec(&trigger_leak).unwrap(),
    )
    .unwrap();
    let output = compatibility_command()
        .arg("validate")
        .arg("--schedule-record")
        .arg("case-execution-fingerprint")
        .arg(&trigger_leak_path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unknown field"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut execution_drift = case_execution_record_json(case_hash);
    execution_drift["input"]["candidate"]["sha256"] = serde_json::json!(digest('9'));
    let execution_drift_path = temp.path().join("execution-drift.json");
    fs::write(
        &execution_drift_path,
        serde_json::to_vec(&execution_drift).unwrap(),
    )
    .unwrap();
    let output = compatibility_command()
        .arg("validate")
        .arg("--schedule-record")
        .arg("case-execution-fingerprint")
        .arg(&execution_drift_path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("fingerprint mismatch"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut wrong_authority = admission_attempt_record_json(
        "f3b351b9ee199f754755a65efe8562d075cc942f638dd37c975eba473c3cc127",
    );
    wrong_authority["input"]["trigger"]["source"] =
        serde_json::json!("manual_characterization_cli");
    wrong_authority["input"]["trigger"]["kind"] = serde_json::json!("manual_characterization");
    let wrong_authority_path = temp.path().join("wrong-authority.json");
    fs::write(
        &wrong_authority_path,
        serde_json::to_vec(&wrong_authority).unwrap(),
    )
    .unwrap();
    let output = compatibility_command()
        .arg("validate")
        .arg("--schedule-record")
        .arg("admission-attempt-fingerprint")
        .arg(&wrong_authority_path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("authority does not match"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn r3d0_schedule_record_validates_a_strict_failure_disposition() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("failure-disposition.json");
    fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "characterization_profile": {"schema_version": 1, "sha256": digest('b')},
            "case_execution": {"schema_version": 1, "sha256": digest('a')},
            "evidence_sha256": digest('c'),
            "failure_kind": "typed_transient",
            "typed_code": "provider.timeout",
            "identical_complete_occurrences": 1,
            "action": "confirmation_due",
            "first_seen_ms": 1,
            "last_seen_ms": 1
        }))
        .unwrap(),
    )
    .unwrap();

    let output = compatibility_command()
        .arg("validate")
        .arg("--schedule-record")
        .arg("failure-disposition")
        .arg(&path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "schedule record valid: failure-disposition\n"
    );
}

#[test]
fn r3d0_schedule_record_rejects_prompt_material_and_wrong_suppression() {
    let temp = tempfile::tempdir().unwrap();
    let unknown_field_path = temp.path().join("unknown-field.json");
    fs::write(
        &unknown_field_path,
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "characterization_profile": {"schema_version": 1, "sha256": digest('b')},
            "case_execution": {"schema_version": 1, "sha256": digest('a')},
            "evidence_sha256": digest('c'),
            "failure_kind": "candidate_unknown",
            "typed_code": "catalog.unavailable",
            "identical_complete_occurrences": 1,
            "action": "unknown_retained",
            "first_seen_ms": 1,
            "last_seen_ms": 1,
            "caller_prompt": "must never be persisted"
        }))
        .unwrap(),
    )
    .unwrap();
    let unknown_field = compatibility_command()
        .arg("validate")
        .arg("--schedule-record")
        .arg("failure-disposition")
        .arg(&unknown_field_path)
        .output()
        .unwrap();
    assert!(!unknown_field.status.success());
    assert!(
        String::from_utf8_lossy(&unknown_field.stderr).contains("unknown field"),
        "stderr: {}",
        String::from_utf8_lossy(&unknown_field.stderr)
    );

    let premature_suppression_path = temp.path().join("premature-suppression.json");
    fs::write(
        &premature_suppression_path,
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "characterization_profile": {"schema_version": 1, "sha256": digest('a')},
            "case_execution": {"schema_version": 1, "sha256": digest('b')},
            "evidence_sha256": digest('c'),
            "failure_kind": "typed_transient",
            "typed_code": "provider.timeout",
            "identical_complete_occurrences": 1,
            "action": "suppressed",
            "first_seen_ms": 1,
            "last_seen_ms": 1
        }))
        .unwrap(),
    )
    .unwrap();
    let premature_suppression = compatibility_command()
        .arg("validate")
        .arg("--schedule-record")
        .arg("failure-disposition")
        .arg(&premature_suppression_path)
        .output()
        .unwrap();
    assert!(!premature_suppression.status.success());
    assert!(
        String::from_utf8_lossy(&premature_suppression.stderr)
            .contains("confirmation and suppression policy"),
        "stderr: {}",
        String::from_utf8_lossy(&premature_suppression.stderr)
    );
}
