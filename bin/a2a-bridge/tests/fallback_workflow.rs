#[path = "fallback_direct.rs"]
pub(crate) mod direct;
use bridge_core::error::BridgeError;
use bridge_core::{
    accounting::AccountingV1, domain::*, execution_policy::*, ids::*, ports::*, task_store::*,
    SessionCwd,
};
use bridge_workflow::{accounting::WorkflowAccountingInvocationV1, admission::*, executor::*};
use direct::Fixture;
use std::sync::{atomic::Ordering, Arc, Mutex};
use tokio_util::sync::CancellationToken;
type Result<T, E = BridgeError> = std::result::Result<T, E>;

#[derive(Default)]
struct Calls {
    canceled: Mutex<Vec<SessionId>>,
    cleaned: Mutex<Vec<SessionId>>,
    preserved: Mutex<Vec<CheckoutPreservationReasonV1>>,
    models: Mutex<std::collections::HashMap<SessionId, Option<String>>>,
}
struct BoundFixture(Arc<direct::Provider>, Arc<Calls>, bool, usize);
#[async_trait::async_trait]
impl AgentBackend for BoundFixture {
    fn terminal_evidence_capability(&self) -> bridge_core::terminal_evidence::EvidenceCapability {
        if self.0.behavior == "v1-final" {
            bridge_core::terminal_evidence::EvidenceCapability::V1
        } else {
            self.0.terminal_evidence_capability()
        }
    }
    async fn configure_turn(&self, session: &SessionId, meta: bridge_core::permission::TurnMeta) {
        self.0.configure_turn_liveness(session, meta.liveness).await;
    }
    async fn configure_bound_session(
        &self,
        session: &SessionId,
        spec: &BoundSessionSpecV1,
    ) -> Result<()> {
        self.1
            .models
            .lock()
            .unwrap()
            .insert(session.clone(), spec.session.config.model.clone());
        if self.0.behavior == "retry-setup" && self.0.prompts.load(Ordering::SeqCst) > 0 {
            return Err(BridgeError::ModelNotAvailable);
        }
        if self.0.behavior == "blocked-setup-error" {
            self.0.configure.0.notify_one();
            self.0.configure.1.notified().await;
            return Err(BridgeError::ModelNotAvailable);
        }
        self.0
            .configure_session(
                session,
                &SessionSpec {
                    config: spec.session.config.clone(),
                    cwd: None,
                },
            )
            .await
    }
    async fn prompt(&self, _: &SessionId, _: Vec<Part>) -> Result<BackendStream> {
        unreachable!()
    }
    async fn prompt_with_observers(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
        obs: BackendObservers,
    ) -> Result<BackendStream> {
        if self.0.behavior == "preflight-displaced" {
            self.0.prompts.fetch_add(1, Ordering::SeqCst);
            self.0.inputs.lock().unwrap().push(parts.clone());
            if self
                .1
                .models
                .lock()
                .unwrap()
                .get(session)
                .unwrap()
                .as_deref()
                == Some("alternate")
                && parts[0].text.contains("PONG")
            {
                return Ok(Box::pin(futures::stream::iter([
                    Ok(Update::FinalAnswer("PONG".into())),
                    Ok(Update::done("end_turn")),
                ])));
            }
            return Err(BridgeError::AgentFailure { diagnostic: Box::new(serde_json::from_value(serde_json::json!({
                "schema_version":1, "failed_phase":"prompt_start", "class":"quota_exhausted", "disposition":"fatal",
                "code":"fixture.rejected", "summary":"original rejection", "causes":[], "stderr_observed":false,
                "stderr_line_count":0, "prompt_may_have_been_accepted":false
            })).unwrap()) });
        }
        if matches!(
            self.0.behavior,
            "v1-final" | "empty-final" | "whitespace-final"
        ) {
            self.0.prompts.fetch_add(1, Ordering::SeqCst);
            self.0.inputs.lock().unwrap().push(parts);
            if self.0.behavior == "v1-final" {
                let mut envelope =
                    serde_json::to_value(obs.terminal_evidence.binding().unwrap()).unwrap();
                envelope.as_object_mut().unwrap().extend(serde_json::json!({
                    "version":bridge_core::terminal_evidence::TURN_EVIDENCE_VERSION, "native_turn_id":"native", "sequence":1, "producer":"completed",
                    "final_presence":"nonempty", "ordered_notifications_drained":true, "complete":true
                }).as_object().unwrap().clone());
                obs.terminal_evidence
                    .accept(serde_json::from_value(envelope).unwrap());
            }
            let mut updates = vec![];
            if self.0.behavior != "empty-final" {
                updates.push(Ok(Update::FinalAnswer(
                    if self.0.behavior == "v1-final" {
                        "answer"
                    } else {
                        "  "
                    }
                    .into(),
                )));
            }
            updates.push(Ok(Update::done("end_turn")));
            return Ok(Box::pin(futures::stream::iter(updates)));
        }
        if self.0.behavior.starts_with("retry") {
            let first = self.0.prompts.fetch_add(1, Ordering::SeqCst) == 0;
            self.0.inputs.lock().unwrap().push(parts);
            return Err(BridgeError::AgentFailure { diagnostic: Box::new(serde_json::from_value(serde_json::json!({
                "schema_version":1, "failed_phase":if first { "spawn" } else { "prompt_start" },
                "class":if first { "overloaded" } else { "quota_exhausted" },
                "disposition":if first { "retry_same_target" } else { "fatal" }, "code":if self.3 == 1 { "fixture.target" } else { "fixture.rejected" }, "summary":"original rejection",
                "causes":[], "stderr_observed":false, "stderr_line_count":0, "prompt_may_have_been_accepted":false
            })).unwrap()) });
        }
        if self.0.behavior == "lifecycle-reject" {
            use bridge_core::attempt_activity::{ActivityReason, AttemptPhase};
            self.0.prompts.fetch_add(1, Ordering::SeqCst);
            obs.activity
                .record(AttemptPhase::Adapter, ActivityReason::PhaseTransition, 1);
            return Err(BridgeError::AgentFailure {
                diagnostic: Box::new(serde_json::from_value(serde_json::json!({
                    "schema_version":1, "failed_phase":"prompt_start", "class":"quota_exhausted",
                    "disposition":"fatal", "code":"fixture.rejected", "summary":"original rejection",
                    "causes":[], "stderr_observed":false, "stderr_line_count":0,
                    "prompt_may_have_been_accepted":false
                })).unwrap()),
            });
        }
        if self.2
            || matches!(
                self.0.behavior,
                "final-only" | "mixed-final" | "merged-usage"
            )
        {
            self.0.prompts.fetch_add(1, Ordering::SeqCst);
            self.0.inputs.lock().unwrap().push(parts);
            let mut updates = vec![];
            if self.0.behavior == "mixed-final" {
                updates.push(Ok(Update::Text("prefix ".into())));
            }
            if self.0.behavior == "merged-usage" {
                updates.push(Ok(Update::Usage(bridge_core::orch::UsageSnapshot {
                    used: Some(7),
                    size: Some(100),
                    ..Default::default()
                })));
                updates.push(Ok(Update::Usage(bridge_core::orch::UsageSnapshot {
                    terminal: Some(bridge_core::orch::TerminalUsage {
                        total_tokens: 15,
                        input_tokens: 10,
                        output_tokens: 5,
                        thought_tokens: None,
                        cached_read_tokens: None,
                        cached_write_tokens: None,
                    }),
                    ..Default::default()
                })));
            }
            updates.extend([
                Ok(Update::FinalAnswer("answer".into())),
                Ok(Update::done("end_turn")),
            ]);
            return Ok(Box::pin(futures::stream::iter(updates)));
        }
        self.0.prompt_with_observers(session, parts, obs).await
    }
    async fn cancel(&self, session: &SessionId) -> Result<()> {
        self.1.canceled.lock().unwrap().push(session.clone());
        self.0.cancel(session).await
    }
    async fn release_session_checked(
        &self,
        session: &SessionId,
    ) -> Result<BackendCleanupDispositionV1, bridge_core::error::BridgeError> {
        let result = self.0.release_session_checked(session).await;
        self.1.cleaned.lock().unwrap().push(session.clone());
        result
    }
    async fn preserve_checkout_v1(
        &self,
        _: &SessionId,
        reason: CheckoutPreservationReasonV1,
    ) -> CheckoutPreservationV1 {
        self.1.preserved.lock().unwrap().push(reason);
        CheckoutPreservationV1::NoCheckoutUnderCustody
    }
}

