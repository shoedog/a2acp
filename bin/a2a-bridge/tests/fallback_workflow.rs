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
    resolution_cancel: Mutex<Option<CancellationToken>>,
    entry_check: Mutex<Option<Box<dyn Fn() + Send>>>,
    canceled: Mutex<Vec<SessionId>>,
    setups: std::sync::atomic::AtomicUsize,
    forgotten: Mutex<Vec<SessionId>>,
    cleaned: Mutex<Vec<SessionId>>,
    preserved: Mutex<Vec<CheckoutPreservationReasonV1>>,
    turns: Mutex<std::collections::HashMap<SessionId, bridge_core::permission::TurnMeta>>,
    transfers: Mutex<Vec<SessionId>>,
    flight: Mutex<Option<Arc<bridge_core::retained_resource_flight::RetainedResourceFlight>>>,
    models: Mutex<std::collections::HashMap<SessionId, Option<String>>>,
}
struct BoundFixture(Arc<direct::Provider>, Arc<Calls>, bool, usize);
#[async_trait::async_trait]
impl AgentBackend for BoundFixture {
    fn terminal_evidence_capability(&self) -> bridge_core::terminal_evidence::EvidenceCapability {
        if matches!(self.0.behavior, "v1-final" | "v1-missing") {
            bridge_core::terminal_evidence::EvidenceCapability::V1
        } else {
            self.0.terminal_evidence_capability()
        }
    }
    fn prefix_attestation_capability(
        &self,
    ) -> bridge_core::attestation::PrefixAttestationCapability {
        if self.0.behavior.starts_with("attested") {
            bridge_core::attestation::PrefixAttestationCapability::codex_commit_marker_v1()
        } else {
            Default::default()
        }
    }
    async fn configure_turn(&self, session: &SessionId, meta: bridge_core::permission::TurnMeta) {
        self.0
            .configure_turn_liveness(session, meta.liveness.clone())
            .await;
        self.1.turns.lock().unwrap().insert(session.clone(), meta);
    }
    async fn configure_bound_session(
        &self,
        session: &SessionId,
        spec: &BoundSessionSpecV1,
    ) -> Result<()> {
        if let Some(check) = self.1.entry_check.lock().unwrap().take() {
            check();
        }
        if self.0.behavior == "blocked-entry" {
            self.0.configure.0.notify_one();
            self.0.configure.1.notified().await;
        }
        let setup = self.1.setups.fetch_add(1, Ordering::SeqCst);
        if self.0.behavior == "unpolled-timeout"
            || self.0.behavior == "retry-unpolled" && setup == 1
        {
            return Err(BridgeError::AgentTimedOut);
        }
        if self.0.behavior == "deadline" {
            use bridge_core::resource_flight::*;
            let attempt = AttemptIdentity::initial().unwrap().attempt_id;
            let reservation = ResourceFlightRegistryV1::new(attempt.clone())
                .reserve(RetainedResourceFlightConfigV1 {
                    flight_id: ResourceFlightIdV1::mint().unwrap(),
                    attempt_id: attempt,
                    key: ResourceFlightKeyV1::AcpGeneration {
                        generation: session.as_str().into(),
                    },
                    journal: Arc::new(InMemoryResourceFlightJournal::new(16)),
                    clock: Arc::new(TestClock(tokio::time::Instant::now())),
                    result_publisher: Arc::new(NoopResourceFlightResultPublisher),
                })
                .unwrap();
            let flight = reservation.flight().unwrap().clone();
            let owner =
                ResourceFlightOwnerV1::new(NodeId::parse("node").unwrap(), session.as_str())
                    .unwrap();
            flight.attach_owner(owner.clone()).unwrap();
            flight.close_admission().unwrap();
            flight
                .journal_intent(ResourceActionIntentV1 {
                    initiator: owner,
                    capability_digest: Sha256HexV1::parse("1".repeat(64)).unwrap(),
                    cause: None,
                })
                .unwrap();
            flight.begin_journaled_dispatch().unwrap();
            *self.1.flight.lock().unwrap() = Some(flight);
        }
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
        if self.0.behavior == "preflight-pass" && parts[0].text.contains("PONG") {
            self.0.prompts.fetch_add(1, Ordering::SeqCst);
            self.0.inputs.lock().unwrap().push(parts);
            return Ok(Box::pin(futures::stream::iter([
                Ok(Update::FinalAnswer("PONG".into())),
                Ok(Update::done("end_turn")),
            ])));
        }
        if self.0.behavior.starts_with("parity-") {
            use futures::StreamExt;
            self.0.prompts.fetch_add(1, Ordering::SeqCst);
            self.0.inputs.lock().unwrap().push(parts);
            obs.activity.record(
                bridge_core::attempt_activity::AttemptPhase::Adapter,
                bridge_core::attempt_activity::ActivityReason::PhaseTransition,
                1,
            );
            if let Some(sink) = &obs.rich {
                sink.record(bridge_core::orch::OrchEventKind::Progress {
                    progress: bridge_core::orch::ProgressPayload::legacy("fixture observation"),
                });
            }
            let usage = Ok(Update::Usage(bridge_core::orch::UsageSnapshot {
                used: Some(7),
                size: Some(100),
                ..Default::default()
            }));
            if self.0.behavior.contains("open-cancel") {
                self.0.configure.0.notify_one();
                return std::future::pending().await;
            }
            if self.0.behavior.contains("cancel") {
                let gate = self.0.clone();
                return Ok(Box::pin(futures::stream::iter([usage]).chain(
                    futures::stream::once(async move {
                        gate.configure.0.notify_one();
                        std::future::pending().await
                    }),
                )));
            }
            let tail = if self.0.behavior == "parity-error" {
                vec![Err(BridgeError::PermissionDenied)]
            } else {
                vec![
                    Ok(Update::FinalAnswer("answer".into())),
                    Ok(Update::done("end_turn")),
                ]
            };
            return Ok(Box::pin(
                futures::stream::iter([usage]).chain(futures::stream::iter(tail)),
            ));
        }
        if matches!(
            self.0.behavior,
            "missing" | "native-missing" | "attested" | "attested-invalid" | "unsupported-prefix"
        ) {
            use bridge_core::attestation::*;
            use ring::digest::{digest, SHA256};
            let first = self.0.prompts.fetch_add(1, Ordering::SeqCst) == 0;
            self.0.inputs.lock().unwrap().push(parts);
            if self.0.behavior == "native-missing" {
                return Ok(Box::pin(futures::stream::iter([Err(
                    BridgeError::MissingTerminal,
                )])));
            }
            if self.0.behavior == "missing" && first {
                return Ok(Box::pin(futures::stream::empty()));
            }
            let body = if self.0.behavior == "missing" {
                "answer"
            } else {
                "process\nanswer"
            };
            let meta = self.1.turns.lock().unwrap().get(session).unwrap().clone();
            let prefix_attestation = if self.0.behavior.starts_with("attested") {
                PrefixAttestationStatus::AttestedV1(AttestedPrefixV1 {
                    issuer_id: ATTESTED_PREFIX_ISSUER_V1.into(),
                    producer_id: ["source", "target", "last", "other"][self.3].into(),
                    turn_id: if self.0.behavior == "attested-invalid" {
                        "wrong-turn".into()
                    } else {
                        meta.turn_id.as_str().into()
                    },
                    body_len_bytes: body.len() as u64,
                    body_sha256: digest(&SHA256, body.as_bytes())
                        .as_ref()
                        .try_into()
                        .unwrap(),
                    process_prefix_bytes: "process\n".len() as u64,
                })
            } else {
                Default::default()
            };
            return Ok(Box::pin(futures::stream::iter([
                Ok(Update::FinalAnswer(body.into())),
                Ok(Update::Done {
                    stop_reason: "end_turn".into(),
                    prefix_attestation,
                }),
            ])));
        }
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
            "v1-final" | "v1-missing" | "empty-final" | "whitespace-final"
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
            let mut updates = vec![Ok(Update::Usage(bridge_core::orch::UsageSnapshot {
                used: Some(7),
                size: Some(100),
                ..Default::default()
            }))];
            if self.0.behavior != "empty-final" {
                updates.push(Ok(Update::FinalAnswer(
                    if matches!(self.0.behavior, "v1-final" | "v1-missing") {
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
        if matches!(self.0.behavior, "retry-unpolled" | "retry-unpolled-resolve") {
            let first = self.0.prompts.fetch_add(1, Ordering::SeqCst) == 0;
            self.0.inputs.lock().unwrap().push(parts);
            return if first {
                Err(BridgeError::AgentFailure { diagnostic: Box::new(serde_json::from_value(serde_json::json!({
                    "schema_version":1, "failed_phase":"spawn", "class":"overloaded",
                    "disposition":"retry_same_target", "code":"fixture.retry_dispatched", "summary":"dispatched transient",
                    "causes":[], "stderr_observed":false, "stderr_line_count":0,
                    "prompt_may_have_been_accepted":false
                })).unwrap()) })
            } else {
                Ok(Box::pin(futures::stream::iter([
                    Ok(Update::FinalAnswer("answer".into())),
                    Ok(Update::done("end_turn")),
                ])) as BackendStream)
            };
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
        if self.0.behavior.ends_with("cancel-error") {
            return Err(BridgeError::PermissionDenied);
        }
        self.0.cancel(session).await
    }
    async fn forget_session_checked(
        &self,
        session: &SessionId,
    ) -> Result<BackendCleanupDispositionV1> {
        self.1.forgotten.lock().unwrap().push(session.clone());
        if self.0.behavior == "forget-failed" {
            self.1.cleaned.lock().unwrap().push(session.clone());
            return Err(BridgeError::PermissionDenied);
        }
        if self.0.behavior == "forget-unknown" {
            self.1.cleaned.lock().unwrap().push(session.clone());
            return Ok(BackendCleanupDispositionV1::Unknown);
        }
        if self.0.behavior == "release-failed" {
            self.1.cleaned.lock().unwrap().push(session.clone());
            return Ok(BackendCleanupDispositionV1::Complete);
        }
        self.release_session_checked(session).await
    }
    async fn release_session_checked(
        &self,
        session: &SessionId,
    ) -> Result<BackendCleanupDispositionV1, bridge_core::error::BridgeError> {
        if self.0.behavior == "deadline" {
            self.0.cleanup.0.notify_one();
            self.0.cleanup.1.notified().await;
        }
        if matches!(
            self.0.behavior,
            "cleanup-failed"
                | "release-failed"
                | "parity-cancel-cleanup-error"
                | "parity-open-cancel-cleanup-error"
        ) {
            self.1.cleaned.lock().unwrap().push(session.clone());
            return Err(BridgeError::StoreFailure);
        }
        if self.0.behavior == "forget-unknown" {
            self.1.cleaned.lock().unwrap().push(session.clone());
            return Ok(BackendCleanupDispositionV1::Unknown);
        }
        let result = self.0.release_session_checked(session).await;
        self.1.cleaned.lock().unwrap().push(session.clone());
        result
    }
    fn transfer_cleanup_deadline_v1(
        &self,
        session: &SessionId,
        reason: bridge_core::resource_flight::BoundedRecoveryReasonV1,
    ) -> Result<bridge_core::retained_resource_flight::CleanupDeadlineTransferV1> {
        self.1.transfers.lock().unwrap().push(session.clone());
        self.1
            .flight
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .transfer_cleanup_deadline_now(reason)
            .map_err(|_| BridgeError::StoreFailure)
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
            let resolving = {
                let mut log = log.lock().unwrap();
                let resolving = log
                    .iter()
                    .filter(|id| id.as_str() == entry.id.as_str())
                    .count();
                log.push(entry.id.as_str().to_owned());
                resolving
            };
            let backend = providers[i].clone();
            let record = records[i].clone();
            Box::pin(async move {
                if backend.behavior == "retry-unpolled-resolve" && resolving == 1 {
                    return Err(BridgeError::AgentTimedOut);
                }
                if backend.behavior == "blocked-resolve" {
                    backend.configure.0.notify_one();
                    backend.configure.1.notified().await;
                }
                if let Some(cancel) = record.resolution_cancel.lock().unwrap().take() {
                    cancel.cancel();
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
        assert!(matches!(error, BridgeError::AgentCrashed { .. }));
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
        // The shared ordinary cold path signals a pending/open prompt. Before
        // dispatch (or after a rejected prompt) preservation plus Forget owns teardown.
        let signaled = target == "parked";
        assert_eq!(
            providers[index].cancels.load(Ordering::SeqCst),
            usize::from(signaled)
        );
        let visit = &h.run.progress.visits[index];
        let session = SessionId::parse(visit.cold_context.as_str()).unwrap();
        assert_eq!(
            h.calls[index].canceled.lock().unwrap().as_slice(),
            if signaled {
                std::slice::from_ref(&session)
            } else {
                &[]
            }
        );
        assert_eq!(*h.calls[index].cleaned.lock().unwrap(), [session]);
        let expected_preservation =
            if source_cleanup || matches!(target, "blocked-setup-error" | "pre-poll") {
                CheckoutPreservationReasonV1::NodeFailure
            } else {
                CheckoutPreservationReasonV1::Cancellation
            };
        assert!(
            h.calls[index]
                .preserved
                .lock()
                .unwrap()
                .contains(&expected_preservation),
            "{target}"
        );
        if target == "blocked-resolve" {
            assert_eq!(h.calls[1].setups.load(Ordering::SeqCst), 0);
        }
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
    let mut h = CandidateFixture::new("reject", "final-only").await;
    rusqlite::Connection::open(h.f.dir.path().join("tasks.sqlite")).unwrap()
        .execute_batch("CREATE TRIGGER fail_polled_publication BEFORE UPDATE ON task_candidate_progress_v1 WHEN json_extract(NEW.progress_json,'$.visits[#-1].phase')='Polled' BEGIN SELECT RAISE(ABORT,'publication fault'); END;").unwrap();
    let failure = h
        .execute("source", &CancellationToken::new())
        .await
        .unwrap_err();
    assert_original(&failure.original, "reject");
    assert!(matches!(
        failure.persistence,
        Some(BridgeError::StoreFailure)
    ));
    assert!(matches!(
        failure.surfaced_error(),
        BridgeError::StoreFailure
    ));
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
    let mut h = CandidateFixture::new("preflight-pass", "final-only").await;
    let mut snapshot = h.f.snapshot();
    snapshot.entries[0].preflight = true;
    h.registry.apply(snapshot).await.unwrap();
    h.execute("source", &CancellationToken::new())
        .await
        .unwrap();
    let prompts = h.f.providers[0].prompts.load(Ordering::SeqCst);
    let setups = h.calls[0].setups.load(Ordering::SeqCst);
    let rows = h.f.rows().len();
    assert!(h
        .execute("source", &CancellationToken::new())
        .await
        .is_err());
    assert_eq!(
        h.f.providers[0].prompts.load(Ordering::SeqCst),
        prompts,
        "terminal admission must precede preflight effects"
    );
    assert_eq!(h.calls[0].setups.load(Ordering::SeqCst), setups);
    assert_eq!(h.f.rows().len(), rows);
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
                "retry-exhaust" | "empty-final" | "whitespace-final" | "local-cancel"
            )
            && !matches!(target, "setup" | "retry" | "retry-setup" | "local-cancel");
        assert_eq!(result.is_ok(), expected_ok, "{source}/{target}: {result:?}");
        let rows = h.f.rows();
        let expected = if source == "missing" {
            vec!["source", "source"]
        } else if source_only {
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
                TaskRecordStatus::Failed
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
                u32::from(source.starts_with("retry") || source == "missing")
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
        self.drive_context(admitted, cancel, barrier, WorkflowRunContext::default())
            .await
    }
    async fn drive_context(
        &self,
        admitted: AdmittedWorkflowRunV1,
        cancel: CancellationToken,
        barrier: Option<PromptDispatchBarrier>,
        mut request: WorkflowRunContext,
    ) -> Vec<Result<WorkflowEvent>> {
        use futures::StreamExt;
        let graph = Arc::new(admitted.run_spec.graph.clone());
        request.session_cwd = admitted.run_spec.requested_session_cwd.clone();
        let mut context = WorkflowDiagnosticContext::in_memory(request)
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
        assert!(events.iter().any(|event| matches!(
            event,
            Ok(WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Failed,
                ..
            })
        )));
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

fn node_terminal(events: &[Result<WorkflowEvent>]) -> NodeTerminalV1 {
    events
        .iter()
        .find_map(|event| match event {
            Ok(WorkflowEvent::NodeFinished {
                terminal_json: Some(json),
                ..
            }) => Some(NodeTerminalV1::decode_canonical(json.as_bytes()).unwrap()),
            _ => None,
        })
        .expect("node terminal required")
}

#[tokio::test]
async fn round3_cleanup_evidence() {
    for (source, expected) in [
        ("final-only", NodeCleanupDispositionV1::Complete),
        ("cleanup", NodeCleanupDispositionV1::UnknownLegacy),
        ("cleanup-failed", NodeCleanupDispositionV1::Failed),
    ] {
        let h = CandidateFixture::new(source, "final-only").await;
        let admitted = h
            .builder()
            .bind_local_chains(h.admission("source").await, &h.run.progress.chains)
            .unwrap();
        let events = h.drive(admitted, CancellationToken::new(), None).await;
        assert!(events.iter().all(Result::is_ok), "{events:?}");
        assert_eq!(
            node_terminal(&events).cleanup.disposition,
            expected,
            "{source}"
        );
        assert_eq!(h.calls[0].cleaned.lock().unwrap().len(), 1);
        assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn round3_eof_retries_but_native_missing_does_not() {
    for source in ["missing", "native-missing"] {
        let h = CandidateFixture::new(source, "final-only").await;
        let mut initial = h.admission("source").await;
        Arc::make_mut(&mut initial.run_spec).graph.nodes[0].retry =
            Some(bridge_workflow::graph::RetryPolicy {
                max_attempts: 2,
                backoff_ms: 0,
                backoff_cap_ms: None,
            });
        // Re-freeze so retries remain part of the admitted graph.
        let admitted = h
            .builder()
            .freeze(WorkflowAdmissionRequestV1 {
                attempt_id: h.owner.attempt_id.clone(),
                graph: Arc::new(initial.run_spec.graph.clone()),
                requested_session_cwd: initial.run_spec.requested_session_cwd.clone(),
                policy_invocation: Default::default(),
                ledger_admission: initial.run_spec.ledger_admission.clone(),
                r2f1b: None,
            })
            .await
            .unwrap();
        let admitted = h
            .builder()
            .bind_local_chains(admitted, &h.run.progress.chains)
            .unwrap();
        let events = h.drive(admitted, CancellationToken::new(), None).await;
        let expected = if source == "missing" { 2 } else { 1 };
        assert_eq!(
            h.f.providers[0].prompts.load(Ordering::SeqCst),
            expected,
            "{source}"
        );
        assert_eq!(h.f.rows().len(), expected);
        assert_eq!(h.f.providers[1].setup.load(Ordering::SeqCst), 0);
        assert!(events.iter().any(|event| matches!(event, Ok(WorkflowEvent::Terminal { outcome, .. })
            if *outcome == if source == "missing" { WorkflowOutcome::Completed } else { WorkflowOutcome::Failed })));
    }
}

#[tokio::test]
async fn round3_preflight_projection_matches_ordinary() {
    for canceled in [false, true] {
        let mut projections = vec![];
        for chain in [false, true] {
            let h = CandidateFixture::new("reject", "final-only").await;
            let mut snapshot = h.f.snapshot();
            snapshot.entries[0].preflight = true;
            h.registry.apply(snapshot).await.unwrap();
            let admitted = h.admission("source").await;
            let admitted = if chain {
                h.builder()
                    .bind_local_chains(admitted, &h.run.progress.chains)
                    .unwrap()
            } else {
                admitted
            };
            let cancel = CancellationToken::new();
            let barrier = canceled.then(|| {
                let cancel = cancel.clone();
                Arc::new(move || {
                    let cancel = cancel.clone();
                    Box::pin(async move {
                        cancel.cancel();
                    })
                        as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
                }) as PromptDispatchBarrier
            });
            let events = h.drive(admitted, cancel, barrier).await;
            assert!(
                events.iter().all(Result::is_ok),
                "preflight must project terminal events: {events:?}"
            );
            let node = node_terminal(&events);
            let output = events
                .iter()
                .filter_map(|event| match event {
                    Ok(WorkflowEvent::NodeFinished { ok, output, .. }) => {
                        Some(format!("{ok}:{output}"))
                    }
                    Ok(WorkflowEvent::Terminal { outcome, output }) => {
                        Some(format!("{outcome:?}:{output}"))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(output.len(), 2);
            let output = output
                .into_iter()
                .map(|text| {
                    text.split(' ')
                        .map(|word| {
                            if word.starts_with("duration_ms=") {
                                "duration_ms=<elapsed>"
                            } else {
                                word
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect::<Vec<_>>();
            projections.push((
                node.primary,
                output,
                h.f.providers[0].prompts.load(Ordering::SeqCst),
                h.f.rows().len(),
            ));
            assert_eq!(h.f.providers[1].setup.load(Ordering::SeqCst), 0);
        }
        assert_eq!(projections[0], projections[1], "canceled={canceled}");
    }
}

#[tokio::test]
async fn round3_attestation_preserves_winning_turn() {
    use bridge_core::{attestation::*, harvest::HarvestAuditStore};
    for fallback in [false, true] {
        for behavior in ["attested", "attested-invalid", "unsupported-prefix"] {
            let h = CandidateFixture::new(
                if fallback { "reject" } else { behavior },
                if fallback { behavior } else { "final-only" },
            )
            .await;
            let initial = h.admission("source").await;
            let mut graph = initial.run_spec.graph.clone();
            graph.nodes[0].harvest_sanitization = Some(HarvestSanitizationMode::AttestedPrefixV1);
            let admitted = h
                .builder()
                .freeze(WorkflowAdmissionRequestV1 {
                    attempt_id: h.owner.attempt_id.clone(),
                    graph: Arc::new(graph),
                    requested_session_cwd: initial.run_spec.requested_session_cwd.clone(),
                    policy_invocation: Default::default(),
                    ledger_admission: initial.run_spec.ledger_admission.clone(),
                    r2f1b: None,
                })
                .await
                .unwrap();
            let admitted = h
                .builder()
                .bind_local_chains(admitted, &h.run.progress.chains)
                .unwrap();
            let task = TaskId::parse(h.owner.execution_id.as_str()).unwrap();
            let events = h
                .drive_context(
                    admitted,
                    CancellationToken::new(),
                    None,
                    WorkflowRunContext {
                        task_id: Some(task.clone()),
                        harvest_audit_store: h.f.store.clone(),
                        ..Default::default()
                    },
                )
                .await;
            assert!(events.iter().all(Result::is_ok), "{events:?}");
            let expected = if behavior == "attested" {
                "answer"
            } else {
                "process\nanswer"
            };
            assert!(events.iter().any(|event| matches!(event, Ok(WorkflowEvent::Terminal { output, outcome: WorkflowOutcome::Completed }) if output == expected)), "{events:?}");
            let winner = usize::from(fallback);
            let (session, meta) = {
                let turns = h.calls[winner].turns.lock().unwrap();
                let (session, meta) = turns.iter().next().unwrap();
                (session.clone(), meta.clone())
            };
            assert_eq!(
                meta.requested_mode,
                HarvestSanitizationMode::AttestedPrefixV1
            );
            assert_eq!(
                matches!(
                    meta.prefix_attestation_request,
                    PrefixAttestationRequest::CodexCommitMarkerV1 { .. }
                ),
                behavior != "unsupported-prefix"
            );
            assert_eq!(
                h.f.providers[winner].inputs.lock().unwrap()[0][0]
                    .text
                    .contains("B2A ATTESTED PREFIX PROTOCOL"),
                behavior != "unsupported-prefix"
            );
            let bundles =
                h.f.store
                    .list_by_task_id(task.as_str(), None, 10)
                    .await
                    .unwrap();
            assert_eq!(bundles.len(), 1);
            let raw = &bundles[0].raw;
            assert_eq!(raw.turn_id, meta.turn_id.as_str());
            assert_eq!(raw.run_id, session.as_str());
            assert_eq!(raw.attempt_id, 1 + u32::from(fallback));
            assert_eq!(raw.backend_id, if fallback { "target" } else { "source" });
            assert_eq!(raw.producer_id, raw.backend_id);
        }
    }
}

struct TestClock(tokio::time::Instant);
impl bridge_core::attempt_activity::MonotonicClock for TestClock {
    fn elapsed_ms(&self) -> u64 {
        self.0.elapsed().as_millis() as u64
    }
}
struct ClockSink;
#[async_trait::async_trait]
impl RichEventSink for ClockSink {
    fn record(&self, _: bridge_core::orch::OrchEventKind) {}
    async fn flush(&self) -> Result<()> {
        Ok(())
    }
}
struct ClockFactory(Arc<TestClock>);
impl RichEventSinkFactory for ClockFactory {
    fn make(&self, _: &NodeId) -> Arc<dyn RichEventSink> {
        Arc::new(ClockSink)
    }
    fn monotonic_clock(&self) -> Option<Arc<dyn bridge_core::attempt_activity::MonotonicClock>> {
        Some(self.0.clone())
    }
}

async fn deadline_transfer_case(
    chain: bool,
) -> (NodePrimaryDispositionV1, NodeCleanupDispositionV1) {
    let h = CandidateFixture::new("deadline", "final-only").await;
    let initial = h.admission("source").await;
    let admitted = if chain {
        h.builder()
            .bind_local_chains(initial, &h.run.progress.chains)
            .unwrap()
    } else {
        initial
    };
    let cancel = CancellationToken::new();
    let request = WorkflowRunContext {
        make_rich_sink: Some(Arc::new(ClockFactory(Arc::new(TestClock(
            tokio::time::Instant::now(),
        ))))),
        ..Default::default()
    };
    let drive = h.drive_context(admitted, cancel.clone(), None, request);
    let trigger = async {
        h.f.providers[0].cleanup.0.notified().await;
        cancel.cancel();
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        tokio::time::advance(std::time::Duration::from_millis(60_001)).await;
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        let transfers = h.calls[0].transfers.lock().unwrap().len();
        h.f.providers[0].cleanup.1.notify_one();
        assert_eq!(
            transfers, 1,
            "deadline must transfer before checked cleanup returns"
        );
    };
    let (events, _) = tokio::join!(drive, trigger);
    assert_eq!(
        h.calls[0].transfers.lock().unwrap().as_slice(),
        h.f.providers[0].sessions.lock().unwrap().as_slice(),
        "deadline must find the exact candidate owner"
    );
    assert_eq!(h.calls[0].transfers.lock().unwrap().len(), 1);
    assert_eq!(h.f.providers[1].setup.load(Ordering::SeqCst), 0);
    assert!(events.iter().all(Result::is_ok), "{events:?}");
    assert_eq!(
        node_terminal(&events).cleanup.disposition,
        NodeCleanupDispositionV1::UnknownLegacy
    );
    assert!(events.iter().any(|event| matches!(
        event,
        Ok(WorkflowEvent::Terminal {
            outcome: WorkflowOutcome::Canceled,
            ..
        })
    )));
    (
        node_terminal(&events).primary,
        node_terminal(&events).cleanup.disposition,
    )
}

#[tokio::test(start_paused = true)]
async fn round3_cleanup_deadline_transfers_exact_candidate() {
    deadline_transfer_case(true).await;
}

#[tokio::test(start_paused = true)]
async fn round5_deadline_transfer_parity() {
    assert_eq!(
        deadline_transfer_case(false).await,
        deadline_transfer_case(true).await
    );
}

#[tokio::test]
async fn round4_canceled_publication_preserves_storage_failure() {
    for storage_failure in [false, true] {
        let h = CandidateFixture::new("blocked-cleanup", "final-only").await;
        let admitted = h
            .builder()
            .bind_local_chains(h.admission("source").await, &h.run.progress.chains)
            .unwrap();
        let cancel = CancellationToken::new();
        let drive = h.drive(admitted, cancel.clone(), None);
        let interrupt = async {
            h.f.providers[0].cleanup.0.notified().await;
            if storage_failure {
                rusqlite::Connection::open(h.f.dir.path().join("tasks.sqlite")).unwrap()
                    .execute_batch("CREATE TRIGGER fail_candidate_publication BEFORE UPDATE ON task_candidate_progress_v1 BEGIN SELECT RAISE(ABORT,'publication fault'); END;").unwrap();
            }
            cancel.cancel();
            h.f.providers[0].cleanup.1.notify_one();
        };
        let (events, ()) = tokio::join!(drive, interrupt);
        assert_eq!(
            events.iter().any(Result::is_err),
            storage_failure,
            "persistence must outrank cancellation: {events:?}"
        );
        if storage_failure {
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e, Err(BridgeError::StoreFailure))),
                "{events:?}"
            );
        } else {
            // Cancellation arriving during rejected-prompt cleanup keeps the
            // provider failure, as on the ordinary node-attempt path.
            assert!(
                events.iter().any(|e| matches!(
                    e,
                    Ok(WorkflowEvent::Terminal {
                        outcome: WorkflowOutcome::Failed,
                        ..
                    })
                )),
                "{events:?}"
            );
            let progress =
                h.f.store
                    .candidate_progress_v1(
                        &TaskId::parse(h.owner.execution_id.as_str()).unwrap(),
                        &NodeId::parse("node").unwrap(),
                    )
                    .await
                    .unwrap()
                    .unwrap();
            assert!(progress.visits[0].rejection.as_ref().unwrap().canceled);
        }
        assert_eq!(h.f.providers[1].setup.load(Ordering::SeqCst), 0);
        assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 0);
    }
}

async fn round4_success_cleanup(behavior: &'static str, expected: bool) {
    for chain in [false, true] {
        let h = CandidateFixture::new(behavior, "final-only").await;
        let initial = h.admission("source").await;
        let admitted = if chain {
            h.builder()
                .bind_local_chains(initial, &h.run.progress.chains)
                .unwrap()
        } else {
            initial
        };
        let events = h.drive(admitted, CancellationToken::new(), None).await;
        assert!(events.iter().all(Result::is_ok), "{behavior}: {events:?}");
        assert!(events.iter().any(|e| matches!(e, Ok(WorkflowEvent::Terminal { outcome, output })
            if *outcome == if expected { WorkflowOutcome::Completed } else { WorkflowOutcome::Failed }
            && (!expected || output == "answer"))), "successful cleanup contract differs chain={chain} {behavior}: {events:?}");
        assert_eq!(
            h.calls[0].forgotten.lock().unwrap().len(),
            1,
            "terminal success must forget"
        );
        assert_eq!(h.f.providers[1].setup.load(Ordering::SeqCst), 0);
        if !expected {
            assert!(format!("{events:?}").contains("PermissionDenied"));
        }
    }
}

#[tokio::test]
async fn round4_success_forgets_instead_of_releasing() {
    round4_success_cleanup("release-failed", true).await;
}

#[tokio::test]
async fn round4_success_accepts_unknown_cleanup_and_preserves_errors() {
    round4_success_cleanup("forget-unknown", true).await;
    round4_success_cleanup("forget-failed", false).await;
    let h = CandidateFixture::new("cleanup", "final-only").await;
    let admitted = h
        .builder()
        .bind_local_chains(h.admission("source").await, &h.run.progress.chains)
        .unwrap();
    let events = h.drive(admitted, CancellationToken::new(), None).await;
    assert_eq!(h.f.providers[1].setup.load(Ordering::SeqCst), 0);
    assert!(events.iter().any(|e| matches!(
        e,
        Ok(WorkflowEvent::Terminal {
            outcome: WorkflowOutcome::Failed,
            ..
        })
    )));
}

#[tokio::test]
async fn round4_unpolled_transient_keeps_same_candidate_retry_budget() {
    for (source, attempts, expected_rows, success) in [
        ("retry-unpolled", 3, 3, true),
        ("retry-unpolled", 2, 2, false),
        ("retry-unpolled-resolve", 3, 3, true),
        ("retry-unpolled-resolve", 2, 2, false),
        ("retry-setup", 3, 2, false),
    ] {
        let h = CandidateFixture::new(source, "final-only").await;
        let initial = h.admission("source").await;
        let mut graph = initial.run_spec.graph.clone();
        graph.nodes[0].retry = Some(bridge_workflow::graph::RetryPolicy {
            max_attempts: attempts,
            backoff_ms: 0,
            backoff_cap_ms: None,
        });
        let admitted = h
            .builder()
            .freeze(WorkflowAdmissionRequestV1 {
                attempt_id: h.owner.attempt_id.clone(),
                graph: Arc::new(graph),
                requested_session_cwd: initial.run_spec.requested_session_cwd.clone(),
                policy_invocation: Default::default(),
                ledger_admission: initial.run_spec.ledger_admission.clone(),
                r2f1b: None,
            })
            .await
            .unwrap();
        let admitted = h
            .builder()
            .bind_local_chains(admitted, &h.run.progress.chains)
            .unwrap();
        let events = h.drive(admitted, CancellationToken::new(), None).await;
        assert_eq!(
            h.f.rows().len(),
            expected_rows,
            "unpolled transient must consume its own A attempt and allow remaining E6: {events:?}"
        );
        assert!(h.f.rows().iter().all(|r| r.attribution.agent == "source"));
        assert_eq!(
            h.f.providers[0].prompts.load(Ordering::SeqCst),
            if success { 2 } else { 1 }
        );
        assert_eq!(h.f.providers[1].setup.load(Ordering::SeqCst), 0);
        assert!(events.iter().any(|e| matches!(e, Ok(WorkflowEvent::Terminal { outcome, output })
            if *outcome == if success { WorkflowOutcome::Completed } else { WorkflowOutcome::Failed }
            && (!success || output == "answer"))), "{events:?}");
        if !success {
            assert!(format!("{events:?}").contains(if source == "retry-setup" {
                "fixture.rejected"
            } else {
                "fixture.retry_dispatched"
            }));
        }
        let progress =
            h.f.store
                .candidate_progress_v1(
                    &TaskId::parse(h.owner.execution_id.as_str()).unwrap(),
                    &NodeId::parse("node").unwrap(),
                )
                .await
                .unwrap()
                .unwrap();
        assert_eq!(progress.visits.len(), expected_rows);
        assert_eq!(progress.retries_consumed as usize, expected_rows - 1);
        assert!(!progress.visits[1].phase.dispatched());
    }
}

#[allow(dead_code)]
pub(crate) async fn round4_history_admission<F, Fut>(run: F)
where
    F: Fn(
        Vec<String>,
        Arc<dyn AgentRegistry>,
        Arc<bridge_store::sqlite::SqliteStore>,
        bridge_core::workflow_history::LedgerUnavailableReason,
    ) -> Fut,
    Fut: std::future::Future<Output = std::result::Result<(), String>>,
{
    use bridge_core::workflow_history::LedgerUnavailableReason;
    for reason in [
        LedgerUnavailableReason::Locked,
        LedgerUnavailableReason::Collision,
    ] {
        let h = CandidateFixture::new("reject", "final-only").await;
        let root = h.f.dir.path();
        let scope = root.join("scope");
        let mut cfg = format!("default='source'\n[store]\npath={:?}\n[registry]\nallowed_cmds=['test-cmd']\n[server]\n", root.join("tasks.sqlite"));
        for agent in ["source", "target", "last", "other"] {
            cfg += &format!("[[agents]]\nid='{agent}'\ncmd='test-cmd'\nmodel='{agent}'\ncwd={scope:?}\nsession_cwd={scope:?}\n[[billing_identities]]\nagent='{agent}'\nmodel='{agent}'\naccount_group='{agent}'\nbilling_mode='metered'\n");
        }
        cfg += "[[routing.chains]]\nid='original'\nsource={agent='source',model='source'}\ncauses=['quota_exhausted']\nsteps=[{agent='target',model='target'}]\n[[workflows]]\nid='workflow'\n[[workflows.nodes]]\nid='node'\nagent='source'\nprompt_text='{{input}}'\ninputs=[]\n";
        std::fs::write(root.join("config.toml"), cfg).unwrap();
        std::fs::write(root.join("input"), "---\ntask-type: freeform\n---\ninput").unwrap();
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
        let result = run(args, h.registry.clone(), h.f.store.clone(), reason).await;
        if reason == LedgerUnavailableReason::Collision {
            assert!(result
                .unwrap_err()
                .contains("history admission failed closed (collision)"));
            assert!(h.f.spawns.lock().unwrap().is_empty());
            let connection = rusqlite::Connection::open(root.join("tasks.sqlite")).unwrap();
            let usage_tables: i64 = connection.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='usage_attempts_v1'",
                [], |row| row.get(0)
            ).unwrap();
            assert_eq!(
                usage_tables, 0,
                "collision must refuse before accounting preparation"
            );
        } else {
            assert!(
                result.is_ok(),
                "recoverable reservation error must retain local fallback: {result:?}"
            );
            assert_eq!(
                std::fs::read_to_string(root.join("output")).unwrap(),
                "answer"
            );
            assert_eq!(h.f.providers[0].prompts.load(Ordering::SeqCst), 1);
            assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 1);
            assert_eq!(h.f.rows().len(), 2);
        }
    }
}

#[tokio::test]
async fn round5_successor_preflight_refuses_without_execution() {
    for (behavior, primary, expected_ok) in [
        ("final-only", false, false),
        ("preflight-pass", false, true),
        ("final-only", true, true),
        ("preflight-displaced", false, false),
    ] {
        let h =
            CandidateFixture::new(if primary { "final-only" } else { "reject" }, behavior).await;
        let mut snapshot = h.f.snapshot();
        snapshot.entries[1].preflight = true;
        if behavior == "preflight-displaced" {
            snapshot.entries[1].fallback_models = vec!["alternate".into()];
        }
        h.registry.apply(snapshot).await.unwrap();
        let admitted = h
            .builder()
            .bind_local_chains(h.admission("source").await, &h.run.progress.chains)
            .unwrap();
        let events = h.drive(admitted, CancellationToken::new(), None).await;
        assert!(events.iter().all(Result::is_ok), "{events:?}");
        assert_eq!(
            node_terminal(&events).primary,
            if expected_ok {
                NodePrimaryDispositionV1::Completed
            } else {
                NodePrimaryDispositionV1::Failed
            }
        );
        assert_eq!(
            h.f.providers[1]
                .inputs
                .lock()
                .unwrap()
                .iter()
                .filter(|p| !p[0].text.contains("PONG"))
                .count(),
            usize::from(behavior == "preflight-pass"),
            "successor executed without its exact preflight"
        );
        assert_eq!(h.f.providers[2].setup.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn round5_cancel_projection_matches_ordinary() {
    for behavior in [
        "parity-cancel-error",
        "parity-cancel",
        "parity-cancel-cleanup-error",
        "parity-open-cancel",
        "parity-open-cancel-error",
        "parity-open-cancel-cleanup-error",
        "blocked-resolve",
        "blocked-cleanup",
    ] {
        for successor in [false, true] {
            let mut outcomes = vec![];
            for chain in [false, true] {
                let h = CandidateFixture::new(
                    if successor { "reject" } else { behavior },
                    if successor { behavior } else { "final-only" },
                )
                .await;
                let index = usize::from(successor);
                let admitted = h
                    .admission(if successor && !chain {
                        "target"
                    } else {
                        "source"
                    })
                    .await;
                let admitted = if chain {
                    h.builder()
                        .bind_local_chains(admitted, &h.run.progress.chains)
                        .unwrap()
                } else {
                    admitted
                };
                let cancel = CancellationToken::new();
                let drive = h.drive(admitted, cancel.clone(), None);
                let stop = async {
                    if behavior == "blocked-cleanup" {
                        h.f.providers[index].cleanup.0.notified().await;
                    } else {
                        h.f.providers[index].configure.0.notified().await;
                    }
                    cancel.cancel();
                    if behavior == "blocked-resolve" {
                        h.f.providers[index].configure.1.notify_one();
                    }
                    if behavior == "blocked-cleanup" {
                        h.f.providers[index].cleanup.1.notify_one();
                    }
                };
                let (events, ()) = tokio::join!(drive, stop);
                outcomes.push(node_terminal(&events).primary);
                assert_eq!(h.f.providers[2].setup.load(Ordering::SeqCst), 0);
                if !successor {
                    assert_eq!(h.f.providers[1].setup.load(Ordering::SeqCst), 0);
                }
            }
            assert_eq!(
                outcomes[0], outcomes[1],
                "cancel/cleanup projection differs: {behavior} successor={successor}"
            );
        }
    }
}

#[tokio::test]
async fn round5_failed_harvest_keeps_dispatched_context() {
    use bridge_core::harvest::HarvestAuditStore;
    for (source, target, winner) in [
        ("reject", "parity-error", 1),
        ("parity-error", "final-only", 0),
        ("reject", "setup", 0),
        ("reject", "retry-setup", 1),
        ("reject", "retry", 1),
        ("reject", "parity-cancel", 1),
        ("parity-cancel", "final-only", 0),
    ] {
        let h = CandidateFixture::new(source, target).await;
        let initial = h.admission("source").await;
        let mut graph = initial.run_spec.graph.clone();
        if matches!(target, "retry-setup" | "retry") {
            graph.nodes[0].retry = Some(bridge_workflow::graph::RetryPolicy {
                max_attempts: 2,
                backoff_ms: 1000,
                backoff_cap_ms: None,
            });
        }
        graph.nodes[0].harvest_sanitization =
            Some(bridge_core::attestation::HarvestSanitizationMode::AttestedPrefixV1);
        let admitted = h
            .builder()
            .freeze(WorkflowAdmissionRequestV1 {
                attempt_id: h.owner.attempt_id.clone(),
                graph: Arc::new(graph),
                requested_session_cwd: initial.run_spec.requested_session_cwd.clone(),
                policy_invocation: Default::default(),
                ledger_admission: initial.run_spec.ledger_admission.clone(),
                r2f1b: None,
            })
            .await
            .unwrap();
        let admitted = h
            .builder()
            .bind_local_chains(admitted, &h.run.progress.chains)
            .unwrap();
        let task = TaskId::parse(h.owner.execution_id.as_str()).unwrap();
        let cancel = CancellationToken::new();
        let drive = h.drive_context(
            admitted,
            cancel.clone(),
            None,
            WorkflowRunContext {
                task_id: Some(task.clone()),
                harvest_audit_store: h.f.store.clone(),
                observer: if target == "retry" {
                    Arc::new(CancelAfterTargetTurn(cancel.clone()))
                } else {
                    Arc::new(LifecycleObserver::default())
                },
                ..Default::default()
            },
        );
        let events = if source == "parity-cancel" || target == "parity-cancel" {
            tokio::join!(drive, async {
                h.f.providers[winner].configure.0.notified().await;
                cancel.cancel();
            })
            .0
        } else {
            drive.await
        };
        assert!(events.iter().all(Result::is_ok), "{events:?}");
        let bundles =
            h.f.store
                .list_by_task_id(task.as_str(), None, 10)
                .await
                .unwrap();
        assert_eq!(bundles.len(), 1);
        let turns = h.calls[winner].turns.lock().unwrap();
        let (session, meta) = turns.iter().next().unwrap();
        let raw = &bundles[0].raw;
        assert_eq!(raw.backend_id, ["source", "target"][winner]);
        assert_eq!(raw.producer_id, raw.backend_id);
        assert_eq!(raw.run_id, session.as_str());
        assert_eq!(raw.turn_id, meta.turn_id.as_str());
        assert_eq!(raw.attempt_id, winner as u32 + 1);
    }
}

struct CancelAfterTargetTurn(CancellationToken);
impl Observer for CancelAfterTargetTurn {
    fn record(&self, event: &ObsEvent<'_>) {
        if matches!(event, ObsEvent::TurnFinished { ctx, .. } if ctx.agent == "target") {
            self.0.cancel();
        }
    }
}

#[derive(Default)]
struct ParitySink(
    std::sync::atomic::AtomicUsize,
    std::sync::atomic::AtomicUsize,
);
#[async_trait::async_trait]
impl RichEventSink for ParitySink {
    fn record(&self, _: bridge_core::orch::OrchEventKind) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
    async fn flush(&self) -> Result<()> {
        self.1.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
struct ParityFactory(Arc<ParitySink>);
impl RichEventSinkFactory for ParityFactory {
    fn make(&self, _: &NodeId) -> Arc<dyn RichEventSink> {
        self.0.clone()
    }
}

#[tokio::test]
async fn round5_usage_flush_liveness_and_projection_parity() {
    for behavior in ["parity-error", "parity-cancel", "parity-success"] {
        let mut projections = vec![];
        for chain in [false, true] {
            let h = CandidateFixture::new(behavior, "final-only").await;
            let admitted = h.admission("source").await;
            let admitted = if chain {
                h.builder()
                    .bind_local_chains(admitted, &h.run.progress.chains)
                    .unwrap()
            } else {
                admitted
            };
            let sink = Arc::new(ParitySink::default());
            let cancel = CancellationToken::new();
            let drive = h.drive_context(
                admitted,
                cancel.clone(),
                None,
                WorkflowRunContext {
                    make_rich_sink: Some(Arc::new(ParityFactory(sink.clone()))),
                    observer_key: Some("parity-board".into()),
                    ..Default::default()
                },
            );
            let events = if behavior.contains("cancel") {
                let stop = async {
                    h.f.providers[0].configure.0.notified().await;
                    cancel.cancel();
                };
                tokio::join!(drive, stop).0
            } else {
                drive.await
            };
            let usage = events
                .iter()
                .find_map(|event| match event {
                    Ok(WorkflowEvent::NodeFinished { usage, .. }) => Some(usage.clone()),
                    _ => None,
                })
                .unwrap();
            let turns = h.calls[0].turns.lock().unwrap();
            let liveness = turns.values().next().unwrap().liveness.clone();
            let text = events
                .iter()
                .find_map(|event| match event {
                    Ok(WorkflowEvent::NodeFinished { output, .. }) => Some(output.clone()),
                    _ => None,
                })
                .unwrap();
            projections.push((
                usage,
                text,
                node_terminal(&events).primary,
                sink.0.load(Ordering::SeqCst),
                sink.1.load(Ordering::SeqCst),
                liveness,
            ));
            assert_eq!(h.f.providers[1].setup.load(Ordering::SeqCst), 0);
        }
        assert_eq!(projections[0], projections[1], "{behavior}");
    }
}

#[tokio::test]
async fn round5_cold_attempt_contract_parity() {
    // A nonmatching chain exercises its source through the candidate boundary;
    // identical no-chain execution supplies the expected attempt contract.
    for behavior in [
        "final-only",
        "local-cancel",
        "merged-usage",
        "empty-final",
        "whitespace-final",
        "missing",
        "native-missing",
        "retry",
        "retry-unpolled",
        "forget-unknown",
        "forget-failed",
        "cleanup-failed",
    ] {
        let mut projections = vec![];
        for chain in [false, true] {
            let h = CandidateFixture::new(behavior, "final-only").await;
            let initial = h.admission("source").await;
            let mut graph = initial.run_spec.graph.clone();
            graph.nodes[0].retry = Some(bridge_workflow::graph::RetryPolicy {
                max_attempts: 3,
                backoff_ms: 1,
                backoff_cap_ms: None,
            });
            let admitted = h
                .builder()
                .freeze(WorkflowAdmissionRequestV1 {
                    attempt_id: h.owner.attempt_id.clone(),
                    graph: Arc::new(graph),
                    requested_session_cwd: initial.run_spec.requested_session_cwd.clone(),
                    policy_invocation: Default::default(),
                    ledger_admission: initial.run_spec.ledger_admission.clone(),
                    r2f1b: None,
                })
                .await
                .unwrap();
            let mut chains = h.run.progress.chains.clone();
            for declaration in &mut chains {
                declaration.causes =
                    [bridge_core::routing::FallbackCauseV1::RegionRestricted].into();
            }
            let admitted = if chain {
                h.builder().bind_local_chains(admitted, &chains).unwrap()
            } else {
                admitted
            };
            let events = h.drive(admitted, CancellationToken::new(), None).await;
            assert!(
                events.iter().all(Result::is_ok),
                "{behavior} {chain}: {events:?}"
            );
            let mut terminal = node_terminal(&events);
            terminal.cleanup.duration_ms = 0;
            let (output, usage) = events
                .iter()
                .find_map(|event| match event {
                    Ok(WorkflowEvent::NodeFinished { output, usage, .. }) => {
                        Some((output.clone(), usage.clone()))
                    }
                    _ => None,
                })
                .unwrap();
            let rows = h.f.rows();
            projections.push((
                terminal,
                output,
                usage,
                rows.len(),
                h.f.providers[0].prompts.load(Ordering::SeqCst),
                h.calls[0].forgotten.lock().unwrap().len(),
                h.calls[0].cleaned.lock().unwrap().len(),
            ));
            assert_eq!(h.f.providers[1].setup.load(Ordering::SeqCst), 0);
            assert!(rows.iter().all(|r| r.attribution.agent == "source"
                && r.attribution.effective_model.as_deref() == Some("source")));
        }
        assert_eq!(
            projections[0], projections[1],
            "attempt contract: {behavior}"
        );
    }
}

#[allow(dead_code)] // Invoked by the production CLI unit harness.
pub(crate) async fn round5_persisted_attempt_telemetry_parity<F, Fut>(run: F)
where
    F: Fn(Vec<String>, Arc<dyn AgentRegistry>, Arc<bridge_store::sqlite::SqliteStore>) -> Fut,
    Fut: std::future::Future<Output = std::result::Result<(), String>>,
{
    use bridge_core::workflow_history::WorkflowHistoryStore;
    let mut tallies = vec![];
    for chain in [false, true] {
        let h = CandidateFixture::new("parity-success", "final-only").await;
        let root = h.f.dir.path();
        let scope = root.join("scope");
        let mut config = format!("default='source'\n[store]\npath={:?}\n[registry]\nallowed_cmds=['test-cmd']\n[server]\n", root.join("tasks.sqlite"));
        for agent in ["source", "target", "last", "other"] {
            config += &format!("[[agents]]\nid='{agent}'\ncmd='test-cmd'\nmodel='{agent}'\ncwd={scope:?}\nsession_cwd={scope:?}\n");
        }
        if chain {
            config += "[[routing.chains]]\nid='parity'\nsource={agent='source',model='source'}\ncauses=['quota_exhausted']\nsteps=[{agent='target',model='target'}]\n";
        }
        config += "[[workflows]]\nid='workflow'\n[[workflows.nodes]]\nid='node'\nagent='source'\nprompt_text='{{input}}'\ninputs=[]\n";
        std::fs::write(root.join("config.toml"), config).unwrap();
        std::fs::write(root.join("input"), "---\ntask-type: freeform\n---\ninput").unwrap();
        run(
            vec![
                "workflow".into(),
                "--config".into(),
                root.join("config.toml").display().to_string(),
                "--input".into(),
                root.join("input").display().to_string(),
                "--out".into(),
                root.join("output").display().to_string(),
                "--no-heartbeat".into(),
            ],
            h.registry.clone(),
            h.f.store.clone(),
        )
        .await
        .unwrap();
        let row = h.f.rows().pop().unwrap();
        let owner = AttemptId::parse(row.attribution.run_attempt.unwrap()).unwrap();
        let tally = h.f.store.activity_tally(&owner).await.unwrap().unwrap();
        let history = h.f.store.attempt(&owner).await.unwrap().unwrap();
        assert_eq!(history.terminal.unwrap().node_counts.completed, 1);
        assert!(
            tally.phase_activity[bridge_core::attempt_activity::AttemptPhase::Adapter as usize] > 0,
            "adapter telemetry was swallowed"
        );
        tallies.push(tally.phase_activity);
    }
    assert_eq!(tallies[0], tallies[1]);
}

struct FailingParityFactory(Arc<ParitySink>);
struct FailingParitySink(Arc<ParitySink>);
impl RichEventSinkFactory for FailingParityFactory {
    fn make(&self, _: &NodeId) -> Arc<dyn RichEventSink> {
        Arc::new(FailingParitySink(self.0.clone()))
    }
}
#[async_trait::async_trait]
impl RichEventSink for FailingParitySink {
    fn record(&self, kind: bridge_core::orch::OrchEventKind) {
        self.0.record(kind);
    }
    async fn flush(&self) -> Result<()> {
        self.0.flush().await?;
        Err(BridgeError::StoreFailure)
    }
}

#[tokio::test]
async fn round5_rich_flush_failure_parity() {
    for behavior in ["parity-success", "parity-error", "parity-cancel", "reject"] {
        let mut projections = vec![];
        for chain in [false, true] {
            let h = CandidateFixture::new(behavior, "final-only").await;
            let admitted = h.admission("source").await;
            let mut chains = h.run.progress.chains.clone();
            for declaration in &mut chains {
                declaration.causes =
                    [bridge_core::routing::FallbackCauseV1::RegionRestricted].into();
            }
            let admitted = if chain {
                h.builder().bind_local_chains(admitted, &chains).unwrap()
            } else {
                admitted
            };
            let sink = Arc::new(ParitySink::default());
            let cancel = CancellationToken::new();
            let drive = h.drive_context(
                admitted,
                cancel.clone(),
                None,
                WorkflowRunContext {
                    make_rich_sink: Some(Arc::new(FailingParityFactory(sink.clone()))),
                    ..Default::default()
                },
            );
            let events = if behavior == "parity-cancel" {
                tokio::join!(drive, async {
                    h.f.providers[0].configure.0.notified().await;
                    cancel.cancel();
                })
                .0
            } else {
                drive.await
            };
            let (output, usage) = events
                .iter()
                .find_map(|event| match event {
                    Ok(WorkflowEvent::NodeFinished { output, usage, .. }) => {
                        Some((output.clone(), usage.clone()))
                    }
                    _ => None,
                })
                .unwrap();
            projections.push((
                node_terminal(&events).primary,
                output,
                usage,
                sink.1.load(Ordering::SeqCst),
            ));
            assert_eq!(sink.1.load(Ordering::SeqCst), 1);
            assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 0);
        }
        assert_eq!(projections[0], projections[1], "{behavior}");
    }
}

#[tokio::test]
async fn round5_pre_canceled_attempt_parity() {
    for chain in [false, true] {
        let h = CandidateFixture::new("final-only", "final-only").await;
        h.f.store.ensure_usage_coverage_v1().await.unwrap();
        let admitted = h.admission("source").await;
        let admitted = if chain {
            h.builder()
                .bind_local_chains(admitted, &h.run.progress.chains)
                .unwrap()
        } else {
            admitted
        };
        let cancel = CancellationToken::new();
        cancel.cancel();
        let events = h.drive(admitted, cancel, None).await;
        assert!(
            events.iter().any(|event| matches!(
                event,
                Ok(WorkflowEvent::Terminal {
                    outcome: WorkflowOutcome::Canceled,
                    ..
                })
            )),
            "{events:?}"
        );
        assert!(h.f.rows().is_empty());
        assert!(h
            .calls
            .iter()
            .all(|calls| calls.setups.load(Ordering::SeqCst) == 0));
    }
    let mut h = CandidateFixture::new("preflight-pass", "final-only").await;
    h.f.store.ensure_usage_coverage_v1().await.unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let failure = h.execute("source", &cancel).await.unwrap_err();
    assert_eq!(
        failure.preparation_exit,
        bridge_core::translator::AccountingPreparationExitV1::Canceled
    );
    assert_eq!(
        failure.replay.prompt_acceptance,
        bridge_core::routing::ExclusionEvidenceV1::Excluded
    );
    assert_eq!(
        failure.replay.effects,
        bridge_core::routing::ExclusionEvidenceV1::Excluded
    );
    assert!(h.f.rows().is_empty());
    assert!(h
        .calls
        .iter()
        .all(|calls| calls.setups.load(Ordering::SeqCst) == 0));
}

#[tokio::test]
async fn round5_attestation_parity() {
    use bridge_core::{attestation::HarvestSanitizationMode, harvest::HarvestAuditStore};
    for behavior in ["attested", "attested-invalid", "unsupported-prefix"] {
        let mut projections = vec![];
        for chain in [false, true] {
            let h = CandidateFixture::new(behavior, "final-only").await;
            let initial = h.admission("source").await;
            let mut graph = initial.run_spec.graph.clone();
            graph.nodes[0].harvest_sanitization = Some(HarvestSanitizationMode::AttestedPrefixV1);
            let admitted = h
                .builder()
                .freeze(WorkflowAdmissionRequestV1 {
                    attempt_id: h.owner.attempt_id.clone(),
                    graph: Arc::new(graph),
                    requested_session_cwd: initial.run_spec.requested_session_cwd.clone(),
                    policy_invocation: Default::default(),
                    ledger_admission: initial.run_spec.ledger_admission.clone(),
                    r2f1b: None,
                })
                .await
                .unwrap();
            let admitted = if chain {
                h.builder()
                    .bind_local_chains(admitted, &h.run.progress.chains)
                    .unwrap()
            } else {
                admitted
            };
            let task = TaskId::parse(h.owner.execution_id.as_str()).unwrap();
            let events = h
                .drive_context(
                    admitted,
                    CancellationToken::new(),
                    None,
                    WorkflowRunContext {
                        task_id: Some(task.clone()),
                        harvest_audit_store: h.f.store.clone(),
                        ..Default::default()
                    },
                )
                .await;
            assert!(events.iter().all(Result::is_ok), "{events:?}");
            let records =
                h.f.store
                    .list_by_task_id(task.as_str(), None, 10)
                    .await
                    .unwrap();
            assert_eq!(records.len(), 1);
            let record = &records[0];
            let output = events
                .iter()
                .find_map(|event| match event {
                    Ok(WorkflowEvent::NodeFinished { output, .. }) => Some(output.clone()),
                    _ => None,
                })
                .unwrap();
            projections.push((
                output,
                record.raw.raw_body.clone(),
                record.raw.declared_capability.clone(),
                record.decision.decision,
                record.decision.reason.clone(),
            ));
        }
        assert_eq!(projections[0], projections[1], "{behavior}");
    }
}

#[tokio::test(start_paused = true)]
async fn round5_work_cutoff_and_heartbeat_parity() {
    use bridge_core::turn_liveness::{LivenessOverrideV1, TurnLivenessValueV1};
    for silence in [LivenessOverrideV1::Off, LivenessOverrideV1::Ms(5000)] {
        let mut projections = vec![];
        for chain in [false, true] {
            let h = CandidateFixture::new("parked", "final-only").await;
            let initial = h.admission("source").await;
            let mut graph = initial.run_spec.graph.clone();
            graph.controls.as_mut().unwrap().silence_cutoff = Some(silence);
            let admitted = h
                .builder()
                .freeze(WorkflowAdmissionRequestV1 {
                    attempt_id: h.owner.attempt_id.clone(),
                    graph: Arc::new(graph),
                    requested_session_cwd: initial.run_spec.requested_session_cwd.clone(),
                    policy_invocation: Default::default(),
                    ledger_admission: initial.run_spec.ledger_admission.clone(),
                    r2f1b: None,
                })
                .await
                .unwrap();
            let cutoff = admitted.run_spec.controls.effective_work_cutoff_ms();
            let admitted = if chain {
                h.builder()
                    .bind_local_chains(admitted, &h.run.progress.chains)
                    .unwrap()
            } else {
                admitted
            };
            let drive = h.drive_context(
                admitted,
                CancellationToken::new(),
                None,
                WorkflowRunContext {
                    observer_key: Some("parity-heartbeat".into()),
                    make_rich_sink: Some(Arc::new(ClockFactory(Arc::new(TestClock(
                        tokio::time::Instant::now(),
                    ))))),
                    ..Default::default()
                },
            );
            let advance = async {
                while h.f.providers[0].prompts.load(Ordering::SeqCst) == 0 {
                    tokio::task::yield_now().await;
                }
                tokio::time::advance(std::time::Duration::from_millis(cutoff + 1)).await;
            };
            let (events, ()) = tokio::join!(drive, advance);
            assert!(events.iter().all(Result::is_ok), "{events:?}");
            let turns = h.calls[0].turns.lock().unwrap();
            let liveness = turns.values().next().unwrap().liveness.clone();
            assert_eq!(liveness.observer_key.as_deref(), Some("parity-heartbeat"));
            assert_eq!(liveness.work_cutoff, TurnLivenessValueV1::Off);
            assert_eq!(
                liveness.silence,
                match silence {
                    LivenessOverrideV1::Off => TurnLivenessValueV1::Off,
                    LivenessOverrideV1::Ms(ms) => TurnLivenessValueV1::Ms(ms),
                }
            );
            let terminal = node_terminal(&events);
            projections.push((terminal.primary, terminal.cleanup.disposition, liveness));
            assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 0);
        }
        assert_eq!(projections[0], projections[1]);
    }
}

#[derive(Default)]
struct LifecycleObserver(Mutex<Vec<String>>);
impl Observer for LifecycleObserver {
    fn record(&self, event: &ObsEvent<'_>) {
        let value = match event {
            ObsEvent::NodeStarted { .. } => "node-start".into(),
            ObsEvent::NodeFinished { outcome, .. } => format!("node-finish:{outcome:?}"),
            ObsEvent::TurnStarted { ctx } => format!("turn-start:{}", ctx.attempt),
            ObsEvent::TurnFinished {
                outcome, ctx, ttft, ..
            } => {
                format!(
                    "turn-finish:{}:{outcome:?}:ttft={}",
                    ctx.attempt,
                    ttft.is_some()
                )
            }
            ObsEvent::UsageFinalized { usage, .. } => format!("usage:{usage:?}"),
            _ => return,
        };
        self.0.lock().unwrap().push(value);
    }
}

#[tokio::test]
async fn round5_lifecycle_observation_parity() {
    for preflight in [false, true] {
        let mut projections = vec![];
        for chain in [false, true] {
            let h = CandidateFixture::new("preflight-pass", "final-only").await;
            let mut snapshot = h.f.snapshot();
            snapshot.entries[0].preflight = preflight;
            h.registry.apply(snapshot).await.unwrap();
            let admitted = h.admission("source").await;
            let admitted = if chain {
                h.builder()
                    .bind_local_chains(admitted, &h.run.progress.chains)
                    .unwrap()
            } else {
                admitted
            };
            let observer = Arc::new(LifecycleObserver::default());
            let dispatch = observer.clone();
            let barrier: PromptDispatchBarrier = Arc::new(move || {
                let dispatch = dispatch.clone();
                Box::pin(async move {
                    dispatch.0.lock().unwrap().push("dispatch".into());
                })
            });
            let events = h
                .drive_context(
                    admitted,
                    CancellationToken::new(),
                    Some(barrier),
                    WorkflowRunContext {
                        observer: observer.clone(),
                        ..Default::default()
                    },
                )
                .await;
            assert_eq!(
                node_terminal(&events).primary,
                NodePrimaryDispositionV1::Completed
            );
            let observations = observer.0.lock().unwrap().clone();
            assert_eq!(observations[0], "node-start");
            assert_eq!(
                observations
                    .iter()
                    .filter(|value| value.starts_with("node-finish"))
                    .count(),
                1
            );
            projections.push(observations);
        }
        assert_eq!(projections[0], projections[1], "preflight={preflight}");
    }
}

// These run through the production CLI so its output, public checkpoint, and
// durable journal replay are checked against the same approved harvest body.
#[allow(dead_code)] // Also compiled as an integration fixture.
pub(crate) async fn round6_harvest_publication<F, Fut>(run: F, audit_failure: bool)
where
    F: Fn(Vec<String>, Arc<dyn AgentRegistry>, Arc<bridge_store::sqlite::SqliteStore>) -> Fut,
    Fut: std::future::Future<Output = std::result::Result<(), String>>,
{
    for chain in [false, true] {
        let h = CandidateFixture::new("attested", "final-only").await;
        let root = h.f.dir.path();
        let scope = root.join("scope");
        let mut config = format!("default='source'\n[store]\npath={:?}\n[registry]\nallowed_cmds=['test-cmd']\n[server]\n", root.join("tasks.sqlite"));
        for agent in ["source", "target", "last", "other"] {
            config += &format!("[[agents]]\nid='{agent}'\ncmd='test-cmd'\nmodel='{agent}'\ncwd={scope:?}\nsession_cwd={scope:?}\n");
        }
        if chain {
            config += "[[routing.chains]]\nid='harvest'\nsource={agent='source',model='source'}\ncauses=['quota_exhausted']\nsteps=[{agent='target',model='target'}]\n";
        }
        config += "[[workflows]]\nid='workflow'\n[[workflows.nodes]]\nid='node'\nagent='source'\nprompt_text='{{input}}'\ninputs=[]\nharvest_sanitization='attested_prefix_v1'\n";
        std::fs::write(root.join("config.toml"), config).unwrap();
        std::fs::write(root.join("input"), "---\ntask-type: freeform\n---\ninput").unwrap();
        let db = rusqlite::Connection::open(root.join("tasks.sqlite")).unwrap();
        if audit_failure {
            db.execute_batch("CREATE TRIGGER fail_harvest BEFORE INSERT ON harvest_raw_records_v1 BEGIN SELECT RAISE(ABORT,'audit fault'); END;").unwrap();
        }
        let result = run(
            vec![
                "workflow".into(),
                "--config".into(),
                root.join("config.toml").display().to_string(),
                "--input".into(),
                root.join("input").display().to_string(),
                "--out".into(),
                root.join("output").display().to_string(),
                "--no-heartbeat".into(),
            ],
            h.registry.clone(),
            h.f.store.clone(),
        )
        .await;
        assert_eq!(result.is_err(), audit_failure, "{chain}: {result:?}");
        let task = TaskId::parse(h.f.rows()[0].attribution.execution.clone().unwrap()).unwrap();
        let checkpoints = h.f.store.node_checkpoints(&task).await.unwrap();
        let replay = db
            .prepare("SELECT event_json FROM task_journal WHERE task_id=?1 ORDER BY seq")
            .unwrap()
            .query_map([task.as_str()], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|row| serde_json::from_str::<bridge_core::orch::OrchEvent>(&row.unwrap()).unwrap())
            .filter_map(|event| match event.kind {
                bridge_core::orch::OrchEventKind::NodeFinished { output, ok, .. } => {
                    Some((output, ok))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let progress =
            h.f.store
                .candidate_progress_v1(&task, &NodeId::parse("node").unwrap())
                .await
                .unwrap();
        if audit_failure {
            assert!(
                checkpoints.is_empty(),
                "audit failure published a public checkpoint: {checkpoints:?}"
            );
            assert!(
                replay.is_empty(),
                "audit failure published NodeFinished: {replay:?}"
            );
            assert!(progress.is_none_or(|p| p.terminal.is_none()));
            assert_ne!(
                std::fs::read_to_string(root.join("output")).unwrap_or_default(),
                "answer"
            );
        } else {
            assert_eq!(
                std::fs::read_to_string(root.join("output")).unwrap(),
                "answer"
            );
            // Ordinary fresh CLI runs do not allocate a candidate task/checkpoint.
            if !chain {
                assert!(checkpoints.is_empty());
                assert!(replay.is_empty());
                assert!(progress.is_none());
                continue;
            }
            assert_eq!(checkpoints.len(), 1);
            assert_eq!(
                checkpoints[0].1, "answer",
                "checkpoint must contain the harvested body"
            );
            assert_eq!(
                replay,
                [("answer".into(), true)],
                "journal must replay the harvested body once"
            );
            if let Some(progress) = progress {
                assert_eq!(progress.terminal.unwrap().output, "answer");
            }
        }
    }
}

#[tokio::test]
async fn round6_successor_accounting_failure_dominates_predecessor() {
    // The trigger fails B's pre-poll accounting update, not visit publication or
    // preparation settlement. A and its safe rejection must remain queryable.
    for direct in [false, true] {
        let mut h = CandidateFixture::new("reject", "final-only").await;
        h.f.store.ensure_usage_coverage_v1().await.unwrap();
        rusqlite::Connection::open(h.f.dir.path().join("tasks.sqlite.usage-v1.sqlite")).unwrap()
            .execute_batch("PRAGMA journal_mode=PERSIST; CREATE TRIGGER fail_target_pre_poll BEFORE UPDATE ON usage_attempts_v1 WHEN json_extract(NEW.record_json,'$.attribution.agent')='target' AND json_extract(NEW.record_json,'$.dispatched')=0 AND json_extract(NEW.record_json,'$.finalized')=0 BEGIN SELECT RAISE(ABORT,'accounting fault'); END;").unwrap();
        let task = TaskId::parse(h.owner.execution_id.as_str()).unwrap();
        if direct {
            let cancel = CancellationToken::new();
            let rejection = h.execute("source", &cancel).await.unwrap_err();
            h.run
                .transition(rejection, &cancel, |c| Ok(c.clone()))
                .unwrap();
            let failure = h.execute("target", &cancel).await.unwrap_err();
            assert!(
                matches!(failure.persistence, Some(BridgeError::StoreFailure)),
                "pre-poll accounting lost persistence tag: {failure:?}"
            );
            assert!(matches!(
                failure.surfaced_error(),
                BridgeError::StoreFailure
            ));
        } else {
            let admitted = h
                .builder()
                .bind_local_chains(h.admission("source").await, &h.run.progress.chains)
                .unwrap();
            let events = h.drive(admitted, CancellationToken::new(), None).await;
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e, Err(BridgeError::StoreFailure))),
                "accounting must outrank predecessor: {events:?}"
            );
        }
        let progress =
            h.f.store
                .candidate_progress_v1(&task, &NodeId::parse("node").unwrap())
                .await
                .unwrap()
                .unwrap();
        assert!(progress.visits[0].phase.dispatched());
        assert_original(
            &BridgeError::AgentFailure {
                diagnostic: progress.visits[0]
                    .rejection
                    .as_ref()
                    .unwrap()
                    .diagnostic
                    .clone(),
            },
            "reject",
        );
        assert_eq!(h.f.providers[0].prompts.load(Ordering::SeqCst), 1);
        assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 0);
        assert_eq!(h.f.providers[2].prompts.load(Ordering::SeqCst), 0);
        assert!(h.f.store.node_checkpoints(&task).await.unwrap().is_empty());
    }
    unpolled_case("setup").await;
}

async fn round6_unpolled_wording(behavior: &'static str) {
    let mut projections = vec![];
    for chain in [false, true] {
        let h = CandidateFixture::new(behavior, "final-only").await;
        let initial = h.admission("source").await;
        let mut graph = initial.run_spec.graph.clone();
        graph.nodes[0].retry = Some(bridge_workflow::graph::RetryPolicy {
            max_attempts: 2,
            backoff_ms: 1,
            backoff_cap_ms: None,
        });
        let admitted = h
            .builder()
            .freeze(WorkflowAdmissionRequestV1 {
                attempt_id: h.owner.attempt_id.clone(),
                graph: Arc::new(graph),
                requested_session_cwd: initial.run_spec.requested_session_cwd.clone(),
                policy_invocation: Default::default(),
                ledger_admission: initial.run_spec.ledger_admission.clone(),
                r2f1b: None,
            })
            .await
            .unwrap();
        let admitted = if chain {
            h.builder()
                .bind_local_chains(admitted, &h.run.progress.chains)
                .unwrap()
        } else {
            admitted
        };
        let events = h.drive(admitted, CancellationToken::new(), None).await;
        assert!(events.iter().all(Result::is_ok), "{events:?}");
        let output = events
            .iter()
            .find_map(|e| match e {
                Ok(WorkflowEvent::NodeFinished { output, .. }) => Some(output.clone()),
                _ => None,
            })
            .unwrap();
        if !chain {
            assert_eq!(
                output,
                if behavior == "setup" {
                    "[node node failed: configure ModelNotAvailable]"
                } else {
                    "[node node failed after 2 attempts: AgentTimedOut]"
                }
            );
        }
        projections.push(output);
        assert_eq!(h.f.providers[0].prompts.load(Ordering::SeqCst), 0);
        assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 0);
    }
    assert_eq!(
        projections[0], projections[1],
        "unpolled wording: {behavior}"
    );
}

#[tokio::test]
async fn round6_first_unpolled_failure_keeps_ordinary_wording() {
    round6_unpolled_wording("setup").await;
}

#[tokio::test]
async fn round6_unpolled_retry_exhaustion_keeps_ordinary_wording() {
    round6_unpolled_wording("unpolled-timeout").await;
}

#[tokio::test]
async fn round6_resolution_cancel_restores_ordinary_boundary() {
    let mut terminals = vec![];
    for chain in [false, true] {
        let h = CandidateFixture::new("final-only", "final-only").await;
        let admitted = h.admission("source").await;
        let admitted = if chain {
            h.builder()
                .bind_local_chains(admitted, &h.run.progress.chains)
                .unwrap()
        } else {
            admitted
        };
        let cancel = CancellationToken::new();
        *h.calls[0].resolution_cancel.lock().unwrap() = Some(cancel.clone());
        let observer = Arc::new(LifecycleObserver::default());
        let drive = h.drive_context(
            admitted,
            cancel.clone(),
            None,
            WorkflowRunContext {
                observer: observer.clone(),
                ..Default::default()
            },
        );
        let events = drive.await;
        assert!(events.iter().all(Result::is_ok), "{events:?}");
        assert_eq!(
            h.calls[0].setups.load(Ordering::SeqCst),
            usize::from(!chain),
            "ordinary resolution must still configure; candidates stop before configure"
        );
        let expected = if chain {
            vec!["node-start", "node-finish:Canceled"]
        } else {
            vec![
                "node-start",
                "turn-start:1",
                "turn-finish:1:Canceled:ttft=false",
                "usage:None",
                "node-finish:Canceled",
            ]
        };
        assert_eq!(*observer.0.lock().unwrap(), expected);
        let mut terminal = node_terminal(&events);
        terminal.cleanup.duration_ms = 0;
        terminals.push(terminal);
        assert_eq!(h.f.providers[0].prompts.load(Ordering::SeqCst), 0);
        assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 0);
    }
    assert_eq!(terminals[0], terminals[1]);
}

#[derive(Clone, Copy)]
#[allow(dead_code)] // Also compiled as an integration fixture.
pub(crate) enum TerminalPublicationShape {
    Single,
    Predecessor,
    Successor,
}

// Exercise terminal arbitration through the production CLI, including its
// negotiated-evidence resolver and the real checkpoint/journal transaction.
#[allow(dead_code)] // Also compiled as an integration fixture.
pub(crate) async fn round7_terminal_publication<F, Fut>(
    run: F,
    behavior: &'static str,
    chain: bool,
    shape: TerminalPublicationShape,
) -> (bool, String, Vec<(String, String)>)
where
    F: Fn(Vec<String>, Arc<dyn AgentRegistry>, Arc<bridge_store::sqlite::SqliteStore>) -> Fut,
    Fut: std::future::Future<Output = std::result::Result<(), String>>,
{
    let predecessor = matches!(shape, TerminalPublicationShape::Predecessor);
    let successor = matches!(shape, TerminalPublicationShape::Successor);
    let two_nodes = predecessor || successor;
    let h = CandidateFixture::new(behavior, "final-only").await;
    let root = h.f.dir.path();
    let scope = root.join("scope");
    let mut config = format!(
        "default='source'\n[store]\npath={:?}\n[registry]\nallowed_cmds=['test-cmd']\n[server]\n",
        root.join("tasks.sqlite")
    );
    for agent in ["source", "target", "last", "other"] {
        config += &format!("[[agents]]\nid='{agent}'\ncmd='test-cmd'\nmodel='{agent}'\ncwd={scope:?}\nsession_cwd={scope:?}\npreflight=false\n");
    }
    if chain {
        let successor = if two_nodes { "last" } else { "target" };
        config += &format!("[[routing.chains]]\nid='terminal'\nsource={{agent='source',model='source'}}\ncauses=['quota_exhausted']\nsteps=[{{agent='{successor}',model='{successor}'}}]\n");
    }
    config += "[[workflows]]\nid='workflow'\n";
    if predecessor {
        config += "[[workflows.nodes]]\nid='before'\nagent='target'\nprompt_text='{{input}}'\ninputs=[]\n";
    }
    let inputs = if predecessor { "['before']" } else { "[]" };
    config += &format!("[[workflows.nodes]]\nid='node'\nagent='source'\nprompt_text='{{{{input}}}}'\ninputs={inputs}\n");
    if successor {
        config += "[[workflows.nodes]]\nid='after'\nagent='target'\nprompt_text='{{node}}'\ninputs=['node']\n";
    }
    std::fs::write(root.join("config.toml"), config).unwrap();
    std::fs::write(root.join("input"), "---\ntask-type: freeform\n---\ninput").unwrap();
    let result = run(
        vec![
            "workflow".into(),
            "--config".into(),
            root.join("config.toml").display().to_string(),
            "--input".into(),
            root.join("input").display().to_string(),
            "--out".into(),
            root.join("output").display().to_string(),
            "--no-heartbeat".into(),
        ],
        h.registry.clone(),
        h.f.store.clone(),
    )
    .await;
    let success = two_nodes || behavior != "v1-missing";
    assert_eq!(
        result.is_ok(),
        success,
        "chain={chain}, {behavior}: {result:?}"
    );
    assert_eq!(h.f.providers[0].prompts.load(Ordering::SeqCst), 1);
    assert_eq!(
        h.f.providers[1].prompts.load(Ordering::SeqCst),
        usize::from(two_nodes)
    );
    assert_eq!(h.f.providers[2].prompts.load(Ordering::SeqCst), 0);
    let task = TaskId::parse(h.f.rows()[0].attribution.execution.clone().unwrap()).unwrap();
    let db = rusqlite::Connection::open(root.join("tasks.sqlite")).unwrap();
    assert_seam_publication(&db, chain, true);
    let output = std::fs::read_to_string(root.join("output")).unwrap_or_default();
    let terminals = db
        .prepare(
            "SELECT node_id, terminal_json FROM workflow_attempt_node_terminals ORDER BY node_id",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .map(|row| {
            let (node, json) = row.unwrap();
            let terminal: NodeTerminalV1 = serde_json::from_str(&json).unwrap();
            (node, format!("{:?}", terminal.primary))
        })
        .collect::<Vec<_>>();
    assert!(terminals.iter().all(|(_, primary)| primary == "Completed"));
    assert_eq!(terminals.len(), if two_nodes { 2 } else { 1 });
    let (outcome, reason): (String, String) = db
        .query_row(
            "SELECT outcome, json_extract(terminal_json,'$.terminal_reason') FROM workflow_attempt_summaries WHERE execution_id=?1",
            [task.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(outcome, if success { "completed" } else { "failed" });
    if !success {
        assert!(reason.contains("missing"), "{reason}");
    }
    if chain {
        assert_eq!(
            h.f.store.get(&task).await.unwrap().unwrap().status,
            if success {
                TaskRecordStatus::Completed
            } else {
                TaskRecordStatus::Failed
            }
        );
    }
    (result.is_ok(), output, terminals)
}

// Candidate durability describes NodeCompletion, independently of CLI/run success.
fn assert_seam_publication(db: &rusqlite::Connection, chain: bool, published: bool) {
    let checkpoints: Vec<(String, bool, Option<String>)> = db
        .prepare("SELECT output,ok,usage_json FROM task_node_checkpoints WHERE node_id='node'")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let progress: Vec<CandidateProgressV1> = db
        .prepare("SELECT progress_json FROM task_candidate_progress_v1 WHERE node_id='node'")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
        .collect();
    let journal: Vec<_> = db
        .prepare("SELECT event_json FROM task_journal ORDER BY seq")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| serde_json::from_str::<bridge_core::orch::OrchEvent>(&r.unwrap()).unwrap())
        .filter_map(|e| match e.kind {
            bridge_core::orch::OrchEventKind::NodeFinished {
                node,
                output,
                ok,
                usage,
                ..
            } if node == "node" => Some((output, ok, usage)),
            _ => None,
        })
        .collect();
    if chain && published {
        assert_eq!(
            checkpoints.len(),
            1,
            "candidate must be committed before consumption"
        );
        assert_eq!(journal.len(), 1);
        assert_eq!(progress.len(), 1);
        let terminal = progress[0].terminal.as_ref().unwrap();
        assert_eq!(terminal.output, "answer");
        assert!(terminal.ok);
        if let Some(usage) = terminal.usage.as_ref() {
            assert_eq!(usage.used, Some(7));
        }
        assert_eq!(checkpoints[0].0, terminal.output);
        assert_eq!(checkpoints[0].1, terminal.ok);
        assert_eq!(
            checkpoints[0]
                .2
                .as_ref()
                .map(|j| serde_json::from_str::<bridge_core::orch::UsageSnapshot>(j).unwrap()),
            terminal.usage
        );
        assert_eq!(
            journal[0],
            (terminal.output.clone(), terminal.ok, terminal.usage.clone())
        );
    } else {
        assert!(checkpoints.is_empty(), "{checkpoints:?}");
        assert!(journal.is_empty(), "{journal:?}");
        assert!(progress.iter().all(|p| p.terminal.is_none()));
    }
    if published {
        let history: Vec<String> = db
            .prepare(
                "SELECT terminal_json FROM workflow_attempt_node_terminals WHERE node_id='node'",
            )
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            history.len(),
            1,
            "one emitted completion must reach CLI history"
        );
        assert_eq!(
            serde_json::from_str::<NodeTerminalV1>(&history[0])
                .unwrap()
                .primary,
            NodePrimaryDispositionV1::Completed
        );
    }
}

#[allow(dead_code)] // Also compiled as an integration fixture.
pub(crate) async fn seam_publication_exits<F, Fut>(run: F, case: &'static str)
where
    F: Fn(
        Vec<String>,
        Arc<dyn AgentRegistry>,
        Arc<bridge_store::sqlite::SqliteStore>,
        CancellationToken,
    ) -> Fut,
    Fut: std::future::Future<Output = std::result::Result<(), String>>,
{
    let mut projections = Vec::new();
    for chain in [false, true] {
        let own_failure = matches!(case, "own-audit" | "own-transaction");
        if case == "own-transaction" && !chain {
            continue;
        }
        let target = match case {
            "setup" => "setup",
            "audit" => "attested",
            "cancel-running" => "parity-cancel",
            "observe" => "blocked-entry",
            _ => "final-only",
        };
        let h = CandidateFixture::new("v1-final", target).await;
        let root = h.f.dir.path();
        let path = root.join("tasks.sqlite");
        let db = rusqlite::Connection::open(&path).unwrap();
        let scope = root.join("scope");
        let mut config = format!("default='source'\n[store]\npath={path:?}\n[registry]\nallowed_cmds=['test-cmd']\n[server]\n");
        for agent in ["source", "target", "last", "other"] {
            config += &format!("[[agents]]\nid='{agent}'\ncmd='test-cmd'\nmodel='{agent}'\ncwd={scope:?}\nsession_cwd={scope:?}\npreflight=false\n");
        }
        if chain {
            config += "[[routing.chains]]\nid='seam'\nsource={agent='source',model='source'}\ncauses=['quota_exhausted']\nsteps=[{agent='last',model='last'}]\n";
        }
        config += "[[workflows]]\nid='workflow'\n[[workflows.nodes]]\nid='node'\nagent='source'\nprompt_text='{{input}}'\ninputs=[]\n";
        if case == "own-audit" {
            config += "harvest_sanitization='attested_prefix_v1'\n";
        }
        config += "[[workflows.nodes]]\nid='after'\nagent='target'\nprompt_text='{{node}}'\ninputs=['node']\n";
        if case == "audit" {
            config += "harvest_sanitization='attested_prefix_v1'\n";
        }
        let audit_fault = "CREATE TRIGGER fail_seam_audit BEFORE INSERT ON harvest_raw_records_v1 BEGIN SELECT RAISE(ABORT,'seam audit fault'); END;";
        if own_failure {
            db.execute_batch(if case == "own-audit" { audit_fault } else {
                // The journal insert follows the checkpoint insert in the SAME transaction.
                "CREATE TRIGGER fail_seam_publication BEFORE INSERT ON task_journal WHEN NEW.event_json LIKE '%node_finished%' BEGIN SELECT RAISE(ABORT,'seam publication fault'); END;"
            }).unwrap();
        }
        let cancel = CancellationToken::new();
        if !own_failure {
            let path = path.clone();
            let cancel = cancel.clone();
            *h.calls[1].entry_check.lock().unwrap() = Some(Box::new(move || {
                let db = rusqlite::Connection::open(&path).unwrap();
                if case == "observe" {
                    assert_seam_publication(&db, chain, true);
                }
                if case == "audit" {
                    db.execute_batch(audit_fault).unwrap();
                }
                if case == "cancel-before" {
                    cancel.cancel();
                }
            }));
        }
        std::fs::write(root.join("config.toml"), config).unwrap();
        std::fs::write(root.join("input"), "---\ntask-type: freeform\n---\ninput").unwrap();
        let command = run(
            vec![
                "workflow".into(),
                "--config".into(),
                root.join("config.toml").display().to_string(),
                "--input".into(),
                root.join("input").display().to_string(),
                "--out".into(),
                root.join("output").display().to_string(),
                "--no-heartbeat".into(),
            ],
            h.registry.clone(),
            h.f.store.clone(),
            cancel.clone(),
        );
        tokio::pin!(command);
        let result = tokio::time::timeout(std::time::Duration::from_secs(20), async {
            if matches!(case, "observe" | "cancel-running") {
                tokio::select! {
                    result = &mut command => panic!("command exited before dependent gate: {result:?}"),
                    () = h.f.providers[1].configure.0.notified() => {}
                }
                if case == "observe" {
                    assert_seam_publication(&db, chain, true);
                    assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 0);
                    h.f.providers[1].configure.1.notify_one();
                } else { cancel.cancel(); }
            }
            command.await
        }).await.expect("bounded deterministic dependent gate");
        assert_eq!(
            result.is_ok(),
            case == "observe",
            "{case}, chain={chain}: {result:?}"
        );
        assert_seam_publication(&db, chain, !own_failure);
        assert_eq!(
            h.f.providers[2].prompts.load(Ordering::SeqCst),
            0,
            "run arbitration cannot authorize fallback"
        );
        if own_failure || case == "cancel-before" || case == "setup" {
            assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 0);
        } else {
            assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 1);
            let inputs = h.f.providers[1].inputs.lock().unwrap();
            assert!(inputs[0][0].text.starts_with("answer"));
            if case != "audit" {
                assert_eq!(inputs[0][0].text, "answer");
            }
        }
        // Executor errors leave incomplete node evidence, so the existing history
        // store refuses summary finalization. Compare that ordinary behavior too.
        let (outcome, reason, counts): (Option<String>, Option<String>, Option<String>) = db.query_row(
            "SELECT outcome,json_extract(terminal_json,'$.terminal_reason'),json_extract(terminal_json,'$.node_counts') FROM workflow_attempt_summaries", [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
        let counts: Option<serde_json::Value> = counts.map(|j| serde_json::from_str(&j).unwrap());
        if matches!(case, "audit" | "own-audit" | "own-transaction") {
            assert!(outcome.is_none());
            assert!(counts.is_none());
        } else {
            assert_eq!(
                counts.as_ref().unwrap()["completed"],
                if case == "observe" { 2 } else { 1 }
            );
        }
        if case == "setup" {
            let json: String = db.query_row("SELECT terminal_json FROM workflow_attempt_node_terminals WHERE node_id='after'", [], |r| r.get(0)).unwrap();
            assert_eq!(
                serde_json::from_str::<NodeTerminalV1>(&json)
                    .unwrap()
                    .primary,
                NodePrimaryDispositionV1::Failed
            );
        }
        if chain {
            let task = TaskId::parse(h.f.rows()[0].attribution.execution.clone().unwrap()).unwrap();
            assert_eq!(
                h.f.store.get(&task).await.unwrap().unwrap().status,
                if outcome.as_deref() == Some("completed") {
                    TaskRecordStatus::Completed
                } else if outcome.as_deref() == Some("canceled") {
                    TaskRecordStatus::Canceled
                } else {
                    TaskRecordStatus::Failed
                }
            );
        }
        projections.push((
            outcome,
            reason,
            counts,
            std::fs::read_to_string(root.join("output")).unwrap_or_default(),
        ));
    }
    if projections.len() == 2 {
        assert_eq!(projections[0], projections[1], "{case}");
    }
}

#[tokio::test]
async fn direct_candidate_defers_v1_run_arbitration() {
    let mut h = CandidateFixture::new("v1-missing", "final-only").await;
    let admitted = h.admission("source").await;
    let telemetry = bridge_core::attempt_activity::AttemptTelemetrySinkFactory::new(
        h.owner.attempt_id.as_str(),
    );
    let result = WorkflowExecutor::new(h.registry.clone())
        .run_durable_candidate_v1(
            &mut h.run,
            &h.accounting,
            &admitted,
            vec![Part {
                text: "input".into(),
            }],
            &CancellationToken::new(),
            BackendObservers::new(
                Arc::new(bridge_core::diagnostics::NoopDiagnosticObserver::default()),
                Some(telemetry.make(&NodeId::parse("node").unwrap())),
            ),
        )
        .await
        .unwrap();
    let evidence = telemetry.evidence().single_turn().unwrap();
    assert_eq!(
        evidence.0,
        bridge_core::terminal_evidence::EvidenceCapability::V1
    );
    assert_eq!(
        evidence.1,
        bridge_core::terminal_evidence::EvidenceCompleteness::Missing
    );
    assert!(result.ok);
    assert_eq!(result.output, "answer");
}
