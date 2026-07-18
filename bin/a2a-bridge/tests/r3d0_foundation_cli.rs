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

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
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

fn rotate_array_table_section(text: &str, header: &str, next_header: Option<&str>) -> String {
    let start = text.find(header).expect("array-table header must exist");
    let end = next_header
        .and_then(|next| text[start..].find(next).map(|offset| start + offset))
        .unwrap_or(text.len());
    let prefix = &text[..start];
    let suffix = &text[end..];
    let section = &text[start..end];
    let starts = section
        .match_indices(header)
        .map(|(offset, _)| offset)
        .collect::<Vec<_>>();
    assert!(starts.len() > 1, "section must have multiple blocks");
    let mut blocks = starts
        .iter()
        .enumerate()
        .map(|(index, start)| {
            let end = starts.get(index + 1).copied().unwrap_or(section.len());
            &section[*start..end]
        })
        .collect::<Vec<_>>();
    blocks.rotate_left(1);
    format!("{prefix}{}{suffix}", blocks.concat())
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
                "head_oid": {"algorithm": "sha1", "hex": "b".repeat(40)},
                "tree_oid": {"algorithm": "sha1", "hex": "c".repeat(40)},
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
            "requested_identity": {
                "model": "gpt-5.6-luna",
                "effort": {"kind": "text", "value": "low"},
                "mode": {"kind": "absent"}
            },
            "expected_effective_identity": {
                "model": "gpt-5.6-luna",
                "effort": {"kind": "text", "value": "low"},
                "mode": {"kind": "absent"}
            },
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

#[test]
fn r3d0_foundation_rejects_forbidden_config_before_environment_expansion() {
    let temp = tempfile::tempdir().unwrap();
    copy_foundation(temp.path());
    let path = temp.path().join("configs/codex-luna-host.toml");
    let config = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        format!(
            "{config}\n[delegation]\npeer_url = \"http://127.0.0.1:9\"\nauth = \"${{REAL_SECRET_ENV}}\"\n"
        ),
    )
    .unwrap();

    let output = compatibility_command()
        .arg("validate")
        .arg("--schedule-foundation")
        .arg(temp.path())
        .env_remove("REAL_SECRET_ENV")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown fields"), "stderr: {stderr}");
    assert!(!stderr.contains("REAL_SECRET_ENV"), "stderr: {stderr}");
}