struct CandidateFixture {
    f: Fixture,
    owner: AttemptIdentity,
    registry: Arc<dyn AgentRegistry>,
    accounting: WorkflowAccountingInvocationV1,
    run: WorkflowCandidateRunV1,
    calls: Vec<Arc<Calls>>,
}
impl CandidateFixture {
    async fn new(source: &'static str, target: &'static str) -> Self {
        let f = Fixture::new(source, target, true);
        let owner = AttemptIdentity::initial().unwrap();
        let task = TaskId::parse(owner.execution_id.as_str()).unwrap();
        f.store
            .create_with_attempt_locator(
                &TaskRecord {
                    id: task.clone(),
                    workflow: "workflow".into(),
                    status: TaskRecordStatus::Working,
                    result: None,
                    error: None,
                    created_ms: 1,
                    updated_ms: 1,
                    last_artifact_ms: None,
                    input: String::new(),
                    workflow_spec_json: None,
                    resume_attempts: 0,
                    session_cwd: None,
                    batch_id: None,
                    item_id: None,
                    artifacts_purged_at: None,
                },
                &TaskAttemptLocator {
                    identity: owner.clone(),
                    telemetry_unavailable: None,
                },
            )
            .await
            .unwrap();
        let routing = f
            .coordinator
            .freeze_direct_fallback(
                &AgentId::parse("source").unwrap(),
                &AgentOverride::default(),
                None,
            )
            .unwrap();
        let progress = routing.progress();
        let source_behavior = source;
        let source = progress.source;
        let routing = bridge_core::fallback_runtime::FrozenFallbackInvocationV1::freeze(
            progress
                .chains
                .into_iter()
                .filter(|c| c.source == source)
                .collect(),
            source,
        )
        .unwrap();
        let run = WorkflowCandidateRunV1::new(
            f.store.clone(),
            task,
            NodeId::parse("node").unwrap(),
            routing,
        );
        let accounting = WorkflowAccountingInvocationV1::new(
            AccountingV1::new(f.store.clone()),
            owner.clone(),
            WorkflowId::parse("workflow").unwrap(),
        );
        let calls: Vec<_> = (0..4).map(|_| Arc::new(Calls::default())).collect();
        let records = calls.clone();
        let providers = f.providers.clone();
        let log = f.spawns.clone();
        let spawn: bridge_registry::registry::SpawnFn = Arc::new(move |entry| {
            let i = ["source", "target", "last", "other"]
                .iter()
                .position(|id| *id == entry.id.as_str())
                .unwrap();
            log.lock().unwrap().push(entry.id.as_str().to_owned());
            let backend = providers[i].clone();
            let record = records[i].clone();
            Box::pin(async move {
                if backend.behavior == "blocked-resolve" {
                    backend.configure.0.notify_one();
                    backend.configure.1.notified().await;
                }
                Ok(Arc::new(BoundFixture(
                    backend,
                    record,
                    i == 2 && source_behavior == "retry",
                    i,
                )) as Arc<dyn AgentBackend>)
            })
        });
        let mut snapshot = f.snapshot();
        snapshot.entries.push(
            (*f.coordinator
                .registry()
                .entry_snapshot(&AgentId::parse("other").unwrap())
                .unwrap())
            .clone(),
        );
        let registry = Arc::new(bridge_registry::registry::Registry::new(snapshot, spawn).unwrap());
        Self {
            f,
            owner,
            accounting,
            run,
            registry,
            calls,
        }
    }
    fn reopen(self) -> (tempfile::TempDir, bridge_store::sqlite::SqliteStore) {
        let Self {
            f, run, accounting, ..
        } = self;
        let Fixture {
            dir,
            store,
            coordinator,
            ..
        } = f;
        let path = dir.path().join("tasks.sqlite");
        drop((run, accounting, coordinator, store));
        let store = bridge_store::sqlite::SqliteStore::open(&path).unwrap();
        (dir, store)
    }
    async fn admission(&self, agent: &str) -> AdmittedWorkflowRunV1 {
        let cwd =
            SessionCwd::parse(&self.f.dir.path().join("scope").display().to_string()).unwrap();
        self.admission_at(agent, cwd).await
    }
    async fn admission_at(&self, agent: &str, cwd: SessionCwd) -> AdmittedWorkflowRunV1 {
        WorkflowAdmissionV1::new(self.registry.clone(), Arc::new(DirectWorkflowCheckoutPlannerV1), cwd.clone(), None)
            .freeze(WorkflowAdmissionRequestV1 {
                attempt_id: self.owner.attempt_id.clone(),
                graph: Arc::new(serde_json::from_value(serde_json::json!({
                    "id":"workflow", "controls": WorkflowControlDefaultsV1::default(),
                    "nodes":[{"id":"node", "agent":agent, "prompt_template":"{{input}}", "inputs":[]}]
                })).unwrap()), requested_session_cwd: Some(cwd), policy_invocation: ExecutionPolicyInvocationV1::default(),
                ledger_admission: LedgerAdmissionV1::HistoryLedgerAdmitted { kind: HistoryAllocationKindV1::Configured }, r2f1b: None,
            }).await.unwrap()
    }
    async fn execute(
        &mut self,
        agent: &str,
        cancel: &CancellationToken,
    ) -> Result<CandidateOutputV1, bridge_core::translator::AccountingFailureV1> {
        let admitted = self.admission(agent).await;
        self.execute_admitted(&admitted, cancel).await
    }
    async fn execute_admitted(
        &mut self,
        admitted: &AdmittedWorkflowRunV1,
        cancel: &CancellationToken,
    ) -> Result<CandidateOutputV1, bridge_core::translator::AccountingFailureV1> {
        WorkflowExecutor::new(self.registry.clone())
            .run_durable_candidate_v1(
                &mut self.run,
                &self.accounting,
                admitted,
                vec![Part {
                    text: "same input".into(),
                }],
                cancel,
                BackendObservers::diagnostic_only(Arc::new(
                    bridge_core::diagnostics::NoopDiagnosticObserver::default(),
                )),
            )
            .await
    }
}

