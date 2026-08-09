use async_trait::async_trait;
use bridge_core::diagnostics::{
    DiagnosticFailureClass, DiagnosticPhase, DiagnosticRedactor, FailureDiagnostic,
    FailureDiagnosticInput, FailureDisposition,
};
use bridge_core::domain::{AgentEntry, AgentKind, Part, RegistrySnapshot};
use bridge_core::error::BridgeError;
use bridge_core::execution_policy::{
    freeze_direct_checkout_v1, freeze_node_execution_identity_v1, freeze_provider_attempt_v1,
    freeze_worktree_checkout_v1, resolve_execution_policy_v1, BoundMcpDeliveryPayloadV1,
    BoundSessionSpecV1, ExecutionPolicyInvocationV1, FanOutPolicyV1,
    FrozenProviderLogicalSessionV1, HistoryAllocationKindV1, LedgerAdmissionV1,
    NodeCleanupDispositionV1, NodePrimaryDispositionV1, NodeTerminalV1, PolicyActivationV1,
    PolicyNodeRefV1, ProviderFreezeInputV1, SynthesisModeV1, WorkflowControlDefaultsV1,
    WorktreeCheckoutInputV1,
};
use bridge_core::ids::{AgentId, AttemptId, NodeId, SessionId, WorkflowId};
use bridge_core::mcp::{McpDelivery, McpServerSpec};
use bridge_core::ports::{
    AgentBackend, AgentRegistry, BackendStream, BoundEntryUseV1, EntryUseTokenV1, Lease, Resolved,
    Update,
};
use bridge_core::SessionCwd;
use bridge_workflow::executor::{
    NodeTurn, PolicyTriggerBarrier, PolicyTriggerCheckpointV1, WorkflowDiagnosticContext,
    WorkflowEvent, WorkflowExecutor, WorkflowNodeDispatcher, WorkflowOutcome, WorkflowRunContext,
};
use bridge_workflow::fanout::PolicyTriggerBarrierResultV1;
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
    policy_cancels: AtomicUsize,
    synth_prompts: AtomicUsize,
    bound_specs: Mutex<Vec<BoundSessionSpecV1>>,
}

struct RecordingBackend {
    calls: Arc<Calls>,
}

struct FanoutBackend {
    calls: Arc<Calls>,
}

