use async_trait::async_trait;
use bridge_core::domain::{AgentEntry, AgentKind, Part, RegistrySnapshot};
use bridge_core::error::BridgeError;
use bridge_core::execution_policy::{
    freeze_node_execution_identity_v1, freeze_provider_attempt_v1, freeze_worktree_checkout_v1,
    resolve_execution_policy_v1, BoundMcpDeliveryPayloadV1, BoundSessionSpecV1,
    ExecutionPolicyInvocationV1, FrozenProviderLogicalSessionV1, HistoryAllocationKindV1,
    LedgerAdmissionV1, PolicyActivationV1, PolicyNodeRefV1, ProviderFreezeInputV1,
    WorkflowControlDefaultsV1, WorktreeCheckoutInputV1,
};
use bridge_core::ids::{AgentId, AttemptId, NodeId, SessionId, WorkflowId};
use bridge_core::mcp::{McpDelivery, McpServerSpec};
use bridge_core::ports::{
    AgentBackend, AgentRegistry, BackendStream, BoundEntryUseV1, EntryUseTokenV1, Lease, Resolved,
    Update,
};
use bridge_core::SessionCwd;
use bridge_workflow::executor::{
    WorkflowDiagnosticContext, WorkflowEvent, WorkflowExecutor, WorkflowOutcome, WorkflowRunContext,
};
use bridge_workflow::graph::{WorkflowGraph, WorkflowNode};
use bridge_workflow::run_spec::WorkflowRunSpecV1;
use futures::StreamExt;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

struct NoopLease;
impl Lease for NoopLease {}

#[derive(Default)]
struct Calls {
    unbound_resolves: AtomicUsize,
    binds: AtomicUsize,
    bound_resolves: AtomicUsize,
    legacy_configures: AtomicUsize,
    bound_specs: Mutex<Vec<BoundSessionSpecV1>>,
}

struct RecordingBackend {
    calls: Arc<Calls>,
}

#[async_trait]
impl AgentBackend for RecordingBackend {
    async fn prompt(
        &self,
        _session: &SessionId,
        _parts: Vec<Part>,
    ) -> Result<BackendStream, BridgeError> {
        Ok(Box::pin(tokio_stream::iter(vec![
            Ok(Update::Text("BOUND_OK".into())),
            Ok(Update::done("end_turn")),
        ])))
    }

    async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
        Ok(())
    }

    async fn configure_session(
        &self,
        _session: &SessionId,
        _spec: &bridge_core::domain::SessionSpec,
    ) -> Result<(), BridgeError> {
        self.calls.legacy_configures.fetch_add(1, Ordering::SeqCst);
        Err(BridgeError::ConfigInvalid {
            reason: "legacy configure reached".into(),
        })
    }

    async fn configure_bound_session(
        &self,
        _session: &SessionId,
        spec: &BoundSessionSpecV1,
    ) -> Result<(), BridgeError> {
        self.calls.bound_specs.lock().unwrap().push(spec.clone());
        Ok(())
    }
}

struct BoundOnlyRegistry {
    entry: Arc<AgentEntry>,
    backend: Arc<dyn AgentBackend>,
    slot: Arc<()>,
    calls: Arc<Calls>,
}

#[async_trait]
impl AgentRegistry for BoundOnlyRegistry {
    async fn resolve(&self, _id: &AgentId) -> Result<Resolved, BridgeError> {
        self.calls.unbound_resolves.fetch_add(1, Ordering::SeqCst);
        Err(BridgeError::ConfigInvalid {
            reason: "unbound resolve reached".into(),
        })
    }

    fn bind_entry_use(&self, id: &AgentId) -> Option<BoundEntryUseV1> {
        assert_eq!(id, &self.entry.id);
        self.calls.binds.fetch_add(1, Ordering::SeqCst);
        Some(BoundEntryUseV1 {
            entry: self.entry.clone(),
            lease: Box::new(NoopLease),
            use_token: EntryUseTokenV1::new(self.slot.clone(), &self.entry, 1),
        })
    }

    async fn resolve_bound(
        &self,
        bound: &BoundEntryUseV1,
        _effect: &bridge_core::execution_policy::BoundProviderEffectV1,
        _observer: Arc<dyn bridge_core::ports::DiagnosticObserver>,
    ) -> Result<Arc<dyn AgentBackend>, BridgeError> {
        assert!(bound.use_token.downcast_slot::<()>().is_some());
        assert!(bound.use_token.matches_entry(&self.entry));
        self.calls.bound_resolves.fetch_add(1, Ordering::SeqCst);
        Ok(self.backend.clone())
    }

    fn default_id(&self) -> AgentId {
        self.entry.id.clone()
    }

    fn list(&self) -> Vec<AgentId> {
        vec![self.entry.id.clone()]
    }

