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
        AttemptId::parse("attempt-11111111111111111111111111111111").unwrap(),
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

#[test]
fn v2_run_spec_round_trips_and_revalidates_every_fingerprint() {
    let spec = run_spec();
    let json = spec.encode_snapshot_v2().unwrap();
    let resumed = WorkflowRunSpecV1::decode_snapshot_v2(&json).unwrap();
    assert_eq!(resumed, spec);
    assert_eq!(resumed.graph.controls, spec.graph.controls);
}

#[test]
fn v2_run_spec_rejects_fingerprint_tampering_before_use() {
    let spec = run_spec();
    let mut value: serde_json::Value =
        serde_json::from_slice(&spec.encode_snapshot_v2().unwrap()).unwrap();
    value["run_spec"]["workload_fingerprint"] = serde_json::json!("shape-deadbeef");
    let tampered = serde_json::to_vec(&value).unwrap();
    assert!(WorkflowRunSpecV1::decode_snapshot_v2(&tampered).is_err());
}

#[test]
fn run_spec_refuses_graph_identity_mismatch() {
    let mut spec = run_spec();
    spec.node_execution_identities[0].node = PolicyNodeRefV1::from_node_id(0, "other");
    assert!(spec.validate().is_err());
}
