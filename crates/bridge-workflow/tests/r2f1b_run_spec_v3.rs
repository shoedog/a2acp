//! R2f1b A3: `WorkflowSnapshotV3` had zero tests. This file covers
//! `encode_snapshot_v3`/`decode_snapshot_v3` round-tripping, every individual
//! `validate_successor` lineage/content invariant, and that the V2 and V3
//! snapshot envelopes never mutually reinterpret each other's bytes.
//!
//! Fixture idiom follows `crates/bridge-workflow/tests/r2f1a_run_spec.rs` (~99-108):
//! build one frozen provider attempt / node execution identity over a one-node
//! graph, resolve controls, then `WorkflowRunSpecV1::build`.

use bridge_core::domain::{AgentEntry, AgentKind};
use bridge_core::execution_policy::{
    freeze_direct_checkout_v1, freeze_node_execution_identity_v1, freeze_provider_attempt_v1,
    resolve_execution_policy_v1, DeadlineActivationV2, ExecutionPolicyInvocationV1,
    FrozenProviderLogicalSessionV1, FrozenR2f1bContractV1, FrozenWorktreeCustodyPlanV1,
    HistoryAllocationKindV1, LedgerAdmissionV1, PolicyActivationV1, PolicyNodeRefV1,
    ProviderFreezeInputV1, Sha256HexV1, WorkflowControlDefaultsV1, WorktreeCustodyIdV1,
};
use bridge_core::ids::{AgentId, AttemptId, AttemptIdentity, ExecutionId, NodeId, WorkflowId};
use bridge_core::mcp::McpDelivery;
use bridge_core::SessionCwd;
use bridge_workflow::graph::{WorkflowGraph, WorkflowNode};
use bridge_workflow::run_spec::{RunSpecError, WorkflowRunSpecV1, WorkflowSnapshotV3};
use std::collections::BTreeMap;

const ROOT_ATTEMPT: &str = "attempt-11111111111111111111111111111111";
const RETRY_ATTEMPT: &str = "attempt-22222222222222222222222222222222";

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

/// One self-consistent `WorkflowRunSpecV1`. `requested_cwd` is not covered by
/// `controls_fingerprint`/`workload_fingerprint` (see `run_spec.rs`
/// `workload_fingerprint`), so varying it alone yields a spec that is
/// individually valid but byte-different from another built with a different
/// `requested_cwd` -- used below to build "changed content, same attempt_id"
/// fixtures without disturbing any other invariant.
fn run_spec_with_cwd(attempt_id: &str, requested_cwd: &str) -> WorkflowRunSpecV1 {
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
        AttemptId::parse(attempt_id).unwrap(),
        graph,
        controls,
        Some(SessionCwd::parse(requested_cwd).unwrap()),
        vec![identity],
        LedgerAdmissionV1::HistoryLedgerAdmitted {
            kind: HistoryAllocationKindV1::Configured,
        },
    )
    .unwrap()
}

fn run_spec_for(attempt_id: &str) -> WorkflowRunSpecV1 {
    run_spec_with_cwd(attempt_id, "/repo")
}

/// One valid `FrozenR2f1bContractV1` with a single custody plan keyed by `digit`
/// (distinct digits give distinct, individually-valid contracts).
fn contract(digit: char) -> FrozenR2f1bContractV1 {
    let plan = FrozenWorktreeCustodyPlanV1 {
        custody_id: WorktreeCustodyIdV1::parse(format!("custody-{}", digit.to_string().repeat(64)))
            .unwrap(),
        checkout_fingerprint: Sha256HexV1::digest(format!("contract-plan-{digit}").as_bytes()),
        target_cwd: SessionCwd::parse("/repo").unwrap(),
    };
    FrozenR2f1bContractV1::with_computed_fingerprint(
        DeadlineActivationV2::ManualOnlyR2f1a,
        vec![plan],
    )
    .unwrap()
}

