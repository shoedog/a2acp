#![recursion_limit = "256"]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
fn top_level_help_discovers_the_read_only_schedule_status_surface() {
    let output = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("schedule status [--json]"), "{stdout}");
}

#[test]
fn schedule_status_ignores_redirected_home_and_leaves_it_unchanged() {
    let home = tempfile::tempdir().unwrap();
    let before = fs::read_dir(home.path()).unwrap().count();

    let human = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .arg("compatibility")
        .arg("schedule")
        .arg("status")
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(human.status.success());
    let human_stdout = String::from_utf8_lossy(&human.stdout);
    assert!(human_stdout.contains("state: "));
    assert!(human_stdout.contains("activation: r3d5_activation_not_enabled"));
    assert!(human_stdout.contains("effects: no_effects"));
    assert!(human.stderr.is_empty());

    let json = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .arg("compatibility")
        .arg("schedule")
        .arg("status")
        .arg("--json")
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(json.status.success());
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(value["activation"], "r3d5_activation_not_enabled");
    assert_eq!(value["effects"], "no_effects");
    assert!(json.stderr.is_empty());
    assert_eq!(fs::read_dir(home.path()).unwrap().count(), before);
}

#[test]
fn schedule_status_rejects_unknown_or_duplicate_flags_without_writes() {
    let home = tempfile::tempdir().unwrap();
    let before = fs::read_dir(home.path()).unwrap().count();
    for args in [
        vec!["schedule", "status", "--write"],
        vec!["schedule", "status", "--json", "--json"],
        vec!["schedule", "unknown"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
            .arg("compatibility")
            .args(args)
            .env("HOME", home.path())
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("compatibility schedule"));
    }
    assert_eq!(fs::read_dir(home.path()).unwrap().count(), before);
}

#[test]
fn schedule_tick_is_recognized_but_refuses_before_provider_capable_spawn() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("provider-spawned");
    let trap = directory.path().join("codex");
    fs::write(&trap, format!("#!/bin/sh\n: > {:?}\nexit 99\n", marker)).unwrap();
    fs::set_permissions(&trap, fs::Permissions::from_mode(0o700)).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .arg("compatibility")
        .arg("schedule-tick")
        .env("PATH", directory.path())
        .env("OPENAI_API_KEY", "must-not-be-read")
        .env("ANTHROPIC_API_KEY", "must-not-be-read")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("r3d5_activation_not_enabled"));
    assert!(stderr.contains("no_effects"));
    assert!(!stderr.contains("must-not-be-read"));
    assert!(!marker.exists(), "schedule-tick spawned the provider trap");
}

#[test]
fn schedule_tick_rejects_all_source_arguments_without_inspecting_them() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("provider-spawned");
    let trap = directory.path().join("codex");
    fs::write(&trap, format!("#!/bin/sh\n: > {:?}\nexit 99\n", marker)).unwrap();
    fs::set_permissions(&trap, fs::Permissions::from_mode(0o700)).unwrap();
    let untrusted_source = directory.path().join("must-not-be-read-or-reported");

    let output = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .arg("compatibility")
        .arg("schedule-tick")
        .arg("--source")
        .arg(&untrusted_source)
        .env("PATH", directory.path())
        .env("OPENAI_API_KEY", "must-not-be-read")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("r3d5_activation_not_enabled"));
    assert!(stderr.contains("arguments are disabled"));
    assert!(stderr.contains("no_effects"));
    assert!(!stderr.contains("must-not-be-read-or-reported"));
    assert!(!stderr.contains("must-not-be-read"));
    assert!(!marker.exists(), "schedule-tick spawned the provider trap");
}

fn write_manifest(dir: &Path) -> PathBuf {
    let manifest = dir.join("manifest.toml");
    fs::write(
        &manifest,
        format!(
            r#"schema_version = 1

[budget]
timeout_secs = 30
max_tokens = 100000
max_cost_usd = 1.0

[[cases]]
id = "missing-config-control"
lane = "floating-current"
evidence_path = "bridge_smoke"
execution_mode = "host"
os = {os:?}
architecture = {arch:?}
environment_owner = "test-runner"
config = "missing.toml"
agent = "missing-agent"
model = "test-model"
auth_path = "automatic"
required_env = []
probe = "minimal"
billable = true
timeout_secs = 1
max_tokens = 1000
max_cost_usd = 0.01
retry_cap = 0
expected_status = "PASS"
classification = "canary"
baseline_case = "pinned-support-control"

[cases.artifact]
retention_days = 1
redaction = "strict"

[cases.resolved]
resolution_id = "resolution-1"
recipe_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
config_sha256 = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
adapter = "@agentclientprotocol/codex-acp=1.2.3"
agent_cli = "@openai/codex=0.150.0"
package_inventory_sha256 = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
package_tree_sha256 = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
"#,
            os = std::env::consts::OS,
            arch = std::env::consts::ARCH,
        ),
    )
    .unwrap();
    manifest
}

