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
    resolve_execution_policy_v1, DeadlineActivationV2, ExecutionPolicyInvocationV1,
    FrozenProviderLogicalSessionV1, FrozenR2f1bContractV1, FrozenWorktreeCustodyPlanV1,
    HistoryAllocationKindV1, LedgerAdmissionV1, PolicyActivationV1, PolicyNodeRefV1,
    ProviderFreezeInputV1, Sha256HexV1, WorkflowControlDefaultsV1, WorktreeCustodyIdV1,
    R2F1B_RESOURCE_CONTRACT_VERSION_V1,
};
use bridge_core::ids::{AgentId, AttemptId, AttemptIdentity, ExecutionId, NodeId, WorkflowId};
use bridge_core::mcp::McpDelivery;
use bridge_core::SessionCwd;
use bridge_workflow::graph::{WorkflowGraph, WorkflowNode};
use bridge_workflow::run_spec::{
    bound_workload_fingerprint_v2, RunSpecError, WorkflowRunSpecV1, WorkflowSnapshotV3,
};
use std::collections::BTreeMap;

const ROOT_ATTEMPT: &str = "attempt-11111111111111111111111111111111";
const RETRY_ATTEMPT: &str = "attempt-22222222222222222222222222222222";

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

fn graph(node_id: &str) -> WorkflowGraph {
    WorkflowGraph {
        id: WorkflowId::parse("review").unwrap(),
        nodes: vec![WorkflowNode {
            id: NodeId::parse(node_id).unwrap(),
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

/// One fixed spec per `node_id`. Every input is literal and deterministic (no
/// minted ids, no clock, no environment): the frozen provider attempt is a pure
/// function of `entry()`, the node ref, and the `/repo` checkout. Varying
/// `node_id` changes the frozen workload shape, hence the manual fingerprint.
fn run_spec_with_node(node_id: &str) -> WorkflowRunSpecV1 {
    let graph = graph(node_id);
    let entry = entry();
    let node = PolicyNodeRefV1::from_node_id(0, node_id);
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

/// The exact spec the golden pins.
fn run_spec() -> WorkflowRunSpecV1 {
    run_spec_with_node("review-node")
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

/// One custody plan keyed by `digit` (distinct digits give distinct,
/// individually valid plans). `target` is the plan's protected checkout.
fn plan(digit: char, target: &str) -> FrozenWorktreeCustodyPlanV1 {
    FrozenWorktreeCustodyPlanV1 {
        custody_id: WorktreeCustodyIdV1::parse(format!("custody-{}", digit.to_string().repeat(64)))
            .unwrap(),
        checkout_fingerprint: Sha256HexV1::digest(format!("contract-plan-{digit}").as_bytes()),
        target_cwd: SessionCwd::parse(target).unwrap(),
    }
}

fn contract(
    activation: DeadlineActivationV2,
    plans: Vec<FrozenWorktreeCustodyPlanV1>,
) -> FrozenR2f1bContractV1 {
    FrozenR2f1bContractV1::with_computed_fingerprint(activation, plans).unwrap()
}

/// The default one-plan contract under `activation`.
fn one_plan_contract(activation: DeadlineActivationV2) -> FrozenR2f1bContractV1 {
    contract(activation, vec![plan('a', "/repo")])
}

/// A ROOT (ordinal 0) V3 snapshot over `spec` and `contract`, taken through the
/// real `encode_snapshot_v3` / `decode_snapshot_v3` pair so every fixture below
/// is a decoded, validated snapshot rather than a struct literal.
fn root_snapshot(spec: &WorkflowRunSpecV1, contract: FrozenR2f1bContractV1) -> WorkflowSnapshotV3 {
    let attempt = AttemptIdentity {
        execution_id: ExecutionId::mint().unwrap(),
        attempt_id: spec.attempt_id.clone(),
        ordinal: 0,
        parent_attempt_id: None,
    };
    let bytes = spec.encode_snapshot_v3(attempt, contract).unwrap();
    WorkflowRunSpecV1::decode_snapshot_v3(&bytes).unwrap()
}

/// The default automatic-activation snapshot over the golden spec.
fn automatic_snapshot() -> WorkflowSnapshotV3 {
    root_snapshot(
        &run_spec(),
        one_plan_contract(DeadlineActivationV2::AutomaticR2f1b),
    )
}

/// The one correct successor of `predecessor`: same execution id, fresh attempt
/// id, ordinal+1, parent pinned, digest pinned, delivery/contract carried over
/// byte-for-byte. Idiom from `r2f1b_run_spec_v3.rs` (~157).
fn baseline_successor(predecessor: &WorkflowSnapshotV3) -> WorkflowSnapshotV3 {
    WorkflowSnapshotV3 {
        attempt: AttemptIdentity {
            execution_id: predecessor.attempt.execution_id.clone(),
            attempt_id: AttemptId::parse(RETRY_ATTEMPT).unwrap(),
            ordinal: predecessor.attempt.ordinal + 1,
            parent_attempt_id: Some(predecessor.attempt.attempt_id.clone()),
        },
        delivery_spec: predecessor.delivery_spec.clone(),
        predecessor_snapshot_digest: Some(predecessor.digest().unwrap()),
        r2f1b: predecessor.r2f1b.clone(),
    }
}

/// Gate §2.6 "historical V2/manual behavior remains manual": a manual V3 attempt
/// keeps the UNBOUND historical identity, byte-identical to the golden, so manual
/// V2 and manual V3 observations of the same workload still group together.
///
/// Discriminates: an implementation that binds unconditionally (which would also
/// make every manual attempt's identity unique, because `WorktreeCustodyIdV1` is
/// randomly minted per attempt and feeds `contract_fingerprint`).
#[test]
fn manual_v3_identity_is_the_unbound_historical_fingerprint() {
    let spec = run_spec();
    let snapshot = root_snapshot(
        &spec,
        one_plan_contract(DeadlineActivationV2::ManualOnlyR2f1a),
    );
    assert_eq!(
        snapshot.workload_identity().unwrap(),
        MANUAL_WORKLOAD_FINGERPRINT_GOLDEN
    );
    assert_eq!(
        snapshot.workload_identity().unwrap(),
        spec.workload_fingerprint
    );
}

/// Gate §2.6 bullet 1: manual and automatic activation produce different
/// identities for the SAME spec and the SAME custody plans, and the automatic
/// identity is domain-separated (`bound-`) from the manual computation
/// (`shape-`, shared with `bridge_core::workflow_history::fingerprint_workload_shape`).
///
/// Discriminates: `workload_identity` ignoring activation and returning the
/// delivery fingerprint for both.
#[test]
fn manual_and_automatic_activation_produce_different_identities() {
    let spec = run_spec();
    let manual = root_snapshot(
        &spec,
        one_plan_contract(DeadlineActivationV2::ManualOnlyR2f1a),
    )
    .workload_identity()
    .unwrap();
    let automatic = root_snapshot(
        &spec,
        one_plan_contract(DeadlineActivationV2::AutomaticR2f1b),
    )
    .workload_identity()
    .unwrap();
    assert_ne!(manual, automatic);
    assert!(manual.starts_with("shape-"), "manual identity: {manual}");
    assert!(
        automatic.starts_with("bound-"),
        "automatic identity: {automatic}"
    );
    assert_ne!(
        automatic, spec.workload_fingerprint,
        "the automatic identity must not pass the unbound fingerprint through"
    );
}

/// Gate §2.6 final clause: the explicit `DeadlineActivationV2` is committed in
/// the bound buffer INDEPENDENTLY of the contract-hash algorithm. Holding the
/// base fingerprint AND the contract fingerprint fixed, flipping only the
/// explicit activation still moves the identity, so the semantic boundary stays
/// auditable even if two contracts ever collided on one fingerprint.
///
/// This input is unreachable through `WorkflowSnapshotV3` (activation is inside
/// the contract, so flipping it changes `contract_fingerprint` and invalidates
/// the contract) -- it is exactly why the versioned computation is exposed as a
/// free function.
///
/// Discriminates: an implementation that commits only `contract_fingerprint` and
/// relies on activation being transitively inside it.
#[test]
fn explicit_activation_is_committed_separately_from_the_contract_hash() {
    let base = &run_spec().workload_fingerprint;
    let fingerprint = Sha256HexV1::digest(b"one-fixed-contract-fingerprint");
    assert_ne!(
        bound_workload_fingerprint_v2(base, DeadlineActivationV2::ManualOnlyR2f1a, &fingerprint),
        bound_workload_fingerprint_v2(base, DeadlineActivationV2::AutomaticR2f1b, &fingerprint)
    );
}

/// The bound identity commits the EXISTING frozen workload identity, not just
/// activation and the contract. Two different frozen workload shapes under one
/// identical contract must not collide.
///
/// Discriminates: a bound buffer that omits `delivery_spec.workload_fingerprint`.
#[test]
fn the_bound_identity_commits_the_frozen_workload_identity() {
    let spec = run_spec();
    let other = run_spec_with_node("review-node-alt");
    assert_ne!(
        spec.workload_fingerprint, other.workload_fingerprint,
        "fixture precondition: the two specs must have different frozen shapes"
    );
    let contract = one_plan_contract(DeadlineActivationV2::AutomaticR2f1b);
    assert_ne!(
        root_snapshot(&spec, contract.clone())
            .workload_identity()
            .unwrap(),
        root_snapshot(&other, contract).workload_identity().unwrap()
    );
}

/// Gate §2.6 bullet 2 (the "changes" arm): any custody-plan change -- an added
/// plan, a different custody id, a different checkout fingerprint, a different
/// protected target -- changes the automatic identity.
///
/// Discriminates: a bound buffer that omits `contract_fingerprint`, or one that
/// commits only a subset of the contract (e.g. plan count).
#[test]
fn any_custody_plan_change_changes_the_automatic_identity() {
    let spec = run_spec();
    let activation = DeadlineActivationV2::AutomaticR2f1b;
    let identity = |plans: Vec<FrozenWorktreeCustodyPlanV1>| {
        root_snapshot(&spec, contract(activation, plans))
            .workload_identity()
            .unwrap()
    };
    let base = identity(vec![plan('a', "/repo")]);
    for (label, plans) in [
        (
            "added plan",
            vec![plan('a', "/repo"), plan('b', "/repo-two")],
        ),
        ("different custody id", vec![plan('c', "/repo")]),
        ("different target cwd", vec![plan('a', "/repo-elsewhere")]),
    ] {
        assert_ne!(base, identity(plans), "unchanged identity after: {label}");
    }
}

/// Gate §2.6 bullet 2 (the "invalidates" arm): a custody-plan or
/// resource-contract mutation that keeps the recorded `contract_fingerprint`
/// yields NO identity at all. The identity commits a VALIDATED contract
/// fingerprint, so a forged contract cannot be laundered into a bound identity.
///
/// Discriminates: `workload_identity` computing over `self.r2f1b` without
/// validating it first.
#[test]
fn a_forged_contract_yields_no_automatic_identity() {
    let mutated_plan = {
        let mut snapshot = automatic_snapshot();
        snapshot.r2f1b.custody_plans[0].target_cwd = SessionCwd::parse("/repo-swapped").unwrap();
        snapshot
    };
    let mutated_resource_contract = {
        let mut snapshot = automatic_snapshot();
        snapshot.r2f1b.resource_contract_version = R2F1B_RESOURCE_CONTRACT_VERSION_V1 + 1;
        snapshot
    };
    let forged_fingerprint = {
        let mut snapshot = automatic_snapshot();
        snapshot.r2f1b.contract_fingerprint = Sha256HexV1::digest(b"forged-contract-fingerprint");
        snapshot
    };
    for (label, snapshot) in [
        ("mutated custody plan", mutated_plan),
        ("mutated resource contract", mutated_resource_contract),
        ("forged contract fingerprint", forged_fingerprint),
    ] {
        assert_eq!(
            snapshot.workload_identity(),
            Err(RunSpecError::InvalidR2f1bContract),
            "accepted a forged contract: {label}"
        );
    }
}

/// The bound identity's OTHER input is a persisted, forgeable field. A tampered
/// `delivery_spec.workload_fingerprint` must yield no identity rather than a
/// bound digest over an unverified base.
///
/// Discriminates: `workload_identity` validating only the contract and trusting
/// the recorded delivery fingerprint. NOTE: both activations must refuse -- the
/// manual arm returns that field verbatim, so skipping validation there would
/// hand back an attacker-chosen identity.
#[test]
fn a_forged_delivery_workload_fingerprint_yields_no_identity() {
    for activation in [
        DeadlineActivationV2::ManualOnlyR2f1a,
        DeadlineActivationV2::AutomaticR2f1b,
    ] {
        let mut snapshot = root_snapshot(&run_spec(), one_plan_contract(activation));
        snapshot.delivery_spec.workload_fingerprint = "shape-deadbeef".into();
        assert_eq!(
            snapshot.workload_identity(),
            Err(RunSpecError::FingerprintMismatch),
            "accepted a forged delivery workload fingerprint under {activation:?}"
        );
    }
}

/// Gate §2.6 bullet 3: a replacement contract that is separately valid on its own
/// terms cannot carry the old workload identity, and cannot be substituted into a
/// resume either. Both mechanisms are asserted: the identity moves, AND
/// `validate_successor` refuses the swap.
#[test]
fn a_separately_valid_replacement_contract_cannot_retain_the_workload_identity() {
    let spec = run_spec();
    let activation = DeadlineActivationV2::AutomaticR2f1b;
    let original = root_snapshot(&spec, contract(activation, vec![plan('a', "/repo")]));
    let replacement = contract(activation, vec![plan('b', "/repo")]);
    replacement
        .validate()
        .expect("the replacement contract is valid on its own terms");

    assert_ne!(
        original.workload_identity().unwrap(),
        root_snapshot(&spec, replacement.clone())
            .workload_identity()
            .unwrap()
    );

    let mut successor = baseline_successor(&original);
    successor.r2f1b = replacement;
    assert_eq!(
        WorkflowSnapshotV3::validate_successor(&original, &successor),
        Err(RunSpecError::DeliverySpecChanged)
    );
}

/// Gate §2.6 bullet 4: decode and successor resume recompute the SAME bound
/// identity.
///
/// The identity is a pure function of `delivery_spec.workload_fingerprint`,
/// `r2f1b.activation`, and `r2f1b.contract_fingerprint` -- all three inside the
/// bytes `validate_successor` already compares exactly (delivery bytes, then
/// `predecessor.r2f1b != successor.r2f1b`). So an accepted successor cannot have
/// a different identity by construction, and a redundant identity comparison
/// inside `validate_successor` would be unreachable; this test pins the
/// end-to-end property instead of adding a dead guard.
#[test]
fn decode_and_successor_resume_recompute_the_same_bound_identity() {
    let original = automatic_snapshot();
    let identity = original.workload_identity().unwrap();

    let decoded = WorkflowSnapshotV3::decode(&original.encode().unwrap()).unwrap();
    assert_eq!(decoded.workload_identity().unwrap(), identity);

    let successor = baseline_successor(&decoded);
    WorkflowSnapshotV3::validate_successor(&decoded, &successor)
        .expect("fixture precondition: the successor is a valid resume");
    assert_eq!(successor.workload_identity().unwrap(), identity);

    let resumed = WorkflowSnapshotV3::decode(&successor.encode().unwrap()).unwrap();
    assert_eq!(resumed.workload_identity().unwrap(), identity);
}
