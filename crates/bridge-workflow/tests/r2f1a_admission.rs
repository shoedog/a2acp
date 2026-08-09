use async_trait::async_trait;
use bridge_core::domain::{AgentEntry, AgentKind, Effort, RegistrySnapshot};
use bridge_core::error::BridgeError;
use bridge_core::execution_policy::{
    freeze_worktree_checkout_v1, ExecutionPolicyInvocationV1, FrozenCheckoutEffectV1,
    FrozenProviderLogicalSessionV1, HistoryAllocationKindV1, LedgerAdmissionV1,
    LivenessProfileIdV1, ProviderEffectKeyV1, TaskClassV1, WorkflowControlDefaultsV1,
    WorktreeCheckoutInputV1,
};
use bridge_core::ids::{AgentId, AttemptIdentity, NodeId, WorkflowId};
use bridge_core::ports::{AgentRegistry, Resolved};
use bridge_core::SessionCwd;
use bridge_workflow::admission::{
    CheckoutPlanInputV1, DirectWorkflowCheckoutPlannerV1, WorkflowAdmissionRequestV1,
    WorkflowAdmissionV1, WorkflowCheckoutPlannerV1,
};
use bridge_workflow::graph::{WorkflowGraph, WorkflowNode};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

struct SnapshotRegistry {
    entry: Arc<AgentEntry>,
}

#[async_trait]
impl AgentRegistry for SnapshotRegistry {
    async fn resolve(&self, _id: &AgentId) -> Result<Resolved, BridgeError> {
        panic!("admission must not resolve a backend")
    }

    fn default_id(&self) -> AgentId {
        self.entry.id.clone()
    }

    async fn apply(&self, _snapshot: RegistrySnapshot) -> Result<(), BridgeError> {
        panic!("admission must not mutate the registry")
    }

    fn entry_snapshot(&self, id: &AgentId) -> Option<Arc<AgentEntry>> {
        (id == &self.entry.id).then(|| self.entry.clone())
    }

    fn list(&self) -> Vec<AgentId> {
        vec![self.entry.id.clone()]
    }
}

struct WorktreePlanner;

#[async_trait]
impl WorkflowCheckoutPlannerV1 for WorktreePlanner {
    async fn freeze_checkout(
        &self,
        _entry: &AgentEntry,
        input: &CheckoutPlanInputV1,
    ) -> Result<FrozenCheckoutEffectV1, BridgeError> {
        freeze_worktree_checkout_v1(&WorktreeCheckoutInputV1 {
            attempt_id: input.attempt_id.clone(),
            node: input.node.clone(),
            logical_session: input.logical_session,
            source_cwd: input.source_cwd.clone(),
            canonical_source_cwd: SessionCwd::parse("/allowed/repo")?,
            canonical_worktree_root: SessionCwd::parse("/worktrees")?,
            worktree_owner: "owner1".into(),
        })
        .map_err(|error| BridgeError::ConfigInvalid {
            reason: error.to_string(),
        })
    }
}

fn entry() -> AgentEntry {
    AgentEntry {
        id: AgentId::parse("reader").unwrap(),
        cmd: Some("reader".into()),
        base_url: None,
        api_key_env: None,
        args: vec![],
        kind: AgentKind::Acp,
        model_provider: None,
        model: Some("primary".into()),
        effort: Some(Effort::Max),
        mode: Some("read-only".into()),
        preflight: true,
        fallback_models: vec!["fallback".into()],
        cwd: Some("relative-default".into()),
        session_cwd: None,
        sandbox: None,
        watchdog: None,
        auth_method: None,
        pre_authenticated: false,
        host_fallback_eligible: false,
        name: None,
        description: None,
        tags: vec![],
        version: None,
        mcp: vec![],
        mcp_delivery: Default::default(),
        extensions: BTreeMap::new(),
    }
}

fn graph() -> Arc<WorkflowGraph> {
    Arc::new(WorkflowGraph {
        id: WorkflowId::parse("review").unwrap(),
        nodes: vec![WorkflowNode {
            id: NodeId::parse("inspect").unwrap(),
            agent: AgentId::parse("reader").unwrap(),
            prompt_template: "{{input}}".into(),
            inputs: vec![],
            retry: None,
            harvest_sanitization: None,
        }],
        panel: None,
        controls: Some(WorkflowControlDefaultsV1 {
            task_class: Some(TaskClassV1::ReviewHighXhigh),
            liveness_profile: Some(LivenessProfileIdV1::ReviewHighXhighV1),
            max_work_cutoff_ms: Some(7_200_001),
            max_reason: Some("max review is operator-qualified".into()),
            ..WorkflowControlDefaultsV1::default()
        }),
    })
}