fn write_two_case_manifest(dir: &Path) -> PathBuf {
    let manifest = write_manifest(dir);
    let first = fs::read_to_string(&manifest).unwrap();
    let second = first
        .split("[[cases]]")
        .nth(1)
        .expect("fixture contains one case")
        .replace(
            "id = \"missing-config-control\"",
            "id = \"second-missing-config-control\"",
        )
        .replace(
            "baseline_case = \"pinned-support-control\"",
            "baseline_case = \"second-pinned-support-control\"",
        );
    fs::write(&manifest, format!("{first}\n[[cases]]{second}")).unwrap();
    manifest
}

fn write_unresolved_manifest(dir: &Path) -> PathBuf {
    let manifest = write_manifest(dir);
    let raw = fs::read_to_string(&manifest).unwrap();
    let unresolved = raw.split("\n[cases.resolved]").next().unwrap();
    fs::write(&manifest, unresolved).unwrap();
    manifest
}

fn sha256_hex(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn write_pinned_manifest(dir: &Path, config_sha256: &str) -> PathBuf {
    let manifest = write_manifest(dir);
    let raw = fs::read_to_string(&manifest).unwrap();
    let pinned = raw
        .split("\n[cases.resolved]")
        .next()
        .unwrap()
        .replace("lane = \"floating-current\"", "lane = \"pinned\"")
        .replace(
            "classification = \"canary\"",
            "classification = \"non_goal\"",
        )
        .replace("baseline_case = \"pinned-support-control\"\n", "");
    fs::write(
        &manifest,
        format!(
            "{pinned}\n[cases.pins]\nconfig_sha256 = {config_sha256:?}\nmodel = \"test-model\"\nadapter = \"test-adapter=1.2.3\"\nagent_cli = \"test-cli=4.5.6\"\n"
        ),
    )
    .unwrap();
    manifest
}

fn write_recipes(dir: &Path) -> PathBuf {
    let compatibility = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../compatibility");
    let manifest = fs::read_to_string(compatibility.join("manifest.toml"))
        .unwrap()
        .replacen(
            "os = \"macos\"",
            &format!("os = {:?}", std::env::consts::OS),
            1,
        )
        .replacen(
            "architecture = \"aarch64\"",
            &format!("architecture = {:?}", std::env::consts::ARCH),
            1,
        )
        .replacen(
            "environment_owner = \"wesley-macbook\"",
            "environment_owner = \"fixture-owner\"",
            1,
        );
    fs::write(dir.join("manifest.toml"), manifest).unwrap();
    fs::create_dir(dir.join("configs")).unwrap();
    fs::copy(
        compatibility.join("configs/codex-host.toml"),
        dir.join("configs/codex-host.toml"),
    )
    .unwrap();
    let recipes = dir.join("floating-current.toml");
    fs::write(
        &recipes,
        r#"schema_version = 1
production_manifest = "manifest.toml"

[limits]
timeout_secs = 900
max_download_bytes = 536870912
max_unpacked_bytes = 1073741824
max_files = 100000

[artifact]
retention_days = 30
redaction = "strict"

[[package_sets]]
id = "codex-current"
ecosystem = "npm"
registry = "npmjs"
adapter = "@agentclientprotocol/codex-acp"
adapter_selector = "latest"
agent_cli = "@openai/codex"

[[cases]]
id = "codex-host-floating-current"
baseline_case = "codex-host-bridge-gpt56-sol"
package_set = "codex-current"
target = "host-package-tree"
config_template = "codex-host-read-only-v1"
"#,
    )
    .unwrap();
    recipes
}

fn write_bound_host_resolution(dir: &Path) -> (PathBuf, PathBuf) {
    let recipes_path = write_recipes(dir);
    let production_path = dir.join("manifest.toml");
    let production_raw = fs::read_to_string(&production_path).unwrap();
    let production: toml::Value = toml::from_str(&production_raw).unwrap();
    let budget = production.get("budget").unwrap().clone();
    let baseline = production
        .get("cases")
        .and_then(toml::Value::as_array)
        .unwrap()
        .iter()
        .find(|case| {
            case.get("id").and_then(toml::Value::as_str) == Some("codex-host-bridge-gpt56-sol")
        })
        .unwrap()
        .as_table()
        .unwrap()
        .clone();
    let pins = baseline
        .get("pins")
        .and_then(toml::Value::as_table)
        .unwrap();
    let adapter = pins.get("adapter").and_then(toml::Value::as_str).unwrap();
    let agent_cli = pins.get("agent_cli").and_then(toml::Value::as_str).unwrap();
    let (adapter_name, adapter_version) = adapter.split_once('=').unwrap();
    let (agent_cli_name, agent_cli_version) = agent_cli.split_once('=').unwrap();

    let bundle = dir.join("bundle");
    fs::create_dir(&bundle).unwrap();
    fs::set_permissions(&bundle, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(bundle.join("configs")).unwrap();
    let generated_config = bundle.join("configs/codex-host-floating-current.toml");
    fs::write(&generated_config, b"generated config before drift\n").unwrap();
    let config_sha256 = sha256_hex(&fs::read(&generated_config).unwrap());
    let recipe_sha256 = sha256_hex(&fs::read(&recipes_path).unwrap());
    let production_sha256 = sha256_hex(production_raw.as_bytes());
    let resolution_id = "resolution-cli-1";
    let inventory_sha256 = "1".repeat(64);
    let tree_sha256 = "2".repeat(64);

    let mut generated = baseline.clone();
    generated.insert(
        "id".into(),
        toml::Value::String("codex-host-floating-current".into()),
    );
    generated.insert(
        "lane".into(),
        toml::Value::String("floating-current".into()),
    );
    generated.insert(
        "classification".into(),
        toml::Value::String("canary".into()),
    );
    generated.insert(
        "baseline_case".into(),
        toml::Value::String("codex-host-bridge-gpt56-sol".into()),
    );
    generated.insert(
        "config".into(),
        toml::Value::String("configs/codex-host-floating-current.toml".into()),
    );
    generated.remove("pins");
    generated.insert(
        "resolved".into(),
        toml::Value::Table(toml::map::Map::from_iter([
            (
                "resolution_id".into(),
                toml::Value::String(resolution_id.into()),
            ),
            (
                "recipe_sha256".into(),
                toml::Value::String(recipe_sha256.clone()),
            ),
            (
                "config_sha256".into(),
                toml::Value::String(config_sha256.clone()),
            ),
            ("adapter".into(), toml::Value::String(adapter.into())),
            ("agent_cli".into(), toml::Value::String(agent_cli.into())),
            (
                "package_inventory_sha256".into(),
                toml::Value::String(inventory_sha256.clone()),
            ),
            (
                "package_tree_sha256".into(),
                toml::Value::String(tree_sha256.clone()),
            ),
        ])),
    );
    let execution = toml::Value::Table(toml::map::Map::from_iter([
        ("schema_version".into(), toml::Value::Integer(1)),
        ("budget".into(), budget),
        (
            "cases".into(),
            toml::Value::Array(vec![toml::Value::Table(generated)]),
        ),
    ]));
    let execution_path = bundle.join("execution-manifest.toml");
    fs::write(&execution_path, toml::to_string_pretty(&execution).unwrap()).unwrap();
    let execution_sha256 = sha256_hex(&fs::read(&execution_path).unwrap());

    let candidate = fs::canonicalize(env!("CARGO_BIN_EXE_a2a-bridge")).unwrap();
    let candidate_bytes = fs::read(&candidate).unwrap();
    let candidate_sha256 = sha256_hex(&candidate_bytes);
    let canonical_bundle = fs::canonicalize(&bundle).unwrap();
    let canonical_recipes = fs::canonicalize(&recipes_path).unwrap();
    let canonical_production = fs::canonicalize(&production_path).unwrap();
    let canonical_execution = fs::canonicalize(&execution_path).unwrap();
    let canonical_config = fs::canonicalize(&generated_config).unwrap();
    let prerequisites: Vec<_> = baseline
        .get("required_env")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .map(|required| {
            serde_json::json!({
                "name": required.get("name").and_then(toml::Value::as_str).unwrap()
            })
        })
        .collect();
    let integrity = format!("sha512-{}==", "A".repeat(86));
    let resolution = serde_json::json!({
        "schema_version": 1,
        "state": "complete",
        "resolution_id": resolution_id,
        "recipes": {
            "schema_version": 1,
            "canonical_path": canonical_recipes,
            "sha256": recipe_sha256
        },
        "production_manifest": {
            "schema_version": 1,
            "canonical_path": canonical_production,
            "sha256": production_sha256
        },
        "candidate": {
            "canonical_path": candidate,
            "sha256": candidate_sha256,
            "byte_length": candidate_bytes.len()
        },
        "environment": {
            "environment_owner": "fixture-owner",
            "os": std::env::consts::OS,
            "architecture": std::env::consts::ARCH,
            "runtime": "docker",
            "runtime_executable": {
                "canonical_path": candidate,
                "sha256": candidate_sha256,
                "byte_length": candidate_bytes.len()
            }
        },
        "limits": {
            "timeout_secs": 900,
            "max_download_bytes": 536870912_u64,
            "max_unpacked_bytes": 1073741824_u64,
            "max_files": 100000_u64
        },
        "execution_manifest": {
            "schema_version": 1,
            "canonical_path": canonical_execution,
            "sha256": execution_sha256
        },
        "packages": [{
            "id": "codex-current",
            "requested": {
                "adapter": adapter_name,
                "adapter_selector": "latest",
                "agent_cli": agent_cli_name
            },
            "adapter": {"name": adapter_name, "version": adapter_version, "integrity": integrity},
            "agent_cli": {"name": agent_cli_name, "version": agent_cli_version, "integrity": integrity},
            "resolution_lock_sha256": "3".repeat(64),
            "inventory_sha256": inventory_sha256,
            "tree_sha256": tree_sha256,
            "adapter_executable": {
                "canonical_path": canonical_bundle.join("packages/codex-current/tree/adapter"),
                "sha256": "4".repeat(64)
            },
            "adapter_executable_relative": "adapter"
        }],
        "images": [],
        "cases": [{
            "id": "codex-host-floating-current",
            "baseline_case": "codex-host-bridge-gpt56-sol",
            "package_set": "codex-current",
            "model": baseline.get("model").and_then(toml::Value::as_str).unwrap(),
            "effort": baseline.get("effort").and_then(toml::Value::as_str),
            "mode": baseline.get("mode").and_then(toml::Value::as_str),
            "prerequisites": prerequisites,
            "generated_config": {
                "canonical_path": canonical_config,
                "sha256": config_sha256
            },
            "binding": {
                "resolution_id": resolution_id,
                "recipe_sha256": recipe_sha256,
                "config_sha256": config_sha256,
                "adapter": adapter,
                "agent_cli": agent_cli,
                "package_inventory_sha256": inventory_sha256,
                "package_tree_sha256": tree_sha256
            }
        }],
        "model_catalog": {"state": "deferred_to_authorized_smoke"},
        "protected_inputs": [{
            "path": canonical_production,
            "before_sha256": production_sha256,
            "after_sha256": production_sha256
        }],
        "owned_resources": [{"kind": "bundle", "identity": canonical_bundle}]
    });
    let resolution_path = bundle.join("resolution.json");
    fs::write(
        &resolution_path,
        serde_json::to_vec_pretty(&resolution).unwrap(),
    )
    .unwrap();
    fs::set_permissions(&resolution_path, fs::Permissions::from_mode(0o600)).unwrap();
    (resolution_path, generated_config)
}

fn compatibility_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"));
    command.arg("compatibility");
    command
}

#[test]
fn validate_is_non_billable_and_accepts_the_versioned_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = write_manifest(dir.path());

    let output = compatibility_command()
        .arg("validate")
        .arg("--manifest")
        .arg(&manifest)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 case"));
}

