//! Executor activation, with independent provider observations and durable reopen.
mod common;
#[path = "../src/local_workflow_accounting.rs"]
mod local_workflow_accounting;
#[allow(dead_code)]
#[path = "accounting/support.rs"]
mod support;
#[allow(dead_code)]
#[path = "accounting/workflow_support.rs"]
mod workflow_support;

use bridge_core::ids::{AttemptIdentity, WorkflowId};
use bridge_workflow::{
    accounting::WorkflowAccountingInvocationV1,
    executor::{WorkflowDiagnosticContext, WorkflowExecutor, WorkflowRunContext},
    graph::WorkflowGraph,
};
use futures::StreamExt;
use std::sync::Arc;
use support::Fixture;
use tokio_util::sync::CancellationToken;
use workflow_support::WorkflowRecording;

fn graph() -> Arc<WorkflowGraph> {
    Arc::new(serde_json::from_value(serde_json::json!({
        "id": "accounted", "nodes": [
            {"id":"draft", "agent":"agent", "prompt_template":"input={{input}}", "inputs":[]},
            {"id":"review", "agent":"agent", "prompt_template":"input={{input}} draft={{draft}}", "inputs":["draft"]}
        ]
    })).unwrap())
}
fn context(f: &Fixture, owner: &AttemptIdentity) -> WorkflowDiagnosticContext {
    WorkflowDiagnosticContext::in_memory(WorkflowRunContext::default()).with_accounting(
        WorkflowAccountingInvocationV1::new(
            support::billing_accounting(f.store.clone()),
            owner.clone(),
            WorkflowId::parse("accounted").unwrap(),
        ),
    )
}

#[tokio::test]
async fn expanded_prompts_keep_invocation_owner_and_nodes_without_telemetry() {
    let f = Fixture::new();
    let backend = Arc::new(WorkflowRecording::default());
    let executor = WorkflowExecutor::new(common::single_agent_registry("agent", backend.clone()));
    let owner = AttemptIdentity::initial().unwrap().resume().unwrap();
    let mut mapped_graph = (*graph()).clone();
    for node in &mut mapped_graph.nodes {
        node.model = Some("good".into());
    }
    let events = executor
        .run_with_diagnostic_context(
            Arc::new(mapped_graph),
            "BRIEF".into(),
            "run".into(),
            CancellationToken::new(),
            context(&f, &owner),
        )
        .collect::<Vec<_>>()
        .await;
    assert!(events.iter().all(Result::is_ok), "{events:?}");
    let prompts = backend.prompts.lock().unwrap().clone();
    assert_eq!(prompts.len(), 2);
    assert_eq!(prompts[0].1[0].text, "input=BRIEF");
    assert_eq!(prompts[1].1[0].text, "input=BRIEF draft=answer");
    use bridge_core::task_store::TaskStore;
    assert_eq!(
        f.store
            .usage_summary_v1(&Default::default())
            .await
            .unwrap()
            .buckets
            .len(),
        1
    );
    let rows = f.rows();
    assert_eq!(rows.len(), 2);
    for node in ["draft", "review"] {
        let row = rows
            .iter()
            .find(|r| r.attribution.node.as_deref() == Some(node))
            .unwrap();
        assert_eq!(
            row.attribution.execution.as_deref(),
            Some(owner.execution_id.as_str())
        );
        assert_eq!(
            row.attribution.run_attempt.as_deref(),
            Some(owner.attempt_id.as_str())
        );
        support::assert_billing(row);
        assert_eq!(row.attribution.workflow.as_deref(), Some("accounted"));
        assert_eq!(row.attribution.turn.as_deref(), Some(row.id.as_str()));
        assert_ne!(row.id, owner.attempt_id.as_str());
        assert!(row.dispatched && row.finalized);
    }
    support::assert_usage(&f.reopened(Some(&owner)).await, 2, 0);
}

use bridge_core::{
    domain::{AgentEntry, AgentOverride, Part, RegistrySnapshot, SessionSpec},
    error::BridgeError,
    ids::{AgentId, SessionId},
    ports::{
        AgentBackend, AgentRegistry, BackendCleanupDispositionV1, BackendStream,
        DiagnosticObserver, Update,
    },
};
use bridge_workflow::executor::{NodeTurn, NodeTurnCleanup, NodeTurnExit, WorkflowNodeDispatcher};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Default)]
struct Backend {
    recording: WorkflowRecording,
    retry_once: AtomicBool,
    eof_once: AtomicBool,
    native_missing_once: AtomicBool,
    incomplete_cleanup: AtomicBool,
    preflight: AtomicUsize,
    config_calls: AtomicUsize,
    gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
}
#[async_trait::async_trait]
impl AgentBackend for Backend {
    async fn configure_session(
        &self,
        session: &SessionId,
        spec: &SessionSpec,
    ) -> Result<(), BridgeError> {
        self.config_calls.fetch_add(1, Ordering::SeqCst);
        if spec.config.model.as_deref() == Some("reject") {
            return Err(BridgeError::ModelNotAvailable);
        }
        self.recording.configure_session(session, spec).await
    }
    async fn configure_bound_session(
        &self,
        session: &SessionId,
        spec: &bridge_core::execution_policy::BoundSessionSpecV1,
    ) -> Result<(), BridgeError> {
        self.configure_session(session, &spec.session).await
    }
    async fn prompt(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
    ) -> Result<BackendStream, BridgeError> {
        if session.as_str().starts_with("workflow-preflight-") {
            self.preflight.fetch_add(1, Ordering::SeqCst);
            self.recording
                .prompts
                .lock()
                .unwrap()
                .push((session.clone(), parts));
            if let Some((entered, release)) = &self.gate {
                entered.notify_one();
                release.notified().await;
            }
            return Ok(Box::pin(futures::stream::iter([
                Ok(Update::Usage(support::usage())),
                Ok(Update::Text("PONG".into())),
                Ok(Update::done("end_turn")),
            ])));
        }
        let eof = self.eof_once.swap(false, Ordering::SeqCst);
        let native = self.native_missing_once.swap(false, Ordering::SeqCst);
        if eof || native {
            self.recording
                .prompts
                .lock()
                .unwrap()
                .push((session.clone(), parts));
            let mut items = vec![Ok(Update::Usage(support::usage()))];
            if native {
                items.push(Err(BridgeError::MissingTerminal));
            }
            return Ok(Box::pin(futures::stream::iter(items)));
        }
        if self.retry_once.swap(false, Ordering::SeqCst) {
            self.recording
                .prompts
                .lock()
                .unwrap()
                .push((session.clone(), parts));
            return Err(BridgeError::agent_crashed("permitted cold retry"));
        }
        self.recording.prompt(session, parts).await
    }
    async fn forget_session_checked(
        &self,
        _: &SessionId,
    ) -> Result<BackendCleanupDispositionV1, BridgeError> {
        Ok(if self.incomplete_cleanup.load(Ordering::SeqCst) {
            BackendCleanupDispositionV1::Unknown
        } else {
            BackendCleanupDispositionV1::Complete
        })
    }
    async fn release_session_checked(
        &self,
        _: &SessionId,
    ) -> Result<BackendCleanupDispositionV1, BridgeError> {
        Ok(if self.incomplete_cleanup.load(Ordering::SeqCst) {
            BackendCleanupDispositionV1::Unknown
        } else {
            BackendCleanupDispositionV1::Complete
        })
    }
    async fn cancel(&self, _: &SessionId) -> Result<(), BridgeError> {
        Ok(())
    }
}
fn registry(
    backend: Arc<Backend>,
    preflight: bool,
    fail_resolution: bool,
) -> Arc<dyn AgentRegistry> {
    let mut entry = (*common::single_agent_registry("agent", backend.clone())
        .entry_snapshot(&AgentId::parse("agent").unwrap())
        .unwrap())
    .clone();
    entry.model = Some(if preflight { "reject" } else { "good" }.into());
    entry.preflight = preflight;
    entry.fallback_models = if preflight {
        vec!["good".into()]
    } else {
        vec![]
    };
    let spawn: bridge_registry::registry::BoundObservedSpawnFn = Arc::new(move |_, _, _| {
        let backend = backend.clone();
        Box::pin(async move {
            if fail_resolution {
                Err(BridgeError::AgentNotAuthenticated)
            } else {
                Ok(backend as Arc<dyn AgentBackend>)
            }
        })
    });
    Arc::new(
        bridge_registry::registry::Registry::new_bound_observed(
            RegistrySnapshot {
                billing_identities: vec![],
                default: entry.id.clone(),
                entries: vec![entry],
                allowed_cmds: vec!["test-cmd".into()],
            },
            spawn,
        )
        .unwrap(),
    )
}
fn single_graph(retry: bool) -> Arc<WorkflowGraph> {
    let mut g = (*graph()).clone();
    g.nodes.truncate(1);
    if retry {
        g.nodes[0].retry = Some(bridge_workflow::graph::RetryPolicy {
            max_attempts: 2,
            backoff_ms: 0,
            backoff_cap_ms: None,
        });
    }
    Arc::new(g)
}