fn request(
    attempt_id: bridge_core::ids::AttemptId,
    graph: Arc<WorkflowGraph>,
    requested_session_cwd: Option<SessionCwd>,
) -> WorkflowAdmissionRequestV1 {
    WorkflowAdmissionRequestV1 {
        attempt_id,
        graph,
        requested_session_cwd,
        policy_invocation: ExecutionPolicyInvocationV1::default(),
        ledger_admission: LedgerAdmissionV1::HistoryLedgerAdmitted {
            kind: HistoryAllocationKindV1::Configured,
        },
        r2f1b: None,
    }
}

fn admission(configured: AgentEntry, launch_cwd: &str) -> WorkflowAdmissionV1 {
    WorkflowAdmissionV1::new(
        Arc::new(SnapshotRegistry {
            entry: Arc::new(configured),
        }),
        Arc::new(DirectWorkflowCheckoutPlannerV1),
        SessionCwd::parse(launch_cwd).unwrap(),
        None,
    )
}

fn frozen_source(admitted: &bridge_workflow::admission::AdmittedWorkflowRunV1) -> &SessionCwd {
    admitted.run_spec.node_execution_identities[0].provider_attempts[0]
        .checkout
        .source_cwd()
}

#[tokio::test]
async fn admission_freezes_complete_attempt_matrix_before_provider_effects() {
    let registry: Arc<dyn AgentRegistry> = Arc::new(SnapshotRegistry {
        entry: Arc::new(entry()),
    });
    let admission = WorkflowAdmissionV1::new(
        registry,
        Arc::new(WorktreePlanner),
        SessionCwd::parse("/launch").unwrap(),
        Some(Arc::new(ProviderEffectKeyV1::from_bytes([7; 32]))),
    );
    let attempt_id = AttemptIdentity::initial().unwrap().attempt_id;
    let admitted = admission
        .freeze(request(
            attempt_id.clone(),
            graph(),
            Some(SessionCwd::parse("/allowed/repo").unwrap()),
        ))
        .await
        .unwrap();

    admitted.run_spec.validate().unwrap();
    assert_eq!(admitted.run_spec.attempt_id, attempt_id);
    assert_eq!(admitted.run_spec.node_execution_identities.len(), 1);
    let identity = &admitted.run_spec.node_execution_identities[0];
    assert_eq!(identity.provider_attempts.len(), 4);
    assert_eq!(
        identity.provider_attempts[0].logical_session,
        FrozenProviderLogicalSessionV1::Preflight {
            candidate_ordinal: 0
        }
    );
    assert_eq!(
        identity.provider_attempts[3].logical_session,
        FrozenProviderLogicalSessionV1::Execute {
            candidate_ordinal: 1
        }
    );
    assert!(identity
        .provider_attempts
        .iter()
        .all(|attempt| matches!(attempt.checkout, FrozenCheckoutEffectV1::Worktree { .. })));
    let encoded = admitted.run_spec.encode_snapshot_v2().unwrap();
    assert_eq!(
        bridge_workflow::run_spec::WorkflowRunSpecV1::decode_snapshot_v2(&encoded).unwrap(),
        *admitted.run_spec
    );
}

#[tokio::test]
async fn admission_refuses_missing_agent_before_registry_or_provider_effects() {
    let mut missing = (*graph()).clone();
    missing.nodes[0].agent = AgentId::parse("missing").unwrap();
    let result = admission(entry(), "/launch")
        .freeze(request(
            AttemptIdentity::initial().unwrap().attempt_id,
            Arc::new(missing),
            None,
        ))
        .await;
    assert!(matches!(
        result,
        Err(BridgeError::ConfigInvalid { reason })
            if reason == "workflow agent has no immutable registry snapshot"
    ));
}

