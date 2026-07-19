#![cfg(unix)]

use std::path::Path;
use std::process::Command;

use serde::Serialize;

fn digest(ch: char) -> String {
    ch.to_string().repeat(64)
}

fn sha256_hex(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn validate(kind: &str, path: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .arg("compatibility")
        .arg("validate")
        .arg("--schedule-record")
        .arg(kind)
        .arg(path)
        .output()
        .unwrap()
}

#[derive(Serialize)]
struct CaseBudget<'a> {
    case_id: &'a str,
    timeout_ms: u64,
}

#[derive(Serialize)]
struct PhaseBudgets<'a> {
    metadata_fetch_ms: u64,
    checkout_candidate_build_ms: u64,
    preflight_ms: u64,
    resolution_materialization_ms: u64,
    selected_cases: Vec<CaseBudget<'a>>,
    evidence_publication_ms: u64,
    cold_archive_handoff_ms: u64,
    cleanup_grace_ms: u64,
    fixed_margin_ms: u64,
}

#[derive(Serialize)]
struct Containment {
    schedule_window_remaining_ms: u64,
    grant_remaining_ms: u64,
    time_budget_remaining_ms: u64,
}

#[derive(Serialize)]
struct DeadlineInput<'a> {
    schema_version: u16,
    run_id: &'a str,
    window_id: &'a str,
    process_entry_elapsed_ms: u64,
    budgets: PhaseBudgets<'a>,
    total_bound_ms: u64,
    remaining_at_derivation_ms: u64,
    containment: Containment,
}

fn deadline_record() -> serde_json::Value {
    let input = DeadlineInput {
        schema_version: 1,
        run_id: "run-1",
        window_id: "window-1",
        process_entry_elapsed_ms: 5,
        budgets: PhaseBudgets {
            metadata_fetch_ms: 10,
            checkout_candidate_build_ms: 20,
            preflight_ms: 30,
            resolution_materialization_ms: 40,
            selected_cases: vec![CaseBudget {
                case_id: "case-1",
                timeout_ms: 50,
            }],
            evidence_publication_ms: 60,
            cold_archive_handoff_ms: 0,
            cleanup_grace_ms: 70,
            fixed_margin_ms: 80,
        },
        total_bound_ms: 360,
        remaining_at_derivation_ms: 355,
        containment: Containment {
            schedule_window_remaining_ms: 355,
            grant_remaining_ms: 400,
            time_budget_remaining_ms: 500,
        },
    };
    let canonical = serde_json::to_vec(&input).unwrap();
    let mut material = b"a2a-bridge:r3d1:deadline-derivation-input:v1\0".to_vec();
    material.extend_from_slice(&canonical);
    serde_json::json!({
        "schema_version": 1,
        "input": serde_json::to_value(input).unwrap(),
        "derivation": {"schema_version": 1, "sha256": sha256_hex(&material)}
    })
}

fn process(pid: i32, parent_pid: i32, process_group: i32) -> serde_json::Value {
    serde_json::json!({
        "pid": pid,
        "parent_pid": parent_pid,
        "process_group": process_group,
        "session_id": 41,
        "start": {
            "kind": "linux_boot_ticks",
            "boot_id": "01234567-89ab-cdef-0123-456789abcdef",
            "start_ticks": pid * 10
        }
    })
}

fn supervisor_record(deadline_sha256: &str) -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "supervisor_record_id": "supervisor-1",
        "generation": 1,
        "previous_record": {"kind": "absent"},
        "run_id": "run-1",
        "window_id": "window-1",
        "trigger": "daily",
        "deadline_derivation_sha256": deadline_sha256,
        "scheduler": process(42, 1, 42),
        "runner": {"kind": "process", "value": process(44, 42, 43)},
        "groups": [{
            "process_group": 43,
            "session_id": 41,
            "anchor": process(43, 42, 43),
            "workloads": [process(44, 42, 43)],
            "anchor_lifecycle": "released_reaped"
        }],
        "container_run_labels": ["a2a-compat-run-1"],
        "phase": "complete",
        "term_journal_elapsed_ms": {"kind": "absent"},
        "kill_journal_elapsed_ms": {"kind": "absent"},
        "kill_cause": {"kind": "absent"},
        "later_group_signal_permitted": false,
        "outcome": {"kind": "outcome", "value": "completed"},
        "safety_hold": {"kind": "absent"},
        "child_artifact": {
            "kind": "artifact",
            "value": {
                "record_id": "aggregate-1",
                "run_id": "run-1",
                "window_id": "window-1",
                "artifact_sha256": digest('b'),
                "aggregate_sha256": {"kind": "sha256", "value": digest('c')}
            }
        },
        "recorded_at_ms": 100
    })
}

#[test]
fn r3d1_cli_validates_deadline_and_joined_supervisor_records() {
    let directory = tempfile::tempdir().unwrap();
    let deadline = deadline_record();
    let deadline_path = directory.path().join("deadline.json");
    std::fs::write(&deadline_path, serde_json::to_vec(&deadline).unwrap()).unwrap();
    let deadline_output = validate("deadline-derivation", &deadline_path);
    assert!(
        deadline_output.status.success(),
        "{}",
        String::from_utf8_lossy(&deadline_output.stderr)
    );

    let deadline_sha256 = deadline["derivation"]["sha256"].as_str().unwrap();
    let supervisor = supervisor_record(deadline_sha256);
    let supervisor_path = directory.path().join("supervisor.json");
    std::fs::write(&supervisor_path, serde_json::to_vec(&supervisor).unwrap()).unwrap();
    let supervisor_output = validate("supervisor", &supervisor_path);
    assert!(
        supervisor_output.status.success(),
        "{}",
        String::from_utf8_lossy(&supervisor_output.stderr)
    );
}

#[test]
fn r3d1_cli_rejects_deadline_sum_and_child_window_substitution() {
    let directory = tempfile::tempdir().unwrap();
    let mut deadline = deadline_record();
    deadline["input"]["total_bound_ms"] = 361.into();
    let deadline_path = directory.path().join("bad-deadline.json");
    std::fs::write(&deadline_path, serde_json::to_vec(&deadline).unwrap()).unwrap();
    assert!(!validate("deadline-derivation", &deadline_path)
        .status
        .success());

    let mut supervisor = supervisor_record(&digest('a'));
    supervisor["child_artifact"]["value"]["window_id"] = "other-window".into();
    let supervisor_path = directory.path().join("bad-supervisor.json");
    std::fs::write(&supervisor_path, serde_json::to_vec(&supervisor).unwrap()).unwrap();
    assert!(!validate("supervisor", &supervisor_path).status.success());
}