#[test]
fn r3d0_foundation_rejects_config_row_cross_binding_mutations() {
    let mutations = [
        (
            "scheduled-cases.toml",
            "credential_env = \"OLLAMA_API_KEY\"",
            "credential_env = \"OTHER_API_KEY\"",
            "auth/pre-auth/API-key bindings disagree",
        ),
        (
            "scheduled-cases.toml",
            "adapter_family = \"@agentclientprotocol/codex-acp\"",
            "adapter_family = \"@agentclientprotocol/claude-agent-acp\"",
            "provider/adapter/command families disagree",
        ),
        (
            "scheduled-cases.toml",
            "resolution_case = \"codex-host-floating-current\"",
            "resolution_case = \"claude-host-floating-current\"",
            "semantic resolution recipe disagree",
        ),
        (
            "scheduled-cases.toml",
            "allowed_effects = [\"registry_read\", \"provider_prompt\"]",
            "allowed_effects = [\"provider_prompt\"]",
            "effect classes do not match",
        ),
        (
            "configs/codex-luna-host.toml",
            "cmd = \"codex-acp\"",
            "cmd = \"claude-agent-acp\"",
            "provider/adapter/command families disagree",
        ),
        (
            "configs/codex-luna-host.toml",
            "pre_authenticated = true",
            "pre_authenticated = false",
            "auth/pre-auth/API-key bindings disagree",
        ),
        (
            "configs/codex-luna-host.toml",
            r#"sandbox_mode=\"read-only\""#,
            r#"sandbox_mode=\"danger-full-access\""#,
            "command arguments contradict",
        ),
        (
            "configs/ollama-local.toml",
            "http://127.0.0.1:11434/v1",
            "https://unreviewed.example/v1",
            "provider endpoint contradicts",
        ),
        (
            "configs/codex-luna-reader.toml",
            "mount = \"/Users/wesleyjinks/code\"",
            "mount = \"/Users/wesleyjinks\"",
            "sandbox/mount/egress/proxy/credential-volume contract drifted",
        ),
        (
            "configs/codex-luna-reader.toml",
            "egress = \"locked\"",
            "egress = \"open\"",
            "sandbox/mount/egress/proxy/credential-volume contract drifted",
        ),
        (
            "configs/codex-luna-reader.toml",
            "proxy = \"http://a2a-egress-proxy:8888\"",
            "proxy = \"http://127.0.0.1:9999\"",
            "sandbox/mount/egress/proxy/credential-volume contract drifted",
        ),
        (
            "configs/codex-luna-reader.toml",
            "/Users/wesleyjinks/.config/a2a-creds/codex/auth.json:/root/.codex/auth.json",
            "/Users/wesleyjinks/.ssh/id_rsa:/root/.ssh/id_rsa",
            "sandbox/mount/egress/proxy/credential-volume contract drifted",
        ),
        (
            "configs/codex-luna-reader.toml",
            "allowed_cwd_root = \"/Users/wesleyjinks/code\"",
            "allowed_cwd_root = \"/Users/wesleyjinks\"",
            "sandbox/mount/egress/proxy/credential-volume contract drifted",
        ),
        (
            "configs/claude-haiku-host.toml",
            "addr = \"127.0.0.1:8080\"",
            "addr = \"0.0.0.0:8080\"",
            "inert loopback bridge server binding",
        ),
    ];
    for (relative, from, to, expected) in mutations {
        let temp = tempfile::tempdir().unwrap();
        copy_foundation(temp.path());
        let path = temp.path().join(relative);
        let original = fs::read_to_string(&path).unwrap();
        let changed = original.replacen(from, to, 1);
        assert_ne!(changed, original, "mutation did not apply: {relative}");
        fs::write(path, changed).unwrap();
        let output = validate_foundation(temp.path());
        assert!(
            !output.status.success(),
            "{relative} unexpectedly validated"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(expected), "{relative}: {stderr}");
    }
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
fn r3d0_semantic_bundle_ignores_comments_but_validates_recipe_constraints() {
    let baseline = validate_foundation(&compatibility_root());
    assert!(baseline.status.success());
    let baseline_bundle = bundle_sha256(&baseline);

    let comment_temp = tempfile::tempdir().unwrap();
    copy_foundation(comment_temp.path());
    for relative in [
        "scheduling-policy.toml",
        "characterization-profiles.toml",
        "scheduled-cases.toml",
        "manifest.toml",
        "floating-current.toml",
        "configs/codex-luna-host.toml",
    ] {
        let path = comment_temp.path().join(relative);
        let original = fs::read_to_string(&path).unwrap();
        fs::write(path, format!("# semantics-preserving comment\n{original}")).unwrap();
        let comments = validate_foundation(comment_temp.path());
        assert!(
            comments.status.success(),
            "comment in {relative} changed semantics: {}",
            String::from_utf8_lossy(&comments.stderr)
        );
        assert_eq!(bundle_sha256(&comments), baseline_bundle, "{relative}");
    }
    let comments = validate_foundation(comment_temp.path());
    assert!(
        comments.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&comments.stderr)
    );
    assert_eq!(bundle_sha256(&comments), baseline_bundle);

    let constraint_temp = tempfile::tempdir().unwrap();
    copy_foundation(constraint_temp.path());
    let recipe_path = constraint_temp.path().join("floating-current.toml");
    let recipe = fs::read_to_string(&recipe_path).unwrap().replacen(
        "timeout_secs = 900",
        "timeout_secs = 899",
        1,
    );
    fs::write(recipe_path, recipe).unwrap();
    let changed_constraint = validate_foundation(constraint_temp.path());
    assert!(!changed_constraint.status.success());
    assert!(
        String::from_utf8_lossy(&changed_constraint.stderr)
            .contains("characterization fingerprint mismatch"),
        "stderr: {}",
        String::from_utf8_lossy(&changed_constraint.stderr)
    );

    let malformed_temp = tempfile::tempdir().unwrap();
    copy_foundation(malformed_temp.path());
    let recipe_path = malformed_temp.path().join("floating-current.toml");
    let recipe = fs::read_to_string(&recipe_path).unwrap();
    fs::write(recipe_path, format!("this is not valid TOML\n{recipe}")).unwrap();
    let malformed = validate_foundation(malformed_temp.path());
    assert!(!malformed.status.success());
    assert!(
        String::from_utf8_lossy(&malformed.stderr).contains("floating recipes"),
        "stderr: {}",
        String::from_utf8_lossy(&malformed.stderr)
    );
}

