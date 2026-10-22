//! Executor activation, with independent provider observations and durable reopen.
mod common;
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
        let run = WorkflowExecutor::new(registry.clone())
            .run_with_diagnostic_context(
                single_graph(false),
                "BRIEF".into(),
                "reload".into(),
                CancellationToken::new(),
                context(&f, &owner),
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