fn assert_original(error: &BridgeError, source: &str) {
    if source == "missing" {
        assert_eq!(*error, BridgeError::MissingTerminal);
        return;
    }
    let BridgeError::AgentFailure { diagnostic } = error else {
        panic!("lost source rejection: {error:?}");
    };
    let value = serde_json::to_value(diagnostic).unwrap();
    assert_eq!(value["code"], "fixture.rejected");
    assert_eq!(value["summary"], "original rejection");
    assert_eq!(
        value["class"],
        if source == "billing" {
            "billing_failure"
        } else {
            "quota_exhausted"
        }
    );
}

#[tokio::test]
async fn workflow_fallback_refusals() {
    for source in [
        "accepted",
        "text",
        "tool",
        "permission",
        "cleanup",
        "billing",
        "cleanup-effect",
        "missing",
    ] {
        let mut h = CandidateFixture::new(source, "success").await;
        let cancel = CancellationToken::new();
        let gate = h.f.providers[0].cleanup.clone();
        let drive = async {
            let failure = h.execute("source", &cancel).await.unwrap_err();
            assert_original(&failure.original, source);
            let original = failure.original.clone();
            let failure = h
                .run
                .transition(failure, &cancel, |c| Ok(c.clone()))
                .unwrap_err();
            assert_eq!(failure.original, original, "{source}");
            assert_eq!(*h.f.spawns.lock().unwrap(), ["source"]);
            assert_eq!(h.f.rows().len(), 1);
            assert_eq!(h.f.providers[1].setup.load(Ordering::SeqCst), 0);
            assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 0);
            let task = TaskId::parse(h.owner.execution_id.as_str()).unwrap();

            let (_dir, store) = h.reopen();
            let progress = store
                .candidate_progress_v1(&task, &NodeId::parse("node").unwrap())
                .await
                .unwrap()
                .unwrap();
            if matches!(source, "tool" | "cleanup-effect" | "text" | "permission") {
                assert!(!progress.visits[0].rejection.as_ref().unwrap().no_effects);
            }
        };
        let release = async {
            if source == "cleanup-effect" {
                gate.0.notified().await;
                gate.1.notify_one();
            }
        };
        tokio::join!(drive, release);
    }
}

