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
expected_status = "FAIL"
classification = "non_goal"

[cases.artifact]
retention_days = 1
redaction = "strict"
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
        );
    fs::write(&manifest, format!("{first}\n[[cases]]{second}")).unwrap();
    manifest
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
    let manifest = write_manifest(&manifest_dir);
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
    let manifest = write_manifest(dir.path());
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
fn acknowledged_run_calls_the_smoke_contract_once_and_keeps_failure_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = write_manifest(dir.path());
    let out = dir.path().join("aggregate.json");

    // The selected config intentionally does not exist. The nested R2c smoke therefore emits a
    // deterministic pre-spawn FAIL artifact; no provider process or billable turn can start.
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

    assert!(
        output.status.success(),
        "stderr: {}\naggregate: {}",
        String::from_utf8_lossy(&output.stderr),
        fs::read_to_string(&out).unwrap_or_else(|error| format!("<unreadable: {error}>"))
    );
    assert!(output.stdout.is_empty());
    let aggregate: serde_json::Value = serde_json::from_slice(&fs::read(&out).unwrap()).unwrap();
    assert_eq!(aggregate["schema_version"], 1);
    assert_eq!(aggregate["candidate"]["sha256"].as_str().unwrap().len(), 64);
    assert!(aggregate["candidate"]["byte_length"].as_u64().unwrap() > 0);
    assert_eq!(aggregate["results"].as_array().unwrap().len(), 1);
    assert_eq!(aggregate["results"][0]["case_id"], "missing-config-control");
    assert_eq!(aggregate["results"][0]["execution"], "completed");
    assert_eq!(aggregate["results"][0]["actual_status"], "FAIL");
    assert_eq!(aggregate["results"][0]["expectation_met"], true);
    assert_eq!(aggregate["results"][0]["smoke"]["schema_version"], 2);
    assert_eq!(
        aggregate["results"][0]["smoke"]["diagnostics"]["failure"]["code"],
        "smoke.config_path"
    );
    assert_eq!(
        fs::metadata(&out).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn case_selection_is_not_all_and_explicit_all_keeps_every_row() {
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
    assert!(
        selected.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&selected.stderr)
    );
    let selected: serde_json::Value =
        serde_json::from_slice(&fs::read(&selected_out).unwrap()).unwrap();
    assert_eq!(selected["results"].as_array().unwrap().len(), 1);
    assert_eq!(
        selected["results"][0]["case_id"],
        "second-missing-config-control"
    );

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
    assert!(
        all.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&all.stderr)
    );
    let all: serde_json::Value = serde_json::from_slice(&fs::read(&all_out).unwrap()).unwrap();
    assert_eq!(all["results"].as_array().unwrap().len(), 2);
    assert_eq!(all["results"][0]["case_id"], "missing-config-control");
    assert_eq!(
        all["results"][1]["case_id"],
        "second-missing-config-control"
    );
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