/// Build and decode a ROOT (ordinal 0) V3 snapshot through the real public
/// `encode_snapshot_v3` / `decode_snapshot_v3` pair, over `run_spec_for(ROOT_ATTEMPT)`
/// and `contract('a')`.
fn predecessor_snapshot() -> WorkflowSnapshotV3 {
    let spec = run_spec_for(ROOT_ATTEMPT);
    let attempt = AttemptIdentity {
        execution_id: ExecutionId::mint().unwrap(),
        attempt_id: AttemptId::parse(ROOT_ATTEMPT).unwrap(),
        ordinal: 0,
        parent_attempt_id: None,
    };
    let bytes = spec.encode_snapshot_v3(attempt, contract('a')).unwrap();
    WorkflowRunSpecV1::decode_snapshot_v3(&bytes).unwrap()
}

/// The one correct successor of `predecessor`: same execution id, a freshly
/// minted attempt id, ordinal advanced by one, parent set to the predecessor's
/// attempt id, digest pinned to the predecessor, and delivery_spec/r2f1b carried
/// over byte-for-byte. Every violation test below clones this and mutates
/// exactly one field.
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

#[test]
fn v3_snapshot_round_trips_through_encode_and_decode() {
    let predecessor = predecessor_snapshot();
    let bytes = predecessor.encode().unwrap();
    let decoded = WorkflowSnapshotV3::decode(&bytes).unwrap();
    assert_eq!(decoded, predecessor);
    assert_eq!(decoded.digest().unwrap(), predecessor.digest().unwrap());
}

/// Discriminates: `WorkflowSnapshotV3::validate()` not propagating
/// `r2f1b.validate()` failures. A tampered `contract_fingerprint` is invalid
/// CONTENT (not merely differently-encoded content), so `decode`'s
/// `snapshot.validate()?` at run_spec.rs:152 rejects it before the
/// byte-equality guard at run_spec.rs:153-155 is ever reached — see
/// `v3_decode_rejects_reordered_but_content_identical_bytes` below for that
/// guard exercised in isolation.
#[test]
fn v3_decode_rejects_a_tampered_contract_fingerprint_field() {
    let predecessor = predecessor_snapshot();
    let bytes = predecessor.encode().unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["r2f1b"]["contract_fingerprint"] =
        serde_json::json!(Sha256HexV1::digest(b"tampered").as_str());
    let tampered = serde_json::to_vec(&value).unwrap();
    assert_eq!(
        WorkflowSnapshotV3::decode(&tampered),
        Err(RunSpecError::InvalidR2f1bContract)
    );
}

/// Discriminates: `decode` accepting bytes whose re-encoding does not match
/// the input -- i.e. skipping or weakening the `snapshot.encode()? != bytes`
/// guard at run_spec.rs:153-155, which is the ONLY thing that can reject this
/// input (content is fully valid, so `validate()` at :152 passes first).
///
/// This workspace enables no `preserve_order` feature for `serde_json`
/// anywhere (checked across every `Cargo.toml`), so `serde_json::Value`'s
/// object representation is a `BTreeMap`: round-tripping valid V3 bytes
/// through `Value` WITHOUT changing any value re-serializes every object's
/// keys in alphabetical order (`attempt`, `delivery_spec`,
/// `predecessor_snapshot_digest`, `r2f1b`, `v`) instead of
/// `WorkflowSnapshotEnvelopeV3`'s declared field order (`v`, `attempt`,
/// `delivery_spec`, `predecessor_snapshot_digest`, `r2f1b`) that
/// `WorkflowSnapshotV3::encode` always produces. The `assert_ne!` below pins
/// that the two byte strings genuinely differ, so a future `preserve_order`
/// feature flip (which would make this input byte-identical to `bytes`, and
/// this test vacuously pass) fails loudly here instead of silently.
///
/// Mutation-checked: deleting run_spec.rs:153-155 locally (reverted after)
/// makes ONLY this test go red; `v3_decode_rejects_a_tampered_contract_fingerprint_field`
/// above is unaffected because it is rejected earlier, at `validate()`.
#[test]
fn v3_decode_rejects_reordered_but_content_identical_bytes() {
    let predecessor = predecessor_snapshot();
    let bytes = predecessor.encode().unwrap();
    // Pretty-printing yields content-identical, non-canonical bytes regardless of
    // serde_json key-order features (workspace feature unification enables
    // `preserve_order` here, so a Value round-trip does NOT reorder keys - the
    // original key-reorder fixture went byte-identical under `--workspace` builds).
    // Whitespace-only divergence passes validate() and can only be rejected by the
    // final `snapshot.encode()? != bytes` guard in decode.
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let noncanonical = serde_json::to_vec_pretty(&value).unwrap();
    assert_ne!(
        noncanonical, bytes,
        "pretty-printed bytes must differ from canonical bytes for this test to \
         isolate the byte-equality guard from the content check"
    );
    assert_eq!(
        WorkflowSnapshotV3::decode(&noncanonical),
        Err(RunSpecError::SnapshotEncoding)
    );
}