#[async_trait]
impl AgentBackend for FanoutBackend {
    async fn prompt(
        &self,
        _session: &SessionId,
        parts: Vec<Part>,
    ) -> Result<BackendStream, BridgeError> {
        let prompt = parts
            .iter()
            .map(|part| part.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if prompt.contains("FAIL_ROOT") {
            return Err(BridgeError::AgentOverloaded);
        }
        if prompt.contains("SLOW_ROOT") {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        }
        if prompt.contains("SYNTH_NODE") {
            self.calls.synth_prompts.fetch_add(1, Ordering::SeqCst);
        }
        Ok(Box::pin(tokio_stream::iter(vec![
            Ok(Update::Text("OK".into())),
            Ok(Update::done("end_turn")),
        ])))
    }

    async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
        self.calls.policy_cancels.fetch_add(1, Ordering::SeqCst);
        Ok(())
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

    /// Needed by the REAL `WorkflowAdmissionV1::freeze`, which the end-to-end routing test drives
    /// so the whole admission -> authority -> executor -> backend chain is production code.
    fn entry_snapshot(&self, id: &AgentId) -> Option<Arc<AgentEntry>> {
        (id == &self.entry.id).then(|| self.entry.clone())
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

fn frozen_fail_fast_run_with_ledger(
    entry: &AgentEntry,
    ledger_admission: LedgerAdmissionV1,
) -> WorkflowRunSpecV1 {
    let attempt_id = AttemptId::parse("attempt-33333333333333333333333333333333").unwrap();
    let source_cwd = SessionCwd::parse("/repo/source").unwrap();
    let nodes = vec![
        WorkflowNode {
            id: NodeId::parse("fail-root").unwrap(),
            agent: entry.id.clone(),
            prompt_template: "FAIL_ROOT".into(),
            inputs: vec![],
            retry: None,
            harvest_sanitization: None,
        },
        WorkflowNode {
            id: NodeId::parse("slow-root").unwrap(),
            agent: entry.id.clone(),
            prompt_template: "SLOW_ROOT".into(),
            inputs: vec![],
            retry: None,
            harvest_sanitization: None,
        },
        WorkflowNode {
            id: NodeId::parse("synth").unwrap(),
            agent: entry.id.clone(),
            prompt_template: "SYNTH_NODE {{fail-root}} {{slow-root}}".into(),
            inputs: vec![
                NodeId::parse("fail-root").unwrap(),
                NodeId::parse("slow-root").unwrap(),
            ],
            retry: None,
            harvest_sanitization: None,
        },
    ];
    let graph = WorkflowGraph {
        id: WorkflowId::parse("fail-fast-review").unwrap(),
        nodes: nodes.clone(),
        panel: None,
        controls: Some(WorkflowControlDefaultsV1 {
            fan_out: Some(FanOutPolicyV1::FailFast),
            synthesis: Some(SynthesisModeV1::Strict),
            ..WorkflowControlDefaultsV1::default()
        }),
    };
    let mut sorted_ids = nodes.iter().map(|node| node.id.clone()).collect::<Vec<_>>();
    sorted_ids.sort();
    let identities = nodes
        .iter()
        .map(|node| {
            let ordinal = sorted_ids.iter().position(|id| id == &node.id).unwrap();
            let node_ref =
                PolicyNodeRefV1::from_node_id(u32::try_from(ordinal).unwrap(), node.id.as_str());
            let logical_session = FrozenProviderLogicalSessionV1::Execute {
                candidate_ordinal: 0,
            };
            let provider = freeze_provider_attempt_v1(&ProviderFreezeInputV1 {
                entry,
                overrides: None,
                node: node_ref.clone(),
                logical_session,
                checkout: freeze_direct_checkout_v1(source_cwd.clone()),
                provider_effect_key: None,
            })
            .unwrap();
            freeze_node_execution_identity_v1(node_ref, vec![provider]).unwrap()
        })
        .collect();
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
        identities,
        ledger_admission,
    )
    .unwrap()
}

fn frozen_fail_fast_run(entry: &AgentEntry) -> WorkflowRunSpecV1 {
    frozen_fail_fast_run_with_ledger(
        entry,
        LedgerAdmissionV1::HistoryLedgerUnavailable {
            reason: bridge_core::workflow_history::LedgerUnavailableReason::Open.into(),
        },
    )
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

#[tokio::test]
async fn v2_fail_fast_cancels_running_sibling_and_never_admits_synthesis() {
    let entry = Arc::new(entry());
    let run_spec = Arc::new(frozen_fail_fast_run(&entry));
    run_spec.validate().unwrap();
    let calls = Arc::new(Calls::default());
    let backend: Arc<dyn AgentBackend> = Arc::new(FanoutBackend {
        calls: calls.clone(),
    });
    let registry = Arc::new(BoundOnlyRegistry {
        entry,
        backend,
        slot: Arc::new(()),
        calls: calls.clone(),
        fail_preflight_ordinal: None,
    });
    let context = WorkflowDiagnosticContext::in_memory(WorkflowRunContext {
        session_cwd: run_spec.requested_session_cwd.clone(),
        ..WorkflowRunContext::default()
    })
    .with_frozen_run_spec(run_spec.clone(), None)
    .unwrap();
    let executor = WorkflowExecutor::new(registry);
    let stream = executor.run_with_diagnostic_context(
        Arc::new(run_spec.graph.clone()),
        "review this".into(),
        "bound-fail-fast-run".into(),
        CancellationToken::new(),
        context,
    );
    let events = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        stream.collect::<Vec<_>>(),
    )
    .await
    .expect("fail-fast execution must drain")
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .unwrap();

    assert_eq!(calls.policy_cancels.load(Ordering::SeqCst), 1);
    assert_eq!(calls.synth_prompts.load(Ordering::SeqCst), 0);
    assert!(!events.iter().any(|event| matches!(
        event,
        WorkflowEvent::NodeStarted { node } if node.as_str() == "synth"
    )));
    assert!(matches!(
        events.last(),
        Some(WorkflowEvent::Terminal {
            outcome: WorkflowOutcome::Failed,
            ..
        })
    ));
}

#[tokio::test]
async fn v2_durable_fail_fast_barrier_precedes_policy_cancel_and_publishes_exact_evidence() {
    let entry = Arc::new(entry());
    let run_spec = Arc::new(frozen_fail_fast_run_with_ledger(
        &entry,
        LedgerAdmissionV1::DurablePrimaryTaskStore,
    ));
    run_spec.validate().unwrap();
    let calls = Arc::new(Calls::default());
    let backend: Arc<dyn AgentBackend> = Arc::new(FanoutBackend {
        calls: calls.clone(),
    });
    let registry = Arc::new(BoundOnlyRegistry {
        entry,
        backend,
        slot: Arc::new(()),
        calls: calls.clone(),
        fail_preflight_ordinal: None,
    });
    let checkpoints = Arc::new(Mutex::new(Vec::<PolicyTriggerCheckpointV1>::new()));
    let barrier_calls = checkpoints.clone();
    let cancellation_order = calls.clone();
    let barrier: PolicyTriggerBarrier = Arc::new(move |checkpoint| {
        let barrier_calls = barrier_calls.clone();
        let cancellation_order = cancellation_order.clone();
        Box::pin(async move {
            assert_eq!(
                cancellation_order.policy_cancels.load(Ordering::SeqCst),
                0,
                "the durable acknowledgement must precede sibling cancellation"
            );
            barrier_calls.lock().unwrap().push(checkpoint);
            PolicyTriggerBarrierResultV1::ServedPrimaryCommitted
        })
    });
    let context = WorkflowDiagnosticContext::in_memory(WorkflowRunContext {
        session_cwd: run_spec.requested_session_cwd.clone(),
        ..WorkflowRunContext::default()
    })
    .with_policy_trigger_barrier(barrier)
    .with_frozen_run_spec(run_spec.clone(), None)
    .unwrap();
    let events = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        WorkflowExecutor::new(registry)
            .run_with_diagnostic_context(
                Arc::new(run_spec.graph.clone()),
                "review this".into(),
                "bound-durable-fail-fast-run".into(),
                CancellationToken::new(),
                context,
            )
            .collect::<Vec<_>>(),
    )
    .await
    .expect("durable fail-fast execution must drain")
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .unwrap();

    let checkpoints = checkpoints.lock().unwrap();
    assert_eq!(
        checkpoints.len(),
        1,
        "exactly one trigger reaches the barrier"
    );
    let checkpoint = &checkpoints[0];
    assert_eq!(checkpoint.node.as_str(), "fail-root");
    assert!(!checkpoint.ok);
    let trigger = bridge_core::execution_policy::PolicyTriggerV1::decode_canonical(
        checkpoint.policy_trigger_json.as_bytes(),
    )
    .unwrap();
    let terminal = bridge_core::execution_policy::NodeTerminalV1::decode_canonical(
        checkpoint.terminal_json.as_bytes(),
    )
    .unwrap();
    assert_eq!(terminal.policy_trigger_id.as_ref(), Some(&trigger.id));

    let selected = events
        .iter()
        .find_map(|event| match event {
            WorkflowEvent::NodeFinished {
                node,
                terminal_json,
                policy_trigger_json,
                ..
            } if node.as_str() == "fail-root" => {
                Some((terminal_json.as_deref(), policy_trigger_json.as_deref()))
            }
            _ => None,
        })
        .expect("selected node terminal is emitted");
    assert_eq!(selected.0, Some(checkpoint.terminal_json.as_str()));
    assert_eq!(selected.1, Some(checkpoint.policy_trigger_json.as_str()));
    let sibling_terminal = events
        .iter()
        .find_map(|event| match event {
            WorkflowEvent::NodeFinished {
                node,
                terminal_json: Some(terminal_json),
                ..
            } if node.as_str() == "slow-root" => Some(
                bridge_core::execution_policy::NodeTerminalV1::decode_canonical(
                    terminal_json.as_bytes(),
                )
                .unwrap(),
            ),
            _ => None,
        })
        .expect("the canceled sibling retains a structured terminal");
    assert_eq!(
        sibling_terminal.primary,
        bridge_core::execution_policy::NodePrimaryDispositionV1::CanceledPolicy
    );
    assert_eq!(calls.policy_cancels.load(Ordering::SeqCst), 1);
}

struct TerminalEvidenceBackend {
    prompt_error: Option<BridgeError>,
    cleanup_error: Option<BridgeError>,
}

#[async_trait]
impl AgentBackend for TerminalEvidenceBackend {
    async fn prompt(
        &self,
        _session: &SessionId,
        _parts: Vec<Part>,
    ) -> Result<BackendStream, BridgeError> {
        if let Some(error) = &self.prompt_error {
            return Err(error.clone());
        }
        Ok(Box::pin(tokio_stream::iter(vec![
            Ok(Update::Text("OK".into())),
            Ok(Update::done("end_turn")),
        ])))
    }

