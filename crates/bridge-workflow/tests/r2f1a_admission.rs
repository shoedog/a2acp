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