/// Positive baseline: a correctly derived retry (fresh attempt id, ordinal+1,
/// correct parent, correct predecessor digest, unchanged delivery_spec/r2f1b)
/// passes.
#[test]
fn validate_successor_accepts_a_correctly_derived_retry() {
    let predecessor = predecessor_snapshot();
    let successor = baseline_successor(&predecessor);
    assert!(WorkflowSnapshotV3::validate_successor(&predecessor, &successor).is_ok());
}

/// Discriminates: `validate_successor` not actually comparing delivery_spec
/// bytes between predecessor and successor (or comparing only a subset of
/// `WorkflowRunSpecV1`).
#[test]
fn validate_successor_rejects_changed_delivery_spec_bytes() {
    let predecessor = predecessor_snapshot();
    let mut successor = baseline_successor(&predecessor);
    successor.delivery_spec = run_spec_with_cwd(ROOT_ATTEMPT, "/repo-alt-delivery");
    assert_eq!(
        WorkflowSnapshotV3::validate_successor(&predecessor, &successor),
        Err(RunSpecError::DeliverySpecChanged)
    );
}

/// Discriminates: `validate_successor` not comparing `r2f1b` at all (only
/// delivery_spec), which would let a retry silently swap its custody contract.
#[test]
fn validate_successor_rejects_changed_r2f1b_contract_bytes() {
    let predecessor = predecessor_snapshot();
    let mut successor = baseline_successor(&predecessor);
    successor.r2f1b = contract('b');
    assert_eq!(
        WorkflowSnapshotV3::validate_successor(&predecessor, &successor),
        Err(RunSpecError::DeliverySpecChanged)
    );
}

/// Discriminates: a retry that reuses its predecessor's attempt id going
/// unnoticed. NOTE: because `delivery_spec` is carried over unchanged (its
/// embedded `attempt_id` is the ROOT attempt id), reusing that same id as the
/// successor's own V3 attempt id trips `successor.validate()`'s own
/// `attempt.attempt_id != delivery_spec.attempt_id` invariant before
/// `validate_successor`'s dedicated `successor.attempt.attempt_id ==
/// predecessor.attempt.attempt_id` check is even reached -- both mechanisms
/// agree this is invalid, and either one firing is a correct rejection.
#[test]
fn validate_successor_rejects_a_reused_attempt_id() {
    let predecessor = predecessor_snapshot();
    let mut successor = baseline_successor(&predecessor);
    successor.attempt.attempt_id = predecessor.attempt.attempt_id.clone();
    assert_eq!(
        WorkflowSnapshotV3::validate_successor(&predecessor, &successor),
        Err(RunSpecError::InvalidAttemptLineage)
    );
}

/// Discriminates: `validate_successor` not verifying
/// `predecessor_snapshot_digest` against the predecessor's actual digest (e.g.
/// only checking it is `Some`).
#[test]
fn validate_successor_rejects_a_wrong_predecessor_digest() {
    let predecessor = predecessor_snapshot();
    let mut successor = baseline_successor(&predecessor);
    successor.predecessor_snapshot_digest = Some(Sha256HexV1::digest(b"not-the-real-predecessor"));
    assert_eq!(
        WorkflowSnapshotV3::validate_successor(&predecessor, &successor),
        Err(RunSpecError::InvalidAttemptLineage)
    );
}

/// Discriminates: `validate_successor` not enforcing that ordinal advances by
/// exactly one (letting an attempt be skipped or replayed out of order).
#[test]
fn validate_successor_rejects_a_skipped_attempt_ordinal() {
    let predecessor = predecessor_snapshot();
    let mut successor = baseline_successor(&predecessor);
    successor.attempt.ordinal = 2;
    assert_eq!(
        WorkflowSnapshotV3::validate_successor(&predecessor, &successor),
        Err(RunSpecError::InvalidAttemptLineage)
    );
}