#[tokio::test]
async fn workflow_cancel_transition_boundaries() {
    for target in [
        "blocked-resolve",
        "blocked-setup",
        "blocked-setup-error",
        "pre-poll",
        "parked",
        "rejection-cleanup",
    ] {
        let mut h = CandidateFixture::new(
            if target == "rejection-cleanup" {
                "blocked-cleanup"
            } else {
                "reject"
            },
            target,
        )
        .await;
        let cancel = CancellationToken::new();
        let source_cleanup = target == "rejection-cleanup";
        let index = usize::from(!source_cleanup);
        let gate = if source_cleanup {
            h.f.providers[0].cleanup.clone()
        } else {
            Arc::new((tokio::sync::Notify::new(), tokio::sync::Notify::new()))
        };
        let providers = h.f.providers.clone();
        let drive = async {
            let first = h.execute("source", &cancel).await.unwrap_err();
            if source_cleanup {
                assert_original(&first.original, "reject");
                assert_eq!(
                    first.preparation_exit,
                    bridge_core::translator::AccountingPreparationExitV1::Canceled
                );
                assert!(h.run.transition(first, &cancel, |c| Ok(c.clone())).is_err());
            } else {
                h.run.transition(first, &cancel, |c| Ok(c.clone())).unwrap();
                if target == "pre-poll" {
                    let gate = gate.clone();
                    h.run.before_poll = Some(Arc::new(move || {
                        let gate = gate.clone();
                        Box::pin(async move {
                            gate.0.notify_one();
                            gate.1.notified().await;
                        })
                    }));
                }
                let failure = h.execute("target", &cancel).await.unwrap_err();
                assert_eq!(
                    failure.preparation_exit,
                    bridge_core::translator::AccountingPreparationExitV1::Canceled
                );
                if target == "parked" {
                    assert_eq!(
                        failure.replay.prompt_acceptance,
                        bridge_core::routing::ExclusionEvidenceV1::Unknown,
                        "a polled silent provider may have accepted"
                    );
                }
                assert!(h
                    .run
                    .transition(failure, &cancel, |c| Ok(c.clone()))
                    .is_err());
            }
        };
        let stop = async {
            if matches!(
                target,
                "blocked-resolve" | "blocked-setup" | "blocked-setup-error"
            ) {
                providers[1].configure.0.notified().await;
            } else if target == "parked" {
                while providers[1].prompts.load(Ordering::SeqCst) == 0 {
                    tokio::task::yield_now().await;
                }
            } else {
                gate.0.notified().await;
            }
            cancel.cancel();
            if matches!(
                target,
                "blocked-resolve" | "blocked-setup" | "blocked-setup-error"
            ) {
                providers[1].configure.1.notify_one();
            } else {
                gate.1.notify_one();
            }
        };
        tokio::join!(drive, stop);
        assert_eq!(
            providers[1].prompts.load(Ordering::SeqCst),
            usize::from(target == "parked")
        );
        assert_eq!(providers[2].setup.load(Ordering::SeqCst), 0);
        assert_eq!(providers[index].cancels.load(Ordering::SeqCst), 1);
        let visit = &h.run.progress.visits[index];
        let session = SessionId::parse(visit.cold_context.as_str()).unwrap();
        assert_eq!(
            h.calls[index].canceled.lock().unwrap().as_slice(),
            std::slice::from_ref(&session)
        );
        assert_eq!(*h.calls[index].cleaned.lock().unwrap(), [session]);
        assert!(
            h.calls[index]
                .preserved
                .lock()
                .unwrap()
                .contains(&CheckoutPreservationReasonV1::Cancellation)
                || source_cleanup
        );
        let rows = h.f.rows();
        assert_eq!(rows.len(), if source_cleanup { 1 } else { 2 });
        if target == "parked" {
            assert!(
                !rows
                    .iter()
                    .find(|r| r.attribution.agent == "target")
                    .unwrap()
                    .finalized
            );
        }
        assert!(rows
            .iter()
            .filter(|r| r.attribution.agent == "target")
            .all(|r| r.dispatched == (target == "parked")));
        assert!(h
            .f
            .store
            .node_checkpoints(&TaskId::parse(h.owner.execution_id.as_str()).unwrap())
            .await
            .unwrap()
            .is_empty());
        let task = TaskId::parse(h.owner.execution_id.as_str()).unwrap();
        let progress = h.run.progress.clone();
        let (_dir, reopened) = h.reopen();
        let durable = reopened
            .candidate_progress_v1(&task, &NodeId::parse("node").unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(durable, progress);
        if source_cleanup {
            assert!(durable.visits[0].rejection.as_ref().unwrap().canceled);
        }
    }
}

async fn unpolled_case(fault: &str) {
    let mut h = CandidateFixture::new("reject", "setup").await;
    let cancel = CancellationToken::new();
    let failure = h.execute("source", &cancel).await.unwrap_err();
    assert_original(&failure.original, "reject");
    h.run
        .transition(failure, &cancel, |c| Ok(c.clone()))
        .unwrap();
    let admitted = h.admission("target").await;
    let db = rusqlite::Connection::open(h.f.dir.path().join("tasks.sqlite")).unwrap();
    if fault == "binding" {
        let mut snapshot = h.f.snapshot();
        snapshot
            .entries
            .iter_mut()
            .find(|e| e.id.as_str() == "target")
            .unwrap()
            .model = Some("drift".into());
        h.registry.apply(snapshot).await.unwrap();
    } else if fault == "publication" {
        db.execute_batch("CREATE TRIGGER fail_preparation BEFORE UPDATE ON task_candidate_progress_v1 WHEN json_array_length(NEW.progress_json,'$.visits')=2 BEGIN SELECT RAISE(ABORT,'publication fault'); END;").unwrap();
    }
    let failure = h.execute_admitted(&admitted, &cancel).await.unwrap_err();
    assert_original(&failure.original, "reject");
    assert_eq!(failure.persistence.is_some(), fault == "publication");
    assert!(h
        .run
        .transition(failure, &cancel, |c| Ok(c.clone()))
        .is_err());
    let before = h.f.rows().len();
    assert!(h.execute("last", &cancel).await.is_err());
    assert_eq!(
        h.f.rows().len(),
        before,
        "no accounting for an unauthorized successor"
    );
    assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 0);
    assert_eq!(h.f.providers[2].setup.load(Ordering::SeqCst), 0);
    if fault == "setup" {
        assert_eq!(
            *h.calls[1].preserved.lock().unwrap(),
            [CheckoutPreservationReasonV1::NodeFailure]
        );
        assert!(
            h.f.rows()
                .iter()
                .find(|r| r.attribution.agent == "target")
                .unwrap()
                .finalized
        );
    }
}

#[tokio::test]
async fn workflow_unpolled_successor_preserves_failure() {
    unpolled_case("binding").await;
}
#[tokio::test]
async fn workflow_candidate_publication_preserves_failure() {
    unpolled_case("publication").await;
}
#[tokio::test]
async fn workflow_setup_failure_closes_transition() {
    unpolled_case("setup").await;
}

async fn winner_case(target: &'static str) {
    let mut h = CandidateFixture::new("reject", target).await;
    let cancel = CancellationToken::new();
    let failure = h.execute("source", &cancel).await.unwrap_err();
    assert_original(&failure.original, "reject");
    h.run
        .transition(failure, &cancel, |c| Ok(c.clone()))
        .unwrap();
    let output = h.execute("target", &cancel).await.unwrap();
    if target == "merged-usage" {
        let usage = output.usage.as_ref().unwrap();
        assert_eq!((usage.used, usage.size), (Some(7), Some(100)));
        assert_eq!(usage.terminal.as_ref().unwrap().total_tokens, 15);
    } else {
        assert_eq!(output.usage, None);
    }
    assert_eq!(
        output.output,
        if target == "mixed-final" {
            "prefix answer"
        } else {
            "answer"
        }
    );
    let before = (
        h.f.rows().len(),
        h.f.providers[1].setup.load(Ordering::SeqCst),
        h.f.providers[1].prompts.load(Ordering::SeqCst),
    );
    assert!(h.execute("target", &cancel).await.is_err());
    assert_eq!(
        (
            h.f.rows().len(),
            h.f.providers[1].setup.load(Ordering::SeqCst),
            h.f.providers[1].prompts.load(Ordering::SeqCst)
        ),
        before
    );
    assert_eq!(before, (2, 1, 1));
    assert_eq!(
        *h.f.providers[0].inputs.lock().unwrap(),
        *h.f.providers[1].inputs.lock().unwrap()
    );
    let progress = h.run.progress.clone();
    assert_eq!(progress.visits.len(), 2);
    assert_ne!(
        progress.visits[0].cold_context,
        progress.visits[1].cold_context
    );
    assert!(progress.visits.iter().all(|v| v.owner == h.owner));
    let task = TaskId::parse(h.owner.execution_id.as_str()).unwrap();
    let (_dir, store) = h.reopen();
    assert_eq!(
        store
            .candidate_progress_v1(&task, &NodeId::parse("node").unwrap())
            .await
            .unwrap(),
        Some(progress)
    );
    let checkpoints = store.node_checkpoints(&task).await.unwrap();
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(checkpoints[0].1, output.output);
    assert_eq!(checkpoints[0].3, output.usage);
}

