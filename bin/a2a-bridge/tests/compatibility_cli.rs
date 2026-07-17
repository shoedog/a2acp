use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

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

#[cfg(target_os = "linux")]
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