#[test]
fn validate_rejects_unresolved_floating_manifest_before_any_execution() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = write_unresolved_manifest(dir.path());

    let output = compatibility_command()
        .arg("validate")
        .arg("--manifest")
        .arg(&manifest)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("requires exact candidate resolution evidence"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let missing_baseline = write_manifest(dir.path());
    let raw = fs::read_to_string(&missing_baseline)
        .unwrap()
        .replace("baseline_case = \"pinned-support-control\"\n", "");
    fs::write(&missing_baseline, raw).unwrap();
    let baseline_output = compatibility_command()
        .arg("validate")
        .arg("--manifest")
        .arg(&missing_baseline)
        .output()
        .unwrap();
    assert!(!baseline_output.status.success());
    assert!(
        String::from_utf8_lossy(&baseline_output.stderr).contains("requires baseline_case"),
        "stderr: {}",
        String::from_utf8_lossy(&baseline_output.stderr)
    );
}

#[test]
fn validate_rejects_a_floating_canary_that_treats_failure_as_expected_success() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = write_manifest(dir.path());
    let raw = fs::read_to_string(&manifest)
        .unwrap()
        .replace("expected_status = \"PASS\"", "expected_status = \"FAIL\"");
    fs::write(&manifest, raw).unwrap();

    let output = compatibility_command()
        .arg("validate")
        .arg("--manifest")
        .arg(&manifest)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("expecting PASS"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_accepts_closed_floating_recipes_against_the_pinned_support_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let recipes = write_recipes(dir.path());

    let output = compatibility_command()
        .arg("validate")
        .arg("--recipes")
        .arg(&recipes)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("floating recipes valid: 1 case"));
}