#[tokio::test]
async fn workflow_winner_output_and_duplicate_preparation() {
    for target in ["final-only", "mixed-final"] {
        winner_case(target).await;
    }
}
#[tokio::test]
async fn workflow_winner_context_usage_survives_completion() {
    winner_case("merged-usage").await;
}

#[tokio::test]
async fn workflow_frozen_posture_must_match_admission() {
    for drift in [false, true] {
        let mut h = CandidateFixture::new("final-only", "success").await;
        h.f.store.ensure_usage_coverage_v1().await.unwrap();
        let admitted = if drift {
            let other = h.f.dir.path().join("scope-b");
            std::fs::create_dir(&other).unwrap();
            h.admission_at(
                "source",
                SessionCwd::parse(other.to_str().unwrap()).unwrap(),
            )
            .await
        } else {
            h.admission("source").await
        };
        let result = h
            .execute_admitted(&admitted, &CancellationToken::new())
            .await;
        if drift {
            assert!(matches!(
                result.unwrap_err().original,
                BridgeError::ConfigMismatch {
                    field: "fallback_candidate"
                }
            ));
            assert!(
                h.f.rows().is_empty(),
                "posture mismatch must precede preparation"
            );
            assert!(h.f.spawns.lock().unwrap().is_empty());
            assert_eq!(h.f.providers[0].setup.load(Ordering::SeqCst), 0);
            assert_eq!(h.f.providers[0].prompts.load(Ordering::SeqCst), 0);
        } else {
            assert_eq!(result.unwrap().output, "answer");
            assert_eq!(h.f.providers[0].prompts.load(Ordering::SeqCst), 1);
        }
    }
}

#[tokio::test]
async fn workflow_lifecycle_only_rejection_allows_successor() {
    let mut h = CandidateFixture::new("lifecycle-reject", "final-only").await;
    let cancel = CancellationToken::new();
    let failure = h.execute("source", &cancel).await.unwrap_err();
    assert_eq!(
        failure.replay.effects,
        bridge_core::routing::ExclusionEvidenceV1::Excluded,
        "lifecycle activity is not a replay effect"
    );
    assert!(
        h.run.progress.visits[0]
            .rejection
            .as_ref()
            .unwrap()
            .no_effects
    );
    h.run
        .transition(failure, &cancel, |c| Ok(c.clone()))
        .unwrap();
    assert_eq!(h.execute("target", &cancel).await.unwrap().output, "answer");
    assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 1);
}

