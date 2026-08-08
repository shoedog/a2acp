//! R2f1b A5 (rev-2 gate §2.6): the workload/calibration identity bound to a V3
//! attempt's deadline activation and R2f1b contract.
//!
//! The FIRST test in this file is a compatibility golden. Everything after it
//! is the §2.6 binding.
//!
//! Fixture idiom follows `crates/bridge-workflow/tests/r2f1a_run_spec.rs` (~62-96)
//! and `crates/bridge-workflow/tests/r2f1b_run_spec_v3.rs` (~81-135): one frozen
//! provider attempt / node execution identity over a one-node graph, resolved
//! controls, then `WorkflowRunSpecV1::build`.

use bridge_core::domain::{AgentEntry, AgentKind};
use bridge_core::execution_policy::{
    freeze_direct_checkout_v1, freeze_node_execution_identity_v1, freeze_provider_attempt_v1,
    resolve_execution_policy_v1, ExecutionPolicyInvocationV1, FrozenProviderLogicalSessionV1,
    HistoryAllocationKindV1, LedgerAdmissionV1, PolicyActivationV1, PolicyNodeRefV1,
    ProviderFreezeInputV1, WorkflowControlDefaultsV1,
};
use bridge_core::ids::{AgentId, AttemptId, NodeId, WorkflowId};
use bridge_core::mcp::McpDelivery;
use bridge_core::SessionCwd;
use bridge_workflow::graph::{WorkflowGraph, WorkflowNode};
use bridge_workflow::run_spec::WorkflowRunSpecV1;
use std::collections::BTreeMap;

const ROOT_ATTEMPT: &str = "attempt-11111111111111111111111111111111";

/// The manual (V2 / `ManualOnlyR2f1a`) workload fingerprint of `run_spec()`,
/// measured on merged code at `49274e04` before any A5 change.
///
/// THIS VALUE IS A COMPATIBILITY CONTRACT, NOT AN IMPLEMENTATION DETAIL.
/// Persisted history/calibration rows group observations by this exact string
/// (`crates/bridge-coordinator/src/batch.rs` ~1090 stores
/// `run_spec.workload_fingerprint` verbatim). Changing it silently re-partitions
/// every historical grouping.
///
/// If this test goes red, the manual workload fingerprint changed. That is a
/// compatibility ruling for the owner — never a value to "update to match".
const MANUAL_WORKLOAD_FINGERPRINT_GOLDEN: &str =
    "shape-c52073abb62fad2c28ac57a5e9c137763691a944325787abaf9b0c8a67b782c5";

fn graph() -> WorkflowGraph {
    WorkflowGraph {
        id: WorkflowId::parse("review").unwrap(),
        nodes: vec![WorkflowNode {
            id: NodeId::parse("review-node").unwrap(),
            agent: AgentId::parse("codex").unwrap(),
            prompt_template: "{{input}}".into(),
            inputs: vec![],
            retry: None,
            harvest_sanitization: None,
        }],
        panel: None,
        controls: Some(WorkflowControlDefaultsV1::default()),
    }
}

fn entry() -> AgentEntry {
    AgentEntry {
        id: AgentId::parse("codex").unwrap(),
        cmd: Some("codex-acp".into()),
        base_url: None,
        api_key_env: None,
        args: vec![],
        kind: AgentKind::Acp,
        model_provider: None,
        model: Some("gpt-5.6-sol".into()),
        effort: None,
        mode: None,
        preflight: false,
        fallback_models: vec![],
        cwd: None,
        session_cwd: None,
        sandbox: None,
        watchdog: None,
        mcp: vec![],
        mcp_delivery: McpDelivery::Acp,
        auth_method: None,
        pre_authenticated: true,
        host_fallback_eligible: false,
        name: None,
        description: None,
        tags: vec![],
        version: None,
        extensions: BTreeMap::new(),
    }
}

/// The one fixed spec the golden pins. Every input is literal and deterministic
/// (no minted ids, no clock, no environment): the frozen provider attempt is a
/// pure function of `entry()`, the node ref, and the `/repo` checkout.
fn run_spec() -> WorkflowRunSpecV1 {
    let graph = graph();
    let entry = entry();
    let node = PolicyNodeRefV1::from_node_id(0, "review-node");
    let bundle = freeze_provider_attempt_v1(&ProviderFreezeInputV1 {
        entry: &entry,
        overrides: None,
        node: node.clone(),
        logical_session: FrozenProviderLogicalSessionV1::Execute {
            candidate_ordinal: 0,
        },
        checkout: freeze_direct_checkout_v1(SessionCwd::parse("/repo").unwrap()),
        provider_effect_key: None,
    })
    .unwrap();
    let identity = freeze_node_execution_identity_v1(node, vec![bundle]).unwrap();
    let controls = resolve_execution_policy_v1(
        graph.controls.as_ref().unwrap(),
        &ExecutionPolicyInvocationV1::default(),
        false,
        PolicyActivationV1::Production,
    )
    .unwrap();
    WorkflowRunSpecV1::build(
        AttemptId::parse(ROOT_ATTEMPT).unwrap(),
        graph,
        controls,
        Some(SessionCwd::parse("/repo").unwrap()),
        vec![identity],
        LedgerAdmissionV1::HistoryLedgerAdmitted {
            kind: HistoryAllocationKindV1::Configured,
        },
    )
    .unwrap()
}

/// COMPATIBILITY GOLDEN. Pins the exact manual workload fingerprint of the fixed
/// spec above, and pins that a V2 encode/decode round trip preserves it verbatim.
///
/// Red here means the persisted manual workload identity moved; see the constant's
/// doc comment.
#[test]
fn manual_workload_fingerprint_matches_the_pinned_golden() {
    let spec = run_spec();
    assert_eq!(
        spec.workload_fingerprint, MANUAL_WORKLOAD_FINGERPRINT_GOLDEN,
        "manual workload fingerprint changed: this is a compatibility ruling, \
         not a value to update"
    );
    let resumed =
        WorkflowRunSpecV1::decode_snapshot_v2(&spec.encode_snapshot_v2().unwrap()).unwrap();
    assert_eq!(
        resumed.workload_fingerprint, MANUAL_WORKLOAD_FINGERPRINT_GOLDEN,
        "V2 resume must recompute the identical manual workload fingerprint"
    );
}
