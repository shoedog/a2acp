#[path = "fallback_direct.rs"]
mod direct;
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
}
struct BoundFixture(Arc<direct::Provider>, Arc<Calls>);
#[async_trait::async_trait]
impl AgentBackend for BoundFixture {
    async fn configure_bound_session(
        &self,
        session: &SessionId,
        spec: &BoundSessionSpecV1,
    ) -> Result<()> {
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
        if matches!(
            self.0.behavior,
            "final-only" | "mixed-final" | "merged-usage"
        ) {
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
        let calls: Vec<_> = (0..3).map(|_| Arc::new(Calls::default())).collect();
        let records = calls.clone();
        let providers = f.providers.clone();
        let log = f.spawns.clone();
        let spawn: bridge_registry::registry::SpawnFn = Arc::new(move |entry| {
            let i = ["source", "target", "last"]
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
                Ok(Arc::new(BoundFixture(backend, record)) as Arc<dyn AgentBackend>)
            })
        });
        let registry =
            Arc::new(bridge_registry::registry::Registry::new(f.snapshot(), spawn).unwrap());
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