#[test]
fn validate_rejects_recipe_mapping_to_a_pinned_non_support_control() {
    let dir = tempfile::tempdir().unwrap();
    let recipes = write_recipes(dir.path());
    let raw = fs::read_to_string(&recipes).unwrap().replace(
        "baseline_case = \"codex-host-bridge-gpt56-sol\"",
        "baseline_case = \"claude-direct-host-cli-fable\"",
    );
    fs::write(&recipes, raw).unwrap();

    let output = compatibility_command()
        .arg("validate")
        .arg("--recipes")
        .arg(&recipes)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("pinned minimal bridge-smoke support"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn resolve_acknowledgement_wins_before_recipe_or_output_access() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing-recipes.toml");
    let out = dir.path().join("bundle");

    let output = compatibility_command()
        .env("PATH", "")
        .arg("resolve")
        .arg("--recipes")
        .arg(&missing)
        .arg("--all")
        .arg("--environment-owner")
        .arg("test-runner")
        .arg("--runtime")
        .arg("docker")
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--acknowledge-resolution-effects"),
        "stderr: {stderr}"
    );
    assert!(!stderr.contains("cannot open"), "stderr: {stderr}");
    assert!(!out.exists());
}

#[test]
fn resolved_run_billing_acknowledgement_wins_before_resolution_or_output_access() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing-resolution.json");
    let out = dir.path().join("aggregate.json");

    let output = compatibility_command()
        .arg("run")
        .arg("--resolution")
        .arg(&missing)
        .arg("--all-resolved")
        .arg("--environment-owner")
        .arg("test-runner")
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--acknowledge-billable"),
        "stderr: {stderr}"
    );
    assert!(!stderr.contains("cannot open"), "stderr: {stderr}");
    assert!(!out.exists());
}