    async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
        Ok(())
    }

    async fn configure_bound_session(
        &self,
        _session: &SessionId,
        _spec: &BoundSessionSpecV1,
    ) -> Result<(), BridgeError> {
        Ok(())
    }

    async fn forget_session_observed(
        &self,
        _session: &SessionId,
        _observer: Arc<dyn bridge_core::ports::DiagnosticObserver>,
    ) -> Result<(), BridgeError> {
        self.cleanup_error.clone().map_or(Ok(()), Err)
    }
}

fn accepted_prompt_failure() -> BridgeError {
    BridgeError::agent_failure(
        FailureDiagnostic::build_static_code(
            FailureDiagnosticInput {
                failed_phase: DiagnosticPhase::PromptStart,
                last_completed_phase: Some(DiagnosticPhase::ConfigApply),
                class: DiagnosticFailureClass::Transport,
                disposition: FailureDisposition::Fatal,
                code: String::new(),
                summary: "bounded prompt-open failure".into(),
                causes: vec!["outer cause".into(), "deepest cause".into()],
                stderr_observed: false,
                stderr_line_count: 0,
                stderr_scope: None,
                stderr_tail: None,
                stderr_redaction: None,
                retry_after_ms: None,
                reset_at_ms: None,
                prompt_may_have_been_accepted: true,
            },
            "test.node.prompt_open",
            &DiagnosticRedactor::default(),
        )
        .unwrap(),
    )
}