/// Discriminates: `validate_successor` not verifying `parent_attempt_id` points
/// at the actual predecessor (e.g. only checking it is `Some`).
#[test]
fn validate_successor_rejects_a_wrong_parent_attempt() {
    let predecessor = predecessor_snapshot();
    let mut successor = baseline_successor(&predecessor);
    successor.attempt.parent_attempt_id =
        Some(AttemptId::parse("attempt-77777777777777777777777777777777").unwrap());
    assert_eq!(
        WorkflowSnapshotV3::validate_successor(&predecessor, &successor),
        Err(RunSpecError::InvalidAttemptLineage)
    );
}

/// Discriminates: `validate_successor` not verifying `execution_id` stays
/// constant across the whole retry chain (which would let a retry from a
/// different execution splice into this lineage).
#[test]
fn validate_successor_rejects_a_wrong_execution_id() {
    let predecessor = predecessor_snapshot();
    let mut successor = baseline_successor(&predecessor);
    successor.attempt.execution_id = loop {
        let candidate = ExecutionId::mint().unwrap();
        if candidate != predecessor.attempt.execution_id {
            break candidate;
        }
    };
    assert_eq!(
        WorkflowSnapshotV3::validate_successor(&predecessor, &successor),
        Err(RunSpecError::InvalidAttemptLineage)
    );
}

/// Discriminates: a digest that does not actually cover `delivery_spec`
/// content (e.g. hashing only the `attempt`/`r2f1b` fields). Builds a second,
/// independently self-consistent ROOT snapshot (`alt_predecessor`, same
/// attempt identity as `predecessor` but a different `requested_cwd`, hence
/// different delivery bytes and a different digest) and validates the
/// ORIGINAL predecessor's successor against it. If the digest ignored
/// delivery_spec content, this swap would go undetected via the digest and
/// fall through to a `DeliverySpecChanged`-or-Ok outcome; instead it must be
/// caught as a lineage violation at the digest-equality check, upstream of the
/// dedicated delivery-bytes comparison.
#[test]
fn validate_successor_rejects_a_swapped_origin_delivery() {
    let predecessor = predecessor_snapshot();
    let successor = baseline_successor(&predecessor);

    let alt_spec = run_spec_with_cwd(ROOT_ATTEMPT, "/repo-alt-origin");
    let alt_attempt = AttemptIdentity {
        execution_id: predecessor.attempt.execution_id.clone(),
        attempt_id: AttemptId::parse(ROOT_ATTEMPT).unwrap(),
        ordinal: 0,
        parent_attempt_id: None,
    };
    let alt_bytes = alt_spec
        .encode_snapshot_v3(alt_attempt, contract('a'))
        .unwrap();
    let alt_predecessor = WorkflowRunSpecV1::decode_snapshot_v3(&alt_bytes).unwrap();
    assert_ne!(
        alt_predecessor.digest().unwrap(),
        predecessor.digest().unwrap()
    );

    assert_eq!(
        WorkflowSnapshotV3::validate_successor(&alt_predecessor, &successor),
        Err(RunSpecError::InvalidAttemptLineage)
    );
}

/// Discriminates: `decode_snapshot_v3` (the V3 path) accidentally accepting a
/// V2 envelope's bytes (e.g. by treating an absent/mismatched `r2f1b` field as
/// optional instead of required).
#[test]
fn v2_snapshot_bytes_are_rejected_by_the_v3_decode_path() {
    let spec = run_spec_for(ROOT_ATTEMPT);
    let v2_bytes = spec.encode_snapshot_v2().unwrap();
    assert!(matches!(
        WorkflowRunSpecV1::decode_snapshot_v3(&v2_bytes),
        Err(RunSpecError::SnapshotEncoding)
    ));
}

/// Discriminates: `decode_snapshot_v2` (the V2 path used by
/// `r2f1a_run_spec.rs`) accidentally accepting a V3 envelope's bytes.
#[test]
fn v3_snapshot_bytes_are_rejected_by_the_v2_decode_path() {
    let predecessor = predecessor_snapshot();
    let v3_bytes = predecessor.encode().unwrap();
    assert!(matches!(
        WorkflowRunSpecV1::decode_snapshot_v2(&v3_bytes),
        Err(RunSpecError::SnapshotEncoding)
    ));
}