    async fn apply(&self, _snapshot: RegistrySnapshot) -> Result<(), BridgeError> {
        unreachable!("the executor must not mutate registry configuration")
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
        mcp: vec![McpServerSpec {
            name: "repo-reader".into(),
            command: "reader".into(),
            args: vec!["--root={cwd}".into()],
            env: vec![],
        }],
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

fn frozen_worktree_run(entry: &AgentEntry) -> WorkflowRunSpecV1 {
    let attempt_id = AttemptId::parse("attempt-22222222222222222222222222222222").unwrap();
    let source_cwd = SessionCwd::parse("/repo/source").unwrap();
    let node_ref = PolicyNodeRefV1::from_node_id(0, "review-node");
    let logical_session = FrozenProviderLogicalSessionV1::Execute {
        candidate_ordinal: 0,
    };
    let checkout = freeze_worktree_checkout_v1(&WorktreeCheckoutInputV1 {
        attempt_id: attempt_id.clone(),
        node: node_ref.clone(),
        logical_session,
        source_cwd: source_cwd.clone(),
        canonical_source_cwd: source_cwd.clone(),
        canonical_worktree_root: SessionCwd::parse("/private/tmp/a2a-bound-worktrees").unwrap(),
        worktree_owner: "bound-test".into(),
    })
    .unwrap();
    let provider = freeze_provider_attempt_v1(&ProviderFreezeInputV1 {
        entry,
        overrides: None,
        node: node_ref.clone(),
        logical_session,
        checkout,
        provider_effect_key: None,
    })
    .unwrap();
    let identity = freeze_node_execution_identity_v1(node_ref, vec![provider]).unwrap();
    let graph = WorkflowGraph {
        id: WorkflowId::parse("review").unwrap(),
        nodes: vec![WorkflowNode {
            id: NodeId::parse("review-node").unwrap(),
            agent: entry.id.clone(),
            prompt_template: "{{input}}".into(),
            inputs: vec![],
            retry: None,
            harvest_sanitization: None,
        }],
        panel: None,
        controls: Some(WorkflowControlDefaultsV1::default()),
    };
    let controls = resolve_execution_policy_v1(
        graph.controls.as_ref().unwrap(),
        &ExecutionPolicyInvocationV1::default(),
        false,
        PolicyActivationV1::Production,
    )
    .unwrap();
    WorkflowRunSpecV1::build(
        attempt_id,
        graph,
        controls,
        Some(source_cwd),
        vec![identity],
        LedgerAdmissionV1::HistoryLedgerAdmitted {
            kind: HistoryAllocationKindV1::Configured,
        },
    )
    .unwrap()
}

#[tokio::test]
async fn v2_worktree_attempt_uses_only_bound_registry_cwd_and_delivery() {
    let entry = Arc::new(entry());
    let run_spec = Arc::new(frozen_worktree_run(&entry));
    run_spec.validate().unwrap();
    let calls = Arc::new(Calls::default());
    let backend: Arc<dyn AgentBackend> = Arc::new(RecordingBackend {
        calls: calls.clone(),
    });
    let registry = Arc::new(BoundOnlyRegistry {
        entry,
        backend,
        slot: Arc::new(()),
        calls: calls.clone(),
    });
    let request = WorkflowRunContext {
        session_cwd: run_spec.requested_session_cwd.clone(),
        ..WorkflowRunContext::default()
    };
    let context = WorkflowDiagnosticContext::in_memory(request)
        .with_frozen_run_spec(run_spec.clone(), None)
        .unwrap();
    let executor = WorkflowExecutor::new(registry);
    let mut stream = executor.run_with_diagnostic_context(
        Arc::new(run_spec.graph.clone()),
        "review this".into(),
        "bound-run".into(),
        CancellationToken::new(),
        context,
    );
    let mut node_ok = None;
    let mut terminal = None;
    while let Some(event) = stream.next().await {
        match event.unwrap() {
            WorkflowEvent::NodeFinished { ok, .. } => node_ok = Some(ok),
            WorkflowEvent::Terminal { outcome, output } => terminal = Some((outcome, output)),
            WorkflowEvent::NodeStarted { .. } | WorkflowEvent::CleanupObserved { .. } => {}
        }
    }

    assert_eq!(node_ok, Some(true), "V2 must never take the unbound path");
    assert_eq!(
        terminal,
        Some((WorkflowOutcome::Completed, "BOUND_OK".into()))
    );
    assert_eq!(calls.unbound_resolves.load(Ordering::SeqCst), 0);
    assert_eq!(calls.binds.load(Ordering::SeqCst), 1);
    assert_eq!(calls.bound_resolves.load(Ordering::SeqCst), 1);
    assert_eq!(calls.legacy_configures.load(Ordering::SeqCst), 0);
    let specs = calls.bound_specs.lock().unwrap();
    assert_eq!(specs.len(), 1);
    let persisted = &run_spec.node_execution_identities[0].provider_attempts[0];
    assert_eq!(specs[0].provider_effect.frozen(), persisted);
    assert_eq!(
        specs[0].session.cwd.as_ref(),
        Some(persisted.checkout.effective_cwd())
    );
    let BoundMcpDeliveryPayloadV1::Acp(servers) = specs[0].provider_effect.delivery().payload()
    else {
        panic!("expected frozen ACP delivery")
    };
    assert_eq!(
        servers[0].args,
        vec![format!(
            "--root={}",
            persisted.checkout.effective_cwd().as_str()
        )]
    );
    assert_ne!(
        persisted.checkout.effective_cwd(),
        run_spec.requested_session_cwd.as_ref().unwrap()
    );
}