async fn execute_terminal_evidence_case(
    prompt_error: Option<BridgeError>,
    cleanup_error: Option<BridgeError>,
) -> NodeTerminalV1 {
    let entry = Arc::new(entry());
    let run_spec = Arc::new(frozen_worktree_run(&entry));
    run_spec.validate().unwrap();
    let calls = Arc::new(Calls::default());
    let registry = Arc::new(BoundOnlyRegistry {
        entry,
        backend: Arc::new(TerminalEvidenceBackend {
            prompt_error,
            cleanup_error,
        }),
        slot: Arc::new(()),
        calls,
        fail_preflight_ordinal: None,
    });
    let context = WorkflowDiagnosticContext::in_memory(WorkflowRunContext {
        session_cwd: run_spec.requested_session_cwd.clone(),
        ..WorkflowRunContext::default()
    })
    .with_frozen_run_spec(run_spec.clone(), None)
    .unwrap();
    let events = WorkflowExecutor::new(registry)
        .run_with_diagnostic_context(
            Arc::new(run_spec.graph.clone()),
            "review this".into(),
            "bound-terminal-evidence-run".into(),
            CancellationToken::new(),
            context,
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let terminal_json = events
        .iter()
        .find_map(|event| match event {
            WorkflowEvent::NodeFinished {
                terminal_json: Some(terminal_json),
                ..
            } => Some(terminal_json),
            _ => None,
        })
        .expect("V2 emits one canonical node terminal");
    NodeTerminalV1::decode_canonical(terminal_json.as_bytes()).unwrap()
}

#[tokio::test]
async fn v2_node_terminal_records_successful_cleanup_before_publication() {
    let terminal = execute_terminal_evidence_case(None, None).await;
    assert_eq!(terminal.primary, NodePrimaryDispositionV1::Completed);
    assert_eq!(
        terminal.cleanup.disposition,
        NodeCleanupDispositionV1::Complete
    );
    assert!(terminal.cause.is_none());
}

#[tokio::test]
async fn v2_node_terminal_records_cleanup_failure_and_bounded_cause() {
    let terminal = execute_terminal_evidence_case(None, Some(BridgeError::StoreFailure)).await;
    assert_eq!(terminal.primary, NodePrimaryDispositionV1::Failed);
    assert_eq!(
        terminal.cleanup.disposition,
        NodeCleanupDispositionV1::Failed
    );
    let cause = terminal.cause.expect("cleanup failure retains a cause");
    assert_eq!(cause.failure_class, DiagnosticFailureClass::Persistence);
    assert_eq!(cause.code.as_str(), "bridge.store_failure");
}

#[tokio::test]
async fn v2_node_terminal_preserves_diagnostic_class_code_cause_and_prompt_acceptance() {
    let terminal = execute_terminal_evidence_case(Some(accepted_prompt_failure()), None).await;
    assert_eq!(terminal.primary, NodePrimaryDispositionV1::Failed);
    assert_eq!(
        terminal.cleanup.disposition,
        NodeCleanupDispositionV1::Complete
    );
    assert!(terminal.prompt_may_have_been_accepted);
    let cause = terminal.cause.expect("diagnostic failure retains a cause");
    assert_eq!(cause.failure_class, DiagnosticFailureClass::Transport);
    assert_eq!(cause.code.as_str(), "test.node.prompt_open");
    assert_eq!(cause.deepest_cause.as_deref(), Some("deepest cause"));
}

#[tokio::test]
async fn v2_node_terminal_keeps_primary_diagnostic_when_cleanup_also_fails() {
    let terminal = execute_terminal_evidence_case(
        Some(accepted_prompt_failure()),
        Some(BridgeError::StoreFailure),
    )
    .await;
    assert_eq!(terminal.primary, NodePrimaryDispositionV1::Failed);
    assert_eq!(
        terminal.cleanup.disposition,
        NodeCleanupDispositionV1::Failed
    );
    assert!(terminal.prompt_may_have_been_accepted);
    let cause = terminal
        .cause
        .expect("the primary diagnostic remains authoritative");
    assert_eq!(cause.failure_class, DiagnosticFailureClass::Transport);
    assert_eq!(cause.code.as_str(), "test.node.prompt_open");
    assert_eq!(cause.deepest_cause.as_deref(), Some("deepest cause"));
}

#[derive(Default)]
struct SynthesisState {
    prompts: Mutex<Vec<String>>,
}

struct SynthesisBackend {
    state: Arc<SynthesisState>,
}

#[async_trait]
impl AgentBackend for SynthesisBackend {
    async fn prompt(
        &self,
        _session: &SessionId,
        parts: Vec<Part>,
    ) -> Result<BackendStream, BridgeError> {
        let prompt = parts
            .iter()
            .map(|part| part.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        self.state.prompts.lock().unwrap().push(prompt.clone());
        if prompt.contains("ROOT_FAIL") {
            return Err(BridgeError::AgentOverloaded);
        }
        let reply = if prompt.contains("FINAL_NODE") {
            "FINAL_OK"
        } else {
            "MID_OK"
        };
        Ok(Box::pin(tokio_stream::iter(vec![
            Ok(Update::Text(reply.into())),
            Ok(Update::done("end_turn")),
        ])))
    }

    async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
        Ok(())
    }

    async fn configure_bound_session(
        &self,
        _session: &SessionId,
        _spec: &BoundSessionSpecV1,
    ) -> Result<(), BridgeError> {
        Ok(())
    }
}

fn frozen_synthesis_run(entry: &AgentEntry, synthesis: SynthesisModeV1) -> WorkflowRunSpecV1 {
    let attempt_id = AttemptId::parse("attempt-44444444444444444444444444444444").unwrap();
    let source_cwd = SessionCwd::parse("/repo/source").unwrap();
    let nodes = vec![
        WorkflowNode {
            id: NodeId::parse("root").unwrap(),
            agent: entry.id.clone(),
            prompt_template: "ROOT_FAIL".into(),
            inputs: vec![],
            retry: None,
            harvest_sanitization: None,
        },
        WorkflowNode {
            id: NodeId::parse("middle").unwrap(),
            agent: entry.id.clone(),
            prompt_template: "MID_NODE {{root}}".into(),
            inputs: vec![NodeId::parse("root").unwrap()],
            retry: None,
            harvest_sanitization: None,
        },
        WorkflowNode {
            id: NodeId::parse("terminal").unwrap(),
            agent: entry.id.clone(),
            prompt_template: "FINAL_NODE {{middle}}".into(),
            inputs: vec![NodeId::parse("middle").unwrap()],
            retry: None,
            harvest_sanitization: None,
        },
    ];
    let graph = WorkflowGraph {
        id: WorkflowId::parse("synthesis-policy").unwrap(),
        nodes: nodes.clone(),
        panel: None,
        controls: Some(WorkflowControlDefaultsV1 {
            fan_out: Some(FanOutPolicyV1::BoundedIndependent),
            synthesis: Some(synthesis),
            ..WorkflowControlDefaultsV1::default()
        }),
    };
    let mut sorted_ids = nodes.iter().map(|node| node.id.clone()).collect::<Vec<_>>();
    sorted_ids.sort();
    let identities = nodes
        .iter()
        .map(|node| {
            let ordinal = sorted_ids.iter().position(|id| id == &node.id).unwrap();
            let node_ref =
                PolicyNodeRefV1::from_node_id(u32::try_from(ordinal).unwrap(), node.id.as_str());
            let logical_session = FrozenProviderLogicalSessionV1::Execute {
                candidate_ordinal: 0,
            };
            let provider = freeze_provider_attempt_v1(&ProviderFreezeInputV1 {
                entry,
                overrides: None,
                node: node_ref.clone(),
                logical_session,
                checkout: freeze_direct_checkout_v1(source_cwd.clone()),
                provider_effect_key: None,
            })
            .unwrap();
            freeze_node_execution_identity_v1(node_ref, vec![provider]).unwrap()
        })
        .collect();
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
        identities,
        LedgerAdmissionV1::HistoryLedgerUnavailable {
            reason: bridge_core::workflow_history::LedgerUnavailableReason::Open.into(),
        },
    )
    .unwrap()
}