#[tokio::test]
async fn fallback_preflight_and_cold_retry_capture_actual_selections() {
    let f = Fixture::new();
    let backend = Arc::new(Backend::default());
    backend.retry_once.store(true, Ordering::SeqCst);
    let owner = AttemptIdentity::initial().unwrap();
    let events = WorkflowExecutor::new(registry(backend.clone(), true, false))
        .run_with_diagnostic_context(
            single_graph(true),
            "BRIEF".into(),
            "retry".into(),
            CancellationToken::new(),
            context(&f, &owner),
        )
        .collect::<Vec<_>>()
        .await;
    assert!(events.iter().all(Result::is_ok), "{events:?}");
    assert!(events.iter().any(|e| matches!(
        e,
        Ok(bridge_workflow::executor::WorkflowEvent::NodeFinished { ok: true, .. })
    )));
    assert_eq!(backend.recording.prompts.lock().unwrap().len(), 3);
    assert_eq!(backend.preflight.load(Ordering::SeqCst), 1);
    let rows = f.rows();
    assert_eq!(rows.len(), 4);
    assert_eq!(
        rows.iter()
            .map(|r| &r.id)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        4
    );
    let rejected = rows
        .iter()
        .find(|r| r.attribution.effective_model.as_deref() == Some("reject"))
        .unwrap();
    assert!(!rejected.dispatched && rejected.finalized);
    assert_eq!(rejected.attribution.attempt_kind, "preflight");
    assert_eq!(
        rows.iter()
            .filter(|r| r.attribution.attempt_kind == "node")
            .count(),
        2
    );
    assert!(rows
        .iter()
        .filter(|r| r.dispatched)
        .all(|r| r.attribution.effective_model.as_deref() == Some("good")));
    assert!(backend
        .recording
        .applied
        .lock()
        .unwrap()
        .iter()
        .all(|(_, c)| c.model.as_deref() == Some("good")));
    let b = f.reopened(Some(&owner)).await;
    assert_eq!((b.attempted, b.dispatched, b.pending), (4, 3, 1));
}

#[tokio::test]
async fn concurrent_first_miss_and_cache_hit_only_account_initializer_preflight() {
    let f = Fixture::new();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let backend = Arc::new(Backend {
        gate: Some((entered.clone(), release.clone())),
        ..Default::default()
    });
    let mut g = (*graph()).clone();
    g.nodes[1].inputs.clear(); // Both nodes contend for the same first miss.
    g.nodes[1].prompt_template = "parallel {{input}}".into();
    // A third downstream node also exercises the completed cache entry.
    let mut terminal = g.nodes[1].clone();
    terminal.id = bridge_core::ids::NodeId::parse("terminal").unwrap();
    terminal.inputs = g.nodes.iter().map(|n| n.id.clone()).collect();
    g.nodes.push(terminal);
    let executor = WorkflowExecutor::new(registry(backend.clone(), true, false));
    let owner = AttemptIdentity::initial().unwrap();
    let stream = executor.run_with_diagnostic_context(
        Arc::new(g),
        "BRIEF".into(),
        "concurrent".into(),
        CancellationToken::new(),
        context(&f, &owner),
    );
    let run = async { stream.collect::<Vec<_>>().await };
    let control = async {
        entered.notified().await;
        tokio::task::yield_now().await;
        assert_eq!(backend.preflight.load(Ordering::SeqCst), 1);
        release.notify_one();
    };
    let (events, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(run, control)
    })
    .await
    .unwrap();
    assert!(events.iter().all(Result::is_ok), "{events:?}");
    assert_eq!(backend.preflight.load(Ordering::SeqCst), 1);
    assert_eq!(backend.recording.prompts.lock().unwrap().len(), 4);
    assert_eq!(f.rows().len(), 5); // rejected primary, successful fallback, three executions
    let b = f.reopened(Some(&owner)).await;
    assert_eq!((b.attempted, b.dispatched, b.pending), (5, 4, 0));
}

fn refuse(f: &Fixture, operation: &str) {
    // Preserve the store's descriptor-pinned journal during external fault injection.
    let conn =
        rusqlite::Connection::open(f.dir.path().join("tasks.sqlite.usage-v1.sqlite")).unwrap();
    conn.execute_batch("PRAGMA journal_mode=PERSIST;").unwrap();
    conn.execute_batch(&format!("CREATE TRIGGER refuse BEFORE {operation} ON usage_attempts_v1 BEGIN SELECT RAISE(ABORT, 'refused'); END;")).unwrap();
}
#[tokio::test]
async fn resolution_configuration_and_admission_failures_never_prompt() {
    use bridge_core::task_store::TaskStore;
    for mode in ["resolve", "configure", "admit", "settle"] {
        let f = Fixture::new();
        f.store.usage_summary_v1(&Default::default()).await.unwrap();
        let backend = Arc::new(Backend::default());
        if mode == "configure" || mode == "settle" {
            *backend.recording.reject_config.lock().unwrap() =
                Some(BridgeError::agent_crashed("original preparation failure"));
        }
        if mode == "admit" {
            refuse(&f, "INSERT");
        }
        if mode == "settle" {
            refuse(&f, "UPDATE");
        }
        let owner = AttemptIdentity::initial().unwrap();
        let events = WorkflowExecutor::new(registry(backend.clone(), false, mode == "resolve"))
            .run_with_diagnostic_context(
                single_graph(true),
                "BRIEF".into(),
                mode.into(),
                CancellationToken::new(),
                context(&f, &owner),
            )
            .collect::<Vec<_>>()
            .await;
        assert!(backend.recording.prompts.lock().unwrap().is_empty());
        let rows = f.rows();
        if mode == "admit" {
            assert!(rows.is_empty());
        } else {
            assert_eq!(
                rows.len(),
                if mode == "configure" { 2 } else { 1 },
                "mode={mode} events={events:?}"
            );
            assert!(rows
                .iter()
                .all(|r| !r.dispatched && r.finalized == (mode != "settle")));
        }
        if mode == "settle" || mode == "admit" {
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e, Err(BridgeError::StoreFailure))),
                "{events:?}"
            );
            assert_eq!(
                backend.config_calls.load(Ordering::SeqCst),
                usize::from(mode == "settle")
            );
        }
    }
}

#[tokio::test]
async fn missing_terminal_keeps_observed_usage_pending_and_seeds_do_no_work() {
    for seeded in [0, 1, 2] {
        let f = Fixture::new();
        let backend = Arc::new(WorkflowRecording::default());
        backend.interrupt.store(true, Ordering::SeqCst);
        let seed = ["draft", "review"]
            .into_iter()
            .take(seeded)
            .map(|n| (n.into(), ("seed".into(), true, None)))
            .collect();
        let owner = AttemptIdentity::initial().unwrap();
        let events = WorkflowExecutor::new(common::single_agent_registry("agent", backend.clone()))
            .run_from_with_diagnostic_context(
                graph(),
                "BRIEF".into(),
                "resume".into(),
                CancellationToken::new(),
                seed,
                context(&f, &owner),
            )
            .collect::<Vec<_>>()
            .await;
        assert!(events.iter().all(Result::is_ok), "{events:?}");
        let count = 2 - seeded; // Legacy DAGs continue with a failed upstream marker.
        assert_eq!(backend.prompts.lock().unwrap().len(), count);
        if count == 0 {
            use bridge_core::task_store::TaskStore;
            assert!(f
                .store
                .usage_summary_v1(&Default::default())
                .await
                .unwrap()
                .buckets
                .is_empty());
        } else {
            let rows = f.rows();
            assert_eq!(rows.len(), count);
            assert!(rows
                .iter()
                .all(|r| r.dispatched && !r.finalized && r.usage == support::usage()));
            let b = f.reopened(Some(&owner)).await;
            assert_eq!(
                (b.attempted, b.dispatched, b.pending),
                (count as u64, count as u64, count as u64)
            );
            if seeded == 1 {
                assert_eq!(
                    backend.prompts.lock().unwrap()[0].1[0].text,
                    "input=BRIEF draft=seed"
                );
            }
        }
    }
}