#[test]
fn r3d0_semantic_bundle_ignores_set_and_row_order() {
    let baseline = validate_foundation(&compatibility_root());
    assert!(baseline.status.success());
    let baseline_bundle = bundle_sha256(&baseline);

    let temp = tempfile::tempdir().unwrap();
    copy_foundation(temp.path());

    let inventory_path = temp.path().join("characterization-profiles.toml");
    let inventory = fs::read_to_string(&inventory_path).unwrap();
    fs::write(
        &inventory_path,
        rotate_array_table_section(&inventory, "[[profiles]]", None),
    )
    .unwrap();

    let registry_path = temp.path().join("scheduled-cases.toml");
    let registry = fs::read_to_string(&registry_path).unwrap();
    fs::write(
        &registry_path,
        rotate_array_table_section(&registry, "[[cases]]", None),
    )
    .unwrap();

    let policy_path = temp.path().join("scheduling-policy.toml");
    let policy = fs::read_to_string(&policy_path).unwrap().replace(
        "allowed_triggers = [\"manual_characterization\", \"manual_compatibility\", \"daily\", \"scheduled_main\", \"test_merge\"]",
        "allowed_triggers = [\"test_merge\", \"scheduled_main\", \"daily\", \"manual_compatibility\", \"manual_characterization\"]",
    );
    fs::write(policy_path, policy).unwrap();

    let recipes_path = temp.path().join("floating-current.toml");
    let recipes = fs::read_to_string(&recipes_path).unwrap();
    let recipes = rotate_array_table_section(&recipes, "[[package_sets]]", Some("[[images]]"));
    let recipes = rotate_array_table_section(&recipes, "[[cases]]", None);
    fs::write(recipes_path, recipes).unwrap();

    let reordered = validate_foundation(temp.path());
    assert!(
        reordered.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&reordered.stderr)
    );
    assert_eq!(bundle_sha256(&reordered), baseline_bundle);
}

#[test]
fn r3d0_support_expected_status_changes_the_profile_identity() {
    let temp = tempfile::tempdir().unwrap();
    copy_foundation(temp.path());
    let manifest_path = temp.path().join("manifest.toml");
    let manifest = fs::read_to_string(&manifest_path).unwrap().replacen(
        "expected_status = \"PASS\"",
        "expected_status = \"FAIL\"",
        1,
    );
    fs::write(manifest_path, manifest).unwrap();
    let changed = validate_foundation(temp.path());
    assert!(!changed.status.success());
    assert!(
        String::from_utf8_lossy(&changed.stderr).contains("characterization fingerprint mismatch"),
        "stderr: {}",
        String::from_utf8_lossy(&changed.stderr)
    );
}

#[test]
fn r3d0_claimed_support_config_bytes_must_match_the_manifest_pin() {
    let temp = tempfile::tempdir().unwrap();
    copy_foundation(temp.path());
    let config_path = temp.path().join("configs/codex-host.toml");
    let config = fs::read_to_string(&config_path).unwrap();
    fs::write(config_path, format!("# changed execution bytes\n{config}")).unwrap();
    let changed = validate_foundation(temp.path());
    assert!(!changed.status.success());
    assert!(
        String::from_utf8_lossy(&changed.stderr).contains("exact manifest pin"),
        "stderr: {}",
        String::from_utf8_lossy(&changed.stderr)
    );
}

#[test]
fn r3d0_claimed_support_pin_update_cannot_bypass_reviewed_effect_semantics() {
    let temp = tempfile::tempdir().unwrap();
    copy_foundation(temp.path());
    let config_path = temp.path().join("configs/codex-host.toml");
    let original = fs::read(&config_path).unwrap();
    let original_text = std::str::from_utf8(&original).unwrap();
    let changed_text = original_text.replacen(
        r#"sandbox_mode=\"read-only\""#,
        r#"sandbox_mode=\"danger-full-access\""#,
        1,
    );
    assert_ne!(changed_text, original_text);
    fs::write(&config_path, changed_text.as_bytes()).unwrap();

    let manifest_path = temp.path().join("manifest.toml");
    let manifest = fs::read_to_string(&manifest_path).unwrap();
    let manifest = manifest.replacen(
        &sha256_hex(&original),
        &sha256_hex(changed_text.as_bytes()),
        1,
    );
    fs::write(manifest_path, manifest).unwrap();

    let output = validate_foundation(temp.path());
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("command arguments contradict"), "{stderr}");
}