async fn execute_synthesis_case(
    synthesis: SynthesisModeV1,
) -> (Arc<SynthesisState>, Vec<WorkflowEvent>) {
    let entry = Arc::new(entry());
    let run_spec = Arc::new(frozen_synthesis_run(&entry, synthesis));
    run_spec.validate().unwrap();
    let calls = Arc::new(Calls::default());
    let state = Arc::new(SynthesisState::default());
    let registry = Arc::new(BoundOnlyRegistry {
        entry,
        backend: Arc::new(SynthesisBackend {
            state: state.clone(),
        }),
        slot: Arc::new(()),
        calls,
        fail_preflight_ordinal: None,
    });
    let context = WorkflowDiagnosticContext::in_memory(WorkflowRunContext {
        session_cwd: run_spec.requested_session_cwd.clone(),
        ..WorkflowRunContext::default()
    })
    .with_frozen_run_spec(run_spec.clone(), None)
    .unwrap();
    let events = WorkflowExecutor::new(registry)
        .run_with_diagnostic_context(
            Arc::new(run_spec.graph.clone()),
            "review this".into(),
            "bound-synthesis-run".into(),
            CancellationToken::new(),
            context,
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    (state, events)
}

fn node_terminal(events: &[WorkflowEvent], wanted: &str) -> (String, NodeTerminalV1) {
    let json = events
        .iter()
        .find_map(|event| match event {
            WorkflowEvent::NodeFinished {
                node,
                terminal_json: Some(json),
                ..
            } if node.as_str() == wanted => Some(json.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing terminal for {wanted}"));
    let terminal = NodeTerminalV1::decode_canonical(json.as_bytes()).unwrap();
    (json, terminal)
}

#[tokio::test]
async fn v2_strict_synthesis_structurally_skips_failed_dependency_chain() {
    let (state, events) = execute_synthesis_case(SynthesisModeV1::Strict).await;
    assert_eq!(
        state.prompts.lock().unwrap().len(),
        1,
        "only the failed root prompts"
    );
    assert!(!events.iter().any(|event| matches!(
        event,
        WorkflowEvent::NodeStarted { node }
            if matches!(node.as_str(), "middle" | "terminal")
    )));
    for node in ["middle", "terminal"] {
        let (_, terminal) = node_terminal(&events, node);
        assert_eq!(
            terminal.primary,
            NodePrimaryDispositionV1::SkippedDependency
        );
        assert_eq!(
            terminal.cleanup.disposition,
            NodeCleanupDispositionV1::NotNeeded
        );
        let dependency = terminal
            .cause
            .and_then(|cause| cause.dependency_set)
            .expect("strict skip binds its exact direct failed dependency set");
        assert_eq!(dependency.count, 1);
    }
    assert!(matches!(
        events.last(),
        Some(WorkflowEvent::Terminal {
            outcome: WorkflowOutcome::Failed,
            ..
        })
    ));
}

#[tokio::test]
async fn v2_degraded_synthesis_uses_typed_marker_and_propagates_ancestry() {
    let (state, events) = execute_synthesis_case(SynthesisModeV1::Degraded).await;
    let prompts = state.prompts.lock().unwrap();
    assert_eq!(prompts.len(), 3);
    let (root_json, root) = node_terminal(&events, "root");
    assert_eq!(root.primary, NodePrimaryDispositionV1::Failed);
    let marker = format!("{{\"type\":\"a2a_bridge.node_failure.v1\",\"terminal\":{root_json}}}");
    assert!(
        prompts[1].contains(&marker),
        "direct child receives typed marker"
    );
    drop(prompts);
    for node in ["middle", "terminal"] {
        let (_, terminal) = node_terminal(&events, node);
        assert_eq!(terminal.primary, NodePrimaryDispositionV1::Completed);
        assert!(
            terminal.degraded_ancestry,
            "taint propagates through {node}"
        );
    }
    let outcome = match events.last() {
        Some(WorkflowEvent::Terminal { outcome, .. }) => outcome,
        other => panic!("missing workflow terminal: {other:?}"),
    };
    assert_eq!(format!("{outcome:?}"), "CompletedDegraded");
}

// ---------------------------------------------------------------------------------------------
// R2f1b slice 2b2 — V3 routing: admission → executor → `BoundSessionSpecV1`.
//
// R-3: `BoundSessionSpecV1` could not distinguish V2 from V3, so a V3-only writer could not
// exist. These tests pin both directions of the discriminator on the SAME harness the V2 tests
// above use, which is what makes "V2 routes byte-identical" observable rather than asserted.
// ---------------------------------------------------------------------------------------------

fn r2f1b_admission_for(
    run_spec: &WorkflowRunSpecV1,
    activation: bridge_core::execution_policy::DeadlineActivationV2,
) -> Arc<bridge_workflow::admission::R2f1bAdmissionV1> {
    use bridge_core::execution_policy::{
        FrozenCheckoutEffectV1, FrozenR2f1bContractV1, FrozenWorktreeCustodyPlanV1, Sha256HexV1,
        WorktreeCustodyIdV1,
    };
    let mut plans = Vec::new();
    for identity in &run_spec.node_execution_identities {
        for attempt in &identity.provider_attempts {
            if let FrozenCheckoutEffectV1::Worktree {
                target_cwd,
                checkout_digest,
                ..
            } = &attempt.checkout
            {
                plans.push(FrozenWorktreeCustodyPlanV1 {
                    custody_id: WorktreeCustodyIdV1::parse(format!(
                        "custody-{}",
                        Sha256HexV1::digest(checkout_digest.as_str().as_bytes()).as_str()
                    ))
                    .unwrap(),
                    checkout_fingerprint: checkout_digest.clone(),
                    target_cwd: target_cwd.clone(),
                });
            }
        }
    }
    Arc::new(bridge_workflow::admission::R2f1bAdmissionV1 {
        attempt: bridge_core::ids::AttemptIdentity {
            execution_id: bridge_core::ids::ExecutionId::parse(format!("exec-{}", "7".repeat(32)))
                .unwrap(),
            attempt_id: run_spec.attempt_id.clone(),
            ordinal: 0,
            parent_attempt_id: None,
        },
        contract: FrozenR2f1bContractV1::with_computed_fingerprint(activation, plans).unwrap(),
    })
}

async fn execute_v3(
    entry: AgentEntry,
    r2f1b: Arc<bridge_workflow::admission::R2f1bAdmissionV1>,
    run_spec: Arc<WorkflowRunSpecV1>,
) -> (Arc<Calls>, Option<(WorkflowOutcome, String)>) {
    let entry = Arc::new(entry);
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
        .unwrap()
        .with_frozen_r2f1b_contract(r2f1b)
        .expect("a manual-only contract covering every checkout binds");
    let executor = WorkflowExecutor::new(registry);
    let mut stream = executor.run_with_diagnostic_context(
        Arc::new(run_spec.graph.clone()),
        "review this".into(),
        "v3-run".into(),
        CancellationToken::new(),
        context,
    );
    let mut terminal = None;
    while let Some(event) = stream.next().await {
        if let WorkflowEvent::Terminal { outcome, output } = event.unwrap() {
            terminal = Some((outcome, output));
        }
    }
    (calls, terminal)
}

/// V3 direction. Discriminates a route that drops the plan (leaving the backend unable to tell
/// V2 from V3 — R-3 unfixed), and one that attaches *a* plan rather than *the* plan for this
/// checkout: the asserted custody id is derived from this attempt's own frozen digest, so a
/// first-plan or nearest-plan selector fails here.
#[tokio::test]
async fn v3_routing_carries_the_exactly_matching_custody_plan_to_the_backend() {
    let configured = entry();
    let run_spec = Arc::new(frozen_worktree_run(&configured));
    let r2f1b = r2f1b_admission_for(
        &run_spec,
        bridge_core::execution_policy::DeadlineActivationV2::ManualOnlyR2f1a,
    );
    let (calls, terminal) = execute_v3(configured, r2f1b.clone(), run_spec.clone()).await;

    assert_eq!(
        terminal,
        Some((WorkflowOutcome::Completed, "BOUND_OK".into()))
    );
    let specs = calls.bound_specs.lock().unwrap();
    assert_eq!(specs.len(), 1);
    let custody = specs[0]
        .custody()
        .expect("a V3 route must carry its custody binding");
    let persisted = &run_spec.node_execution_identities[0].provider_attempts[0];
    let bridge_core::execution_policy::FrozenCheckoutEffectV1::Worktree {
        target_cwd,
        checkout_digest,
        ..
    } = &persisted.checkout
    else {
        panic!("the frozen checkout is a worktree")
    };
    assert_eq!(&custody.plan.checkout_fingerprint, checkout_digest);
    assert_eq!(&custody.plan.target_cwd, target_cwd);
    assert_eq!(custody.attempt, r2f1b.attempt);
    assert_eq!(custody.origin_attempt_id, run_spec.attempt_id);
    assert_eq!(
        custody.node, run_spec.node_execution_identities[0].node,
        "the writer needs the node ref the claim records"
    );
}

/// V2 direction, on the same harness: with no contract bound, nothing changes. Discriminates a
/// routing change that synthesises a custody binding for every worktree checkout, which would
/// make every existing V2 run take the V3 writer.
#[tokio::test]
async fn v2_routing_carries_no_custody_binding() {
    let configured = entry();
    let run_spec = Arc::new(frozen_worktree_run(&configured));
    let (_run_spec, calls, node_ok, _terminal) = execute_bound(configured, None, None, 0).await;

    assert_eq!(node_ok, Some(true));
    let specs = calls.bound_specs.lock().unwrap();
    assert_eq!(specs.len(), 1);
    assert!(
        specs[0].custody().is_none(),
        "a V2 route must reach the backend indistinguishable from before this slice"
    );
    assert!(matches!(
        run_spec.node_execution_identities[0].provider_attempts[0].checkout,
        bridge_core::execution_policy::FrozenCheckoutEffectV1::Worktree { .. }
    ));
}

/// The executor-side boundary enforces the same activation rule as admission. Discriminates a
/// rule enforced at only one of the two entrances: `with_frozen_r2f1b_contract` is `pub`, so a
/// caller can reach the executor without going through `WorkflowAdmissionV1::freeze`.
#[test]
fn the_executor_boundary_also_refuses_automatic_activation() {
    let configured = entry();
    let run_spec = Arc::new(frozen_worktree_run(&configured));
    let automatic = r2f1b_admission_for(
        &run_spec,
        bridge_core::execution_policy::DeadlineActivationV2::AutomaticR2f1b,
    );
    let context = WorkflowDiagnosticContext::in_memory(WorkflowRunContext {
        session_cwd: run_spec.requested_session_cwd.clone(),
        ..WorkflowRunContext::default()
    })
    .with_frozen_run_spec(run_spec, None)
    .unwrap();

    let Err(BridgeError::ConfigInvalid { reason }) = context.with_frozen_r2f1b_contract(automatic)
    else {
        panic!("the executor boundary must refuse automatic activation");
    };
    assert!(
        reason.contains("automatic R2f1b deadline activation is refused"),
        "unexpected refusal reason: {reason}"
    );
}

/// A contract cannot be bound without the delivery spec it is matched against. Discriminates an
/// ordering-free API where a caller can attach a contract nothing checks its coverage against.
#[test]
fn an_r2f1b_contract_needs_its_frozen_run_spec_first() {
    let configured = entry();
    let run_spec = Arc::new(frozen_worktree_run(&configured));
    let manual = r2f1b_admission_for(
        &run_spec,
        bridge_core::execution_policy::DeadlineActivationV2::ManualOnlyR2f1a,
    );
    let context = WorkflowDiagnosticContext::in_memory(WorkflowRunContext::default());

    assert!(context.with_frozen_r2f1b_contract(manual).is_err());
}

// ---------------------------------------------------------------------------------------------
// R2f1b slice 2b2 repair R1 — the ADMISSION -> AUTHORITY -> EXECUTOR handoff, end to end.
//
// The bug this closes: both production authority consumers (`main.rs` run-workflow,
// `detached.rs`) took `authority.run_spec` and `authority.provider_effect_key` apart and passed
// them to `with_frozen_run_spec`, which hardcodes `r2f1b: None`. So `AdmittedWorkflowRunV1::r2f1b`
// was write-only in production: an admitted `ManualOnlyR2f1a` contract was dropped between
// admission and the executor, the run silently degraded to V2 (legacy `add`, `.meta.json`, no
// custody record), and nothing failed. Every test above bound the contract by hand and therefore
// could not see it.
// ---------------------------------------------------------------------------------------------

struct WorktreeAdmissionPlanner {
    root: SessionCwd,
}

#[async_trait]
impl bridge_workflow::admission::WorkflowCheckoutPlannerV1 for WorktreeAdmissionPlanner {
    async fn freeze_checkout(
        &self,
        _entry: &AgentEntry,
        input: &bridge_workflow::admission::CheckoutPlanInputV1,
    ) -> Result<bridge_core::execution_policy::FrozenCheckoutEffectV1, BridgeError> {
        freeze_worktree_checkout_v1(&WorktreeCheckoutInputV1 {
            attempt_id: input.attempt_id.clone(),
            node: input.node.clone(),
            logical_session: input.logical_session,
            source_cwd: input.source_cwd.clone(),
            canonical_source_cwd: input.source_cwd.clone(),
            canonical_worktree_root: self.root.clone(),
            worktree_owner: "bound-test".into(),
        })
        .map_err(|error| BridgeError::ConfigInvalid {
            reason: format!("{error:?}"),
        })
    }
}

/// Drive the REAL `WorkflowAdmissionV1::freeze`, optionally offering a contract covering every
/// frozen worktree checkout it produces.
async fn admit_through_production_admission(
    registry: Arc<dyn AgentRegistry>,
    with_contract: bool,
) -> bridge_workflow::admission::AdmittedWorkflowRunV1 {
    use bridge_core::execution_policy::{
        FrozenCheckoutEffectV1, FrozenR2f1bContractV1, FrozenWorktreeCustodyPlanV1,
        HistoryAllocationKindV1, Sha256HexV1, WorktreeCustodyIdV1,
    };
    let source_cwd = SessionCwd::parse("/repo/source").unwrap();
    let graph = Arc::new(WorkflowGraph {
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
    });
    let admission = bridge_workflow::admission::WorkflowAdmissionV1::new(
        registry,
        Arc::new(WorktreeAdmissionPlanner {
            root: SessionCwd::parse("/private/tmp/a2a-bound-worktrees").unwrap(),
        }),
        source_cwd.clone(),
        None,
    );
    let attempt = bridge_core::ids::AttemptIdentity::initial().unwrap();
    let attempt_id = attempt.attempt_id.clone();
    let request = |r2f1b| bridge_workflow::admission::WorkflowAdmissionRequestV1 {
        attempt_id: attempt_id.clone(),
        graph: graph.clone(),
        requested_session_cwd: Some(source_cwd.clone()),
        policy_invocation: ExecutionPolicyInvocationV1::default(),
        ledger_admission: LedgerAdmissionV1::HistoryLedgerAdmitted {
            kind: HistoryAllocationKindV1::Configured,
        },
        r2f1b,
    };
    let probe = admission.freeze(request(None)).await.unwrap();
    if !with_contract {
        return probe;
    }
    let plans = probe.run_spec.node_execution_identities[0]
        .provider_attempts
        .iter()
        .filter_map(|attempt| match &attempt.checkout {
            FrozenCheckoutEffectV1::Worktree {
                target_cwd,
                checkout_digest,
                ..
            } => Some(FrozenWorktreeCustodyPlanV1 {
                custody_id: WorktreeCustodyIdV1::parse(format!(
                    "custody-{}",
                    Sha256HexV1::digest(checkout_digest.as_str().as_bytes()).as_str()
                ))
                .unwrap(),
                checkout_fingerprint: checkout_digest.clone(),
                target_cwd: target_cwd.clone(),
            }),
            FrozenCheckoutEffectV1::Direct { .. } => None,
        })
        .collect();
    admission
        .freeze(request(Some(
            bridge_workflow::admission::R2f1bAdmissionV1 {
                attempt,
                contract: FrozenR2f1bContractV1::with_computed_fingerprint(
                    bridge_core::execution_policy::DeadlineActivationV2::ManualOnlyR2f1a,
                    plans,
                )
                .unwrap(),
            },
        )))
        .await
        .unwrap()
}

async fn run_through_production_binder(
    with_contract: bool,
) -> (Arc<Calls>, Arc<WorkflowRunSpecV1>) {
    let entry = Arc::new(entry());
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
    let admitted = admit_through_production_admission(
        registry.clone() as Arc<dyn AgentRegistry>,
        with_contract,
    )
    .await;
    let run_spec = admitted.run_spec.clone();
    let request = WorkflowRunContext {
        session_cwd: run_spec.requested_session_cwd.clone(),
        ..WorkflowRunContext::default()
    };
    // THE production binder. Nothing in this test reaches into the admission result by hand.
    let context = WorkflowDiagnosticContext::in_memory(request)
        .with_admitted_workflow_run(admitted)
        .expect("the admitted run binds");
    let executor = WorkflowExecutor::new(registry);
    let mut stream = executor.run_with_diagnostic_context(
        Arc::new(run_spec.graph.clone()),
        "review this".into(),
        "production-binder-run".into(),
        CancellationToken::new(),
        context,
    );
    while stream.next().await.is_some() {}
    (calls, run_spec)
}

/// R1's red test. An admitted `ManualOnlyR2f1a` contract must survive the real production binder
/// and reach the backend as a custody binding.
///
/// Discriminates exactly the shipped defect: with the consumers calling `with_frozen_run_spec`
/// (which hardcodes `r2f1b: None`) this asserts `custody().is_some()` on a spec that carries
/// `None`, and it is the ONLY test that can — every other V3 test binds the contract by hand and
/// so never exercises the handoff at all. A dropped contract means the worktree backend takes the
/// V2 leg: legacy `add`, `.meta.json`, no custody record (pinned separately by
/// `v3_path_writes_no_legacy_meta_json` at the backend boundary, which keys on this same
/// `custody()` discriminator).
#[tokio::test]
async fn an_admitted_contract_survives_the_production_authority_binder() {
    let (calls, run_spec) = run_through_production_binder(true).await;

    let specs = calls.bound_specs.lock().unwrap();
    assert_eq!(specs.len(), 1);
    let custody = specs[0]
        .custody()
        .expect("the admitted contract must reach the backend, not be dropped at the handoff");
    let bridge_core::execution_policy::FrozenCheckoutEffectV1::Worktree {
        checkout_digest,
        target_cwd,
        ..
    } = &run_spec.node_execution_identities[0].provider_attempts[0].checkout
    else {
        panic!("admission froze a worktree checkout")
    };
    assert_eq!(&custody.plan.checkout_fingerprint, checkout_digest);
    assert_eq!(&custody.plan.target_cwd, target_cwd);
    assert_eq!(custody.origin_attempt_id, run_spec.attempt_id);
    assert_eq!(calls.legacy_configures.load(Ordering::SeqCst), 0);
}

/// The V2 negative through the SAME binder: admission with no contract still routes V2, so the
/// test above cannot pass by making every run look V3.
#[tokio::test]
async fn an_admission_with_no_contract_still_routes_v2_through_the_same_binder() {
    let (calls, _run_spec) = run_through_production_binder(false).await;

    let specs = calls.bound_specs.lock().unwrap();
    assert_eq!(specs.len(), 1);
    assert!(
        specs[0].custody().is_none(),
        "a V2 admission must reach the backend indistinguishable from before this slice"
    );
}