struct Warm(Arc<Backend>, Arc<dyn AgentRegistry>);
struct Cleanup;
#[async_trait::async_trait]
impl NodeTurnCleanup for Cleanup {
    async fn on_exit(self: Box<Self>, _: NodeTurnExit) {}
}
#[async_trait::async_trait]
impl WorkflowNodeDispatcher for Warm {
    async fn checkout(
        &self,
        _: &str,
        _: &bridge_workflow::graph::WorkflowNode,
        _: &str,
        _: &WorkflowRunContext,
    ) -> Result<NodeTurn, BridgeError> {
        unreachable!()
    }
    async fn checkout_observed_with_overrides(
        &self,
        _: &str,
        node: &bridge_workflow::graph::WorkflowNode,
        _: &str,
        _: &WorkflowRunContext,
        overrides: Option<AgentOverride>,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<NodeTurn, BridgeError> {
        let resolved = self.1.resolve_observed(&node.agent, observer).await?;
        let session = SessionId::parse(format!("warm-{}", node.id.as_str())).unwrap();
        let config = bridge_core::domain::effective_config(&resolved.entry, overrides.as_ref());
        self.0
            .configure_session(&session, &SessionSpec::from_config(config.clone()))
            .await?;
        Ok(NodeTurn {
            selection: Some((node.agent.clone(), config)),
            backend: self.0.clone(),
            session,
            seed: Some("earlier".into()),
            cleanup: Box::new(Cleanup),
        })
    }
}
#[tokio::test]
async fn ordinary_warm_executor_captures_each_prompt_without_observers() {
    let f = Fixture::new();
    let backend = Arc::new(Backend::default());
    let registry = registry(backend.clone(), false, false);
    let owner = AttemptIdentity::initial().unwrap();
    let events = WorkflowExecutor::new(registry.clone())
        .run_with_diagnostic_context_and_dispatcher(
            graph(),
            "BRIEF".into(),
            "warm".into(),
            CancellationToken::new(),
            context(&f, &owner),
            Arc::new(Warm(backend.clone(), registry)),
        )
        .collect::<Vec<_>>()
        .await;
    assert!(events.iter().all(Result::is_ok), "{events:?}");
    assert_eq!(backend.recording.prompts.lock().unwrap().len(), 2);
    assert!(backend
        .recording
        .prompts
        .lock()
        .unwrap()
        .iter()
        .all(|(_, p)| p[0].text.contains("earlier")));
    support::assert_usage(&f.reopened(Some(&owner)).await, 2, 0);
}

#[tokio::test]
async fn bound_cold_executor_keeps_owner_and_selected_config() {
    let f = Fixture::new();
    let backend = Arc::new(Backend::default());
    let registry = registry(backend.clone(), false, false);
    let owner = AttemptIdentity::initial().unwrap();
    let (g, ctx) = frozen_context(&f, &owner, &registry);
    let events = WorkflowExecutor::new(registry)
        .run_with_diagnostic_context(
            Arc::new(g),
            "BRIEF".into(),
            "bound".into(),
            CancellationToken::new(),
            ctx,
        )
        .collect::<Vec<_>>()
        .await;
    assert!(events.iter().all(Result::is_ok), "{events:?}");
    assert_eq!(backend.recording.prompts.lock().unwrap().len(), 1);
    assert_eq!(
        f.rows()[0].attribution.effective_model.as_deref(),
        Some("good")
    );
    support::assert_usage(&f.reopened(Some(&owner)).await, 1, 0);
}

fn frozen_context(
    f: &Fixture,
    owner: &AttemptIdentity,
    registry: &Arc<dyn AgentRegistry>,
) -> (WorkflowGraph, WorkflowDiagnosticContext) {
    use bridge_core::execution_policy::*;
    let entry = registry
        .entry_snapshot(&AgentId::parse("agent").unwrap())
        .unwrap();
    let mut g = (*single_graph(false)).clone();
    g.controls = Some(Default::default());
    let controls = resolve_execution_policy_v1(
        g.controls.as_ref().unwrap(),
        &Default::default(),
        false,
        PolicyActivationV1::Production,
    )
    .unwrap();
    let cwd = bridge_core::SessionCwd::parse(f.dir.path().to_str().unwrap()).unwrap();
    let node = PolicyNodeRefV1::from_node_id(0, "draft");
    let mut providers = Vec::new();
    for logical_session in [
        FrozenProviderLogicalSessionV1::Preflight {
            candidate_ordinal: 0,
        },
        FrozenProviderLogicalSessionV1::Execute {
            candidate_ordinal: 0,
        },
    ] {
        if matches!(
            logical_session,
            FrozenProviderLogicalSessionV1::Preflight { .. }
        ) && !entry.preflight
        {
            continue;
        }
        let provider = freeze_provider_attempt_v1(&ProviderFreezeInputV1 {
            entry: &entry,
            overrides: None,
            node: node.clone(),
            logical_session,
            checkout: freeze_direct_checkout_v1(cwd.clone()),
            provider_effect_key: None,
        })
        .unwrap();
        providers.push(provider);
    }
    let identity = freeze_node_execution_identity_v1(node, providers).unwrap();
    let spec = Arc::new(
        bridge_workflow::run_spec::WorkflowRunSpecV1::build(
            owner.attempt_id.clone(),
            g.clone(),
            controls,
            Some(cwd.clone()),
            vec![identity],
            LedgerAdmissionV1::DurablePrimaryTaskStore,
        )
        .unwrap(),
    );
    let ctx = WorkflowDiagnosticContext::in_memory(WorkflowRunContext {
        session_cwd: Some(cwd),
        ..Default::default()
    })
    .with_accounting(WorkflowAccountingInvocationV1::new(
        support::billing_accounting(f.store.clone()),
        owner.clone(),
        g.id.clone(),
    ))
    .with_frozen_run_spec(spec, None)
    .unwrap();
    (g, ctx)
}

fn snapshot(entry: AgentEntry) -> RegistrySnapshot {
    RegistrySnapshot {
        billing_identities: vec![],
        default: entry.id.clone(),
        entries: vec![entry],
        allowed_cmds: vec!["test-cmd".into()],
    }
}

#[tokio::test]
async fn review_reload_between_admission_and_resolution_never_misattributes() {
    use bridge_core::domain::Effort;
    for drift in ["unchanged", "model", "effort", "mode"] {
        let f = Fixture::new();
        let backend = Arc::new(Backend::default());
        let entry = registry(backend.clone(), false, false)
            .entry_snapshot(&AgentId::parse("agent").unwrap())
            .unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let (at_spawn, resume, provider) = (entered.clone(), release.clone(), backend.clone());
        let spawn: bridge_registry::registry::SpawnFn = Arc::new(move |_| {
            let (at_spawn, resume, provider) = (at_spawn.clone(), resume.clone(), provider.clone());
            Box::pin(async move {
                at_spawn.notify_one();
                resume.notified().await;
                Ok(provider as Arc<dyn AgentBackend>)
            })
        });
        let registry: Arc<dyn AgentRegistry> = Arc::new(
            bridge_registry::registry::Registry::new(snapshot((*entry).clone()), spawn).unwrap(),
        );
        let owner = AttemptIdentity::initial().unwrap();
        let invocation = governed(&f, &owner, budget(10, None)).await;
        let run = WorkflowExecutor::new(registry.clone())
            .run_with_diagnostic_context(
                single_graph(false),
                "BRIEF".into(),
                "reload".into(),
                CancellationToken::new(),
                WorkflowDiagnosticContext::in_memory(WorkflowRunContext::default())
                    .with_accounting(invocation),
            )
            .collect::<Vec<_>>();
        let control = async {
            entered.notified().await;
            assert_eq!(f.rows().len(), 1); // Admission precedes this real registry resolution barrier.
            let mut next = (*entry).clone();
            match drift {
                "model" => next.model = Some("other".into()),
                "effort" => next.effort = Some(Effort::High),
                "mode" => next.mode = Some("plan".into()),
                _ => {}
            }
            registry.apply(snapshot(next)).await.unwrap();
            release.notify_one();
        };
        let (events, ()) = tokio::join!(run, control);
        assert!(events.iter().all(Result::is_ok), "{events:?}");
        assert_eq!(
            backend.recording.prompts.lock().unwrap().len(),
            usize::from(drift == "unchanged"),
            "{drift}"
        );
        assert_eq!(reservations(&f).len(), usize::from(drift == "unchanged"));
        let rows = f.rows();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].finalized);
        assert_eq!(rows[0].dispatched, drift == "unchanged");
        for (_, config) in backend.recording.applied.lock().unwrap().iter() {
            assert_eq!(rows[0].attribution.effective_model, config.model);
            assert_eq!(
                rows[0].attribution.effective_effort,
                config.effort.map(|e| format!("{e:?}").to_ascii_lowercase())
            );
            assert_eq!(rows[0].attribution.effective_mode, config.mode);
        }
        assert_eq!(f.reopened(Some(&owner)).await.pending, 0);
    }
}

// Cold model/effort/mode reload coverage above already passed on f39cf10f.
// Bound paths and preflight were also pinned/checked there; warm drift was not.
#[tokio::test]
async fn admitted_selection_survives_reload_on_every_other_dispatch_path() {
    for lane in [
        "bound-cold",
        "preflight",
        "bound-preflight",
        "warm-control",
        "warm",
    ] {
        let f = Fixture::new();
        let backend = Arc::new(Backend::default());
        let mut entry = (*registry(backend.clone(), false, false)
            .entry_snapshot(&AgentId::parse("agent").unwrap())
            .unwrap())
        .clone();
        entry.preflight = lane.contains("preflight");
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let (at_spawn, resume, provider) = (entered.clone(), release.clone(), backend.clone());
        let spawn: bridge_registry::registry::BoundObservedSpawnFn = Arc::new(move |_, _, _| {
            let (at_spawn, resume, provider) = (at_spawn.clone(), resume.clone(), provider.clone());
            Box::pin(async move {
                at_spawn.notify_one();
                resume.notified().await;
                Ok(provider as Arc<dyn AgentBackend>)
            })
        });
        let registry: Arc<dyn AgentRegistry> = Arc::new(
            bridge_registry::registry::Registry::new_bound_observed(snapshot(entry.clone()), spawn)
                .unwrap(),
        );
        let owner = AttemptIdentity::initial().unwrap();
        let (g, ctx) = if lane.starts_with("bound") {
            let (g, ctx) = frozen_context(&f, &owner, &registry);
            (Arc::new(g), ctx)
        } else {
            (single_graph(false), context(&f, &owner))
        };
        let executor = WorkflowExecutor::new(registry.clone());
        let stream = if lane.starts_with("warm") {
            executor.run_with_diagnostic_context_and_dispatcher(
                g,
                "BRIEF".into(),
                lane.into(),
                CancellationToken::new(),
                ctx,
                Arc::new(Warm(backend.clone(), registry.clone())),
            )
        } else {
            executor.run_with_diagnostic_context(
                g,
                "BRIEF".into(),
                lane.into(),
                CancellationToken::new(),
                ctx,
            )
        };
        let control = async {
            entered.notified().await;
            let rows = f.rows();
            assert_eq!(
                rows.len(),
                1,
                "{lane}: admission must precede resolution/checkout"
            );
            assert!(!rows[0].dispatched && !rows[0].finalized);
            if lane != "warm-control" {
                entry.model = Some("other".into());
            }
            registry.apply(snapshot(entry)).await.unwrap();
            release.notify_one();
        };
        let (events, ()) = tokio::join!(stream.collect::<Vec<_>>(), control);
        assert!(events.iter().all(Result::is_ok), "{lane}: {events:?}");
        let prompts = backend.recording.prompts.lock().unwrap().clone();
        assert_eq!(prompts.len(), usize::from(lane != "warm"), "{lane}");
        let rows = f.rows();
        assert!(rows.iter().all(|r| r.finalized), "{lane}: {rows:?}");
        assert_eq!(rows[0].dispatched, lane != "warm", "{lane}");
        assert_eq!(rows[0].attribution.effective_model.as_deref(), Some("good"));
        let applied = backend.recording.applied.lock().unwrap().clone();
        for (session, _) in prompts.iter() {
            let config = &applied.iter().find(|(id, _)| id == session).unwrap().1;
            assert_eq!(rows[0].attribution.effective_model, config.model, "{lane}");
            assert_eq!(
                rows[0].attribution.effective_effort,
                config.effort.map(|e| format!("{e:?}").to_ascii_lowercase())
            );
            assert_eq!(rows[0].attribution.effective_mode, config.mode);
        }
        assert_eq!(f.reopened(Some(&owner)).await.pending, 0, "{lane}");
    }
}