#[tokio::test]
async fn cwd_precedence_and_relative_fallback_are_frozen_once() {
    let requested = admission(entry(), "/launch")
        .freeze(request(
            AttemptIdentity::initial().unwrap().attempt_id,
            graph(),
            Some(SessionCwd::parse("/request").unwrap()),
        ))
        .await
        .unwrap();
    assert_eq!(frozen_source(&requested).as_str(), "/request");

    let mut configured = entry();
    configured.session_cwd = Some("session-default".into());
    configured.cwd = Some("cwd-default".into());
    let session_default = admission(configured, "/launch")
        .freeze(request(
            AttemptIdentity::initial().unwrap().attempt_id,
            graph(),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        frozen_source(&session_default).as_str(),
        "/launch/session-default"
    );

    let cwd_default = admission(entry(), "/launch")
        .freeze(request(
            AttemptIdentity::initial().unwrap().attempt_id,
            graph(),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        frozen_source(&cwd_default).as_str(),
        "/launch/relative-default"
    );

    let mut no_default = entry();
    no_default.cwd = None;
    let launch_default = admission(no_default, "/launch")
        .freeze(request(
            AttemptIdentity::initial().unwrap().attempt_id,
            graph(),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(frozen_source(&launch_default).as_str(), "/launch");
}

#[tokio::test]
async fn provider_effect_key_overlap_refuses_fresh_and_restored_runs() {
    let overlapping = admission(entry(), "/launch")
        .with_provider_effect_key_path(PathBuf::from("/request/.bridge-provider-effect.key"));
    let result = overlapping
        .freeze(request(
            AttemptIdentity::initial().unwrap().attempt_id,
            graph(),
            Some(SessionCwd::parse("/request").unwrap()),
        ))
        .await;
    assert!(matches!(
        result,
        Err(BridgeError::ConfigInvalid { reason })
            if reason == "provider-effect key path overlaps the workflow session root"
    ));

    let admitted = admission(entry(), "/launch")
        .freeze(request(
            AttemptIdentity::initial().unwrap().attempt_id,
            graph(),
            Some(SessionCwd::parse("/request").unwrap()),
        ))
        .await
        .unwrap();
    let restored = admission(entry(), "/launch")
        .with_provider_effect_key_path(PathBuf::from("/request/.bridge-provider-effect.key"))
        .restore((*admitted.run_spec).clone());
    assert!(matches!(
        restored,
        Err(BridgeError::ConfigInvalid { reason })
            if reason == "provider-effect key path overlaps the workflow session root"
    ));
}

#[tokio::test]
async fn persisted_attempt_identity_partitions_every_worktree_target() {
    let registry: Arc<dyn AgentRegistry> = Arc::new(SnapshotRegistry {
        entry: Arc::new(entry()),
    });
    let admission = WorkflowAdmissionV1::new(
        registry,
        Arc::new(WorktreePlanner),
        SessionCwd::parse("/launch").unwrap(),
        Some(Arc::new(ProviderEffectKeyV1::from_bytes([7; 32]))),
    );
    let first = admission
        .freeze(request(
            AttemptIdentity::initial().unwrap().attempt_id,
            graph(),
            Some(SessionCwd::parse("/allowed/repo").unwrap()),
        ))
        .await
        .unwrap();
    let second = admission
        .freeze(request(
            AttemptIdentity::initial().unwrap().attempt_id,
            graph(),
            Some(SessionCwd::parse("/allowed/repo").unwrap()),
        ))
        .await
        .unwrap();

    let first_attempts = &first.run_spec.node_execution_identities[0].provider_attempts;
    let second_attempts = &second.run_spec.node_execution_identities[0].provider_attempts;
    assert_eq!(first_attempts.len(), second_attempts.len());
    for (left, right) in first_attempts.iter().zip(second_attempts) {
        assert_ne!(
            left.checkout.effective_cwd(),
            right.checkout.effective_cwd()
        );
    }
    for attempts in [first_attempts, second_attempts] {
        for pair in attempts.windows(2) {
            assert_ne!(
                pair[0].checkout.effective_cwd(),
                pair[1].checkout.effective_cwd()
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// R2f1b slice 2b2 — the production admission boundary for an R2f1b contract.
//
// The refusal is deliberately NOT in `FrozenR2f1bContractV1::validate` (slice-2 brief §3, Sol 13):
// that would make `with_computed_fingerprint` fail for automatic activation and break the landed
// A3/A5 offline construction and workload-identity work, which slice 4 needs to keep building on.
// ---------------------------------------------------------------------------------------------

fn contract_for(
    admitted: &bridge_workflow::admission::AdmittedWorkflowRunV1,
    activation: bridge_core::execution_policy::DeadlineActivationV2,
) -> bridge_core::execution_policy::FrozenR2f1bContractV1 {
    let mut plans = Vec::new();
    for identity in &admitted.run_spec.node_execution_identities {
        for attempt in &identity.provider_attempts {
            if let FrozenCheckoutEffectV1::Worktree {
                target_cwd,
                checkout_digest,
                ..
            } = &attempt.checkout
            {
                plans.push(bridge_core::execution_policy::FrozenWorktreeCustodyPlanV1 {
                    custody_id: bridge_core::execution_policy::WorktreeCustodyIdV1::parse(format!(
                        "custody-{}",
                        &bridge_core::execution_policy::Sha256HexV1::digest(
                            checkout_digest.as_str().as_bytes()
                        )
                        .as_str()[..64]
                    ))
                    .unwrap(),
                    checkout_fingerprint: checkout_digest.clone(),
                    target_cwd: target_cwd.clone(),
                });
            }
        }
    }
    bridge_core::execution_policy::FrozenR2f1bContractV1::with_computed_fingerprint(
        activation, plans,
    )
    .unwrap()
}

async fn admit_with_contract(
    activation: bridge_core::execution_policy::DeadlineActivationV2,
) -> Result<bridge_workflow::admission::AdmittedWorkflowRunV1, BridgeError> {
    let registry: Arc<dyn AgentRegistry> = Arc::new(SnapshotRegistry {
        entry: Arc::new(entry()),
    });
    let admission = WorkflowAdmissionV1::new(
        registry,
        Arc::new(WorktreePlanner),
        SessionCwd::parse("/launch").unwrap(),
        None,
    );
    let attempt = AttemptIdentity::initial().unwrap();
    // Freeze once with no contract to learn the frozen checkout digests, then offer a contract
    // whose plans cover them exactly. This is the shape slice 4 will have to produce.
    let probe = admission
        .freeze(request(
            attempt.attempt_id.clone(),
            graph(),
            Some(SessionCwd::parse("/allowed/repo").unwrap()),
        ))
        .await
        .unwrap();
    let mut with_contract = request(
        attempt.attempt_id.clone(),
        graph(),
        Some(SessionCwd::parse("/allowed/repo").unwrap()),
    );
    with_contract.r2f1b = Some(bridge_workflow::admission::R2f1bAdmissionV1 {
        attempt,
        contract: contract_for(&probe, activation),
    });
    admission.freeze(with_contract).await
}

/// The mandated refusal. Discriminates an admission that accepts an automatically armed contract
/// — which would let slice 2's writer become production-reachable with no preparation flight, no
/// timer owner, and no runner, defeating the §5.2 inactive-writer ruling this whole slice rests on.
#[tokio::test]
async fn automatic_r2f1b_refused_at_production_admission() {
    let refused =
        admit_with_contract(bridge_core::execution_policy::DeadlineActivationV2::AutomaticR2f1b)
            .await;

    let Err(BridgeError::ConfigInvalid { reason }) = refused else {
        panic!("automatic activation must be refused at admission");
    };
    assert!(
        reason.contains("automatic R2f1b deadline activation is refused"),
        "unexpected refusal reason: {reason}"
    );
}

/// The other half of the pair: manual activation is ADMITTED, and its plans are carried forward.
/// Without this, `automatic_r2f1b_refused_at_production_admission` would pass just as well
/// against an admission that refused every contract, which proves nothing about the activation.
#[tokio::test]
async fn manual_only_r2f1a_admitted() {
    let admitted =
        admit_with_contract(bridge_core::execution_policy::DeadlineActivationV2::ManualOnlyR2f1a)
            .await
            .expect("a manual-only contract is admissible");

    let r2f1b = admitted.r2f1b.expect("the admitted contract is carried");
    assert_eq!(
        r2f1b.contract.activation,
        bridge_core::execution_policy::DeadlineActivationV2::ManualOnlyR2f1a
    );
    assert_eq!(
        r2f1b.contract.custody_plans.len(),
        admitted.run_spec.node_execution_identities[0]
            .provider_attempts
            .len(),
        "every frozen worktree checkout must be covered by exactly one plan"
    );
}

/// The A3/A5 regression guard. Offline construction, encoding, decoding and workload identity of
/// an automatic contract stay legal — the refusal lives at the admission boundary, not in the
/// type. Discriminates the tempting-but-wrong placement inside
/// `FrozenR2f1bContractV1::validate`, which every one of these calls goes through.
#[test]
fn offline_automatic_contract_construction_still_legal() {
    let plan = bridge_core::execution_policy::FrozenWorktreeCustodyPlanV1 {
        custody_id: bridge_core::execution_policy::WorktreeCustodyIdV1::parse(format!(
            "custody-{}",
            "a".repeat(64)
        ))
        .unwrap(),
        checkout_fingerprint: bridge_core::execution_policy::Sha256HexV1::parse("1".repeat(64))
            .unwrap(),
        target_cwd: SessionCwd::parse("/wt/one").unwrap(),
    };
    let contract = bridge_core::execution_policy::FrozenR2f1bContractV1::with_computed_fingerprint(
        bridge_core::execution_policy::DeadlineActivationV2::AutomaticR2f1b,
        vec![plan],
    )
    .expect("offline construction of an automatic contract stays legal");
    contract
        .validate()
        .expect("validate must not enforce the admission rule");
}

/// A contract whose plans do not cover every frozen worktree checkout is refused BEFORE the run
/// starts. Discriminates deferring coverage to per-node bind time, which would let a graph
/// materialize some checkouts and then refuse mid-run, leaving live siblings behind.
#[tokio::test]
async fn incomplete_custody_plan_coverage_is_refused_at_admission() {
    let registry: Arc<dyn AgentRegistry> = Arc::new(SnapshotRegistry {
        entry: Arc::new(entry()),
    });
    let admission = WorkflowAdmissionV1::new(
        registry,
        Arc::new(WorktreePlanner),
        SessionCwd::parse("/launch").unwrap(),
        None,
    );
    let attempt = AttemptIdentity::initial().unwrap();
    let probe = admission
        .freeze(request(
            attempt.attempt_id.clone(),
            graph(),
            Some(SessionCwd::parse("/allowed/repo").unwrap()),
        ))
        .await
        .unwrap();
    let mut contract = contract_for(
        &probe,
        bridge_core::execution_policy::DeadlineActivationV2::ManualOnlyR2f1a,
    );
    contract
        .custody_plans
        .truncate(contract.custody_plans.len() - 1);
    let contract = bridge_core::execution_policy::FrozenR2f1bContractV1::with_computed_fingerprint(
        contract.activation,
        contract.custody_plans,
    )
    .unwrap();
    let mut incomplete = request(
        attempt.attempt_id.clone(),
        graph(),
        Some(SessionCwd::parse("/allowed/repo").unwrap()),
    );
    incomplete.r2f1b = Some(bridge_workflow::admission::R2f1bAdmissionV1 { attempt, contract });

    let Err(BridgeError::ConfigInvalid { reason }) = admission.freeze(incomplete).await else {
        panic!("incomplete custody-plan coverage must be refused");
    };
    assert!(
        reason.contains("custody plan coverage is incomplete"),
        "unexpected refusal reason: {reason}"
    );
}

/// A successor attempt cannot enter through fresh admission: §5.8's claim exchange is 2d's
/// mechanism and slice 5's production path. Discriminates an admission that accepts any attempt
/// identity, which would route a claim-less V3 write over a predecessor's live checkout.
#[tokio::test]
async fn a_successor_attempt_identity_is_refused_by_fresh_admission() {
    let registry: Arc<dyn AgentRegistry> = Arc::new(SnapshotRegistry {
        entry: Arc::new(entry()),
    });
    let admission = WorkflowAdmissionV1::new(
        registry,
        Arc::new(WorktreePlanner),
        SessionCwd::parse("/launch").unwrap(),
        None,
    );
    let attempt = AttemptIdentity::initial().unwrap();
    let successor = attempt.clone().resume().unwrap();
    let probe = admission
        .freeze(request(
            attempt.attempt_id.clone(),
            graph(),
            Some(SessionCwd::parse("/allowed/repo").unwrap()),
        ))
        .await
        .unwrap();
    let mut resumed = request(
        attempt.attempt_id.clone(),
        graph(),
        Some(SessionCwd::parse("/allowed/repo").unwrap()),
    );
    resumed.r2f1b = Some(bridge_workflow::admission::R2f1bAdmissionV1 {
        attempt: successor,
        contract: contract_for(
            &probe,
            bridge_core::execution_policy::DeadlineActivationV2::ManualOnlyR2f1a,
        ),
    });

    let Err(BridgeError::ConfigInvalid { reason }) = admission.freeze(resumed).await else {
        panic!("a successor attempt must be refused by fresh admission");
    };
    assert!(
        reason.contains("fresh attempt identity"),
        "unexpected refusal reason: {reason}"
    );
}
