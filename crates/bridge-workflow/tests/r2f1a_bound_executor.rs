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
    NodeTurn, WorkflowDiagnosticContext, WorkflowEvent, WorkflowExecutor, WorkflowNodeDispatcher,
    WorkflowOutcome, WorkflowRunContext,
};
use bridge_workflow::graph::{RetryPolicy, WorkflowGraph, WorkflowNode};
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
    bound_invalidations: AtomicUsize,
    unbound_invalidations: AtomicUsize,
    legacy_configures: AtomicUsize,
    main_prompts: AtomicUsize,
    fail_main_prompts: AtomicUsize,
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
        parts: Vec<Part>,
    ) -> Result<BackendStream, BridgeError> {
        let is_preflight = parts
            .first()
            .is_some_and(|part| part.text == "Reply with exactly PONG and nothing else.");
        let reply = if is_preflight {
            "PONG"
        } else {
            let prompt = self.calls.main_prompts.fetch_add(1, Ordering::SeqCst);
            if prompt < self.calls.fail_main_prompts.load(Ordering::SeqCst) {
                return Err(BridgeError::AgentOverloaded);
            }
            "BOUND_OK"
        };
        Ok(Box::pin(tokio_stream::iter(vec![
            Ok(Update::Text(reply.into())),
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
    fail_preflight_ordinal: Option<u16>,
}

#[derive(Default)]
struct LegacyDispatcher {
    checkouts: AtomicUsize,
}

#[async_trait]
impl WorkflowNodeDispatcher for LegacyDispatcher {
    async fn checkout(
        &self,
        _wf_id: &str,
        _node: &WorkflowNode,
        _run_id: &str,
        _ctx: &WorkflowRunContext,
    ) -> Result<NodeTurn, BridgeError> {
        self.checkouts.fetch_add(1, Ordering::SeqCst);
        Err(BridgeError::ConfigInvalid {
            reason: "legacy dispatcher reached".into(),
        })
    }
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
        if matches!(
            _effect.frozen().logical_session,
            FrozenProviderLogicalSessionV1::Preflight { candidate_ordinal }
                if Some(candidate_ordinal) == self.fail_preflight_ordinal
        ) {
            return Err(BridgeError::AgentCrashed {
                reason: "injected pre-acceptance preflight resolve failure".into(),
            });
        }
        Ok(self.backend.clone())
    }

    async fn invalidate_bound(
        &self,
        bound: &BoundEntryUseV1,
        effect_digest: &bridge_core::execution_policy::Sha256HexV1,
    ) {
        assert!(bound.use_token.matches_entry(&self.entry));
        assert!(!effect_digest.as_str().is_empty());
        self.calls
            .bound_invalidations
            .fetch_add(1, Ordering::SeqCst);
    }

    async fn invalidate(&self, _agent: &AgentId) {
        self.calls
            .unbound_invalidations
            .fetch_add(1, Ordering::SeqCst);
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

fn frozen_worktree_run_with_retry(
    entry: &AgentEntry,
    retry: Option<RetryPolicy>,
) -> WorkflowRunSpecV1 {
    let attempt_id = AttemptId::parse("attempt-22222222222222222222222222222222").unwrap();
    let source_cwd = SessionCwd::parse("/repo/source").unwrap();
    let node_ref = PolicyNodeRefV1::from_node_id(0, "review-node");
    let mut logical_sessions = Vec::new();
    if entry.preflight {
        for ordinal in 0..=entry.fallback_models.len() {
            let candidate_ordinal = u16::try_from(ordinal).unwrap();
            logical_sessions.push(FrozenProviderLogicalSessionV1::Preflight { candidate_ordinal });
            logical_sessions.push(FrozenProviderLogicalSessionV1::Execute { candidate_ordinal });
        }
    } else {
        logical_sessions.push(FrozenProviderLogicalSessionV1::Execute {
            candidate_ordinal: 0,
        });
    }
    let providers = logical_sessions
        .into_iter()
        .map(|logical_session| {
            let checkout = freeze_worktree_checkout_v1(&WorktreeCheckoutInputV1 {
                attempt_id: attempt_id.clone(),
                node: node_ref.clone(),
                logical_session,
                source_cwd: source_cwd.clone(),
                canonical_source_cwd: source_cwd.clone(),
                canonical_worktree_root: SessionCwd::parse("/private/tmp/a2a-bound-worktrees")
                    .unwrap(),
                worktree_owner: "bound-test".into(),
            })
            .unwrap();
            freeze_provider_attempt_v1(&ProviderFreezeInputV1 {
                entry,
                overrides: None,
                node: node_ref.clone(),
                logical_session,
                checkout,
                provider_effect_key: None,
            })
            .unwrap()
        })
        .collect();
    let identity = freeze_node_execution_identity_v1(node_ref, providers).unwrap();
    let graph = WorkflowGraph {
        id: WorkflowId::parse("review").unwrap(),
        nodes: vec![WorkflowNode {
            id: NodeId::parse("review-node").unwrap(),
            agent: entry.id.clone(),
            prompt_template: "{{input}}".into(),
            inputs: vec![],
            retry,
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

fn frozen_worktree_run(entry: &AgentEntry) -> WorkflowRunSpecV1 {
    frozen_worktree_run_with_retry(entry, None)
}

async fn execute_bound(
    entry: AgentEntry,
    fail_preflight_ordinal: Option<u16>,
    retry: Option<RetryPolicy>,
    fail_main_prompts: usize,
) -> (
    Arc<WorkflowRunSpecV1>,
    Arc<Calls>,
    Option<bool>,
    Option<(WorkflowOutcome, String)>,
) {
    let entry = Arc::new(entry);
    let run_spec = Arc::new(frozen_worktree_run_with_retry(&entry, retry));
    run_spec.validate().unwrap();
    let calls = Arc::new(Calls::default());
    calls
        .fail_main_prompts
        .store(fail_main_prompts, Ordering::SeqCst);
    let backend: Arc<dyn AgentBackend> = Arc::new(RecordingBackend {
        calls: calls.clone(),
    });
    let registry = Arc::new(BoundOnlyRegistry {
        entry,
        backend,
        slot: Arc::new(()),
        calls: calls.clone(),
        fail_preflight_ordinal,
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
        "bound-preflight-run".into(),
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
    (run_spec, calls, node_ok, terminal)
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
        fail_preflight_ordinal: None,
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

#[tokio::test]
async fn v2_preflight_binds_candidate_and_execute_as_distinct_attempts() {
    let mut configured = entry();
    configured.preflight = true;
    let (_run_spec, calls, node_ok, terminal) = execute_bound(configured, None, None, 0).await;

    assert_eq!(node_ok, Some(true));
    assert_eq!(
        terminal,
        Some((WorkflowOutcome::Completed, "BOUND_OK".into()))
    );
    assert_eq!(calls.unbound_resolves.load(Ordering::SeqCst), 0);
    assert_eq!(calls.binds.load(Ordering::SeqCst), 2);
    assert_eq!(calls.bound_resolves.load(Ordering::SeqCst), 2);
    assert_eq!(calls.bound_invalidations.load(Ordering::SeqCst), 0);
    let specs = calls.bound_specs.lock().unwrap();
    assert_eq!(specs.len(), 2);
    assert!(matches!(
        specs[0].provider_effect.frozen().logical_session,
        FrozenProviderLogicalSessionV1::Preflight {
            candidate_ordinal: 0
        }
    ));
    assert!(matches!(
        specs[1].provider_effect.frozen().logical_session,
        FrozenProviderLogicalSessionV1::Execute {
            candidate_ordinal: 0
        }
    ));
}

#[tokio::test]
async fn v2_preflight_fallback_exact_invalidates_then_executes_persisted_ordinal() {
    let mut configured = entry();
    configured.preflight = true;
    configured.fallback_models = vec!["gpt-5.6-luna".into()];
    let (_run_spec, calls, node_ok, terminal) = execute_bound(configured, Some(0), None, 0).await;

    assert_eq!(node_ok, Some(true));
    assert_eq!(
        terminal,
        Some((WorkflowOutcome::Completed, "BOUND_OK".into()))
    );
    assert_eq!(calls.unbound_resolves.load(Ordering::SeqCst), 0);
    assert_eq!(calls.binds.load(Ordering::SeqCst), 3);
    assert_eq!(calls.bound_resolves.load(Ordering::SeqCst), 3);
    assert_eq!(calls.bound_invalidations.load(Ordering::SeqCst), 1);
    let specs = calls.bound_specs.lock().unwrap();
    assert_eq!(specs.len(), 2);
    for (spec, logical_session) in specs.iter().zip([
        FrozenProviderLogicalSessionV1::Preflight {
            candidate_ordinal: 1,
        },
        FrozenProviderLogicalSessionV1::Execute {
            candidate_ordinal: 1,
        },
    ]) {
        assert_eq!(
            spec.provider_effect.frozen().logical_session,
            logical_session
        );
        assert_eq!(spec.session.config.model.as_deref(), Some("gpt-5.6-luna"));
    }
}

#[tokio::test]
async fn v2_retry_rebinds_execute_row_and_never_invalidates_by_agent_id() {
    let configured = entry();
    let retry = RetryPolicy {
        max_attempts: 2,
        backoff_ms: 0,
        backoff_cap_ms: None,
    };
    let (run_spec, calls, node_ok, terminal) =
        execute_bound(configured, None, Some(retry), 1).await;

    assert_eq!(node_ok, Some(true));
    assert_eq!(
        terminal,
        Some((WorkflowOutcome::Completed, "BOUND_OK".into()))
    );
    assert_eq!(calls.main_prompts.load(Ordering::SeqCst), 2);
    assert_eq!(calls.unbound_resolves.load(Ordering::SeqCst), 0);
    assert_eq!(calls.binds.load(Ordering::SeqCst), 2);
    assert_eq!(calls.bound_resolves.load(Ordering::SeqCst), 2);
    assert_eq!(calls.bound_invalidations.load(Ordering::SeqCst), 1);
    assert_eq!(calls.unbound_invalidations.load(Ordering::SeqCst), 0);
    let specs = calls.bound_specs.lock().unwrap();
    assert_eq!(specs.len(), 2);
    let persisted = &run_spec.node_execution_identities[0].provider_attempts[0];
    for spec in specs.iter() {
        assert_eq!(spec.provider_effect.frozen(), persisted);
        assert!(matches!(
            spec.provider_effect.frozen().logical_session,
            FrozenProviderLogicalSessionV1::Execute {
                candidate_ordinal: 0
            }
        ));
    }
}

#[tokio::test]
async fn v2_refuses_legacy_dispatcher_before_any_unbound_read_or_checkout() {
    let entry = Arc::new(entry());
    let run_spec = Arc::new(frozen_worktree_run(&entry));
    let calls = Arc::new(Calls::default());
    let backend: Arc<dyn AgentBackend> = Arc::new(RecordingBackend {
        calls: calls.clone(),
    });
    let registry = Arc::new(BoundOnlyRegistry {
        entry,
        backend,
        slot: Arc::new(()),
        calls: calls.clone(),
        fail_preflight_ordinal: None,
    });
    let dispatcher = Arc::new(LegacyDispatcher::default());
    let context = WorkflowDiagnosticContext::in_memory(WorkflowRunContext {
        session_cwd: run_spec.requested_session_cwd.clone(),
        ..WorkflowRunContext::default()
    })
    .with_frozen_run_spec(run_spec.clone(), None)
    .unwrap();
    let mut stream = WorkflowExecutor::new(registry).run_with_diagnostic_context_and_dispatcher(
        Arc::new(run_spec.graph.clone()),
        "review this".into(),
        "bound-dispatcher-run".into(),
        CancellationToken::new(),
        context,
        dispatcher.clone(),
    );
    let mut saw_refusal = false;
    while let Some(event) = stream.next().await {
        if matches!(event, Err(BridgeError::BindUnsupported)) {
            saw_refusal = true;
        }
    }

    assert!(
        saw_refusal,
        "V2 must expose the typed bound-dispatch refusal"
    );
    assert_eq!(dispatcher.checkouts.load(Ordering::SeqCst), 0);
    assert_eq!(calls.unbound_resolves.load(Ordering::SeqCst), 0);
    assert_eq!(calls.binds.load(Ordering::SeqCst), 0);
    assert_eq!(calls.bound_resolves.load(Ordering::SeqCst), 0);
    assert!(calls.bound_specs.lock().unwrap().is_empty());
}