#[test]
fn resolved_run_revalidates_generated_config_before_any_provider_spawn() {
    let dir = tempfile::tempdir().unwrap();
    let (resolution, generated_config) = write_bound_host_resolution(dir.path());
    fs::write(&generated_config, b"drifted after resolution\n").unwrap();
    let out = dir.path().join("aggregate.json");

    let output = compatibility_command()
        .arg("run")
        .arg("--resolution")
        .arg(&resolution)
        .arg("--all-resolved")
        .arg("--environment-owner")
        .arg("fixture-owner")
        .arg("--acknowledge-billable")
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        out.exists(),
        "drift must remain explicit aggregate evidence"
    );
    let aggregate: serde_json::Value = serde_json::from_slice(&fs::read(&out).unwrap()).unwrap();
    assert_eq!(aggregate["success"], false);
    assert_eq!(aggregate["results"][0]["execution"], "not_run");
    assert_eq!(
        aggregate["results"][0]["not_run_reason"],
        "resolution_generated_config_changed"
    );
    assert_eq!(aggregate["results"][0]["smoke"], serde_json::Value::Null);
    assert_eq!(aggregate["resolution"]["resolution_id"], "resolution-cli-1");
}

#[test]
fn acknowledged_resolve_rejects_owner_mismatch_before_tool_or_output_effects() {
    let dir = tempfile::tempdir().unwrap();
    let recipes = write_recipes(dir.path());
    let out = dir.path().join("bundle");

    let output = compatibility_command()
        .env("PATH", "")
        .arg("resolve")
        .arg("--recipes")
        .arg(&recipes)
        .arg("--case")
        .arg("codex-host-floating-current")
        .arg("--environment-owner")
        .arg("test-runner")
        .arg("--runtime")
        .arg("docker")
        .arg("--acknowledge-resolution-effects")
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("environment owner mismatch"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!out.exists());
}

#[test]
fn acknowledged_resolve_rejects_missing_runtime_before_output_effects() {
    let dir = tempfile::tempdir().unwrap();
    let recipes = write_recipes(dir.path());
    let out = dir.path().join("bundle");

    let output = compatibility_command()
        .env("PATH", "")
        .arg("resolve")
        .arg("--recipes")
        .arg(&recipes)
        .arg("--case")
        .arg("codex-host-floating-current")
        .arg("--environment-owner")
        .arg("fixture-owner")
        .arg("--runtime")
        .arg("docker")
        .arg("--acknowledge-resolution-effects")
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("required container runtime")
            && stderr.contains("docker")
            && stderr.contains("is not executable on PATH"),
        "stderr: {stderr}"
    );
    assert!(!out.exists());
}

#[test]
fn run_refuses_without_acknowledgement_before_manifest_or_output_access() {
    let dir = tempfile::tempdir().unwrap();
    let missing_manifest = dir.path().join("missing.toml");
    let out = dir.path().join("aggregate.json");

    let output = compatibility_command()
        .arg("run")
        .arg("--manifest")
        .arg(&missing_manifest)
        .arg("--case")
        .arg("missing-config-control")
        .arg("--environment-owner")
        .arg("test-runner")
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--acknowledge-billable"),
        "stderr: {stderr}"
    );
    assert!(!stderr.contains("cannot open"), "stderr: {stderr}");
    assert!(!out.exists());
}

#[test]
fn run_requires_explicit_case_lane_or_all_selection() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = write_manifest(dir.path());
    let out = dir.path().join("aggregate.json");

    let output = compatibility_command()
        .arg("run")
        .arg("--manifest")
        .arg(&manifest)
        .arg("--environment-owner")
        .arg("test-runner")
        .arg("--acknowledge-billable")
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("explicit selection"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!out.exists());
}

#[test]
fn run_rejects_mixed_lane_and_case_selection_before_output_access() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = write_manifest(dir.path());
    let out = dir.path().join("aggregate.json");

    let output = compatibility_command()
        .arg("run")
        .arg("--manifest")
        .arg(&manifest)
        .arg("--lane")
        .arg("floating-current")
        .arg("--case")
        .arg("missing-config-control")
        .arg("--environment-owner")
        .arg("test-runner")
        .arg("--acknowledge-billable")
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--lane cannot be combined with --case"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!out.exists());
}