#[tokio::test]
async fn review_bound_preflight_binding_refusal_settles_or_keeps_persistence_visible() {
    use bridge_core::task_store::TaskStore;
    for fail_settlement in [false, true] {
        let f = Fixture::new();
        let backend = Arc::new(Backend::default());
        let registry = registry(backend.clone(), false, false);
        let mut entry = (*registry
            .entry_snapshot(&AgentId::parse("agent").unwrap())
            .unwrap())
        .clone();
        entry.preflight = true;
        registry.apply(snapshot(entry.clone())).await.unwrap();
        let owner = AttemptIdentity::initial().unwrap();
        let (g, ctx) = frozen_context(&f, &owner, &registry);
        entry.model = Some("drift".into());
        registry.apply(snapshot(entry)).await.unwrap();
        f.store.usage_summary_v1(&Default::default()).await.unwrap();
        if fail_settlement {
            refuse(&f, "UPDATE");
        }
        let events = WorkflowExecutor::new(registry)
            .run_with_diagnostic_context(
                Arc::new(g),
                "BRIEF".into(),
                "bound-drift".into(),
                CancellationToken::new(),
                ctx,
            )
            .collect::<Vec<_>>()
            .await;
        assert!(backend.recording.prompts.lock().unwrap().is_empty());
        assert_eq!(backend.config_calls.load(Ordering::SeqCst), 0);
        let rows = f.rows();
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].dispatched);
        assert_eq!(rows[0].finalized, !fail_settlement);
        assert_eq!(
            f.reopened(Some(&owner)).await.pending,
            u64::from(fail_settlement)
        );
        if fail_settlement {
            assert!(events
                .iter()
                .any(|e| matches!(e, Err(BridgeError::StoreFailure))));
        }
    }
}

#[tokio::test]
async fn review_eof_retry_policy_and_pending_usage_match_legacy() {
    for accounted in [false, true] {
        for (native, incomplete) in [(false, false), (false, true), (true, false)] {
            let f = Fixture::new();
            let backend = Arc::new(Backend::default());
            backend.eof_once.store(!native, Ordering::SeqCst);
            backend.native_missing_once.store(native, Ordering::SeqCst);
            backend
                .incomplete_cleanup
                .store(incomplete, Ordering::SeqCst);
            let owner = AttemptIdentity::initial().unwrap();
            let ctx = if accounted {
                context(&f, &owner)
            } else {
                WorkflowDiagnosticContext::in_memory(Default::default())
            };
            let events = WorkflowExecutor::new(registry(backend.clone(), false, false))
                .run_with_diagnostic_context(
                    single_graph(true),
                    "BRIEF".into(),
                    "eof-retry".into(),
                    CancellationToken::new(),
                    ctx,
                )
                .collect::<Vec<_>>()
                .await;
            let retry = !native && !incomplete;
            assert!(events.iter().all(Result::is_ok), "{events:?}");
            assert_eq!(
                backend.recording.prompts.lock().unwrap().len(),
                1 + usize::from(retry),
                "accounted={accounted} native={native} incomplete={incomplete}"
            );
            assert_eq!(
                events.iter().any(|e| matches!(
                    e,
                    Ok(bridge_workflow::executor::WorkflowEvent::NodeFinished { ok: true, .. })
                )),
                retry
            );
            if accounted {
                let rows = f.rows();
                assert_eq!(
                    rows.iter()
                        .filter(|r| !r.finalized && r.dispatched && r.usage == support::usage())
                        .count(),
                    1
                );
                assert_eq!(
                    rows.iter().filter(|r| r.finalized).count(),
                    usize::from(retry)
                );
                assert_eq!(f.reopened(Some(&owner)).await.pending, 1);
            }
        }
    }
}

use bridge_core::{accounting::AccountingV1, governance::*};

fn budget(turns: u64, tokens: Option<u64>) -> Arc<BudgetPolicyV1> {
    Arc::new(
        BudgetPolicyV1::new(
            [
                BudgetSelectorV1::EachRequest,
                BudgetSelectorV1::EachWorkflowRun,
            ]
            .into_iter()
            .enumerate()
            .map(|(i, selector)| BudgetRuleV1 {
                id: format!("rule-{i}"),
                selector,
                limits: BudgetLimitsV1 {
                    turns: Some(turns),
                    terminal_tokens: tokens,
                },
            })
            .collect(),
            tokens
                .map(|_| TurnAllowanceV1 {
                    target: BudgetCandidateV1 {
                        agent: AgentId::parse("agent").unwrap(),
                        model: Some("good".into()),
                    },
                    terminal_tokens: std::num::NonZeroU64::new(100).unwrap(),
                })
                .into_iter()
                .collect(),
        )
        .unwrap(),
    )
}
async fn governed(
    f: &Fixture,
    owner: &AttemptIdentity,
    policy: Arc<BudgetPolicyV1>,
) -> WorkflowAccountingInvocationV1 {
    use bridge_core::workflow_history::*;
    // A real initial ledger allocation proves owner coverage, independently of telemetry.
    f.store
        .reserve(&AttemptReservation {
            identity: owner.clone(),
            task_id: None,
            workflow: "accounted".into(),
            task_class: "test".into(),
            surface: ExecutionSurface::Offline,
            policy: "test".into(),
            workload_fingerprint: "shape-test".into(),
            started_ms: bridge_core::task_store::system_wall_now_ms(),
            workload_fingerprint_complete: true,
            prompt_acceptance: "not_dispatched".into(),
            pinned: false,
        })
        .await
        .unwrap();
    let mut accounting = AccountingV1::new(f.store.clone()); // No billing or metrics.
    accounting.budget_policy = Some(policy);
    WorkflowAccountingInvocationV1::new(
        accounting,
        owner.clone(),
        WorkflowId::parse("accounted").unwrap(),
    )
}
fn reservations(f: &Fixture) -> Vec<serde_json::Value> {
    let conn =
        rusqlite::Connection::open(f.dir.path().join("tasks.sqlite.usage-v1.sqlite")).unwrap();
    let count: u64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name LIKE 'governance_%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    if count == 0 {
        return vec![];
    }
    let mut stmt = conn
        .prepare("SELECT record_json FROM governance_reservations_v1 ORDER BY id")
        .unwrap();
    stmt.query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
        .collect()
}
async fn prepared(
    invocation: &WorkflowAccountingInvocationV1,
) -> bridge_core::translator::PreparedAccountingTurnV1 {
    invocation
        .select(
            &bridge_core::ids::NodeId::parse("draft").unwrap(),
            &AgentId::parse("agent").unwrap(),
            "node",
            &bridge_core::domain::EffectiveConfig {
                model: Some("good".into()),
                ..Default::default()
            },
        )
        .unwrap()
        .prepare()
        .await
        .unwrap()
}

#[tokio::test]
async fn budget_boundary_exhaustion_and_undispatched_settlement_failure() {
    for (cap, fault) in [(0, false), (1, false), (0, true)] {
        let f = Fixture::new();
        let owner = AttemptIdentity::initial().unwrap();
        let invocation = governed(&f, &owner, budget(cap, None)).await;
        let turn = prepared(&invocation).await;
        if fault {
            refuse(&f, "UPDATE");
        }
        let backend = Arc::new(WorkflowRecording::default());
        let result = workflow_support::dispatch(
            backend.clone(),
            turn,
            &SessionId::parse("budget").unwrap(),
            vec![],
        )
        .await;
        assert_eq!(backend.prompts.lock().unwrap().len(), cap as usize);
        assert_eq!(reservations(&f).len(), cap as usize);
        if cap == 0 {
            let BridgeError::BudgetRefused { refusal } = result.unwrap_err() else {
                panic!("expected refusal")
            };
            assert_eq!(refusal.reason, BudgetRefusalReasonV1::LimitExceeded);
            assert_eq!(
                refusal.persistence.as_deref(),
                fault.then_some(&BridgeError::StoreFailure)
            );
            assert!(!f.rows()[0].dispatched);
            assert_eq!(f.rows()[0].finalized, !fault);
        } else {
            assert!(result.is_ok());
            assert!(f.rows()[0].finalized && f.rows()[0].dispatched);
            let second = prepared(&invocation).await;
            assert!(workflow_support::dispatch(
                backend.clone(),
                second,
                &SessionId::parse("second").unwrap(),
                vec![]
            )
            .await
            .is_err());
            assert_eq!(backend.prompts.lock().unwrap().len(), 1);
            assert_eq!(reservations(&f).len(), 1);
        }
    }
}