#[test]
fn r3d0_foundation_rejects_secret_shaped_comments() {
    for (relative, comment) in [
        (
            "scheduling-policy.toml",
            "# accidental sk-not-a-real-secret",
        ),
        ("configs/codex-luna-host.toml", "# password: hunter2"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        copy_foundation(temp.path());
        let path = temp.path().join(relative);
        let original = fs::read_to_string(&path).unwrap();
        fs::write(path, format!("{comment}\n{original}")).unwrap();

        let output = validate_foundation(temp.path());
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("secret-shaped"),
            "{relative} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
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
fn r3d0_policy_rejects_duplicate_deferred_profile_records() {
    let temp = tempfile::tempdir().unwrap();
    copy_foundation(temp.path());
    let path = temp.path().join("scheduling-policy.toml");
    let original = fs::read_to_string(&path).unwrap();
    let row = "  \"openrouter: R3e not implemented\",\n";
    let changed = original.replacen(row, &format!("{row}{row}"), 1);
    assert_ne!(changed, original);
    fs::write(path, changed).unwrap();

    let output = validate_foundation(temp.path());
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("deferred profile records must be unique"),
        "{stderr}"
    );
}

#[test]
fn r3d0_case_and_admission_fingerprint_records_are_supported() {
    let temp = tempfile::tempdir().unwrap();
    for (kind, value) in [
        (
            "case-execution-fingerprint",
            case_execution_record_json(
                "8803c9e9a6cec36583ec16be1854daf3f4703d2aa5efe32a0e02112165ecd13a",
            ),
        ),
        (
            "admission-attempt-fingerprint",
            admission_attempt_record_json(
                "c4ebb80360bd7e28014d8aefac9416ae42df0f5e8ddf03c1d063206cf58e9b02",
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
    let case_hash = "b54b484b884757815a60772d91da7d5696f2d03a9b7ad8d2eff23f485eeb6c12";

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

#[test]
fn r3d0_schedule_records_reject_invalid_identity_and_generation_fields() {
    let temp = tempfile::tempdir().unwrap();
    let records = [
        (
            "publication-outbox",
            serde_json::json!({
                "schema_version": 1,
                "outbox_id": "outbox-1",
                "state": "create_intent",
                "repository": "",
                "pull_request": 37,
                "test_merge_oid": {"algorithm": "sha1", "hex": "a".repeat(40)},
                "context": "a2a-bridge/r3d",
                "app_id": "app-1",
                "external_id": "external-1",
                "check_run": {"kind": "absent"},
                "terminal_consumption": {"kind": "absent"},
                "desired_conclusion": {"kind": "absent"},
                "evidence_set": {"kind": "absent"},
                "final_guard": {"kind": "absent"},
                "remote_observation": {"kind": "absent"},
                "remote_observation_attempts": 0
            }),
        ),
        (
            "quarantine",
            serde_json::json!({
                "state": "open",
                "schema_version": 1,
                "quarantine_id": "quarantine-1",
                "profile": fingerprint_json('a'),
                "operator": "",
                "reason": "owner requested quarantine",
                "created_at_ms": 1,
                "expires_at_ms": 2
            }),
        ),
        (
            "status",
            serde_json::json!({
                "schema_version": 1,
                "generated_at_ms": 1,
                "policy_sha256": digest('a'),
                "last_window": {"kind": "absent"},
                "next_window": {"kind": "absent"},
                "provider_grant": {"kind": "absent"},
                "storage_consent": {"kind": "absent"},
                "ledger_headroom_sha256": digest('b'),
                "storage_state": "hot_only",
                "missed_ticks": 0,
                "fresh_one_shot_compatibility": "unknown",
                "shared_operator_health": "not_evaluated",
                "cases": [{
                    "case_id": "case-1",
                    "lifecycle": "scheduled_active",
                    "last_outcome": {"kind": "text", "value": ""},
                    "hold": {"kind": "absent"},
                    "quarantine": {"kind": "absent"}
                }]
            }),
        ),
        (
            "evidence-index",
            serde_json::json!({
                "schema_version": 1,
                "index_id": "index-1",
                "generation": 0,
                "hot_root_sha256": digest('a'),
                "cold_storage": {"kind": "absent"},
                "entries": []
            }),
        ),
    ];

    for (kind, record) in records {
        let path = temp.path().join(format!("invalid-{kind}.json"));
        fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        let output = compatibility_command()
            .arg("validate")
            .arg("--schedule-record")
            .arg(kind)
            .arg(path)
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "{kind} unexpectedly passed: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    let valid_index_path = temp.path().join("valid-evidence-index.json");
    fs::write(
        &valid_index_path,
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "index_id": "index-1",
            "generation": 1,
            "hot_root_sha256": digest('a'),
            "cold_storage": {"kind": "absent"},
            "entries": []
        }))
        .unwrap(),
    )
    .unwrap();
    let valid_index = compatibility_command()
        .arg("validate")
        .arg("--schedule-record")
        .arg("evidence-index")
        .arg(valid_index_path)
        .output()
        .unwrap();
    assert!(
        valid_index.status.success(),
        "valid evidence index failed: {}",
        String::from_utf8_lossy(&valid_index.stderr)
    );
}