#[test]
fn run_rejects_secret_shaped_canonical_manifest_path_before_output_access() {
    let dir = tempfile::tempdir().unwrap();
    let secret_shaped_dir = dir.path().join("token=opaque-value");
    fs::create_dir(&secret_shaped_dir).unwrap();
    let manifest = write_manifest(&secret_shaped_dir);
    let out = dir.path().join("aggregate.json");

    let output = compatibility_command()
        .arg("run")
        .arg("--manifest")
        .arg(&manifest)
        .arg("--case")
        .arg("missing-config-control")
        .arg("--environment-owner")
        .arg("test-runner")
        .arg("--acknowledge-billable")
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("secret-free"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!out.exists());
}

#[test]
fn run_rejects_aggregate_output_inside_the_manifest_repository() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join(".git")).unwrap();
    let manifest_dir = dir.path().join("compatibility");
    fs::create_dir(&manifest_dir).unwrap();
    let manifest = write_pinned_manifest(&manifest_dir, &"a".repeat(64));
    let out = dir.path().join("new-aggregate.json");

    let output = compatibility_command()
        .arg("run")
        .arg("--manifest")
        .arg(&manifest)
        .arg("--case")
        .arg("missing-config-control")
        .arg("--environment-owner")
        .arg("test-runner")
        .arg("--acknowledge-billable")
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--out must be outside any repository"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!out.exists());
}

#[test]
fn existing_aggregate_is_never_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = write_pinned_manifest(dir.path(), &"a".repeat(64));
    let out = dir.path().join("aggregate.json");
    fs::write(&out, b"reviewed evidence").unwrap();
    let mut permissions = fs::metadata(&out).unwrap().permissions();
    permissions.set_mode(0o644);
    fs::set_permissions(&out, permissions).unwrap();

    let output = compatibility_command()
        .arg("run")
        .arg("--manifest")
        .arg(&manifest)
        .arg("--case")
        .arg("missing-config-control")
        .arg("--environment-owner")
        .arg("test-runner")
        .arg("--acknowledge-billable")
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(fs::read(&out).unwrap(), b"reviewed evidence");
    assert_eq!(
        fs::metadata(&out).unwrap().permissions().mode() & 0o777,
        0o644
    );
}

#[test]
fn acknowledged_direct_floating_run_requires_a_resolution_before_output_effects() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = write_manifest(dir.path());
    let out = dir.path().join("aggregate.json");

    let output = compatibility_command()
        .arg("run")
        .arg("--manifest")
        .arg(&manifest)
        .arg("--lane")
        .arg("floating-current")
        .arg("--environment-owner")
        .arg("test-runner")
        .arg("--acknowledge-billable")
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("floating_resolution_required"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!out.exists());
}

#[cfg(target_os = "linux")]
#[test]
fn compatibility_child_closes_staged_capabilities_before_provider_spawn() {
    let dir = tempfile::tempdir().unwrap();
    let report = dir.path().join("provider-fds.txt");
    let adapter = dir.path().join("fd-reporting-agent");
    fs::write(
        &adapter,
        format!(
            "#!/bin/sh\nfor fd in /proc/self/fd/*; do readlink \"$fd\" || true; done > {:?}\nexit 1\n",
            report
        ),
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
    let config_bytes = fs::read(&config).unwrap();
    let manifest = write_pinned_manifest(dir.path(), &sha256_hex(&config_bytes));
    let manifest_text = fs::read_to_string(&manifest)
        .unwrap()
        .replace("config = \"missing.toml\"", "config = \"a2a-bridge.toml\"")
        .replace("agent = \"missing-agent\"", "agent = \"marker\"");
    fs::write(&manifest, manifest_text).unwrap();
    let out = dir.path().join("aggregate.json");

    let output = compatibility_command()
        .arg("run")
        .arg("--manifest")
        .arg(&manifest)
        .arg("--all")
        .arg("--environment-owner")
        .arg("test-runner")
        .arg("--acknowledge-billable")
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}\naggregate: {}",
        String::from_utf8_lossy(&output.stderr),
        fs::read_to_string(&out).unwrap_or_else(|error| format!("<unreadable: {error}>"))
    );
    let inherited = fs::read_to_string(&report).expect("provider process must record its fds");
    assert!(
        !inherited.contains("a2a-bridge-candidate"),
        "provider inherited staged executable capability: {inherited}"
    );
    assert!(
        !inherited.contains("/.a2a-compat-"),
        "provider inherited compatibility scratch capability: {inherited}"
    );
}