#[tokio::test]
async fn governed_preflight_retry_and_cache_keep_one_reservation_per_actual_prompt() {
    for retry in [false, true] {
        let f = Fixture::new();
        let owner = AttemptIdentity::initial().unwrap();
        let invocation = governed(&f, &owner, budget(10, None)).await;
        let backend = Arc::new(Backend::default());
        backend.retry_once.store(retry, Ordering::SeqCst);
        let events = WorkflowExecutor::new(registry(backend.clone(), true, false))
            .run_with_diagnostic_context(
                if retry { single_graph(true) } else { graph() },
                "BRIEF".into(),
                "governed".into(),
                CancellationToken::new(),
                WorkflowDiagnosticContext::in_memory(WorkflowRunContext::default())
                    .with_accounting(invocation),
            )
            .collect::<Vec<_>>()
            .await;
        assert!(events.iter().all(Result::is_ok), "{events:?}");
        assert_eq!(backend.preflight.load(Ordering::SeqCst), 1);
        assert_eq!(backend.recording.prompts.lock().unwrap().len(), 3);
        let saved = reservations(&f);
        assert_eq!(saved.len(), 3);
        assert_eq!(f.rows().len(), 4); // Rejected model gets no reservation; cache hit adds none.
        for r in saved {
            let r = &r["reservation"];
            assert_eq!(
                r["context"]["request"]["Execution"],
                owner.execution_id.as_str()
            );
            assert_eq!(r["context"]["workflow_run"], owner.execution_id.as_str());
            assert_eq!(r["candidate"]["model"], "good");
            assert_eq!(r["scopes"].as_array().unwrap().len(), 2);
            assert_eq!(r["turns"], 1);
        }
    }
}

#[tokio::test]
async fn governed_preparation_and_completed_replay_never_reserve() {
    for mode in ["configure", "resolve", "replay"] {
        let f = Fixture::new();
        let owner = AttemptIdentity::initial().unwrap();
        let invocation = governed(&f, &owner, budget(0, None)).await;
        let backend = Arc::new(Backend::default());
        if mode == "configure" {
            *backend.recording.reject_config.lock().unwrap() = Some(BridgeError::ModelNotAvailable);
        }
        let ctx = WorkflowDiagnosticContext::in_memory(WorkflowRunContext::default())
            .with_accounting(invocation);
        let executor = WorkflowExecutor::new(registry(backend.clone(), false, mode == "resolve"));
        let seeds = if mode == "replay" {
            [("draft".into(), ("cached".into(), true, None))]
                .into_iter()
                .collect()
        } else {
            Default::default()
        };
        let events = executor
            .run_from_with_diagnostic_context(
                single_graph(false),
                "BRIEF".into(),
                "prep".into(),
                CancellationToken::new(),
                seeds,
                ctx,
            )
            .collect::<Vec<_>>()
            .await;
        assert!(events.iter().all(Result::is_ok), "{events:?}");
        assert!(backend.recording.prompts.lock().unwrap().is_empty());
        if mode == "replay" {
            assert!(!f.dir.path().join("tasks.sqlite.usage-v1.sqlite").exists());
            continue;
        }
        assert!(reservations(&f).is_empty());
        assert_eq!(f.rows().len(), usize::from(mode != "replay"));
        assert!(f.rows().iter().all(|r| r.finalized && !r.dispatched));
    }
}

#[tokio::test]
async fn governed_interruption_retains_liability_and_omitted_policy_cannot_bypass() {
    use bridge_core::accounting_backend::AccountingBackendV1;
    for mode in ["error", "eof", "drop", "cancel"] {
        let f = Fixture::new();
        let owner = AttemptIdentity::initial().unwrap();
        let invocation = governed(&f, &owner, budget(10, Some(100))).await;
        let turn = prepared(&invocation).await;
        let backend = Arc::new(Backend::default());
        backend.retry_once.store(mode == "error", Ordering::SeqCst);
        backend.eof_once.store(mode == "eof", Ordering::SeqCst);
        let captured = AccountingBackendV1::new(backend.clone());
        let session = SessionId::parse("pending").unwrap();
        let result = captured
            .prompt_accounted_v1(&session, vec![], turn.into_attempt(), None)
            .await;
        if mode == "error" {
            assert!(result.is_err());
        } else {
            let mut stream = result.unwrap();
            if mode == "eof" {
                assert!(stream.collect::<Vec<_>>().await.last().unwrap().is_err());
            } else {
                if mode == "cancel" {
                    assert!(stream.next().await.unwrap().is_ok());
                    captured.cancel(&session).await.unwrap();
                }
                drop(stream);
            }
        }
        assert_eq!(backend.recording.prompts.lock().unwrap().len(), 1);
        assert!(f.rows()[0].dispatched && !f.rows()[0].finalized);
        assert_eq!(reservations(&f)[0]["liability"]["Pending"], 100);
        let second = prepared(&invocation).await;
        assert!(captured
            .prompt_accounted_v1(&session, vec![], second.into_attempt(), None)
            .await
            .is_err());
        let ungoverned = WorkflowAccountingInvocationV1::new(
            AccountingV1::new(f.store.clone()),
            owner.clone(),
            WorkflowId::parse("accounted").unwrap(),
        );
        let omitted = prepared(&ungoverned).await;
        let error = captured
            .prompt_accounted_v1(&session, vec![], omitted.into_attempt(), None)
            .await
            .err()
            .unwrap();
        let BridgeError::BudgetRefused { refusal } = error else {
            panic!("expected refusal")
        };
        assert_eq!(refusal.reason, BudgetRefusalReasonV1::PolicyMismatch);
        assert_eq!(backend.recording.prompts.lock().unwrap().len(), 1);
        assert_eq!(reservations(&f).len(), 1);
        let reopened = f.reopened(Some(&owner)).await;
        assert_eq!(
            (reopened.attempted, reopened.dispatched, reopened.pending),
            (3, 1, 1)
        );
    }
}

#[tokio::test]
async fn allowance_overrun_preserves_output_and_final_actuals() {
    use bridge_core::accounting_backend::AccountingBackendV1;
    let f = Fixture::new();
    let owner = AttemptIdentity::initial().unwrap();
    let mut policy = (*budget(10, Some(100))).clone();
    policy = BudgetPolicyV1::new(
        policy.rules().to_vec(),
        vec![TurnAllowanceV1 {
            target: policy.allowances()[0].target.clone(),
            terminal_tokens: std::num::NonZeroU64::new(1).unwrap(),
        }],
    )
    .unwrap();
    let invocation = governed(&f, &owner, Arc::new(policy)).await;
    let backend = Arc::new(WorkflowRecording::default());
    let output = workflow_support::dispatch(
        backend.clone(),
        prepared(&invocation).await,
        &SessionId::parse("overrun").unwrap(),
        vec![],
    )
    .await
    .unwrap();
    assert!(output
        .iter()
        .any(|u| matches!(u, Update::Text(t) if t == "answer")));
    assert!(f.rows()[0].finalized);
    assert_eq!(reservations(&f)[0]["liability"]["Known"], 15);
    assert_eq!(backend.prompts.lock().unwrap().len(), 1);
    // Exact selector binding: the prepared model has no allowance for this candidate.
    let turn = invocation
        .select(
            &bridge_core::ids::NodeId::parse("draft").unwrap(),
            &AgentId::parse("agent").unwrap(),
            "node",
            &Default::default(),
        )
        .unwrap()
        .prepare()
        .await
        .unwrap();
    let error = AccountingBackendV1::new(backend.clone())
        .prompt_accounted_v1(
            &SessionId::parse("absent").unwrap(),
            vec![],
            turn.into_attempt(),
            None,
        )
        .await
        .err()
        .unwrap();
    let BridgeError::BudgetRefused { refusal } = error else {
        panic!("expected refusal")
    };
    assert_eq!(refusal.reason, BudgetRefusalReasonV1::MissingAllowance);
    assert_eq!(backend.prompts.lock().unwrap().len(), 1);
    assert_eq!(reservations(&f).len(), 1);
}