#[allow(dead_code)] // Also compiled as a standalone integration fixture.
pub(crate) async fn local_activation<F, Fut>(run: F)
where
    F: Fn(Vec<String>, Arc<dyn AgentRegistry>, Arc<bridge_store::sqlite::SqliteStore>) -> Fut,
    Fut: std::future::Future<Output = std::result::Result<(), String>>,
{
    for (source, target, governed) in [
        ("reject", "merged-usage", false),
        ("region", "merged-usage", false),
        ("billing-match", "merged-usage", false),
        ("rate", "merged-usage", false),
        ("retry", "exhaust", false),
        ("retry-exhaust", "exhaust", false),
        ("reject", "setup", false),
        ("reject", "merged-usage", true),
        ("retry", "retry", false),
        ("reject", "retry-setup", false),
        ("v1-final", "merged-usage", false),
        ("empty-final", "merged-usage", false),
        ("whitespace-final", "merged-usage", false),
        ("missing", "merged-usage", false),
        ("local-cancel", "merged-usage", false),
        ("reject", "local-cancel", false),
    ] {
        let h = CandidateFixture::new(source, target).await;
        let root = h.f.dir.path();
        let scope = root.join("scope");
        let mut cfg = format!("default='source'\n[store]\npath={:?}\n[registry]\nallowed_cmds=['test-cmd']\n[server]\n", root.join("tasks.sqlite"));
        for agent in ["source", "target", "last", "other"] {
            cfg += &format!("[[agents]]\nid='{agent}'\ncmd='test-cmd'\nmodel='{agent}'\ncwd={scope:?}\nsession_cwd={scope:?}\n[[billing_identities]]\nagent='{agent}'\nmodel='{agent}'\naccount_group='{agent}'\nbilling_mode='metered'\n");
        }
        cfg += "[[routing.chains]]\nid='original'\nsource={agent='source',model='source'}\ncauses=['quota_exhausted','region_restricted','billing_failure','rate_limited']\nsteps=[{agent='target',model='target'},{agent='last',model='last'}]\n[[routing.chains]]\nid='unused'\nsource={agent='target',model='target'}\ncauses=['quota_exhausted']\nsteps=[{agent='other',model='other'}]\n[[workflows]]\nid='workflow'\n[[workflows.nodes]]\nid='node'\nagent='source'\nprompt_text='{{input}}'\ninputs=[]\nretry={max_attempts=2,backoff_ms=0}\n";
        let downstream = source == "reject" && target == "merged-usage" && !governed;
        if downstream {
            cfg += "[[workflows.nodes]]\nid='downstream'\nagent='target'\nprompt_text='{{node}}'\ninputs=['node']\n";
        }
        if governed {
            cfg += "[[governance.rules]]\nid='headroom'\nselector='each_request'\nlimits={turns=100}\n";
        }
        std::fs::write(root.join("config.toml"), cfg).unwrap();
        std::fs::write(
            root.join("input"),
            "---\ntask-type: freeform\n---\nidentical input",
        )
        .unwrap();
        let args = vec![
            "workflow".into(),
            "--config".into(),
            root.join("config.toml").display().to_string(),
            "--input".into(),
            root.join("input").display().to_string(),
            "--out".into(),
            root.join("output").display().to_string(),
            "--no-heartbeat".into(),
        ];
        let mut args = args;
        args.extend([
            "--silence-cutoff".into(),
            if source == "region" { "5" } else { "off" }.into(),
        ]);
        let result = run(args, h.registry.clone(), h.f.store.clone()).await;
        let source_only = governed
            || matches!(
                source,
                "v1-final" | "empty-final" | "whitespace-final" | "missing" | "local-cancel"
            );
        let expected_ok = !governed
            && !matches!(
                source,
                "retry-exhaust" | "empty-final" | "whitespace-final" | "missing" | "local-cancel"
            )
            && !matches!(target, "setup" | "retry" | "retry-setup" | "local-cancel");
        assert_eq!(result.is_ok(), expected_ok, "{source}/{target}: {result:?}");
        let rows = h.f.rows();
        let expected = if source_only {
            vec!["source"]
        } else if downstream {
            vec!["source", "target", "target"]
        } else if target == "retry" {
            vec!["source", "source", "target"]
        } else if target == "retry-setup" {
            vec!["source", "target", "target"]
        } else if source.starts_with("retry") {
            vec!["source", "source", "target", "last"]
        } else {
            vec!["source", "target"]
        };
        assert_eq!(rows.len(), expected.len(), "{source}");
        let mut rows = rows;
        rows.sort_by_key(|row| row.started_ms);
        if governed {
            let task = TaskId::parse(rows[0].attribution.execution.clone().unwrap()).unwrap();
            assert!(
                h.f.store.get(&task).await.unwrap().is_none(),
                "governed runs must not admit candidate tasks"
            );
        }
        assert_eq!(
            rows.iter()
                .map(|r| r.attribution.agent.as_str())
                .collect::<Vec<_>>(),
            expected
        );
        assert!(rows.iter().all(|r| matches!(
            r.attribution.node.as_deref(),
            Some("node" | "downstream")
        ) && r.attribution.billing.as_ref().unwrap().account_group
            == r.attribution.agent));
        assert_eq!(
            rows.iter().filter(|r| !r.dispatched).count(),
            usize::from(matches!(target, "setup" | "retry-setup"))
        );
        if !governed && source != "missing" {
            assert!(rows.iter().all(|r| r.finalized));
        }
        assert!(
            h.f.providers[2].prompts.load(Ordering::SeqCst)
                == usize::from(source.starts_with("retry") && target != "retry")
        );
        if !source_only && target != "setup" {
            assert_eq!(
                h.f.providers[0].inputs.lock().unwrap()[0],
                h.f.providers[1].inputs.lock().unwrap()[0]
            );
        }
        if downstream {
            assert_eq!(h.f.providers[1].inputs.lock().unwrap()[1][0].text, "answer");
        }
        assert_eq!(
            h.f.providers[3].setup.load(Ordering::SeqCst),
            0,
            "B's own chain must remain unused"
        );
        let source_liveness = h.f.providers[0].liveness.lock().unwrap().clone();
        assert!(source_liveness[0].observer_key.is_some());
        if !source_only && target != "setup" {
            assert_eq!(
                source_liveness[0].silence,
                h.f.providers[1].liveness.lock().unwrap()[0].silence
            );
        }
        if target == "retry-setup" {
            assert!(std::fs::read_to_string(root.join("output"))
                .unwrap()
                .contains("fixture.target"));
            assert_eq!(rows.iter().filter(|r| !r.dispatched).count(), 1);
        }
        if source == "local-cancel" || target == "local-cancel" {
            let task = TaskId::parse(rows[0].attribution.execution.clone().unwrap()).unwrap();
            assert_eq!(
                h.f.store.get(&task).await.unwrap().unwrap().status,
                TaskRecordStatus::Canceled
            );
        }
        if expected_ok {
            assert_eq!(
                std::fs::read_to_string(root.join("output")).unwrap(),
                "answer"
            );
            assert!(rows[0].usage.terminal.is_none());
            let task = TaskId::parse(rows[0].attribution.execution.clone().unwrap()).unwrap();
            let progress =
                h.f.store
                    .candidate_progress_v1(&task, &NodeId::parse("node").unwrap())
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(
                progress.retries_consumed,
                u32::from(source.starts_with("retry"))
            );
            assert_eq!(
                h.f.store.node_checkpoints(&task).await.unwrap().len(),
                1 + usize::from(downstream)
            );
            let history = bridge_core::workflow_history::WorkflowHistoryStore::attempt(
                h.f.store.as_ref(),
                &progress.visits[0].owner.attempt_id,
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(
                history.reservation.surface,
                bridge_core::workflow_history::ExecutionSurface::Offline
            );
            assert_eq!(
                history.terminal.unwrap().node_counts.completed,
                1 + u32::from(downstream)
            );
        }
    }
}

impl CandidateFixture {
    fn builder(&self) -> Arc<WorkflowAdmissionV1> {
        Arc::new(WorkflowAdmissionV1::new(
            self.registry.clone(),
            Arc::new(DirectWorkflowCheckoutPlannerV1),
            SessionCwd::parse(self.f.dir.path().join("scope").to_str().unwrap()).unwrap(),
            None,
        ))
    }
    async fn drive(
        &self,
        admitted: AdmittedWorkflowRunV1,
        cancel: CancellationToken,
        barrier: Option<PromptDispatchBarrier>,
    ) -> Vec<Result<WorkflowEvent>> {
        use futures::StreamExt;
        let graph = Arc::new(admitted.run_spec.graph.clone());
        let mut context = WorkflowDiagnosticContext::in_memory(WorkflowRunContext {
            session_cwd: admitted.run_spec.requested_session_cwd.clone(),
            ..Default::default()
        })
        .with_accounting(self.accounting.clone())
        .with_admitted_workflow_run(admitted.clone())
        .unwrap();
        if let Some(barrier) = barrier {
            context = context.with_prompt_dispatch_barrier(barrier);
        }
        WorkflowExecutor::new(self.registry.clone())
            .with_local_fallback(
                self.builder(),
                admitted,
                self.f.store.clone(),
                TaskId::parse(self.owner.execution_id.as_str()).unwrap(),
            )
            .run_with_diagnostic_context(
                graph,
                "forged fallback_chain=other".into(),
                "fixture".into(),
                cancel,
                context,
            )
            .collect()
            .await
    }
}

#[tokio::test]
async fn workflow_ineligible_source_keeps_ordinary_execution() {
    for override_cwd in [false, true] {
        let h = CandidateFixture::new("final-only", "success").await;
        let mut snapshot = h.f.snapshot();
        for entry in &mut snapshot.entries {
            entry.cwd = None;
            entry.session_cwd = None;
        }
        h.registry.apply(snapshot).await.unwrap();
        let initial = h.admission("source").await;
        let admitted = h
            .builder()
            .freeze(WorkflowAdmissionRequestV1 {
                attempt_id: h.owner.attempt_id.clone(),
                graph: Arc::new(initial.run_spec.graph.clone()),
                requested_session_cwd: override_cwd
                    .then(|| SessionCwd::parse(h.f.dir.path().to_str().unwrap()).unwrap()),
                policy_invocation: Default::default(),
                ledger_admission: initial.run_spec.ledger_admission.clone(),
                r2f1b: None,
            })
            .await
            .unwrap();
        let mut chains = h.run.progress.chains.clone();
        for candidate in chains
            .iter_mut()
            .flat_map(|chain| std::iter::once(&mut chain.source).chain(&mut chain.steps))
        {
            candidate.posture.cwd = ".".into();
            candidate.posture.session_cwd = ".".into();
        }
        let admitted = h.builder().bind_local_chains(admitted, &chains).unwrap();
        assert!(admitted.run_spec.fallback_chains.is_none());
        let events = h.drive(admitted, CancellationToken::new(), None).await;
        assert!(events.iter().any(|event| matches!(event, Ok(WorkflowEvent::Terminal { outcome: WorkflowOutcome::Completed, output }) if output == "answer")));
        assert_eq!(h.f.providers[0].prompts.load(Ordering::SeqCst), 1);
        assert_eq!(h.f.providers[1].setup.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn workflow_authority_scope_and_barrier_refusals() {
    for fault in ["config", "scope", "cancel", "barrier"] {
        let h = CandidateFixture::new(
            if fault == "barrier" {
                "reject"
            } else {
                "blocked-cleanup"
            },
            "success",
        )
        .await;
        let admitted = h
            .builder()
            .bind_local_chains(h.admission("source").await, &h.run.progress.chains)
            .unwrap();
        let cancel = CancellationToken::new();
        let mut drift = h.f.snapshot();
        drift.entries[1].args.push("changed".into());
        let registry = h.registry.clone();
        let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let barrier = (fault == "barrier").then(|| {
            Arc::new(move || {
                let (registry, drift, counter) = (registry.clone(), drift.clone(), counter.clone());
                Box::pin(async move {
                    if counter.fetch_add(1, Ordering::SeqCst) == 1 {
                        registry.apply(drift).await.unwrap();
                    }
                })
                    as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
            }) as PromptDispatchBarrier
        });
        let drive = h.drive(admitted, cancel.clone(), barrier);
        let mutate = async {
            if fault == "barrier" {
                return;
            }
            h.f.providers[0].cleanup.0.notified().await;
            match fault {
                "config" => {
                    let mut snapshot = h.f.snapshot();
                    snapshot.entries[1].args.push("drift".into());
                    h.registry.apply(snapshot).await.unwrap();
                }
                "scope" => {
                    let scope = h.f.dir.path().join("scope");
                    std::fs::rename(&scope, h.f.dir.path().join("old-scope")).unwrap();
                    std::fs::create_dir(scope).unwrap();
                }
                "cancel" => cancel.cancel(),
                _ => unreachable!(),
            }
            h.f.providers[0].cleanup.1.notify_one();
        };
        let (events, _) = tokio::join!(drive, mutate);
        assert_eq!(
            h.f.providers[1].prompts.load(Ordering::SeqCst),
            0,
            "{fault}"
        );
        assert_eq!(
            h.f.providers[1].setup.load(Ordering::SeqCst),
            usize::from(fault == "barrier")
        );
        assert_eq!(h.f.providers[2].setup.load(Ordering::SeqCst), 0);
        assert_eq!(h.f.rows().len(), 1 + usize::from(fault == "barrier"));
        if fault == "barrier" {
            assert!(
                !h.f.rows()
                    .iter()
                    .find(|r| r.attribution.agent == "target")
                    .unwrap()
                    .dispatched
            );
        }
        assert!(
            events.iter().any(|event| matches!(
                event,
                Ok(WorkflowEvent::Terminal {
                    outcome: WorkflowOutcome::Canceled,
                    ..
                })
            )) == (fault == "cancel")
        );
        if fault != "cancel" {
            assert!(format!("{events:?}").contains("fixture.rejected"));
        }
    }
}

#[tokio::test]
async fn workflow_recovery_consumers_refuse_before_executor_effects() {
    use bridge_coordinator::{batch::*, detached::*};
    for batch in [false, true] {
        let h = CandidateFixture::new("reject", "success").await;
        let admitted = h
            .builder()
            .bind_local_chains(h.admission("source").await, &h.run.progress.chains)
            .unwrap();
        let task = TaskId::parse(h.owner.execution_id.as_str()).unwrap();
        let bid = BatchId::parse("batch-fixture").unwrap();
        if batch {
            h.f.store
                .create_batch(&BatchRecord {
                    id: bid.clone(),
                    workflow: "workflow".into(),
                    concurrency: 1,
                    total: 1,
                    status: BatchStatus::Working,
                    items_json: "{\"v\":1,\"items\":[]}".into(),
                    error: None,
                    created_ms: 1,
                    updated_ms: 1,
                })
                .await
                .unwrap();
        }
        rusqlite::Connection::open(h.f.dir.path().join("tasks.sqlite"))
            .unwrap()
            .execute(
                "UPDATE tasks SET workflow_spec_json=?1,batch_id=?2,item_id=?3 WHERE id=?4",
                rusqlite::params![
                    String::from_utf8(admitted.run_spec.encode_snapshot_v2().unwrap()).unwrap(),
                    batch.then(|| bid.as_str()),
                    batch.then_some("one"),
                    task.as_str()
                ],
            )
            .unwrap();
        let record = h.f.store.get(&task).await.unwrap().unwrap();
        let deps = DetachedDeps {
            task_store: h.f.store.clone(),
            executor: Some(Arc::new(WorkflowExecutor::new(h.registry.clone()))),
            workflows: Arc::new(std::collections::HashMap::from([(
                admitted.run_spec.graph.id.clone(),
                Arc::new(admitted.run_spec.graph.clone()),
            )])),
            workflow_cancels: Default::default(),
            progress_hubs: Default::default(),
            clock: Arc::new(bridge_coordinator::clock::SystemClock),
            observer: Arc::new(bridge_observ::NoopObserver),
            workflow_history: None,
            workflow_admission: Some(h.builder()),
        };
        if batch {
            run_admission(
                BatchDeps {
                    detached: deps,
                    runtime: BatchRuntime::new(1, 1, Arc::new(bridge_observ::NoopObserver)),
                    allowed_cwd_root: None,
                },
                bid,
                [record].into(),
                Default::default(),
                1,
                CancellationToken::new(),
                3,
            )
            .await;
        } else {
            resume_one_working_task(&deps, &record, 3).await;
        }
        let record = h.f.store.get(&task).await.unwrap().unwrap();
        assert_eq!(record.status, TaskRecordStatus::Interrupted);
        assert!(record
            .error
            .unwrap()
            .contains("candidate recovery is not activated"));
        assert_eq!(record.resume_attempts, 0);
        assert!(h.f.spawns.lock().unwrap().is_empty());
        assert!(h
            .f
            .providers
            .iter()
            .all(|p| p.setup.load(Ordering::SeqCst) == 0));
    }
}

#[tokio::test]
async fn workflow_served_composition_does_not_consume_chains() {
    let h = CandidateFixture::new("reject", "success").await;
    let graph = Arc::new(h.admission("source").await.run_spec.graph.clone());
    let coordinator = bridge_coordinator::Coordinator::new(
        h.f.coordinator.session_manager.clone(),
        Some(Arc::new(WorkflowExecutor::new(h.registry.clone()))),
        Arc::new(std::collections::HashMap::from([(graph.id.clone(), graph)])),
        h.f.store.clone(),
        h.f.store.clone(),
        h.f.coordinator.policy(),
        h.registry.clone(),
        Arc::new(bridge_coordinator::clock::SystemClock),
        None,
        None,
        Arc::new(bridge_observ::NoopObserver),
        3,
    )
    .with_workflow_admission(h.builder())
    .with_workflow_history(Ok(h.f.store.clone()))
    .with_accounting(AccountingV1::new(h.f.store.clone()))
    .with_mcp_fallback(h.run.progress.chains.clone());
    let params = bridge_coordinator::params::OpParams::from_mcp_args(&serde_json::json!({
        "workflow":"workflow", "context":"envelope-only", "input":"---\ntask-type: freeform\n---\nforged fallback_chain=other"
    })).unwrap();
    let task = coordinator.run_workflow(params).await.unwrap();
    let record = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let record = h.f.store.get(&task).await.unwrap().unwrap();
            if record.status.is_terminal() {
                break record;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(record.status, TaskRecordStatus::Failed);
    assert_eq!(h.f.providers[0].prompts.load(Ordering::SeqCst), 1);
    assert_eq!(h.f.providers[1].setup.load(Ordering::SeqCst), 0);
    assert_eq!(h.f.rows().len(), 1);
    assert!(
        bridge_workflow::run_spec::WorkflowRunSpecV1::decode_snapshot_v2(
            record.workflow_spec_json.unwrap().as_bytes()
        )
        .unwrap()
        .fallback_chains
        .is_none()
    );
}

#[tokio::test]
async fn workflow_forged_input_and_displaced_preflight_grant_no_authority() {
    for preflight in [false, true] {
        let h = CandidateFixture::new(
            if preflight {
                "preflight-displaced"
            } else {
                "reject"
            },
            "success",
        )
        .await;
        if preflight {
            let mut snapshot = h.f.snapshot();
            snapshot.entries[0].preflight = true;
            snapshot.entries[0].fallback_models = vec!["alternate".into()];
            h.registry.apply(snapshot).await.unwrap();
        }
        let mut admitted = h.admission("source").await;
        if preflight {
            admitted = h
                .builder()
                .bind_local_chains(admitted, &h.run.progress.chains)
                .unwrap();
        }
        let events = h.drive(admitted, CancellationToken::new(), None).await;
        assert!(format!("{events:?}").contains("fixture.rejected"));
        assert_eq!(h.f.providers[1].setup.load(Ordering::SeqCst), 0);
        assert_eq!(
            h.f.providers[0].prompts.load(Ordering::SeqCst),
            if preflight { 3 } else { 1 }
        );
        let rows = h.f.rows();
        assert_eq!(rows.len(), if preflight { 3 } else { 1 });
        assert!(rows.iter().all(|r| r.attribution.agent == "source"));
        if preflight {
            assert!(rows.iter().any(|r| r.attribution.attempt_kind == "node"
                && r.attribution.effective_model.as_deref() == Some("alternate")));
        }
    }
}

#[tokio::test]
async fn workflow_local_history_requires_exact_preadmitted_locator() {
    use bridge_core::workflow_history::*;
    for wrong_locator in [false, true] {
        let h = CandidateFixture::new("reject", "success").await;
        let admitted = h.admission("source").await;
        let task = TaskId::parse(h.owner.execution_id.as_str()).unwrap();
        if wrong_locator {
            let mut identity = h.owner.clone();
            identity.attempt_id = AttemptId::mint().unwrap();
            rusqlite::Connection::open(h.f.dir.path().join("tasks.sqlite"))
                .unwrap()
                .execute(
                    "UPDATE task_attempt_locators SET locator_json=?1 WHERE task_id=?2",
                    rusqlite::params![
                        serde_json::to_string(&TaskAttemptLocator {
                            identity,
                            telemetry_unavailable: None
                        })
                        .unwrap(),
                        task.as_str()
                    ],
                )
                .unwrap();
        }
        let reservation = AttemptReservationV2 {
            schema_version: WORKFLOW_HISTORY_EVIDENCE_SCHEMA_V1,
            reservation: AttemptReservation {
                identity: h.owner.clone(),
                task_id: Some(task),
                workflow: "workflow".into(),
                task_class: "workflow".into(),
                surface: ExecutionSurface::Offline,
                policy: "r2f1a".into(),
                workload_fingerprint: admitted.run_spec.workload_fingerprint.clone(),
                started_ms: 1,
                workload_fingerprint_complete: true,
                prompt_acceptance: "not_dispatched".into(),
                pinned: false,
            },
            controls_json: String::from_utf8(
                admitted.run_spec.controls.encode_canonical().unwrap(),
            )
            .unwrap(),
            controls_fingerprint: admitted.run_spec.controls_fingerprint.clone(),
            expected_node_count: 1,
            nodes: vec![HistoryNodeReservationV1 {
                node: NodeId::parse("node").unwrap(),
                sorted_ordinal: 0,
            }],
        };
        let result = h.f.store.reserve_v2(&reservation).await;
        assert_eq!(result.is_err(), wrong_locator, "{result:?}");
        if !wrong_locator {
            assert_eq!(
                h.f.store
                    .attempt(&h.owner.attempt_id)
                    .await
                    .unwrap()
                    .unwrap()
                    .reservation
                    .surface,
                ExecutionSurface::Offline
            );
        }
        assert!(h.f.spawns.lock().unwrap().is_empty());
    }
}