#[test]
fn case_and_all_selection_cannot_bypass_the_floating_resolution_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = write_two_case_manifest(dir.path());
    let selected_out = dir.path().join("selected.json");
    let all_out = dir.path().join("all.json");

    let selected = compatibility_command()
        .arg("run")
        .arg("--manifest")
        .arg(&manifest)
        .arg("--case")
        .arg("second-missing-config-control")
        .arg("--environment-owner")
        .arg("test-runner")
        .arg("--acknowledge-billable")
        .arg("--out")
        .arg(&selected_out)
        .output()
        .unwrap();
    assert!(!selected.status.success());
    assert!(
        String::from_utf8_lossy(&selected.stderr).contains("floating_resolution_required"),
        "stderr: {}",
        String::from_utf8_lossy(&selected.stderr)
    );
    assert!(!selected_out.exists());

    let all = compatibility_command()
        .arg("run")
        .arg("--manifest")
        .arg(&manifest)
        .arg("--all")
        .arg("--environment-owner")
        .arg("test-runner")
        .arg("--acknowledge-billable")
        .arg("--out")
        .arg(&all_out)
        .output()
        .unwrap();
    assert!(!all.status.success());
    assert!(
        String::from_utf8_lossy(&all.stderr).contains("floating_resolution_required"),
        "stderr: {}",
        String::from_utf8_lossy(&all.stderr)
    );
    assert!(!all_out.exists());
}

#[test]
fn compare_is_non_billable_and_reports_manifest_drift_as_json() {
    let dir = tempfile::tempdir().unwrap();
    let current = dir.path().join("aggregate.json");
    let baseline = dir.path().join("baseline.json");
    fs::write(
        &current,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "candidate": {
                "canonical_path": "/tmp/a2a-bridge",
                "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                "byte_length": 42
            },
            "manifest": {
                "schema_version": 1,
                "canonical_path": "/tmp/manifest.toml",
                "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            },
            "selection": {"cases": [], "all": true},
            "environment_owner": "test-runner",
            "started_at_ms": 1,
            "ended_at_ms": 2,
            "cancelled": false,
            "success": true,
            "budget": {
                "timeout_secs": 1,
                "max_tokens": 1,
                "observed_tokens": 0,
                "observed_cost_usd": 0.0,
                "token_observation_missing_cases": 0,
                "cost_observation_missing_cases": 0,
                "exhausted": false
            },
            "results": []
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        &baseline,
        br#"{
          "schema_version": 1,
          "manifest_schema_version": 1,
          "manifest_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
          "aggregate": {
            "success": true,
            "cancelled": false,
            "budget_exhausted": false,
            "token_observation_missing_cases": 0,
            "cost_observation_missing_cases": 0
          },
          "cases": []
        }"#,
    )
    .unwrap();

    let equal = compatibility_command()
        .arg("compare")
        .arg("--current")
        .arg(&current)
        .arg("--baseline")
        .arg(&baseline)
        .output()
        .unwrap();
    assert!(
        equal.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&equal.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&equal.stdout).unwrap()["equal"],
        true
    );

    let mut changed: serde_json::Value =
        serde_json::from_slice(&fs::read(&baseline).unwrap()).unwrap();
    changed["manifest_sha256"] = serde_json::Value::String("b".repeat(64));
    fs::write(&baseline, serde_json::to_vec_pretty(&changed).unwrap()).unwrap();
    let drift = compatibility_command()
        .arg("compare")
        .arg("--current")
        .arg(&current)
        .arg("--baseline")
        .arg(&baseline)
        .output()
        .unwrap();
    assert!(!drift.status.success());
    let report: serde_json::Value = serde_json::from_slice(&drift.stdout).unwrap();
    assert_eq!(report["equal"], false);
    assert_eq!(report["changes"][0]["case_id"], "__manifest__");
    assert_eq!(
        report["changes"][0]["dimensions"],
        serde_json::json!(["manifest"])
    );
}