#[tokio::test]
async fn ungoverned_capture_keeps_exact_rows_and_creates_no_governance_schema() {
    let f = Fixture::new();
    let owner = AttemptIdentity {
        execution_id: bridge_core::ids::ExecutionId::parse("exec-11111111111111111111111111111111")
            .unwrap(),
        attempt_id: bridge_core::ids::AttemptId::parse("attempt-22222222222222222222222222222222")
            .unwrap(),
        ordinal: 0,
        parent_attempt_id: None,
    };
    let concrete =
        bridge_core::ids::AttemptId::parse("attempt-33333333333333333333333333333333").unwrap();
    let accounting = AccountingV1 {
        clock: Arc::new(|| 123),
        ..AccountingV1::new(f.store.clone())
    };
    let turn = accounting
        .start(
            &owner,
            concrete.clone(),
            bridge_core::accounting::UsageAttributionV1 {
                agent: String::new(), // Legacy rows may lack attribution.
                ..Default::default()
            },
        )
        .await
        .unwrap();
    // Independent G-min.3 contract fixture: no candidate-produced row is used as expected data.
    let expected = bridge_core::accounting::UsageAttemptV1 {
        id: concrete.as_str().into(),
        attribution: bridge_core::accounting::UsageAttributionV1 {
            execution: Some(owner.execution_id.as_str().into()),
            run_attempt: Some(owner.attempt_id.as_str().into()),
            ..Default::default()
        },
        started_ms: 123,
        utc_day: 0,
        dispatched: true,
        finalized: true,
        usage: support::usage(),
        cost_invalid: false,
    };
    let backend = Arc::new(WorkflowRecording::default());
    let output = bridge_core::accounting_backend::AccountingBackendV1::new(backend.clone())
        .prompt_accounted_v1(&SessionId::parse("uncapped").unwrap(), vec![], turn, None)
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;
    assert!(output.iter().all(Result::is_ok));
    assert!(
        matches!(&output[..], [Ok(Update::Usage(usage)), Ok(Update::Text(text)), Ok(Update::Done { .. })]
        if usage == &support::usage() && text == "answer")
    );
    assert_eq!(
        serde_json::to_vec(&f.rows()[0]).unwrap(),
        serde_json::to_vec(&expected).unwrap()
    );
    assert_eq!(backend.prompts.lock().unwrap().len(), 1);
    let conn =
        rusqlite::Connection::open(f.dir.path().join("tasks.sqlite.usage-v1.sqlite")).unwrap();
    let tables: u64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name LIKE 'governance_%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tables, 0);
}

