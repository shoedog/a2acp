#[path = "fallback_direct.rs"]
mod direct;
use bridge_core::error::BridgeError;
use bridge_core::{
    accounting::AccountingV1, domain::*, execution_policy::*, ids::*, ports::*, task_store::*,
    SessionCwd,
};
use bridge_workflow::{accounting::WorkflowAccountingInvocationV1, admission::*, executor::*};
use direct::Fixture;
use std::sync::{atomic::Ordering, Arc};
use tokio_util::sync::CancellationToken;
type Result<T, E = BridgeError> = std::result::Result<T, E>;

struct BoundFixture(Arc<direct::Provider>);
#[async_trait::async_trait]
impl AgentBackend for BoundFixture {
    async fn configure_bound_session(
        &self,
        session: &SessionId,
        spec: &BoundSessionSpecV1,
    ) -> Result<()> {
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
        self.0.prompt_with_observers(session, parts, obs).await
    }
    async fn cancel(&self, session: &SessionId) -> Result<()> {
        self.0.cancel(session).await
    }
    async fn release_session_checked(
        &self,
        session: &SessionId,
    ) -> Result<BackendCleanupDispositionV1, bridge_core::error::BridgeError> {
        self.0.release_session_checked(session).await
    }
}

struct CandidateFixture {
    f: Fixture,
    owner: AttemptIdentity,
    registry: Arc<dyn AgentRegistry>,
    accounting: WorkflowAccountingInvocationV1,
    run: WorkflowCandidateRunV1,
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
        let providers = f.providers.clone();
        let log = f.spawns.clone();
        let spawn: bridge_registry::registry::SpawnFn = Arc::new(move |entry| {
            let i = ["source", "target", "last"]
                .iter()
                .position(|id| *id == entry.id.as_str())
                .unwrap();
            log.lock().unwrap().push(entry.id.as_str().to_owned());
            let backend = providers[i].clone();
            Box::pin(async move {
                if backend.behavior == "blocked-resolve" {
                    backend.configure.0.notify_one();
                    backend.configure.1.notified().await;
                }
                Ok(Arc::new(BoundFixture(backend)) as Arc<dyn AgentBackend>)
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
        }
    }
    async fn admission(&self, agent: &str) -> AdmittedWorkflowRunV1 {
        let cwd =
            SessionCwd::parse(&self.f.dir.path().join("scope").display().to_string()).unwrap();
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
        WorkflowExecutor::new(self.registry.clone())
            .run_durable_candidate_v1(
                &mut self.run,
                &self.accounting,
                &admitted,
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
            let original = failure.original.clone();
            let failure = h
                .run
                .transition(failure, &cancel, |c| Ok(c.clone()))
                .unwrap_err();
            assert_eq!(failure.original, original, "{source}");
            assert_eq!(*h.f.spawns.lock().unwrap(), ["source"]);
            assert_eq!(h.f.rows().len(), 1);
            assert_eq!(h.f.providers[1].setup.load(Ordering::SeqCst), 0);
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
                assert!(h
                    .run
                    .transition(failure, &cancel, |c| Ok(c.clone()))
                    .is_err());
            }
        };
        let stop = async {
            if matches!(target, "blocked-resolve" | "blocked-setup") {
                providers[1].configure.0.notified().await;
            } else if target == "parked" {
                while providers[1].prompts.load(Ordering::SeqCst) == 0 {
                    tokio::task::yield_now().await;
                }
            } else {
                gate.0.notified().await;
            }
            cancel.cancel();
            if matches!(target, "blocked-resolve" | "blocked-setup") {
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
        let rows = h.f.rows();
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
    }
}