#[test]
fn floating_compare_requires_resolution_binding_and_emits_dimensioned_json() {
    let dir = tempfile::tempdir().unwrap();
    let current = dir.path().join("floating-aggregate.json");
    let baseline = dir.path().join("baseline.json");
    let digest = |byte: char| byte.to_string().repeat(64);
    let aggregate = serde_json::json!({
        "schema_version": 1,
        "candidate": {
            "canonical_path": "/tmp/a2a-bridge",
            "sha256": digest('c'),
            "byte_length": 42
        },
        "manifest": {
            "schema_version": 1,
            "canonical_path": "/tmp/execution-manifest.toml",
            "sha256": digest('a')
        },
        "selection": {"cases": [], "all": true},
        "environment_owner": "test-runner",
        "started_at_ms": 1,
        "ended_at_ms": 2,
        "cancelled": false,
        "success": true,
        "budget": {
            "timeout_secs": 30,
            "max_tokens": 100,
            "observed_tokens": 1,
            "observed_cost_usd": 0.0,
            "token_observation_missing_cases": 0,
            "cost_observation_missing_cases": 0,
            "exhausted": false
        },
        "resolution": {
            "resolution_id": "resolution-1",
            "artifact_sha256": digest('d'),
            "recipe_sha256": digest('e'),
            "production_manifest_sha256": digest('b')
        },
        "floating_summary": {
            "candidate_pass": 1,
            "candidate_fail": 0,
            "candidate_unknown": 0
        },
        "results": [{
            "case_id": "floating-only",
            "baseline_case_id": "baseline-only",
            "lane": "floating-current",
            "evidence_path": "bridge_smoke",
            "probe": "minimal",
            "billable": true,
            "execution": "completed",
            "expected_status": "PASS",
            "actual_status": "PASS",
            "expectation_met": true,
            "classification": "canary",
            "candidate_outcome": "candidate_pass",
            "resolved": {
                "resolution_id": "resolution-1",
                "recipe_sha256": digest('e'),
                "config_sha256": digest('f'),
                "adapter": "@agentclientprotocol/codex-acp=1.2.3",
                "agent_cli": "@openai/codex=0.150.0",
                "package_inventory_sha256": digest('1'),
                "package_tree_sha256": digest('2')
            },
            "artifact_policy": {"retention_days": 1, "redaction": "strict"},
            "duration_ms": 1,
            "drift": [],
            "budget_violations": [],
            "smoke": {
                "schema_version": 2,
                "success": true,
                "attempt": {"id": "attempt-1", "timed_out": false},
                "request": {"agent": "codex", "model": "m", "config_sha256": digest('f')},
                "target": {
                    "execution_mode": "host",
                    "provenance": [
                        {
                            "check": "provenance:codex:adapter",
                            "status": "ok",
                            "detail": "package=@agentclientprotocol/codex-acp version=1.2.3"
                        },
                        {
                            "check": "provenance:codex:agent-cli",
                            "status": "ok",
                            "detail": "package=@openai/codex version=0.150.0"
                        }
                    ],
                    "authentication": {"path": "automatic"},
                    "model_catalog": {
                        "state": "available",
                        "current_model": "m",
                        "models": ["m"],
                        "model_configurable": true,
                        "effort_levels": [],
                        "modes": []
                    }
                },
                "session": {"effective_request": {"model": "m"}},
                "turn": {
                    "prompt": "Reply exactly PONG. Do not use tools.",
                    "prompt_calls": 1,
                    "terminal_state": "completed",
                    "exact_pong": true
                },
                "diagnostics": {"failure": null},
                "cleanup": {
                    "cancel": "not_needed",
                    "release": "completed",
                    "retire": "completed"
                }
            }
        }]
    });
    fs::write(&current, serde_json::to_vec_pretty(&aggregate).unwrap()).unwrap();
    fs::write(
        &baseline,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "manifest_schema_version": 1,
            "manifest_sha256": digest('b'),
            "aggregate": {
                "success": true,
                "cancelled": false,
                "budget_exhausted": false,
                "token_observation_missing_cases": 0,
                "cost_observation_missing_cases": 0
            },
            "cases": [{
                "case_id": "baseline-only",
                "outcome": null,
                "status": "PASS",
                "execution_mode": null,
                "provenance": [],
                "capability": {},
                "authentication": null,
                "phase": null,
                "terminal": null,
                "diagnostic": null
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let output = compatibility_command()
        .arg("compare")
        .arg("--mode")
        .arg("floating-to-pinned")
        .arg("--current")
        .arg(&current)
        .arg("--baseline")
        .arg(&baseline)
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "floating differences are explicit"
    );
    assert!(
        !output.stdout.is_empty(),
        "comparison failed before emitting JSON: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let dimensions = report["changes"][0]["dimensions"].as_array().unwrap();
    assert!(dimensions.iter().any(|value| value == "adapter"));
    assert!(dimensions
        .iter()
        .any(|value| value == "catalog.models_added"));

    let mut unrelated_baseline: serde_json::Value =
        serde_json::from_slice(&fs::read(&baseline).unwrap()).unwrap();
    unrelated_baseline["manifest_sha256"] = serde_json::Value::String(digest('c'));
    fs::write(
        &baseline,
        serde_json::to_vec_pretty(&unrelated_baseline).unwrap(),
    )
    .unwrap();
    let rejected = compatibility_command()
        .arg("compare")
        .arg("--mode")
        .arg("floating-to-pinned")
        .arg("--current")
        .arg(&current)
        .arg("--baseline")
        .arg(&baseline)
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("pinned manifest identity mismatch"));
    unrelated_baseline["manifest_sha256"] = serde_json::Value::String(digest('b'));
    fs::write(
        &baseline,
        serde_json::to_vec_pretty(&unrelated_baseline).unwrap(),
    )
    .unwrap();

    let mut unbound = aggregate;
    unbound.as_object_mut().unwrap().remove("resolution");
    fs::write(&current, serde_json::to_vec_pretty(&unbound).unwrap()).unwrap();
    let rejected = compatibility_command()
        .arg("compare")
        .arg("--mode")
        .arg("floating-to-pinned")
        .arg("--current")
        .arg(&current)
        .arg("--baseline")
        .arg(&baseline)
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("resolution binding"));
}