#[tokio::test]
async fn governed_invalid_identity_never_prompts_or_reserves() {
    for agent in ["agent", ""] {
        let f = Fixture::new();
        let owner = AttemptIdentity::initial().unwrap();
        let accounting = AccountingV1 {
            budget_policy: Some(budget(1, None)),
            budget_context: Some(BudgetContextV1 {
                request: BudgetRequestIdV1::Execution(if agent.is_empty() {
                    owner.execution_id.clone()
                } else {
                    AttemptIdentity::initial().unwrap().execution_id
                }),
                workflow_run: None,
            }),
            ..AccountingV1::new(f.store.clone())
        };
        let turn = accounting
            .start(
                &owner,
                bridge_core::ids::AttemptId::mint().unwrap(),
                bridge_core::accounting::UsageAttributionV1 {
                    agent: agent.into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let backend = Arc::new(WorkflowRecording::default());
        let error = bridge_core::accounting_backend::AccountingBackendV1::new(backend.clone())
            .prompt_accounted_v1(&SessionId::parse("identity").unwrap(), vec![], turn, None)
            .await
            .err()
            .unwrap();
        let BridgeError::BudgetRefused { refusal } = error else {
            panic!("expected refusal")
        };
        assert_eq!(refusal.reason, BudgetRefusalReasonV1::IdentityMismatch);
        assert!(backend.prompts.lock().unwrap().is_empty());
        assert!(reservations(&f).is_empty());
        assert!(f.rows()[0].finalized && !f.rows()[0].dispatched);
    }
}

#[tokio::test]
async fn uncapped_admission_lock_failure_keeps_legacy_error_and_pending_row() {
    use bridge_core::{diagnostics::DiagnosticFailureClass, execution_policy::NodeCauseV1};
    for configured in [false, true] {
        let f = Fixture::new();
        let owner = AttemptIdentity::initial().unwrap();
        let invocation = WorkflowAccountingInvocationV1::new(
            AccountingV1 {
                budget_policy: configured.then(|| budget(1, None)),
                ..AccountingV1::new(f.store.clone())
            },
            owner,
            WorkflowId::parse("accounted").unwrap(),
        );
        let turn = prepared(&invocation).await;
        let before = f.rows();
        let blocker =
            rusqlite::Connection::open(f.dir.path().join("tasks.sqlite.usage-v1.sqlite")).unwrap();
        blocker
            .execute_batch("PRAGMA journal_mode=PERSIST; BEGIN IMMEDIATE;")
            .unwrap();
        let backend = Arc::new(WorkflowRecording::default());
        let result = workflow_support::dispatch(
            backend.clone(),
            turn,
            &SessionId::parse("locked").unwrap(),
            vec![],
        )
        .await;
        blocker.execute_batch("ROLLBACK;").unwrap();
        let error = result.unwrap_err();
        let cause = NodeCauseV1::from_bridge_error(&error);
        if configured {
            let BridgeError::BudgetRefused { refusal } = error else {
                panic!("expected refusal")
            };
            assert_eq!(refusal.reason, BudgetRefusalReasonV1::AccountingUnavailable);
            assert_eq!(
                refusal.persistence.as_deref(),
                Some(&BridgeError::StoreFailure)
            );
            assert_eq!(cause.failure_class, DiagnosticFailureClass::Config);
            assert_eq!(cause.code.as_str(), "bridge.budget_refused");
        } else {
            assert_eq!(error, BridgeError::StoreFailure);
            assert_eq!(error.client_message(), "store failure");
            assert_eq!(cause.failure_class, DiagnosticFailureClass::Persistence);
            assert_eq!(cause.code.as_str(), "bridge.store_failure");
        }
        assert_eq!(f.rows(), before);
        assert!(backend.prompts.lock().unwrap().is_empty());
        assert!(reservations(&f).is_empty());
    }
}

#[tokio::test]
async fn independently_selected_targets_keep_distinct_allowances_and_admissions() {
    use bridge_core::{
        accounting_backend::AccountingBackendV1, domain::EffectiveConfig, ids::NodeId,
    };
    let f = Fixture::new();
    let owner = AttemptIdentity::initial().unwrap();
    let targets: Vec<_> = [
        ("agent", Some("good")),
        ("backup", Some("other")),
        ("unused", None),
    ]
    .into_iter()
    .map(|(agent, model)| BudgetCandidateV1 {
        agent: AgentId::parse(agent).unwrap(),
        model: model.map(str::to_owned),
    })
    .collect();
    let policy = BudgetPolicyV1::new(
        budget(10, Some(50)).rules().to_vec(),
        targets
            .iter()
            .zip([20, 30, 40])
            .map(|(target, tokens)| TurnAllowanceV1 {
                target: target.clone(),
                terminal_tokens: std::num::NonZeroU64::new(tokens).unwrap(),
            })
            .collect(),
    )
    .unwrap();
    let invocation = governed(&f, &owner, Arc::new(policy)).await;
    let select = |target: &BudgetCandidateV1| {
        invocation
            .select(
                &NodeId::parse("draft").unwrap(),
                &target.agent,
                "node",
                &EffectiveConfig {
                    model: target.model.clone(),
                    ..Default::default()
                },
            )
            .unwrap()
    };
    let unused = select(&targets[2]); // Inspection grants neither accounting nor dispatch authority.
    let backends = [
        Arc::new(WorkflowRecording::default()),
        Arc::new(WorkflowRecording::default()),
    ];
    for (target, backend) in targets.iter().take(2).zip(&backends) {
        let turn = select(target).prepare().await.unwrap();
        let stream = AccountingBackendV1::new(backend.clone())
            .prompt_accounted_v1(
                &SessionId::parse(target.agent.as_str()).unwrap(),
                vec![],
                turn.into_attempt(),
                None,
            )
            .await
            .unwrap();
        drop(stream); // Keep each target's allowance pending, without terminal reconciliation.
    }
    let saved = reservations(&f);
    assert_eq!(saved.len(), 2);
    assert_ne!(
        saved[0]["reservation"]["attempt"],
        saved[1]["reservation"]["attempt"]
    );
    for (target, tokens) in targets.iter().zip([20, 30]) {
        let r = saved
            .iter()
            .find(|r| r["reservation"]["candidate"] == serde_json::to_value(target).unwrap())
            .unwrap();
        assert_eq!(r["reservation"]["terminal_tokens"], tokens);
        assert_eq!(r["liability"]["Pending"], tokens);
        assert_eq!(
            r["reservation"]["context"]["request"]["Execution"],
            owner.execution_id.as_str()
        );
        assert_eq!(
            r["reservation"]["context"]["workflow_run"],
            owner.execution_id.as_str()
        );
    }
    assert_eq!(f.rows().len(), 2); // The inspected third candidate has no row or reservation.
    let turn = select(&targets[0]).prepare().await.unwrap();
    let error = AccountingBackendV1::new(backends[0].clone())
        .prompt_accounted_v1(
            &SessionId::parse("exhausted").unwrap(),
            vec![],
            turn.into_attempt(),
            None,
        )
        .await
        .err()
        .unwrap();
    let BridgeError::BudgetRefused { refusal } = error else {
        panic!("expected refusal")
    };
    assert_eq!(refusal.reason, BudgetRefusalReasonV1::LimitExceeded);
    assert_eq!(refusal.metric, Some(BudgetMetricV1::TerminalTokens));
    assert!(backends
        .iter()
        .all(|b| b.prompts.lock().unwrap().len() == 1));
    assert_eq!(reservations(&f), saved);
    let rows = f.rows();
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows.iter().filter(|r| r.dispatched && !r.finalized).count(),
        2
    );
    assert!(rows.iter().all(|r| r.attribution.agent != "unused"));
    drop(unused);
}

async fn regression_events(
    backend: Arc<Backend>,
    preflight: bool,
    ctx: WorkflowDiagnosticContext,
) -> Vec<Result<bridge_workflow::executor::WorkflowEvent, BridgeError>> {
    WorkflowExecutor::new(registry(backend, preflight, false))
        .run_with_diagnostic_context(
            single_graph(false),
            String::new(),
            "regression".into(),
            CancellationToken::new(),
            ctx,
        )
        .collect()
        .await
}

#[tokio::test]
async fn fresh_governed_workflow_crosses_real_dispatch_barrier() {
    for (cap, historical) in [(1, false), (0, false), (1, true)] {
        let f = Fixture::new();
        let owner = AttemptIdentity::initial().unwrap();
        local_workflow_accounting::establish_coverage(f.store.as_ref())
            .await
            .unwrap();
        let invocation = governed(&f, &owner, budget(cap, None)).await;
        let conn = rusqlite::Connection::open(f.dir.path().join("tasks.sqlite")).unwrap();
        if historical {
            conn.execute("UPDATE workflow_attempt_summaries SET started_ms=1", [])
                .unwrap();
        }
        // Force preparation into a later millisecond than owner birth on the unfixed base.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let backend = Arc::new(Backend::default());
        let ctx = WorkflowDiagnosticContext::in_memory(Default::default())
            .with_accounting(invocation)
            .with_prompt_dispatch_barrier(local_workflow_accounting::dispatch_barrier(
                f.store.clone(),
                owner.attempt_id.clone(),
                Default::default(),
            ));
        let events = regression_events(backend.clone(), false, ctx).await;
        let query = "SELECT prompt_acceptance FROM workflow_attempt_summaries";
        let acceptance: String = conn.query_row(query, [], |r| r.get(0)).unwrap();
        assert_eq!(acceptance, "dispatch_uncertain");
        let expected = usize::from(cap > 0 && !historical);
        let prompts = backend.recording.prompts.lock().unwrap().len();
        assert_eq!(prompts, expected, "{events:?}");
        assert_eq!(reservations(&f).len(), expected);
    }
}

#[tokio::test]
async fn budget_refusal_workflow_outputs_are_public_with_and_without_preflight() {
    use bridge_workflow::executor::WorkflowEvent;
    for preflight in [false, true] {
        let f = Fixture::new();
        let owner = AttemptIdentity::initial().unwrap();
        let mut rules = budget(0, None).rules().to_vec();
        rules[0].id = "PRIVATE-BUDGET-PROBE-9985".into();
        let policy = Arc::new(BudgetPolicyV1::new(rules, vec![]).unwrap());
        let invocation = governed(&f, &owner, policy).await;
        let backend = Arc::new(Backend::default());
        let ctx =
            WorkflowDiagnosticContext::in_memory(Default::default()).with_accounting(invocation);
        let events = regression_events(backend.clone(), preflight, ctx).await;
        let mut outputs = 0;
        assert!(matches!(
            events.last(),
            Some(Err(BridgeError::BudgetRefused { .. }))
        ));
        for event in events.into_iter().filter_map(Result::ok) {
            if let WorkflowEvent::NodeFinished { output, .. }
            | WorkflowEvent::Terminal { output, .. } = event
            {
                outputs += 1;
                assert!(output.contains("budget refused"), "{output}");
                for private in ["PRIVATE-BUDGET-PROBE-9985", "scope", "metric"] {
                    assert!(!output.contains(private), "{output}");
                }
            }
        }
        assert_eq!(outputs, 2);
        assert!(backend.recording.prompts.lock().unwrap().is_empty());
        assert!(reservations(&f).is_empty());
    }
}

#[tokio::test]
async fn non_budget_prompt_error_keeps_debug_node_output() {
    use bridge_workflow::executor::WorkflowEvent;
    let backend = Arc::new(Backend::default());
    backend.retry_once.store(true, Ordering::SeqCst);
    let ctx = WorkflowDiagnosticContext::in_memory(Default::default());
    let events = regression_events(backend, false, ctx).await;
    assert!(events.iter().any(
        |e| matches!(e, Ok(WorkflowEvent::NodeFinished { output, .. })
        if output == r#"[node draft failed: AgentCrashed { reason: "permitted cold retry" }]"#)
    ));
}

#[derive(Default)]
struct DrainingBackend {
    recording: WorkflowRecording,
    release: Arc<tokio::sync::Notify>,
    cancels: std::sync::atomic::AtomicUsize,
}
#[async_trait::async_trait]
impl AgentBackend for DrainingBackend {
    async fn configure_bound_session(
        &self,
        _: &SessionId,
        _: &bridge_core::execution_policy::BoundSessionSpecV1,
    ) -> Result<(), BridgeError> {
        Ok(())
    }
    async fn prompt(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
    ) -> Result<BackendStream, BridgeError> {
        let stream = self.recording.prompt(session, parts).await?;
        let release = self.release.clone();
        Ok(Box::pin(
            futures::stream::once(async move {
                release.notified().await;
                stream
            })
            .flatten(),
        ))
    }
    async fn cancel(&self, _: &SessionId) -> Result<(), BridgeError> {
        self.cancels.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn budget_stop_drains_siblings_terminalizes_queue_and_preserves_external_cancel() {
    use bridge_core::execution_policy::*;
    use bridge_workflow::executor::WorkflowEvent;
    for (bound, fail_fast, external_cancel) in [
        (false, false, false),
        (true, false, false),
        (true, true, false),
        (true, true, true),
    ] {
        let f = Fixture::new();
        let owner = AttemptIdentity::initial().unwrap();
        let invocation = governed(&f, &owner, budget(1, None)).await;
        let backend = Arc::new(DrainingBackend::default());
        let entry = common::single_agent_registry("agent", backend.clone())
            .entry_snapshot(&AgentId::parse("agent").unwrap())
            .unwrap();
        let provider = backend.clone();
        let spawn: bridge_registry::registry::BoundObservedSpawnFn = Arc::new(move |_, _, _| {
            let provider = provider.clone();
            Box::pin(async move { Ok(provider as Arc<dyn AgentBackend>) })
        });
        let registry: Arc<dyn AgentRegistry> = Arc::new(
            bridge_registry::registry::Registry::new_bound_observed(
                snapshot((*entry).clone()),
                spawn,
            )
            .unwrap(),
        );
        let mut g: WorkflowGraph = serde_json::from_value(serde_json::json!({
            "id":"accounted", "nodes":[
                {"id":"sibling", "agent":"agent", "prompt_template":"{{input}}", "inputs":[]},
                {"id":"refused", "agent":"agent", "prompt_template":"{{input}}", "inputs":[]},
                {"id":"queued", "agent":"agent", "prompt_template":"{{sibling}}{{refused}}", "inputs":["sibling","refused"]}
            ]
        })).unwrap();
        let cwd = bridge_core::SessionCwd::parse(f.dir.path().to_str().unwrap()).unwrap();
        let mut ctx = WorkflowDiagnosticContext::in_memory(WorkflowRunContext {
            session_cwd: Some(cwd.clone()),
            ..Default::default()
        })
        .with_accounting(invocation);
        if bound {
            g.controls = Some(WorkflowControlDefaultsV1 {
                fan_out: Some(if fail_fast {
                    FanOutPolicyV1::FailFast
                } else {
                    FanOutPolicyV1::BoundedIndependent
                }),
                ..Default::default()
            });
            let controls = resolve_execution_policy_v1(
                g.controls.as_ref().unwrap(),
                &Default::default(),
                false,
                PolicyActivationV1::Production,
            )
            .unwrap();

            let entry = registry
                .entry_snapshot(&AgentId::parse("agent").unwrap())
                .unwrap();
            let mut sorted_nodes = g.nodes.iter().collect::<Vec<_>>();
            sorted_nodes.sort_by_key(|node| &node.id);
            let identities = sorted_nodes
                .into_iter()
                .enumerate()
                .map(|(i, n)| {
                    let node = PolicyNodeRefV1::from_node_id(i as u32, n.id.as_str());
                    let provider = freeze_provider_attempt_v1(&ProviderFreezeInputV1 {
                        entry: &entry,
                        overrides: None,
                        node: node.clone(),
                        logical_session: FrozenProviderLogicalSessionV1::Execute {
                            candidate_ordinal: 0,
                        },
                        checkout: freeze_direct_checkout_v1(cwd.clone()),
                        provider_effect_key: None,
                    })
                    .unwrap();
                    freeze_node_execution_identity_v1(node, vec![provider]).unwrap()
                })
                .collect();
            let spec = bridge_workflow::run_spec::WorkflowRunSpecV1::build(
                owner.attempt_id.clone(),
                g.clone(),
                controls,
                Some(cwd),
                identities,
                LedgerAdmissionV1::DurablePrimaryTaskStore,
            )
            .unwrap();
            ctx = ctx.with_frozen_run_spec(Arc::new(spec), None).unwrap();
        }
        let cancel = CancellationToken::new();
        let executor = WorkflowExecutor::new(registry);
        let mut stream = executor.run_with_diagnostic_context(
            Arc::new(g),
            String::new(),
            "drain".into(),
            cancel.clone(),
            ctx,
        );
        let mut events = Vec::new();
        loop {
            let event = tokio::time::timeout(std::time::Duration::from_secs(3), stream.next())
                .await
                .unwrap()
                .unwrap();
            let refused = matches!(&event, Ok(WorkflowEvent::NodeFinished { node, .. }) if node.as_str() == "refused");
            events.push(event);
            if refused {
                break;
            }
        }
        assert_eq!(
            backend.recording.prompts.lock().unwrap().len(),
            1,
            "bound={bound} fail_fast={fail_fast}: {events:?}"
        );
        assert_eq!(backend.cancels.load(Ordering::SeqCst), 0);
        if external_cancel {
            cancel.cancel();
        } else {
            backend.release.notify_one();
        }
        events.extend(
            tokio::time::timeout(
                std::time::Duration::from_secs(3),
                stream.collect::<Vec<_>>(),
            )
            .await
            .unwrap(),
        );
        assert_eq!(
            events.iter().filter(|e| e.is_err()).count(),
            1,
            "{events:?}"
        );
        assert!(
            matches!(events.last(), Some(Err(BridgeError::BudgetRefused { refusal })) if refusal.reason == BudgetRefusalReasonV1::LimitExceeded)
        );
        let terminals: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                Ok(WorkflowEvent::NodeFinished {
                    node,
                    ok,
                    output,
                    terminal_json,
                    ..
                }) => Some((node, ok, output, terminal_json)),
                _ => None,
            })
            .collect();
        assert_eq!(terminals.len(), 3);
        let sibling = terminals
            .iter()
            .find(|t| t.0.as_str() == "sibling")
            .unwrap();
        assert_eq!(*sibling.1, !external_cancel);
        if !external_cancel {
            assert_eq!(sibling.2, "answer");
        }
        if bound {
            for terminal in &terminals {
                NodeTerminalV1::decode_canonical(terminal.3.as_ref().unwrap().as_bytes()).unwrap();
            }
        }
        assert_eq!(
            backend.cancels.load(Ordering::SeqCst),
            usize::from(external_cancel)
        );
        assert_eq!(backend.recording.prompts.lock().unwrap().len(), 1);
        assert_eq!(f.rows().len(), 2); // queued node has no attempt at all
        assert_eq!(f.rows().iter().filter(|r| r.dispatched).count(), 1);
        assert_eq!(
            f.rows().iter().filter(|r| r.finalized).count(),
            if external_cancel { 1 } else { 2 }
        );
    }
}

async fn c2_accounted_events(
    f: &Fixture,
    invocation: &WorkflowAccountingInvocationV1,
    candidate: &bridge_core::routing::FallbackCandidateV1,
    backend: Arc<dyn AgentBackend>,
) -> Vec<Result<bridge_core::translator::Event, bridge_core::translator::AccountingFailureV1>> {
    use bridge_core::{
        domain::EffectiveConfig, ids::*, ports::TurnContext, translator::Translator,
    };
    let turn = invocation
        .select(
            &NodeId::parse("draft").unwrap(),
            &candidate.agent,
            "node",
            &EffectiveConfig {
                model: Some(candidate.model.clone()),
                ..Default::default()
            },
        )
        .unwrap()
        .prepare()
        .await
        .unwrap();
    let session = SessionId::parse(candidate.agent.as_str()).unwrap();
    let task = TaskId::parse("governed-c2").unwrap();
    let context = TurnContext {
        turn_id: TurnId::parse("selected").unwrap(),
        session_id: ContextId::parse("selected").unwrap(),
        task_id: Some(task.clone()),
        workflow: None,
        node: None,
        attempt: 0,
        agent: candidate.agent.as_str().into(),
        model: Some(candidate.model.clone()),
        effort: None,
        mode: None,
        prompt_id: None,
        traceparent: None,
    };
    Translator::for_turn(backend.clone(), Some(turn))
        .run_observed(
            backend.as_ref(),
            f.store.as_ref(),
            &bridge_policy::permission::AutoPolicy,
            &task,
            &session,
            vec![],
            Arc::new(bridge_core::diagnostics::NoopDiagnosticObserver::default()),
            context,
            Arc::new(bridge_core::harvest::NoopHarvestAuditStore),
        )
        .collect::<Vec<_>>()
        .await
}

#[tokio::test]
async fn c2_selected_replacement_has_fresh_exact_admission_and_cannot_reuse_source_allowance() {
    use bridge_core::{
        diagnostics::DiagnosticFailureClass, fallback_runtime::FrozenFallbackInvocationV1,
        ports::BackendCleanupDispositionV1, routing::*,
    };
    #[derive(Default)]
    struct Quota(std::sync::atomic::AtomicUsize);
    #[async_trait::async_trait]
    impl AgentBackend for Quota {
        async fn prompt(&self, _: &SessionId, _: Vec<Part>) -> Result<BackendStream, BridgeError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(BridgeError::AgentFailure { diagnostic: Box::new(serde_json::from_value(serde_json::json!({
                "schema_version":1, "failed_phase":"prompt_start", "class":DiagnosticFailureClass::QuotaExhausted,
                "disposition":"fatal", "code":"fixture.quota", "summary":"quota", "causes":[],
                "stderr_observed":false, "stderr_line_count":0, "prompt_may_have_been_accepted":false
            })).unwrap()) })
        }
        async fn cancel(&self, _: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }
    for (cap, replacement_allowance, refusal) in [
        (50, Some(30), None),
        (49, Some(30), Some(BudgetRefusalReasonV1::LimitExceeded)),
        (50, None, Some(BudgetRefusalReasonV1::MissingAllowance)),
    ] {
        let f = Fixture::new();
        let owner = AttemptIdentity::initial().unwrap();
        let candidate = |agent, model: &str| FallbackCandidateV1 {
            agent: AgentId::parse(agent).unwrap(),
            model: model.into(),
            posture: CandidatePostureV1 {
                kind: "acp".into(),
                cmd: Some("fixture".into()),
                cwd: f.dir.path().display().to_string(),
                session_cwd: f.dir.path().display().to_string(),
                ..Default::default()
            },
        };
        let source = candidate("agent", "good");
        let replacement = candidate("backup", "other");
        let unused = candidate("unused", "last");
        let policy = BudgetPolicyV1::new(
            budget(10, Some(cap)).rules().to_vec(),
            [(&source, Some(20)), (&replacement, replacement_allowance)]
                .into_iter()
                .filter_map(|(candidate, allowance)| {
                    allowance.map(|tokens| TurnAllowanceV1 {
                        target: BudgetCandidateV1 {
                            agent: candidate.agent.clone(),
                            model: Some(candidate.model.clone()),
                        },
                        terminal_tokens: std::num::NonZeroU64::new(tokens).unwrap(),
                    })
                })
                .collect(),
        )
        .unwrap();
        let invocation = governed(&f, &owner, Arc::new(policy)).await;
        let chain = FrozenFallbackChainV1 {
            id: "capacity".into(),
            source: source.clone(),
            causes: [FallbackCauseV1::QuotaExhausted].into(),
            steps: vec![replacement.clone(), unused],
        };
        let fallback = FrozenFallbackInvocationV1::freeze(vec![chain], source.clone()).unwrap();
        let source_backend = Arc::new(Quota::default());
        let failure = c2_accounted_events(&f, &invocation, &source, source_backend.clone())
            .await
            .into_iter()
            .find_map(Result::err)
            .unwrap()
            .with_checked_cleanup(Ok(BackendCleanupDispositionV1::Complete));
        // The real translator establishes exclusion; C2 selects and binds the replacement.
        let (fallback, selected) = fallback
            .transition(failure, false, |effective| Ok(effective.clone()))
            .unwrap();
        assert_eq!(selected, replacement);
        assert_eq!(f.rows().len(), 1); // selection itself has no accounting or admission effects
        let target_backend = Arc::new(WorkflowRecording::default());
        let events = c2_accounted_events(&f, &invocation, &selected, target_backend.clone()).await;
        let saved = reservations(&f);
        assert_eq!(source_backend.0.load(Ordering::SeqCst), 1);
        assert_eq!(saved.len(), if refusal.is_some() { 1 } else { 2 });
        assert_eq!(
            target_backend.prompts.lock().unwrap().len(),
            usize::from(refusal.is_none())
        );
        let first = saved
            .iter()
            .find(|r| r["reservation"]["candidate"]["agent"] == "agent")
            .unwrap();
        assert_eq!(first["liability"]["Pending"], 20);
        if let Some(reason) = refusal {
            let failure = events.into_iter().find_map(Result::err).unwrap();
            assert!(
                matches!(&failure.original, BridgeError::BudgetRefused { refusal } if refusal.reason == reason)
            );
            let expected = failure.clone();
            assert_eq!(
                fallback
                    .transition(failure, false, |_| panic!(
                        "budget refusal advanced fallback"
                    ))
                    .err()
                    .unwrap(),
                expected
            );
        } else {
            let next = saved
                .iter()
                .find(|r| r["reservation"]["candidate"]["agent"] == "backup")
                .unwrap();
            assert_eq!(next["reservation"]["candidate"]["model"], "other");
            assert_eq!(next["reservation"]["terminal_tokens"], 30);
            assert_eq!(next["liability"]["Known"], 15);
            assert!(events.iter().all(Result::is_ok));
            assert!(events.iter().any(|event| matches!(event, Ok(event) if
                *event.kind() == bridge_core::translator::EventKind::Artifact && event.text() == "answer")));
            assert_ne!(
                next["reservation"]["attempt"],
                first["reservation"]["attempt"]
            );
            assert_eq!(
                next["reservation"]["context"],
                first["reservation"]["context"]
            );
        }
        assert_eq!(f.rows().len(), 2);
        assert!(f.rows().iter().all(|row| row.attribution.agent != "unused"));
        assert_eq!(
            f.rows().iter().filter(|row| row.dispatched).count(),
            if refusal.is_some() { 1 } else { 2 }
        );
    }
}
