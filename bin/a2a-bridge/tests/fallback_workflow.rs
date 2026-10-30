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
    resolution_errors: Mutex<Vec<BridgeError>>,
    setup_errors: Mutex<Vec<BridgeError>>,
    cleanup_results: Mutex<Vec<Result<BackendCleanupDispositionV1>>>,
    prompt_error: Mutex<Option<BridgeError>>,
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
        if let Some(error) = self.1.setup_errors.lock().unwrap().pop() {
            return Err(error);
        }
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
        if self.0.behavior == "setup-budget" {
            return Err(BridgeError::BudgetRefused {
                refusal: Box::new(bridge_core::governance::BudgetRefusalV1::new(
                    bridge_core::governance::BudgetRefusalReasonV1::LimitExceeded,
                )),
            });
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
        if let Some(error) = self.1.prompt_error.lock().unwrap().take() {
            self.0.prompts.fetch_add(1, Ordering::SeqCst);
            return Err(error);
        }
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
            "missing"
                | "native-missing"
                | "attested"
                | "attested-usage"
                | "attested-invalid"
                | "unsupported-prefix"
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
            let usage = (self.0.behavior == "attested-usage").then(|| {
                Ok(Update::Usage(bridge_core::orch::UsageSnapshot {
                    used: Some(7),
                    size: Some(100),
                    ..Default::default()
                }))
            });
            return Ok(Box::pin(futures::stream::iter(usage.into_iter().chain([
                Ok(Update::FinalAnswer(body.into())),
                Ok(Update::Done {
                    stop_reason: "end_turn".into(),
                    prefix_attestation,
                }),
            ]))));
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
        if let Some(result) = self.1.cleanup_results.lock().unwrap().pop() {
            self.1.cleaned.lock().unwrap().push(session.clone());
            return result;
        }
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
                if let Some(error) = record.resolution_errors.lock().unwrap().pop() {
                    return Err(error);
                }
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

// Seed durable crash boundaries without running a provider: all recovery effects
// can therefore be asserted absolutely zero, including preparation and accounting.
async fn recovery_candidate_fixture(case: &str) -> (CandidateFixture, AdmittedWorkflowRunV1) {
    recovery_candidate_fixture_target(case, "success").await
}
async fn recovery_candidate_fixture_target(
    case: &str,
    target: &'static str,
) -> (CandidateFixture, AdmittedWorkflowRunV1) {
    let mut h = CandidateFixture::new("reject", target).await;
    if case == "precedence" {
        h.run.progress.chains[0].steps.truncate(1);
    }
    let mut initial = h.admission("source").await;
    if case == "checkpoint-dependencies"
        || case == "completed-dependent"
        || case.starts_with("mixed")
    {
        let mut graph = initial.run_spec.graph.clone();
        let mut upstream = graph.nodes[0].clone();
        upstream.id = NodeId::parse("upstream").unwrap();
        if case == "checkpoint-dependencies" {
            graph.nodes[0].inputs.push(upstream.id.clone());
        } else {
            upstream.id = NodeId::parse("after").unwrap();
            upstream.inputs = vec![graph.nodes[0].id.clone()];
            if case == "completed-dependent" {
                upstream.agent = AgentId::parse("target").unwrap();
                upstream.prompt_template = "{{node}}\n{{workflow.costs}}".into();
                // A seeded node must never be harvested again, even if its frozen
                // policy requires attestation that a new provider did not supply.
                graph.nodes[0].harvest_sanitization =
                    Some(bridge_core::attestation::HarvestSanitizationMode::AttestedPrefixV1);
            }
        }
        graph.nodes.push(upstream);
        initial = h
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
    }
    let admitted = h
        .builder()
        .bind_local_chains(initial, &h.run.progress.chains)
        .unwrap();
    let task = &TaskId::parse(h.owner.execution_id.as_str()).unwrap();
    let node = NodeId::parse("node").unwrap();
    let mut progress = h.run.progress.clone();
    if case == "missing" {
        return (h, admitted);
    }
    h.f.store
        .put_candidate_progress_v1(task, &node, None, &progress)
        .await
        .unwrap();
    if case == "empty" {
        return (h, admitted);
    }
    let rejection: CandidateRejectionV1 = serde_json::from_value(serde_json::json!({
        "diagnostic": {"schema_version":1, "failed_phase":"prompt_start", "class":"quota_exhausted",
            "disposition":"fatal", "code":"fixture.rejected", "summary":"original rejection",
            "causes":[], "stderr_observed":false, "stderr_line_count":0,
            "prompt_may_have_been_accepted":false},
        "non_acceptance":true, "no_effects":true, "cleanup_complete":true,
        "persistence_error":null, "canceled":false
    }))
    .unwrap();
    let completed = case.starts_with("completed")
        || case.starts_with("checkpoint")
        || case.starts_with("terminal")
        || case.starts_with("mixed");
    let successor = match case {
        "preparing" => Some(CandidatePhaseV1::Preparing),
        "prepared" => Some(CandidatePhaseV1::Prepared),
        "fenced" => Some(CandidatePhaseV1::PollFenced),
        "polled" => Some(CandidatePhaseV1::Polled),
        _ => None,
    };
    for position in 0..=usize::from(successor.is_some()) {
        let candidate = if position == 0 {
            &progress.source
        } else {
            &progress.chains[0].steps[0]
        };
        let id = AttemptId::mint().unwrap().as_str().to_owned();
        h.f.store
            .start_usage_attempt_v1(&UsageAttemptV1 {
                id: id.clone(),
                attribution: bridge_core::accounting::UsageAttributionV1 {
                    execution: Some(h.owner.execution_id.as_str().into()),
                    run_attempt: Some(h.owner.attempt_id.as_str().into()),
                    workflow: Some("workflow".into()),
                    node: Some("node".into()),
                    agent: candidate.agent.as_str().into(),
                    effective_model: Some(candidate.model.clone()),
                    effective_mode: candidate.posture.mode.clone(),
                    attempt_kind: "node".into(),
                    ..Default::default()
                },
                started_ms: 1,
                utc_day: 0,
                dispatched: false,
                finalized: false,
                usage: Default::default(),
                cost_invalid: false,
            })
            .await
            .unwrap();
        for phase in [
            CandidatePhaseV1::Preparing,
            CandidatePhaseV1::Prepared,
            CandidatePhaseV1::PollFenced,
            CandidatePhaseV1::Polled,
        ] {
            let old = progress.clone();
            if phase == CandidatePhaseV1::Preparing {
                progress.visits.push(CandidateVisitV1 {
                    position,
                    accounting_id: id.clone(),
                    owner: h.owner.clone(),
                    cold_context: ContextId::parse(format!("cold-{position}")).unwrap(),
                    phase,
                    rejection: None,
                });
            } else {
                progress.visits.last_mut().unwrap().phase = phase;
            }
            h.f.store
                .put_candidate_progress_v1(task, &node, Some(&old), &progress)
                .await
                .unwrap();
            if phase == CandidatePhaseV1::Polled {
                h.f.store
                    .update_usage_attempt_v1(&id, true, None, true)
                    .await
                    .unwrap();
            }
            if position == 1 && Some(phase) == successor {
                break;
            }
        }
        if position == 0 && !completed {
            let old = progress.clone();
            progress.selected_chain = Some(0);
            progress.visits[0].rejection = Some(rejection.clone());
            h.f.store
                .put_candidate_progress_v1(task, &node, Some(&old), &progress)
                .await
                .unwrap();
        }
    }
    if completed {
        let old = progress.clone();
        progress.terminal = Some(CandidateOutputV1 {
            output: "exact checkpoint output".into(),
            ok: true,
            usage: Some(bridge_core::orch::UsageSnapshot {
                used: Some(7),
                at_ms: 3,
                ..Default::default()
            }),
        });
        let terminal = NodeTerminalV1 {
            schema_version: 1,
            primary: NodePrimaryDispositionV1::Completed,
            cleanup: NodeCleanupV1 {
                disposition: NodeCleanupDispositionV1::Complete,
                duration_ms: 0,
            },
            cause: None,
            prompt_may_have_been_accepted: true,
            degraded_ancestry: false,
            policy_trigger_id: None,
        };
        h.f.store
            .complete_candidate_v2(
                task,
                &node,
                &old,
                &progress,
                &OperationId::parse("op-recovery").unwrap(),
                3,
                Some(std::str::from_utf8(&terminal.encode_canonical().unwrap()).unwrap()),
                None,
            )
            .await
            .unwrap();
    }
    let db = rusqlite::Connection::open(h.f.dir.path().join("tasks.sqlite")).unwrap();
    let usage =
        rusqlite::Connection::open(h.f.dir.path().join("tasks.sqlite.usage-v1.sqlite")).unwrap();
    if case.starts_with("mixed") {
        let after = NodeId::parse("after").unwrap();
        let mut pending = h.run.progress.clone();
        let mut row = h.f.rows()[0].clone();
        row.id = AttemptId::mint().unwrap().as_str().into();
        row.attribution.node = Some(after.as_str().into());
        row.dispatched = false;
        row.finalized = false;
        h.f.store.start_usage_attempt_v1(&row).await.unwrap();
        h.f.store
            .put_candidate_progress_v1(task, &after, None, &pending)
            .await
            .unwrap();
        for phase in [
            CandidatePhaseV1::Preparing,
            CandidatePhaseV1::Prepared,
            CandidatePhaseV1::PollFenced,
            CandidatePhaseV1::Polled,
        ] {
            let old = pending.clone();
            if phase == CandidatePhaseV1::Preparing {
                pending.visits.push(CandidateVisitV1 {
                    position: 0,
                    accounting_id: row.id.clone(),
                    owner: h.owner.clone(),
                    cold_context: ContextId::parse("cold-after").unwrap(),
                    phase,
                    rejection: None,
                });
            } else {
                pending.visits[0].phase = phase;
            }
            h.f.store
                .put_candidate_progress_v1(task, &after, Some(&old), &pending)
                .await
                .unwrap();
        }
        h.f.store
            .update_usage_attempt_v1(&row.id, true, None, true)
            .await
            .unwrap();
        let old = pending.clone();
        pending.selected_chain = Some(0);
        pending.visits[0].rejection = Some(rejection);
        h.f.store
            .put_candidate_progress_v1(task, &after, Some(&old), &pending)
            .await
            .unwrap();
        if case == "mixed-conflict" {
            db.execute("UPDATE task_node_checkpoints SET output='conflict'", [])
                .unwrap();
        }
    }
    match case {
        "accounting-missing" => {
            usage.execute("DELETE FROM usage_attempts_v1", []).unwrap();
        }
        "accounting-conflict" => {
            usage.execute("UPDATE usage_attempts_v1 SET record_json=json_set(record_json,'$.attribution.agent','other')", []).unwrap();
        }
        "accounting-dispatch" => {
            usage.execute("UPDATE usage_attempts_v1 SET record_json=json_set(record_json,'$.dispatched',json('false'))", []).unwrap();
        }
        "lineage-missing" => {
            db.execute("DELETE FROM task_attempt_locators", []).unwrap();
        }
        "owner-conflict" => {
            progress.visits[0].owner.attempt_id = AttemptId::mint().unwrap();
        }
        "schema" => progress.schema_version = 99,
        "retries" => progress.retries_consumed = 1,
        "chain-drift" => progress.chains[0].id.push_str("-drift"),
        "scope-drift" => {
            let scope = h.f.dir.path().join("scope");
            std::fs::rename(&scope, h.f.dir.path().join("old-scope")).unwrap();
            std::fs::create_dir(scope).unwrap();
        }
        "authority-drift" => {
            let mut snapshot = h.f.snapshot();
            snapshot.entries[0].args.push("drift".into());
            h.registry.apply(snapshot).await.unwrap();
        }
        "rejection-persistence" => {
            progress.visits[0]
                .rejection
                .as_mut()
                .unwrap()
                .persistence_error = Some(BoundedReasonV1::parse("failed persistence").unwrap());
        }
        "rejection-accepted" => {
            progress.visits[0]
                .rejection
                .as_mut()
                .unwrap()
                .non_acceptance = false
        }
        "rejection-effects" => progress.visits[0].rejection.as_mut().unwrap().no_effects = false,
        "rejection-cleanup" => {
            progress.visits[0]
                .rejection
                .as_mut()
                .unwrap()
                .cleanup_complete = false
        }
        "rejection-canceled" => progress.visits[0].rejection.as_mut().unwrap().canceled = true,
        "completed-missing-progress" => {
            db.execute("DELETE FROM task_candidate_progress_v1", [])
                .unwrap();
        }
        "checkpoint-missing" => {
            db.execute("DELETE FROM task_node_checkpoints", []).unwrap();
        }
        "checkpoint-output" => {
            db.execute("UPDATE task_node_checkpoints SET output='conflict'", [])
                .unwrap();
        }
        "checkpoint-ok" => {
            db.execute("UPDATE task_node_checkpoints SET ok=0", [])
                .unwrap();
        }
        "checkpoint-unreadable-usage" => {
            db.execute("UPDATE task_node_checkpoints SET usage_json='{'", [])
                .unwrap();
        }
        "checkpoint-usage" => {
            db.execute("UPDATE task_node_checkpoints SET usage_json=NULL", [])
                .unwrap();
        }
        "checkpoint-unknown" => {
            db.execute("UPDATE task_node_checkpoints SET node_id='foreign'", [])
                .unwrap();
        }
        "terminal-missing" => {
            db.execute("UPDATE task_node_checkpoints SET terminal_json=NULL", [])
                .unwrap();
        }
        "terminal-corrupt" => {
            db.execute("UPDATE task_node_checkpoints SET terminal_json='{}'", [])
                .unwrap();
        }
        "terminal-disposition" => {
            // Canonical, internally valid Failed terminal paired with ok=true.
            let mut terminal: NodeTerminalV1 = NodeTerminalV1::decode_canonical(
                &db.query_row("SELECT terminal_json FROM task_node_checkpoints", [], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap()
                .into_bytes(),
            )
            .unwrap();
            terminal.primary = NodePrimaryDispositionV1::Failed;
            terminal.cause = Some(NodeCauseV1::from_bridge_error(&BridgeError::AgentTimedOut));
            db.execute(
                "UPDATE task_node_checkpoints SET terminal_json=?1",
                [String::from_utf8(terminal.encode_canonical().unwrap()).unwrap()],
            )
            .unwrap();
        }
        "policy-trigger" => {
            let trigger = PolicyTriggerV1 {
                schema_version: 1,
                id: ControlEventIdV1::for_attempt(&h.owner.attempt_id, 0),
                node: PolicyNodeRefV1::from_node_id(0, "node"),
                policy: FanOutPolicyNameV1::FailFast,
                grace_ms: None,
            };
            db.execute(
                "UPDATE tasks SET policy_trigger_json=?1",
                [String::from_utf8(trigger.encode_canonical().unwrap()).unwrap()],
            )
            .unwrap();
        }
        "pending-outcome" => {
            db.execute("UPDATE tasks SET workflow_outcome='interrupted'", [])
                .unwrap();
        }
        _ => {}
    }
    // Fault injection bypasses publication exclusively to model torn/corrupt storage.
    db.execute(
        "UPDATE task_candidate_progress_v1 SET progress_json=?1 WHERE node_id='node'",
        [serde_json::to_string(&progress).unwrap()],
    )
    .unwrap();
    if case == "corrupt" {
        db.execute(
            "UPDATE task_candidate_progress_v1 SET progress_json='{}'",
            [],
        )
        .unwrap();
    }
    (h, admitted)
}

impl CandidateFixture {
    fn reopen_recovery(self) -> Self {
        let Self {
            f,
            owner,
            registry,
            accounting,
            run,
            calls,
        } = self;
        let Fixture {
            dir,
            store,
            coordinator,
            providers,
            spawns,
            dispatch_gate,
        } = f;
        let chains = run.progress.chains.clone();
        let source = run.progress.source.clone();
        drop((run, accounting, coordinator, store));
        let store = Arc::new(
            bridge_store::sqlite::SqliteStore::open_shared_history(
                &dir.path().join("tasks.sqlite"),
            )
            .unwrap(),
        );
        let coordinator = Arc::new(bridge_coordinator::Coordinator::new(
            Arc::new(bridge_coordinator::session_manager::SessionManager::new(
                registry.clone(),
                std::time::Duration::from_secs(60),
            )),
            None,
            Arc::new(Default::default()),
            store.clone(),
            store.clone(),
            Arc::new(bridge_policy::permission::AutoPolicy),
            registry.clone(),
            Arc::new(bridge_coordinator::clock::SystemClock),
            None,
            None,
            Arc::new(bridge_observ::NoopObserver),
            3,
        ));
        let accounting = WorkflowAccountingInvocationV1::new(
            AccountingV1::new(store.clone()),
            owner.clone(),
            WorkflowId::parse("workflow").unwrap(),
        );
        let run = WorkflowCandidateRunV1::new(
            store.clone(),
            TaskId::parse(owner.execution_id.as_str()).unwrap(),
            NodeId::parse("node").unwrap(),
            bridge_core::fallback_runtime::FrozenFallbackInvocationV1::freeze(chains, source)
                .unwrap(),
        );
        Self {
            f: Fixture {
                dir,
                store,
                coordinator,
                providers,
                spawns,
                dispatch_gate,
            },
            owner,
            registry,
            accounting,
            run,
            calls,
        }
    }
}

// C2c2d5: reopen the durable store used by each recovery consumer.
async fn completed_recovery_deps(
    h: &CandidateFixture,
    admitted: &AdmittedWorkflowRunV1,
    batch: bool,
) -> bridge_coordinator::detached::DetachedDeps {
    use bridge_coordinator::detached::*;
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
    use bridge_core::workflow_history::*;
    h.f.store
        .reserve_v2(
            &structured_history_reservation_v1(
                AttemptReservation {
                    identity: h.owner.clone(),
                    task_id: Some(task),
                    workflow: "workflow".into(),
                    task_class: "workflow".into(),
                    surface: ExecutionSurface::ServedTask,
                    policy: "r2f0a".into(),
                    workload_fingerprint: admitted.run_spec.workload_fingerprint.clone(),
                    started_ms: 1,
                    workload_fingerprint_complete: true,
                    prompt_acceptance: "not_dispatched".into(),
                    pinned: false,
                },
                &admitted.run_spec,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    DetachedDeps {
        task_store: h.f.store.clone(),
        executor: Some(Arc::new(WorkflowExecutor::new(h.registry.clone()))),
        workflows: Arc::new(
            [(
                admitted.run_spec.graph.id.clone(),
                Arc::new(admitted.run_spec.graph.clone()),
            )]
            .into(),
        ),
        workflow_cancels: Default::default(),
        progress_hubs: Default::default(),
        clock: Arc::new(bridge_coordinator::clock::SystemClock),
        observer: Arc::new(bridge_observ::NoopObserver),
        workflow_history: Some(Ok(h.f.store.clone())),
        workflow_admission: Some(h.builder()),
    }
}

async fn consume_completed_recovery(
    h: &CandidateFixture,
    deps: &bridge_coordinator::detached::DetachedDeps,
    batch: bool,
) -> TaskRecord {
    use bridge_coordinator::{batch::*, detached::*};
    let task = TaskId::parse(h.owner.execution_id.as_str()).unwrap();
    if !batch {
        assert!(reconcile_terminal_checkpoints(deps).await);
    }
    use bridge_core::workflow_history::WorkflowHistoryStore;
    let excluded = deps
        .task_store
        .terminal_attempts_with_telemetry_markers()
        .await
        .unwrap();
    h.f.store
        .interrupt_active_excluding(4, &excluded)
        .await
        .unwrap();
    if batch {
        resume_batches_with_accounting(
            &BatchDeps {
                detached: deps.clone(),
                runtime: BatchRuntime::new(1, 1, Arc::new(bridge_observ::NoopObserver)),
                allowed_cwd_root: None,
            },
            3,
            Some(AccountingV1::new(h.f.store.clone())),
        )
        .await;
    } else {
        let record = deps.task_store.get(&task).await.unwrap().unwrap();
        if record.status == TaskRecordStatus::Working {
            resume_one_working_task_with_accounting(
                deps,
                &record,
                3,
                Some(AccountingV1::new(h.f.store.clone())),
            )
            .await;
        }
    }
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let record = deps.task_store.get(&task).await.unwrap().unwrap();
            if record.status != TaskRecordStatus::Working {
                return record;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn workflow_completed_recovery_terminal_reopen() {
    let mut failures = Vec::new();
    for batch in [false, true] {
        let (h, admitted) = recovery_candidate_fixture("completed").await;
        let h = h.reopen_recovery();
        let deps = completed_recovery_deps(&h, &admitted, batch).await;
        let task = TaskId::parse(h.owner.execution_id.as_str()).unwrap();
        let before = joint_receipt(&h);
        let rows = h.f.rows();
        let locator = deps.task_store.get_attempt_locator(&task).await.unwrap();
        let record = consume_completed_recovery(&h, &deps, batch).await;
        if record.status != TaskRecordStatus::Completed {
            failures.push(format!(
                "batch={batch}: {:?}: {:?}",
                record.status, record.error
            ));
            continue;
        }
        assert_eq!(record.result.as_deref(), Some("exact checkpoint output"));
        assert_eq!(record.resume_attempts, 0);
        assert_eq!(
            deps.task_store
                .get_attempt_locator(&task)
                .await
                .unwrap()
                .map(|l| l.identity),
            locator.map(|l| l.identity)
        );
        assert_eq!(joint_receipt(&h), before);
        assert_eq!(h.f.rows(), rows);
        assert!(h.f.spawns.lock().unwrap().is_empty());
        assert!(h
            .f
            .providers
            .iter()
            .all(|p| p.setup.load(Ordering::SeqCst) == 0 && p.prompts.load(Ordering::SeqCst) == 0));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn workflow_completed_recovery_nonterminal_reopen() {
    let mut failures = Vec::new();
    for batch in [false, true] {
        let (h, admitted) = recovery_candidate_fixture("completed-dependent").await;
        let h = h.reopen_recovery();
        let deps = completed_recovery_deps(&h, &admitted, batch).await;
        let task = TaskId::parse(h.owner.execution_id.as_str()).unwrap();
        let before = joint_receipt(&h);
        let rows = h.f.rows();
        // A replay of either publication or harvest must fail observably.
        rusqlite::Connection::open(h.f.dir.path().join("tasks.sqlite")).unwrap().execute_batch(
            "CREATE TRIGGER no_seed_update BEFORE UPDATE ON task_node_checkpoints WHEN OLD.node_id='node' BEGIN SELECT RAISE(ABORT,'seed checkpoint replay'); END;
             CREATE TRIGGER no_seed_insert BEFORE INSERT ON task_node_checkpoints WHEN NEW.node_id='node' BEGIN SELECT RAISE(ABORT,'seed checkpoint replay'); END;
             CREATE TRIGGER no_seed_harvest BEFORE INSERT ON harvest_raw_records_v1 BEGIN SELECT RAISE(ABORT,'seed harvest replay'); END;"
        ).unwrap();
        let record = consume_completed_recovery(&h, &deps, batch).await;
        if record.status != TaskRecordStatus::Completed {
            failures.push(format!(
                "batch={batch}: {:?}: {:?}",
                record.status, record.error
            ));
            continue;
        }
        assert_eq!(record.result.as_deref(), Some("answer"));
        assert_eq!(record.resume_attempts, 1);
        assert_eq!(joint_receipt(&h), before);
        let after_rows = h.f.rows();
        assert_eq!(after_rows.len(), rows.len() + 1);
        assert_eq!(
            after_rows.iter().find(|r| r.id == rows[0].id),
            Some(&rows[0])
        );
        assert_eq!(
            after_rows
                .iter()
                .filter(|r| r.attribution.node.as_deref() == Some("node"))
                .count(),
            1
        );
        let input = format!("{:?}", h.f.providers[1].inputs.lock().unwrap());
        assert!(input.contains("exact checkpoint output"), "{input}");
        assert!(input.contains("| node | 7 |"), "{input}");
        assert_eq!(*h.f.spawns.lock().unwrap(), vec!["target"]);
        assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 1);
        assert_eq!(
            deps.task_store.node_checkpoints(&task).await.unwrap().len(),
            2
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[derive(Clone, Default)]
struct RecoveryTrace(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for RecoveryTrace {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn workflow_completed_recovery_reconciliation_classifies_independently() {
    use bridge_coordinator::detached::reconcile_terminal_checkpoints;
    for (case, expected) in [
        ("completed", "Ok(Completed)"),
        ("continuation", "Ok(Continuation)"),
        ("mixed", "Ok(Mixed)"),
        (
            "checkpoint-output",
            "candidate checked evidence unavailable",
        ),
    ] {
        let (h, admitted) = recovery_candidate_fixture(case).await;
        let deps = completed_recovery_deps(&h, &admitted, false).await;
        let trace = RecoveryTrace::default();
        let writer = trace.clone();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .with_writer(move || writer.clone())
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        assert!(reconcile_terminal_checkpoints(&deps).await);
        drop(guard);
        let logs = String::from_utf8(trace.0.lock().unwrap().clone()).unwrap();
        // This assertion precedes both consumers and fails if reconciliation
        // merely leaves the task Working without classifying its evidence.
        assert!(logs.contains(expected), "{case}: {logs}");
        assert!(h.f.spawns.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn workflow_completed_recovery_refuses_unreadable_completion() {
    for batch in [false, true] {
        let (h, admitted) = recovery_candidate_fixture("checkpoint-unreadable-usage").await;
        let h = h.reopen_recovery();
        let deps = completed_recovery_deps(&h, &admitted, batch).await;
        let before = joint_receipt(&h);
        let rows = h.f.rows();
        let record = consume_completed_recovery(&h, &deps, batch).await;
        assert_eq!(record.status, TaskRecordStatus::Interrupted);
        assert!(record
            .error
            .unwrap()
            .contains("candidate checkpoints unavailable"));
        assert_eq!(record.resume_attempts, 0);
        assert_eq!(joint_receipt(&h), before);
        assert_eq!(h.f.rows(), rows);
        assert!(h.f.spawns.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn workflow_completed_recovery_mixed_refuses_without_effects() {
    check_candidate_recovery_cases(
        &[
            ("mixed", "(mixed)"),
            ("mixed-conflict", "candidate checked evidence unavailable"),
        ],
        false,
    )
    .await;
}

async fn check_candidate_recovery_cases(cases: &[(&str, &str)], persistence_failure: bool) {
    use bridge_coordinator::{batch::*, detached::*};
    let mut mismatches = Vec::new();
    // Reconciliation is exercised independently, so terminal shortcuts cannot
    // hide an unchecked path even when a later consumer would also refuse it.
    for consumer in ["detached", "batch", "reconciliation"] {
        for &(case, expected) in cases {
            let (h, admitted) = recovery_candidate_fixture(case).await;
            let task = TaskId::parse(h.owner.execution_id.as_str()).unwrap();
            let bid = BatchId::parse("batch-fixture").unwrap();
            if consumer == "batch" {
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
            let db = rusqlite::Connection::open(h.f.dir.path().join("tasks.sqlite")).unwrap();
            db.execute(
                "UPDATE tasks SET workflow_spec_json=?1,batch_id=?2,item_id=?3 WHERE id=?4",
                rusqlite::params![
                    String::from_utf8(admitted.run_spec.encode_snapshot_v2().unwrap()).unwrap(),
                    (consumer == "batch").then(|| bid.as_str()),
                    (consumer == "batch").then_some("one"),
                    task.as_str()
                ],
            )
            .unwrap();
            if persistence_failure {
                db.execute_batch("CREATE TRIGGER refuse_interruption BEFORE UPDATE OF status ON tasks WHEN NEW.status='interrupted' BEGIN SELECT RAISE(ABORT,'interruption fault'); END;").unwrap();
            }
            let record = h.f.store.get(&task).await.unwrap().unwrap();
            let locator = h.f.store.get_attempt_locator(&task).await.unwrap();
            let rows = if case == "missing" {
                vec![]
            } else {
                h.f.rows()
            };
            let checkpoints = h.f.store.node_checkpoints(&task).await.unwrap();
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
            match consumer {
                "batch" => {
                    run_admission(
                        BatchDeps {
                            detached: deps.clone(),
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
                    .await
                }
                "detached" => resume_one_working_task(&deps, &record, 3).await,
                _ => {
                    assert!(reconcile_terminal_checkpoints(&deps).await);
                    let after = h.f.store.get(&task).await.unwrap().unwrap();
                    assert_eq!(after.status, TaskRecordStatus::Working);
                    assert!(after.result.is_none());
                    // Reconciliation validates but leaves chain-bearing snapshots
                    // for the existing attempt sweep and consumer refusal ordering.
                    resume_one_working_task(&deps, &record, 3).await;
                }
            }
            let record = h.f.store.get(&task).await.unwrap().unwrap();
            if persistence_failure {
                assert_eq!(
                    record.status,
                    TaskRecordStatus::Working,
                    "{consumer}/{case}"
                );
                assert!(record.error.is_none());
                assert!(h
                    .f
                    .store
                    .workflow_task_evidence(&task)
                    .await
                    .unwrap()
                    .workflow_outcome
                    .is_none());
                assert!(h
                    .f
                    .store
                    .pending_terminal_projection(&task)
                    .await
                    .unwrap()
                    .is_none());
                assert_eq!(
                    db.query_row(
                        "SELECT last_event_seq FROM tasks WHERE id=?1",
                        [task.as_str()],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                    if checkpoints.is_empty() { 0 } else { 1 }
                );
            } else if record.status != TaskRecordStatus::Interrupted
                || !record
                    .error
                    .as_deref()
                    .is_some_and(|reason| reason.contains(expected))
            {
                mismatches.push(format!(
                    "{consumer}/{case}: {:?}, {:?}; expected {expected}",
                    record.status, record.error
                ));
            }
            assert_eq!(record.resume_attempts, 0, "{consumer}/{case}");
            assert!(record.result.is_none());
            assert_eq!(h.f.store.get_attempt_locator(&task).await.unwrap(), locator);
            assert_eq!(
                h.f.store.node_checkpoints(&task).await.unwrap(),
                checkpoints
            );
            if case != "missing" {
                assert_eq!(h.f.rows(), rows);
            }
            assert!(deps.progress_hubs.lock().await.is_empty());
            assert!(deps.workflow_cancels.lock().await.is_empty());
            assert!(h.f.spawns.lock().unwrap().is_empty());
            assert!(h
                .f
                .providers
                .iter()
                .all(|p| p.setup.load(Ordering::SeqCst) == 0
                    && p.prompts.load(Ordering::SeqCst) == 0));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[tokio::test]
async fn workflow_checked_recovery_refuses_continuation() {
    check_candidate_recovery_cases(&[("continuation", "(continuation)")], false).await;
}

#[tokio::test]
async fn workflow_checked_recovery_refuses_unsafe_and_drifted_evidence() {
    check_candidate_recovery_cases(
        &[
            ("missing", "candidate progress missing"),
            ("empty", "candidate state or frozen scope invalid"),
            ("preparing", "candidate state or frozen scope invalid"),
            ("prepared", "candidate state or frozen scope invalid"),
            ("fenced", "candidate state or frozen scope invalid"),
            ("polled", "candidate state or frozen scope invalid"),
            (
                "accounting-missing",
                "candidate checked evidence unavailable",
            ),
            (
                "accounting-conflict",
                "candidate checked evidence unavailable",
            ),
            (
                "accounting-dispatch",
                "candidate checked evidence unavailable",
            ),
            ("owner-conflict", "candidate checked evidence unavailable"),
            ("lineage-missing", "candidate lineage missing"),
            ("schema", "candidate checked evidence unavailable"),
            ("retries", "candidate checked evidence unavailable"),
            ("corrupt", "candidate checked evidence unavailable"),
            ("chain-drift", "candidate state or frozen scope invalid"),
            ("scope-drift", "candidate state or frozen scope invalid"),
            ("authority-drift", "candidate source authority drift"),
            (
                "rejection-persistence",
                "candidate state or frozen scope invalid",
            ),
            (
                "rejection-accepted",
                "candidate state or frozen scope invalid",
            ),
            (
                "rejection-effects",
                "candidate state or frozen scope invalid",
            ),
            (
                "rejection-cleanup",
                "candidate state or frozen scope invalid",
            ),
            (
                "rejection-canceled",
                "candidate state or frozen scope invalid",
            ),
            ("policy-trigger", "committed V2 policy trigger"),
            (
                "pending-outcome",
                "V2 terminal projection is already pending",
            ),
        ],
        false,
    )
    .await;
}

#[tokio::test]
async fn workflow_checked_recovery_validates_terminal_checkpoints_before_shortcuts() {
    check_candidate_recovery_cases(
        &[
            (
                "checkpoint-dependencies",
                "candidate checkpoints are not closed under inputs",
            ),
            (
                "checkpoint-missing",
                "candidate checked evidence unavailable",
            ),
            (
                "checkpoint-output",
                "candidate checked evidence unavailable",
            ),
            ("checkpoint-ok", "candidate checkpoint disposition mismatch"),
            ("checkpoint-usage", "candidate checked evidence unavailable"),
            (
                "checkpoint-unknown",
                "candidate checkpoint references unknown node",
            ),
            ("completed-missing-progress", "candidate progress missing"),
            ("terminal-missing", "V2 checkpoint terminal set mismatch"),
            ("terminal-corrupt", "corrupt V2 node terminal"),
            (
                "terminal-disposition",
                "candidate checkpoint disposition mismatch",
            ),
        ],
        false,
    )
    .await;
}

#[tokio::test]
async fn workflow_checked_recovery_failed_interruption_stays_working() {
    check_candidate_recovery_cases(&[("continuation", ""), ("mixed", "")], true).await;
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
        assert!(record.error.unwrap().contains("candidate progress missing"));
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

// C2c2d2: inspect the real publication transaction before event consumption.
async fn joint_admission(h: &CandidateFixture, chain: bool) -> AdmittedWorkflowRunV1 {
    let initial = h.admission("source").await;
    let graph = serde_json::from_value(serde_json::json!({
        "id":"workflow", "controls": WorkflowControlDefaultsV1::default(),
        "nodes":[
            {"id":"node", "agent":"source", "prompt_template":"{{input}}", "inputs":[],
             "harvest_sanitization":"attested_prefix_v1"},
            {"id":"after", "agent":"target", "prompt_template":"{{node}}", "inputs":["node"]}
        ]
    }))
    .unwrap();
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
    if chain {
        h.builder()
            .bind_local_chains(admitted, &h.run.progress.chains)
            .unwrap()
    } else {
        admitted
    }
}

fn joint_stream(
    h: &CandidateFixture,
    admitted: AdmittedWorkflowRunV1,
    cancel: CancellationToken,
) -> WorkflowStream {
    let graph = Arc::new(admitted.run_spec.graph.clone());
    let context = WorkflowDiagnosticContext::in_memory(WorkflowRunContext {
        session_cwd: admitted.run_spec.requested_session_cwd.clone(),
        harvest_audit_store: h.f.store.clone(),
        task_id: Some(TaskId::parse(h.owner.execution_id.as_str()).unwrap()),
        observer_key: Some(h.owner.execution_id.as_str().into()),
        ..Default::default()
    })
    .with_accounting(h.accounting.clone())
    .with_admitted_workflow_run(admitted.clone())
    .unwrap();
    WorkflowExecutor::new(h.registry.clone())
        .with_local_fallback(
            h.builder(),
            admitted,
            h.f.store.clone(),
            TaskId::parse(h.owner.execution_id.as_str()).unwrap(),
        )
        .run_with_diagnostic_context(graph, "input".into(), "joint".into(), cancel, context)
}

// Retain raw durable bytes, sequence, timestamp and operation identity so later
// sink consumption and run settlement cannot silently rewrite the completion.
fn joint_receipt(h: &CandidateFixture) -> serde_json::Value {
    let db = rusqlite::Connection::open(h.f.dir.path().join("tasks.sqlite")).unwrap();
    let checkpoint: String = db.query_row(
        "SELECT json_array(output,ok,usage_json,terminal_json,policy_trigger_json,seq,ts) FROM task_node_checkpoints WHERE node_id='node'",
        [], |r| r.get(0)).unwrap();
    let journal: Vec<String> = db
        .prepare("SELECT event_json FROM task_journal ORDER BY seq")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .filter(|json| {
            matches!(serde_json::from_str::<bridge_core::orch::OrchEvent>(json).unwrap().kind,
            bridge_core::orch::OrchEventKind::NodeFinished { node, .. } if node == "node")
        })
        .collect();
    assert_eq!(journal.len(), 1);
    let progress: String = db
        .query_row(
            "SELECT progress_json FROM task_candidate_progress_v1 WHERE node_id='node'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    serde_json::json!({"checkpoint":checkpoint,"journal":journal,"progress":progress})
}

fn joint_assert_structured(h: &CandidateFixture, event: &WorkflowEvent) {
    let WorkflowEvent::NodeFinished {
        node,
        output,
        ok,
        usage,
        terminal_json,
        policy_trigger_json,
        ..
    } = event
    else {
        panic!("{event:?}")
    };
    assert_eq!(node.as_str(), "node");
    assert_eq!(output, "answer");
    assert!(*ok);
    assert_eq!(usage.as_ref().unwrap().used, Some(7));
    let receipt = joint_receipt(h);
    let checkpoint: serde_json::Value =
        serde_json::from_str(receipt["checkpoint"].as_str().unwrap()).unwrap();
    assert_eq!(checkpoint[0], *output);
    assert_eq!(checkpoint[1], 1);
    assert_eq!(
        checkpoint[2],
        serde_json::to_string(usage.as_ref().unwrap()).unwrap()
    );
    assert_eq!(
        checkpoint[3],
        terminal_json.as_deref().unwrap(),
        "structured completion must precede emission"
    );
    assert_eq!(
        checkpoint[4],
        serde_json::to_value(policy_trigger_json).unwrap()
    );
    let terminal: NodeTerminalV1 = serde_json::from_str(terminal_json.as_deref().unwrap()).unwrap();
    assert_eq!(terminal.primary, NodePrimaryDispositionV1::Completed);
    let progress: CandidateProgressV1 =
        serde_json::from_str(receipt["progress"].as_str().unwrap()).unwrap();
    assert_eq!(
        progress.terminal.unwrap(),
        CandidateOutputV1 {
            output: output.clone(),
            ok: *ok,
            usage: usage.clone()
        }
    );
    let journal: bridge_core::orch::OrchEvent =
        serde_json::from_str(receipt["journal"][0].as_str().unwrap()).unwrap();
    assert!(
        matches!(journal.kind, bridge_core::orch::OrchEventKind::NodeFinished {
        output: o, ok: true, usage: u, terminal_json: t, policy_trigger_json: p, ..
    } if o == *output && u == *usage && t == *terminal_json && p == *policy_trigger_json)
    );
    assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 0);
}

async fn joint_first_completion(stream: &mut WorkflowStream) -> WorkflowEvent {
    use futures::StreamExt;
    loop {
        let event = stream.next().await.expect("node completion").unwrap();
        if matches!(&event, WorkflowEvent::NodeFinished { node, .. } if node.as_str() == "node") {
            return event;
        }
    }
}

#[tokio::test]
async fn joint_structured_completion_precedes_emission_and_dependent() {
    use futures::StreamExt;
    let h = CandidateFixture::new("attested-usage", "final-only").await;
    let admitted = joint_admission(&h, true).await;
    let mut stream = joint_stream(&h, admitted, CancellationToken::new());
    let event = joint_first_completion(&mut stream).await;
    joint_assert_structured(&h, &event);
    let before = joint_receipt(&h);
    let path = h.f.dir.path().join("tasks.sqlite");
    let expected = before.clone();
    *h.calls[1].entry_check.lock().unwrap() = Some(Box::new(move || {
        let db = rusqlite::Connection::open(&path).unwrap();
        let json: String = db
            .query_row(
                "SELECT terminal_json FROM task_node_checkpoints WHERE node_id='node'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let checkpoint: serde_json::Value =
            serde_json::from_str(expected["checkpoint"].as_str().unwrap()).unwrap();
        assert_eq!(json, checkpoint[3]);
    }));
    let events: Vec<_> = stream.collect().await;
    assert!(events.iter().all(Result::is_ok), "{events:?}");
    assert_eq!(h.f.providers[1].inputs.lock().unwrap()[0][0].text, "answer");
    assert_eq!(joint_receipt(&h), before);
    assert!(h.f.rows().iter().all(|row| row.finalized));
}

async fn joint_later_exit(target: &'static str, canceled: bool) {
    use futures::StreamExt;
    let h = CandidateFixture::new("attested-usage", target).await;
    let admitted = joint_admission(&h, true).await;
    let cancel = CancellationToken::new();
    let mut stream = joint_stream(&h, admitted, cancel.clone());
    let event = joint_first_completion(&mut stream).await;
    joint_assert_structured(&h, &event);
    let before = joint_receipt(&h);
    if canceled {
        cancel.cancel();
    }
    let events: Vec<_> = stream.collect().await;
    assert!(events.iter().all(Result::is_ok), "{events:?}");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Ok(WorkflowEvent::Terminal { outcome, .. })
        if *outcome == if canceled { WorkflowOutcome::Canceled } else { WorkflowOutcome::Failed }))
    );
    assert_eq!(joint_receipt(&h), before);
    assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 0);
    assert!(h.f.rows().iter().all(|row| row.finalized));
}

#[tokio::test]
async fn joint_completion_survives_later_failure() {
    joint_later_exit("setup", false).await;
}

#[tokio::test]
async fn joint_completion_survives_later_cancellation() {
    joint_later_exit("final-only", true).await;
}

async fn joint_failed_publication(audit: bool) {
    use futures::StreamExt;
    let h = CandidateFixture::new("attested-usage", "final-only").await;
    let admitted = joint_admission(&h, true).await;
    let db = rusqlite::Connection::open(h.f.dir.path().join("tasks.sqlite")).unwrap();
    db.execute_batch(if audit {
        "CREATE TRIGGER joint_fault BEFORE INSERT ON harvest_raw_records_v1 BEGIN SELECT RAISE(ABORT,'joint audit fault'); END;"
    } else {
        // Fail after the structured checkpoint INSERT, while inserting its journal
        // event. Progress and checkpoint must roll back with that same transaction.
        "CREATE TRIGGER joint_fault BEFORE INSERT ON task_journal WHEN json_extract(NEW.event_json,'$.terminal_json') IS NOT NULL BEGIN SELECT RAISE(ABORT,'joint publication fault'); END;"
    }).unwrap();
    let events: Vec<_> = joint_stream(&h, admitted, CancellationToken::new())
        .collect()
        .await;
    assert!(
        events.iter().any(|e| if audit {
            matches!(e, Err(BridgeError::HarvestAuditPersistFailed { .. }))
        } else {
            matches!(e, Err(BridgeError::StoreFailure))
        }),
        "{events:?}"
    );
    assert!(!events
        .iter()
        .any(|e| matches!(e, Ok(WorkflowEvent::NodeFinished { ok: true, .. }))));
    assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 0);
    let task = TaskId::parse(h.owner.execution_id.as_str()).unwrap();
    assert!(h.f.store.node_checkpoints(&task).await.unwrap().is_empty());
    assert!(h
        .f
        .store
        .candidate_progress_v1(&task, &NodeId::parse("node").unwrap())
        .await
        .unwrap()
        .unwrap()
        .terminal
        .is_none());
    let completions: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM task_journal WHERE event_json LIKE '%node_finished%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(completions, 0);
    assert!(h.f.rows().iter().all(|row| row.finalized));
}

#[tokio::test]
async fn joint_audit_failure_blocks_completion_and_dependent() {
    joint_failed_publication(true).await;
}

#[tokio::test]
async fn joint_publication_failure_rolls_back_completion() {
    joint_failed_publication(false).await;
}

#[tokio::test]
async fn joint_detached_sink_acknowledges_once_and_refuses_conflicts() {
    use bridge_coordinator::detached::{DetachedProgressSink, TaskProgressHub, WorkflowSink};
    let h = CandidateFixture::new("attested-usage", "final-only").await;
    let admitted = joint_admission(&h, true).await;
    let cancel = CancellationToken::new();
    let mut stream = joint_stream(&h, admitted, cancel.clone());
    let event = joint_first_completion(&mut stream).await;
    let WorkflowEvent::NodeFinished {
        output,
        ok,
        usage,
        terminal_json,
        policy_trigger_json,
        ..
    } = event
    else {
        unreachable!()
    };
    let before = joint_receipt(&h);
    let db = rusqlite::Connection::open(h.f.dir.path().join("tasks.sqlite")).unwrap();
    let seq_before: i64 = db
        .query_row("SELECT last_event_seq FROM tasks", [], |r| r.get(0))
        .unwrap();
    // Exact sink consumption must only acknowledge the publication, even when
    // every possible checkpoint/journal write has been made unavailable.
    db.execute_batch("CREATE TRIGGER joint_no_checkpoint BEFORE INSERT ON task_node_checkpoints BEGIN SELECT RAISE(ABORT,'duplicate checkpoint'); END;
        CREATE TRIGGER joint_no_update BEFORE UPDATE ON task_node_checkpoints BEGIN SELECT RAISE(ABORT,'checkpoint rewrite'); END;
        CREATE TRIGGER joint_no_journal BEFORE INSERT ON task_journal BEGIN SELECT RAISE(ABORT,'duplicate journal'); END;").unwrap();
    let hub = Arc::new(TaskProgressHub::new());
    let mut frames = hub.subscribe();
    let mut sink = DetachedProgressSink::new(
        h.f.store.clone(),
        TaskId::parse(h.owner.execution_id.as_str()).unwrap(),
        hub,
    );
    for _ in 0..2 {
        sink.node_finished(
            "node",
            ok,
            &output,
            usage.as_ref(),
            terminal_json.as_deref(),
            policy_trigger_json.as_deref(),
            None,
        )
        .await
        .unwrap();
    }
    let first = frames.try_recv().unwrap();
    assert_eq!(frames.try_recv().unwrap().seq, first.seq);
    let seq_after: i64 = db
        .query_row("SELECT last_event_seq FROM tasks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(seq_before, seq_after);
    for changed in ["output", "usage", "terminal", "trigger"] {
        let mut terminal: NodeTerminalV1 =
            serde_json::from_str(terminal_json.as_deref().unwrap()).unwrap();
        terminal.degraded_ancestry = true;
        let altered_terminal = String::from_utf8(terminal.encode_canonical().unwrap()).unwrap();
        let mut altered_usage = usage.clone().unwrap();
        altered_usage.used = Some(8);
        let result = sink
            .node_finished(
                "node",
                ok,
                if changed == "output" {
                    "different"
                } else {
                    &output
                },
                if changed == "usage" {
                    Some(&altered_usage)
                } else {
                    usage.as_ref()
                },
                if changed == "terminal" {
                    Some(&altered_terminal)
                } else {
                    terminal_json.as_deref()
                },
                if changed == "trigger" {
                    Some("{}")
                } else {
                    policy_trigger_json.as_deref()
                },
                None,
            )
            .await;
        assert!(
            matches!(result, Err(BridgeError::StoreFailure)),
            "{changed}: {result:?}"
        );
    }
    assert!(frames.try_recv().is_err());
    assert_eq!(joint_receipt(&h), before);
    cancel.cancel();
    use futures::StreamExt;
    let _: Vec<_> = stream.collect().await;
}

#[tokio::test]
async fn joint_absent_chain_keeps_local_completion_without_checkpoint() {
    use futures::StreamExt;
    let h = CandidateFixture::new("attested-usage", "final-only").await;
    let admitted = joint_admission(&h, false).await;
    let events: Vec<_> = joint_stream(&h, admitted, CancellationToken::new())
        .collect()
        .await;
    assert!(events.iter().all(Result::is_ok), "{events:?}");
    assert!(events.iter().any(|e| matches!(e, Ok(WorkflowEvent::NodeFinished { output, ok: true, terminal_json: Some(_), .. }) if output == "answer")));
    let task = TaskId::parse(h.owner.execution_id.as_str()).unwrap();
    assert!(h.f.store.node_checkpoints(&task).await.unwrap().is_empty());
    assert!(h
        .f
        .store
        .candidate_progress_v1(&task, &NodeId::parse("node").unwrap())
        .await
        .unwrap()
        .is_none());
}

async fn joint_startup_checkpoint_recovery(chain: bool) {
    use bridge_coordinator::detached::{
        encode_workflow_run_spec, structured_history_reservation_v1, DetachedProgressSink,
        TaskProgressHub, WorkflowSink,
    };
    use bridge_core::workflow_history::*;
    for &history_consumed in if chain {
        &[false, true][..]
    } else {
        &[true][..]
    } {
        // NodeCompletion is durable independently of later CLI arbitration.
        // Recovery must preserve it even if negotiated run evidence was absent.
        let h = CandidateFixture::new(
            if chain { "v1-missing" } else { "final-only" },
            "final-only",
        )
        .await;
        let admitted = h.admission("source").await;
        let admitted = if chain {
            h.builder()
                .bind_local_chains(admitted, &h.run.progress.chains)
                .unwrap()
        } else {
            admitted
        };
        let task = TaskId::parse(h.owner.execution_id.as_str()).unwrap();
        let db = rusqlite::Connection::open(h.f.dir.path().join("tasks.sqlite")).unwrap();
        db.execute(
            "UPDATE tasks SET workflow_spec_json=?1,session_cwd=?2,input=?3 WHERE id=?4",
            rusqlite::params![
                encode_workflow_run_spec(&admitted.run_spec).unwrap(),
                admitted
                    .run_spec
                    .requested_session_cwd
                    .as_ref()
                    .unwrap()
                    .as_str(),
                "---\ntask-type: freeform\n---\ninput",
                task.as_str()
            ],
        )
        .unwrap();
        h.f.store
            .reserve_v2(
                &structured_history_reservation_v1(
                    AttemptReservation {
                        identity: h.owner.clone(),
                        task_id: Some(task.clone()),
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
                    &admitted.run_spec,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let graph = Arc::new(admitted.run_spec.graph.clone());
        let mut stream = joint_stream(&h, admitted, CancellationToken::new());
        let event = joint_first_completion(&mut stream).await;
        drop(stream); // Simulate a crash before CLI arbitration / task finalization.
        let WorkflowEvent::NodeFinished {
            ok,
            output,
            usage,
            terminal_json,
            policy_trigger_json,
            ..
        } = event
        else {
            unreachable!()
        };
        assert!(ok);
        assert_eq!(output, "answer");
        assert!(terminal_json.is_some());
        if !chain {
            // Ordinary detached completion is checkpointed by its sink, whereas
            // the candidate transaction already checkpointed the local node.
            DetachedProgressSink::new(
                h.f.store.clone(),
                task.clone(),
                Arc::new(TaskProgressHub::new()),
            )
            .node_finished(
                "node",
                ok,
                &output,
                usage.as_ref(),
                terminal_json.as_deref(),
                policy_trigger_json.as_deref(),
                None,
            )
            .await
            .unwrap();
        }
        if history_consumed {
            h.f.store
                .commit_node_terminal_v2(
                    &h.owner.attempt_id,
                    &NodeId::parse("node").unwrap(),
                    terminal_json.as_deref().unwrap(),
                    policy_trigger_json.as_deref(),
                )
                .await
                .unwrap();
        }
        let before = chain.then(|| joint_receipt(&h));
        let checkpoints = h.f.store.node_checkpoints(&task).await.unwrap();
        assert_eq!(checkpoints.len(), 1);
        assert_eq!(
            h.f.store.get(&task).await.unwrap().unwrap().status,
            TaskRecordStatus::Working
        );
        assert!(h
            .f
            .store
            .workflow_task_evidence(&task)
            .await
            .unwrap()
            .workflow_outcome
            .is_none());
        let locator = h.f.store.get_attempt_locator(&task).await.unwrap();
        let spawns = h.f.spawns.lock().unwrap().clone();
        let prompts: Vec<_> =
            h.f.providers
                .iter()
                .map(|p| p.prompts.load(Ordering::SeqCst))
                .collect();
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
        .with_workflow_history(Ok(h.f.store.clone()));
        coordinator.resume().await.unwrap();
        let record = h.f.store.get(&task).await.unwrap().unwrap();
        assert_eq!(
            record.status,
            TaskRecordStatus::Completed,
            "chain={chain}, history_consumed={history_consumed}"
        );
        if chain {
            assert!(record.error.is_none());
            assert_eq!(Some(joint_receipt(&h)), before);
            assert_eq!(
                h.f.store
                    .workflow_task_evidence(&task)
                    .await
                    .unwrap()
                    .workflow_outcome,
                Some(WorkflowDurableOutcomeV1::Completed)
            );
        }
        assert_eq!(record.result.as_deref(), Some("answer"));
        let attempt =
            h.f.store
                .attempt(&h.owner.attempt_id)
                .await
                .unwrap()
                .unwrap();
        if history_consumed {
            assert_eq!(attempt.terminal.unwrap().outcome, "completed");
        } else {
            // The checkpoint owns completion even if optional node history had
            // not consumed the event. Retain ordinary telemetry-marker ordering.
            assert!(attempt.terminal.is_none());
            assert!(h
                .f
                .store
                .get_attempt_locator(&task)
                .await
                .unwrap()
                .unwrap()
                .telemetry_unavailable
                .is_some());
        }
        assert_eq!(record.resume_attempts, 0);
        assert_eq!(
            h.f.store
                .get_attempt_locator(&task)
                .await
                .unwrap()
                .map(|l| l.identity),
            locator.map(|l| l.identity)
        );
        assert_eq!(
            h.f.store.node_checkpoints(&task).await.unwrap(),
            checkpoints
        );
        assert_eq!(*h.f.spawns.lock().unwrap(), spawns);
        assert_eq!(
            h.f.providers
                .iter()
                .map(|p| p.prompts.load(Ordering::SeqCst))
                .collect::<Vec<_>>(),
            prompts
        );
    }
}

#[tokio::test]
async fn joint_startup_recovers_terminal_candidate_checkpoint() {
    joint_startup_checkpoint_recovery(true).await;
}

#[tokio::test]
async fn joint_startup_recovers_ordinary_terminal_checkpoint() {
    joint_startup_checkpoint_recovery(false).await;
}

// C2c2d4: executor composition only; public recovery still refuses these snapshots.
async fn restored_owner(
    h: &CandidateFixture,
    source: &AdmittedWorkflowRunV1,
    retries: u32,
) -> (AdmittedWorkflowRunV1, WorkflowAccountingInvocationV1) {
    let task = TaskId::parse(h.owner.execution_id.as_str()).unwrap();
    let previous = h.f.store.get_attempt_locator(&task).await.unwrap().unwrap();
    let owner = previous.identity.resume().unwrap();
    assert!(matches!(
        h.f.store
            .claim_resume_attempt_with_locator(
                &task,
                3,
                2,
                &previous,
                &TaskAttemptLocator {
                    identity: owner.clone(),
                    telemetry_unavailable: None
                }
            )
            .await
            .unwrap(),
        ResumeClaim::Resumable { .. }
    ));
    let mut graph = source.run_spec.graph.clone();
    graph.nodes[0].retry = Some(bridge_workflow::graph::RetryPolicy {
        max_attempts: retries,
        backoff_ms: 0,
        backoff_cap_ms: None,
    });
    let admitted = h
        .builder()
        .freeze(WorkflowAdmissionRequestV1 {
            attempt_id: owner.attempt_id.clone(),
            graph: Arc::new(graph),
            requested_session_cwd: source.run_spec.requested_session_cwd.clone(),
            policy_invocation: Default::default(),
            ledger_admission: source.run_spec.ledger_admission.clone(),
            r2f1b: None,
        })
        .await
        .unwrap();
    let admitted = h
        .builder()
        .bind_local_chains(
            admitted,
            source
                .run_spec
                .fallback_chains
                .as_ref()
                .unwrap()
                .get(&NodeId::parse("node").unwrap())
                .unwrap(),
        )
        .unwrap();
    (
        admitted,
        WorkflowAccountingInvocationV1::new(
            AccountingV1::new(h.f.store.clone()),
            owner,
            WorkflowId::parse("workflow").unwrap(),
        ),
    )
}

async fn restored_executor(
    h: &CandidateFixture,
    admitted: &AdmittedWorkflowRunV1,
    accounting: &WorkflowAccountingInvocationV1,
) -> Result<WorkflowExecutor> {
    WorkflowExecutor::new(h.registry.clone())
        .with_restored_fallback_v1(
            h.builder(),
            admitted.clone(),
            h.f.store.clone(),
            TaskId::parse(h.owner.execution_id.as_str()).unwrap(),
            accounting,
            |diagnostic| BridgeError::agent_failure(diagnostic.clone()),
        )
        .await
}

async fn drive_restored(
    h: &CandidateFixture,
    executor: &WorkflowExecutor,
    admitted: AdmittedWorkflowRunV1,
    accounting: Option<WorkflowAccountingInvocationV1>,
) -> Vec<Result<WorkflowEvent>> {
    drive_restored_accounted(h, executor, admitted, accounting, None)
        .await
        .into_iter()
        .map(|event| event.map_err(|failure| failure.surfaced_error()))
        .collect()
}

async fn drive_restored_accounted(
    h: &CandidateFixture,
    executor: &WorkflowExecutor,
    admitted: AdmittedWorkflowRunV1,
    accounting: Option<WorkflowAccountingInvocationV1>,
    structured_seed: Option<StructuredWorkflowSeedV1>,
) -> Vec<Result<WorkflowEvent, bridge_core::translator::AccountingFailureV1>> {
    drive_restored_controlled(
        h,
        executor,
        admitted,
        accounting,
        structured_seed,
        CancellationToken::new(),
        None,
    )
    .await
}

async fn drive_restored_controlled(
    h: &CandidateFixture,
    executor: &WorkflowExecutor,
    admitted: AdmittedWorkflowRunV1,
    accounting: Option<WorkflowAccountingInvocationV1>,
    structured_seed: Option<StructuredWorkflowSeedV1>,
    cancel: CancellationToken,
    barrier: Option<PromptDispatchBarrier>,
) -> Vec<Result<WorkflowEvent, bridge_core::translator::AccountingFailureV1>> {
    use futures::StreamExt;
    let graph = Arc::new(admitted.run_spec.graph.clone());
    let mut context = WorkflowDiagnosticContext::in_memory(WorkflowRunContext {
        session_cwd: admitted.run_spec.requested_session_cwd.clone(),
        harvest_audit_store: h.f.store.clone(),
        task_id: Some(TaskId::parse(h.owner.execution_id.as_str()).unwrap()),
        observer_key: Some(h.owner.execution_id.as_str().into()),
        ..Default::default()
    })
    .with_admitted_workflow_run(admitted)
    .unwrap();
    if let Some(barrier) = barrier {
        context = context.with_prompt_dispatch_barrier(barrier);
    }
    if let Some(accounting) = accounting {
        context = context.with_accounting(accounting);
    }
    let seed = structured_seed
        .as_ref()
        .map(|seed| {
            seed.nodes
                .iter()
                .map(|(node, checkpoint)| {
                    (
                        node.as_str().to_owned(),
                        (
                            checkpoint.output.clone(),
                            checkpoint.ok,
                            checkpoint.usage.clone(),
                        ),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    executor
        .run_from_with_accounting_failures(
            graph,
            "restored input".into(),
            "restored".into(),
            cancel,
            seed,
            structured_seed,
            context,
        )
        .collect()
        .await
}

async fn checked_progress(h: &CandidateFixture) -> CandidateProgressV1 {
    h.f.store
        .candidate_progress_v1(
            &TaskId::parse(h.owner.execution_id.as_str()).unwrap(),
            &NodeId::parse("node").unwrap(),
        )
        .await
        .unwrap()
        .unwrap()
}

// Extend the real checked store at a settled crash boundary, without provider work.
async fn seed_rejected_visit(h: &CandidateFixture, position: usize) {
    let task = TaskId::parse(h.owner.execution_id.as_str()).unwrap();
    let node = NodeId::parse("node").unwrap();
    let mut progress = checked_progress(h).await;
    let prior = progress.visits.last().unwrap().clone();
    let candidate = if position == 0 {
        &progress.source
    } else {
        &progress.chains[0].steps[position - 1]
    };
    let id = AttemptId::mint().unwrap().as_str().to_owned();
    let mut row = h.f.rows()[0].clone();
    row.id = id.clone();
    row.dispatched = false;
    row.finalized = false;
    row.attribution.agent = candidate.agent.as_str().into();
    row.attribution.effective_model = Some(candidate.model.clone());
    row.attribution.effective_mode = candidate.posture.mode.clone();
    h.f.store.start_usage_attempt_v1(&row).await.unwrap();
    for phase in [
        CandidatePhaseV1::Preparing,
        CandidatePhaseV1::Prepared,
        CandidatePhaseV1::PollFenced,
        CandidatePhaseV1::Polled,
    ] {
        let old = progress.clone();
        if phase == CandidatePhaseV1::Preparing {
            progress.retries_consumed += u32::from(position == prior.position);
            progress.visits.push(CandidateVisitV1 {
                position,
                accounting_id: id.clone(),
                owner: h.owner.clone(),
                cold_context: ContextId::parse(id.clone()).unwrap(),
                phase,
                rejection: None,
            });
        } else {
            progress.visits.last_mut().unwrap().phase = phase;
        }
        h.f.store
            .put_candidate_progress_v1(&task, &node, Some(&old), &progress)
            .await
            .unwrap();
    }
    h.f.store
        .update_usage_attempt_v1(&id, true, None, true)
        .await
        .unwrap();
    let old = progress.clone();
    progress.visits.last_mut().unwrap().rejection = prior.rejection;
    h.f.store
        .put_candidate_progress_v1(&task, &node, Some(&old), &progress)
        .await
        .unwrap();
}

async fn restored_governed_refusal(execute: bool) {
    use bridge_core::governance::*;
    for turns in [0, 10] {
        for preflight in [false, true] {
            let (h, initial) =
                recovery_candidate_fixture_target("continuation", "preflight-pass").await;
            let mut snapshot = h.f.snapshot();
            snapshot.entries[1].preflight = preflight;
            h.registry.apply(snapshot).await.unwrap();
            let (admitted, ungoverned) = restored_owner(&h, &initial, 2).await;
            let owner =
                h.f.store
                    .get_attempt_locator(&TaskId::parse(h.owner.execution_id.as_str()).unwrap())
                    .await
                    .unwrap()
                    .unwrap()
                    .identity;
            let mut service = AccountingV1::new(h.f.store.clone());
            service.budget_policy = Some(Arc::new(
                BudgetPolicyV1::new(
                    vec![BudgetRuleV1 {
                        id: "restored-cap".into(),
                        selector: BudgetSelectorV1::EachRequest,
                        limits: BudgetLimitsV1 {
                            turns: Some(turns),
                            terminal_tokens: None,
                        },
                    }],
                    vec![],
                )
                .unwrap(),
            ));
            let governed = WorkflowAccountingInvocationV1::new(
                service,
                owner,
                WorkflowId::parse("workflow").unwrap(),
            );
            let prior_rows = h.f.rows();
            let prior_progress = checked_progress(&h).await;
            let expected = BridgeError::ConfigInvalid {
                reason: "governed restored fallback is unsupported".into(),
            };
            if execute {
                let executor = restored_executor(&h, &admitted, &ungoverned).await.unwrap();
                let events = drive_restored(&h, &executor, admitted, Some(governed)).await;
                assert!(
                    events.iter().any(|e| e.as_ref().err() == Some(&expected)),
                    "{events:?}"
                );
            } else {
                let result = restored_executor(&h, &admitted, &governed).await;
                assert_eq!(
                    result.err(),
                    Some(expected),
                    "turns={turns}, preflight={preflight}"
                );
            }
            assert_eq!(h.f.rows(), prior_rows, "no new accounting rows");
            assert_eq!(checked_progress(&h).await, prior_progress);
            let db =
                rusqlite::Connection::open(h.f.dir.path().join("tasks.sqlite.usage-v1.sqlite"))
                    .unwrap();
            let exists: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='governance_reservations_v1')", [], |r| r.get(0)).unwrap();
            if exists {
                let reservations: u64 = db
                    .query_row("SELECT COUNT(*) FROM governance_reservations_v1", [], |r| {
                        r.get(0)
                    })
                    .unwrap();
                assert_eq!(reservations, 0);
            }
            assert!(h.f.spawns.lock().unwrap().is_empty());
            for (provider, calls) in h.f.providers.iter().zip(&h.calls) {
                assert_eq!(calls.setups.load(Ordering::SeqCst), 0);
                assert_eq!(provider.setup.load(Ordering::SeqCst), 0);
                assert_eq!(provider.prompts.load(Ordering::SeqCst), 0);
            }
        }
    }
}

#[tokio::test]
async fn restored_composition_refuses_governed_before_effects() {
    restored_governed_refusal(false).await;
}

#[tokio::test]
async fn restored_execution_refuses_swapped_governed_before_effects() {
    restored_governed_refusal(true).await;
}

#[tokio::test]
async fn restored_unpolled_budget_refusal_remains_policy_stop() {
    let (h, initial) = recovery_candidate_fixture_target("continuation", "setup-budget").await;
    let (admitted, accounting) = restored_owner(&h, &initial, 2).await;
    let executor = restored_executor(&h, &admitted, &accounting).await.unwrap();
    let prior = h.f.rows();
    let events = drive_restored_accounted(&h, &executor, admitted, Some(accounting), None).await;
    let failure = events
        .iter()
        .find_map(|e| e.as_ref().err())
        .expect("budget policy stop");
    assert!(
        matches!(failure.original, BridgeError::BudgetRefused { .. }),
        "{events:?}"
    );
    assert!(failure.persistence.is_none());
    assert_eq!(failure.surfaced_error(), failure.original);
    precedence_cause(&events, Some(&failure.original));
    let projected: Vec<_> = events
        .iter()
        .filter(|event| event.is_ok())
        .cloned()
        .collect();
    precedence_output(&projected, "budget refused", WorkflowOutcome::Failed);
    precedence_rows(&h, &prior, &[(false, true)]);
    assert_eq!(h.calls[1].setups.load(Ordering::SeqCst), 1);
    assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 0);
    assert_eq!(h.f.rows().len(), 2);
    assert!(h.f.rows().iter().all(|row| row.finalized));
}

#[tokio::test]
async fn restored_successor_preserves_chain_position_and_prior_visits() {
    for position in [1, 2] {
        let (h, initial) = recovery_candidate_fixture("continuation").await;
        if position == 2 {
            seed_rejected_visit(&h, 1).await;
        }
        let prior = checked_progress(&h).await;
        let rows = h.f.rows();
        let (admitted, accounting) = restored_owner(&h, &initial, 2).await;
        let executor = restored_executor(&h, &admitted, &accounting).await.unwrap();
        assert!(
            h.f.spawns.lock().unwrap().is_empty(),
            "restore grants no preparation"
        );
        let events = drive_restored(&h, &executor, admitted.clone(), Some(accounting)).await;
        assert!(events.iter().any(|e| matches!(e, Ok(WorkflowEvent::Terminal { outcome, output }) if *outcome == if position == 1 { WorkflowOutcome::Completed } else { WorkflowOutcome::Failed } && (position != 1 || output == "answer"))), "{events:?}");
        let progress = checked_progress(&h).await;
        assert_eq!(progress.visits[..prior.visits.len()], prior.visits);
        assert_eq!(progress.selected_chain, Some(0));
        assert_eq!(progress.visits.last().unwrap().position, position);
        assert_eq!(progress.retries_consumed, prior.retries_consumed);
        assert_eq!(
            progress.visits.last().unwrap().owner.attempt_id,
            admitted.run_spec.attempt_id
        );
        for old in rows {
            assert_eq!(h.f.rows().iter().find(|r| r.id == old.id), Some(&old));
        }
        assert_eq!(h.f.providers[0].setup.load(Ordering::SeqCst), 0);
        assert_eq!(h.f.providers[0].prompts.load(Ordering::SeqCst), 0);
        assert_eq!(h.calls[position].setups.load(Ordering::SeqCst), 1);
        assert_eq!(
            h.f.providers[position].prompts.load(Ordering::SeqCst),
            usize::from(position == 1)
        );
        assert_eq!(h.calls[position].cleaned.lock().unwrap().len(), 1);
        if position == 1 {
            assert_eq!(
                h.f.providers[position].inputs.lock().unwrap()[0][0].text,
                "restored input"
            );
        }
    }
}

#[tokio::test]
async fn restored_successor_keeps_consumed_retry_budget() {
    let (h, initial) = recovery_candidate_fixture_target("continuation", "retry-unpolled").await;
    seed_rejected_visit(&h, 0).await;
    let prior = checked_progress(&h).await;
    let (admitted, accounting) = restored_owner(&h, &initial, 2).await;
    let executor = restored_executor(&h, &admitted, &accounting).await.unwrap();
    let events = drive_restored(&h, &executor, admitted, Some(accounting)).await;
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
    assert_eq!(h.f.providers[0].prompts.load(Ordering::SeqCst), 0);
    assert_eq!(
        h.f.providers[1].prompts.load(Ordering::SeqCst),
        1,
        "spent retry was reused"
    );
    let progress = checked_progress(&h).await;
    assert_eq!(progress.visits[..prior.visits.len()], prior.visits);
    assert_eq!(progress.retries_consumed, 1);
    assert_eq!(progress.visits.len(), 3);
    assert_eq!(h.f.rows().len(), 3);
}

#[tokio::test]
async fn restored_composition_requires_claimed_accounting_and_checked_evidence() {
    for fault in [
        "fresh",
        "missing",
        "stale-owner",
        "missing-row",
        "authority",
        "scope",
        "diagnostic",
    ] {
        let (h, initial) = recovery_candidate_fixture("continuation").await;
        if fault == "fresh" {
            assert!(restored_executor(&h, &initial, &h.accounting)
                .await
                .is_err());
        } else {
            let (admitted, accounting) = restored_owner(&h, &initial, 2).await;
            if fault == "diagnostic" {
                assert!(WorkflowExecutor::new(h.registry.clone())
                    .with_restored_fallback_v1(
                        h.builder(),
                        admitted.clone(),
                        h.f.store.clone(),
                        TaskId::parse(h.owner.execution_id.as_str()).unwrap(),
                        &accounting,
                        |_| BridgeError::StoreFailure,
                    )
                    .await
                    .is_err());
            } else if fault == "missing-row" {
                rusqlite::Connection::open(h.f.dir.path().join("tasks.sqlite.usage-v1.sqlite"))
                    .unwrap()
                    .execute("DELETE FROM usage_attempts_v1", [])
                    .unwrap();
                assert!(restored_executor(&h, &admitted, &accounting).await.is_err());
            } else {
                let executor = restored_executor(&h, &admitted, &accounting).await.unwrap();
                if fault == "stale-owner" {
                    restored_owner(&h, &admitted, 2).await;
                }
                if fault == "authority" {
                    let mut snapshot = h.f.snapshot();
                    snapshot.entries[1].args.push("drift".into());
                    h.registry.apply(snapshot).await.unwrap();
                }
                if fault == "scope" {
                    let scope = h.f.dir.path().join("scope");
                    std::fs::rename(&scope, h.f.dir.path().join("old-scope")).unwrap();
                    std::fs::create_dir(scope).unwrap();
                }
                let events = drive_restored(
                    &h,
                    &executor,
                    admitted,
                    (fault != "missing").then_some(accounting),
                )
                .await;
                assert!(events.iter().any(Result::is_err), "{fault}: {events:?}");
            }
        }
        assert!(h.f.spawns.lock().unwrap().is_empty(), "{fault}");
        assert_eq!(
            h.f.rows().len(),
            usize::from(fault != "missing-row"),
            "{fault}"
        );
        for provider in &h.f.providers {
            assert_eq!(provider.setup.load(Ordering::SeqCst), 0, "{fault}");
            assert_eq!(provider.prompts.load(Ordering::SeqCst), 0, "{fault}");
        }
    }
}

#[tokio::test]
async fn restored_replayed_preparation_never_reaches_provider() {
    for preflight in [false, true] {
        let (h, initial) =
            recovery_candidate_fixture_target("continuation", "preflight-pass").await;
        let mut snapshot = h.f.snapshot();
        snapshot.entries[1].preflight = preflight;
        h.registry.apply(snapshot).await.unwrap();
        let (admitted, accounting) = restored_owner(&h, &initial, 2).await;
        let first = restored_executor(&h, &admitted, &accounting).await.unwrap();
        let replay = restored_executor(&h, &admitted, &accounting).await.unwrap();
        let events = drive_restored(&h, &first, admitted.clone(), Some(accounting.clone())).await;
        assert!(
            events.iter().any(|e| matches!(
                e,
                Ok(WorkflowEvent::Terminal {
                    outcome: WorkflowOutcome::Completed,
                    ..
                })
            )),
            "{events:?}"
        );
        let progress = checked_progress(&h).await;
        let before = h.f.rows();
        let events = drive_restored(&h, &replay, admitted, Some(accounting)).await;
        assert!(events.iter().any(Result::is_err), "{events:?}");
        assert_eq!(checked_progress(&h).await, progress);
        // A losing accounting reservation is settled without any provider preparation.
        assert!(h
            .f
            .rows()
            .iter()
            .filter(|r| !before.iter().any(|old| old.id == r.id))
            .all(|r| !r.dispatched && r.finalized));
        assert_eq!(
            h.calls[1].setups.load(Ordering::SeqCst),
            1 + usize::from(preflight)
        );
        assert_eq!(
            h.f.providers[1].prompts.load(Ordering::SeqCst),
            1 + usize::from(preflight)
        );
        assert_eq!(h.f.providers[0].prompts.load(Ordering::SeqCst), 0);
    }
}
#[tokio::test]
async fn restored_unpolled_failure_retains_primary_and_persistence_errors() {
    for persistence in [false, true] {
        let (h, initial) = recovery_candidate_fixture_target("continuation", "setup").await;
        let (admitted, accounting) = restored_owner(&h, &initial, 2).await;
        let executor = restored_executor(&h, &admitted, &accounting).await.unwrap();
        let prior = h.f.rows();
        let progress = checked_progress(&h).await;
        if persistence {
            rusqlite::Connection::open(h.f.dir.path().join("tasks.sqlite.usage-v1.sqlite")).unwrap().execute_batch(
                "CREATE TRIGGER fail_restored_prepare BEFORE INSERT ON usage_attempts_v1 BEGIN SELECT RAISE(ABORT,'accounting fault'); END;"
            ).unwrap();
        }
        let events =
            drive_restored_accounted(&h, &executor, admitted, Some(accounting), None).await;
        precedence_rows(&h, &prior, if persistence { &[] } else { &[(false, true)] });
        if !persistence {
            let primary = BridgeError::agent_failure(
                *progress.visits[0]
                    .rejection
                    .as_ref()
                    .unwrap()
                    .diagnostic
                    .clone(),
            );
            precedence_cause(&events, Some(&primary));
            precedence_output(&events, "fixture.rejected", WorkflowOutcome::Failed);
        }
        if persistence {
            precedence_error(
                &events,
                BridgeError::StoreFailure,
                Some(BridgeError::StoreFailure),
            );
            let failure = events
                .iter()
                .find_map(|event| event.as_ref().err())
                .unwrap();
            assert_eq!(failure.original, BridgeError::StoreFailure);
            assert_eq!(failure.persistence, Some(BridgeError::StoreFailure));
            assert_eq!(failure.surfaced_error(), BridgeError::StoreFailure);
        }
        assert!(
            format!("{events:?}").contains(if persistence {
                "StoreFailure"
            } else {
                "fixture.rejected"
            }),
            "{events:?}"
        );
        assert_eq!(h.f.providers[0].prompts.load(Ordering::SeqCst), 0);
        assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 0);
        assert_eq!(
            h.calls[1].setups.load(Ordering::SeqCst),
            usize::from(!persistence)
        );
    }
}

async fn restored_preflight_accounting_failure(settlement_fails: bool) {
    let (h, initial) = recovery_candidate_fixture_target("continuation", "preflight-pass").await;
    let prior_rows = h.f.rows();
    let mut snapshot = h.f.snapshot();
    snapshot.entries[1].preflight = true;
    h.registry.apply(snapshot).await.unwrap();
    let (admitted, accounting) = restored_owner(&h, &initial, 2).await;
    let executor = restored_executor(&h, &admitted, &accounting).await.unwrap();
    let db =
        rusqlite::Connection::open(h.f.dir.path().join("tasks.sqlite.usage-v1.sqlite")).unwrap();
    db.execute_batch("PRAGMA journal_mode=PERSIST; CREATE TRIGGER fail_preflight_insert BEFORE INSERT ON usage_attempts_v1 WHEN json_extract(NEW.record_json,'$.attribution.attempt_kind')='preflight' BEGIN SELECT RAISE(ABORT,'preflight accounting fault'); END;").unwrap();
    if settlement_fails {
        db.execute_batch("CREATE TRIGGER fail_node_settlement BEFORE UPDATE ON usage_attempts_v1 WHEN json_extract(NEW.record_json,'$.attribution.agent')='target' AND json_extract(NEW.record_json,'$.finalized')=1 BEGIN SELECT RAISE(ABORT,'node settlement fault'); END;").unwrap();
    }
    let events = drive_restored_accounted(&h, &executor, admitted, Some(accounting), None).await;
    let rows = h.f.rows();
    assert_eq!(rows.len(), 2, "rows={rows:?}; events={events:?}");
    let node = rows
        .iter()
        .find(|row| row.attribution.agent == "target")
        .unwrap();
    assert_eq!(node.attribution.attempt_kind, "node");
    assert!(!node.dispatched);
    assert_eq!(node.finalized, !settlement_fails, "{events:?}");
    for prior in prior_rows {
        assert_eq!(rows.iter().find(|row| row.id == prior.id), Some(&prior));
    }
    let failure = events
        .iter()
        .find_map(|event| event.as_ref().err())
        .unwrap();
    assert_eq!(failure.original, BridgeError::StoreFailure);
    assert_eq!(failure.persistence, Some(BridgeError::StoreFailure));
    assert_eq!(failure.surfaced_error(), BridgeError::StoreFailure);
    assert!(h.f.spawns.lock().unwrap().is_empty());
    for (provider, calls) in h.f.providers.iter().zip(&h.calls) {
        assert_eq!(calls.setups.load(Ordering::SeqCst), 0);
        assert_eq!(provider.prompts.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn restored_preflight_failure_settles_reserved_node() {
    restored_preflight_accounting_failure(false).await;
}

#[tokio::test]
async fn restored_preflight_settlement_failure_retains_both_errors() {
    restored_preflight_accounting_failure(true).await;
}

#[tokio::test]
async fn restored_structured_seed_reuses_ordinary_checkpoint_and_refuses_candidate_seed() {
    use bridge_core::attestation::HarvestSanitizationMode;
    use bridge_core::orch::UsageSnapshot;
    for include_candidate in [false, true] {
        let (h, initial) = recovery_candidate_fixture("continuation").await;
        let mut graph = initial.run_spec.graph.clone();
        let mut upstream = graph.nodes[0].clone();
        upstream.id = NodeId::parse("upstream").unwrap();
        upstream.agent = AgentId::parse("target").unwrap();
        upstream.harvest_sanitization = Some(HarvestSanitizationMode::AttestedPrefixV1);
        graph.nodes[0].inputs = vec![upstream.id.clone()];
        graph.nodes[0].prompt_template = "{{upstream}}\n{{workflow.costs}}".into();
        graph.nodes.push(upstream.clone());
        let initial = h
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
        let initial = h
            .builder()
            .bind_local_chains(initial, &h.run.progress.chains)
            .unwrap();
        let (admitted, accounting) = restored_owner(&h, &initial, 2).await;
        let executor = restored_executor(&h, &admitted, &accounting).await.unwrap();
        let checkpoint = StructuredNodeSeedV1 {
            output: "process\nexact predecessor output".into(),
            ok: true,
            usage: Some(UsageSnapshot {
                used: Some(41),
                size: Some(100),
                at_ms: 7,
                ..Default::default()
            }),
            terminal: NodeTerminalV1 {
                schema_version: 1,
                primary: NodePrimaryDispositionV1::Completed,
                cleanup: NodeCleanupV1 {
                    disposition: NodeCleanupDispositionV1::Complete,
                    duration_ms: 0,
                },
                cause: None,
                prompt_may_have_been_accepted: false,
                degraded_ancestry: false,
                policy_trigger_id: None,
            },
        };
        let mut seed = StructuredWorkflowSeedV1::default();
        seed.nodes.insert(upstream.id.clone(), checkpoint.clone());
        if include_candidate {
            seed.nodes
                .insert(NodeId::parse("node").unwrap(), checkpoint.clone());
        }
        let db = rusqlite::Connection::open(h.f.dir.path().join("tasks.sqlite")).unwrap();
        db.execute_batch("CREATE TRIGGER forbid_seed_harvest BEFORE INSERT ON harvest_raw_records_v1 BEGIN SELECT RAISE(ABORT,'seed reharvested'); END;").unwrap();
        let prior = checked_progress(&h).await;
        let prior_rows = h.f.rows();
        let events =
            drive_restored_accounted(&h, &executor, admitted, Some(accounting), Some(seed)).await;
        if include_candidate {
            assert!(events.iter().any(Result::is_err), "{events:?}");
            assert_eq!(checked_progress(&h).await, prior);
            assert_eq!(h.f.rows(), prior_rows);
            assert!(h.f.spawns.lock().unwrap().is_empty());
        } else {
            assert!(
                events.iter().any(|event| matches!(
                    event,
                    Ok(WorkflowEvent::Terminal {
                        outcome: WorkflowOutcome::Completed,
                        ..
                    })
                )),
                "{events:?}"
            );
            let input = &h.f.providers[1].inputs.lock().unwrap()[0][0].text;
            assert!(
                input.starts_with(&format!("{}\n", checkpoint.output)),
                "{input}"
            );
            assert!(
                input.contains("| upstream | 41 | 100 | 0.4100 | n/a |"),
                "{input}"
            );
            assert_eq!(h.calls[1].setups.load(Ordering::SeqCst), 1);
            assert_eq!(h.f.rows().len(), prior_rows.len() + 1);
            assert!(!events.iter().any(|event| matches!(event, Ok(WorkflowEvent::NodeStarted { node, .. }) if node == &upstream.id)));
        }
        assert_eq!(h.f.providers[0].prompts.load(Ordering::SeqCst), 0);
        assert_eq!(
            db.query_row("SELECT count(*) FROM harvest_raw_records_v1", [], |row| row
                .get::<_, u64>(0))
                .unwrap(),
            0
        );
    }
}

// Each matrix run starts with checked A evidence and has exactly one successor B.
async fn precedence_fixture(
    target: &'static str,
    retries: u32,
) -> (
    CandidateFixture,
    AdmittedWorkflowRunV1,
    WorkflowAccountingInvocationV1,
    WorkflowExecutor,
) {
    let (h, initial) = recovery_candidate_fixture_target("precedence", target).await;
    let (admitted, accounting) = restored_owner(&h, &initial, retries).await;
    let executor = restored_executor(&h, &admitted, &accounting).await.unwrap();
    (h, admitted, accounting, executor)
}

fn precedence_rows(h: &CandidateFixture, prior: &[UsageAttemptV1], states: &[(bool, bool)]) {
    let rows = h.f.rows();
    for old in prior {
        assert_eq!(rows.iter().find(|row| row.id == old.id), Some(old));
    }
    let rows: Vec<_> = rows
        .iter()
        .filter(|r| !prior.iter().any(|old| old.id == r.id))
        .collect();
    assert_eq!(rows.len(), states.len(), "{rows:?}");
    let mut actual = Vec::new();
    for row in rows {
        assert_eq!(row.attribution.agent, "target");
        assert_eq!(row.attribution.effective_model.as_deref(), Some("target"));
        assert_eq!(row.attribution.node.as_deref(), Some("node"));
        assert_eq!(row.attribution.attempt_kind, "node");
        actual.push((row.dispatched, row.finalized));
    }
    actual.sort();
    let mut expected = states.to_vec();
    expected.sort();
    assert_eq!(actual, expected);
    assert_eq!(h.f.providers[0].prompts.load(Ordering::SeqCst), 0);
    assert_eq!(h.f.providers[2].prompts.load(Ordering::SeqCst), 0);
}

fn precedence_output(
    events: &[Result<WorkflowEvent, bridge_core::translator::AccountingFailureV1>],
    cause: &str,
    outcome: WorkflowOutcome,
) {
    assert!(events.iter().all(Result::is_ok), "{events:?}");
    let nodes: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Ok(WorkflowEvent::NodeFinished { ok, output, .. }) => Some((*ok, output)),
            _ => None,
        })
        .collect();
    assert_eq!(nodes.len(), 1, "{events:?}");
    assert_eq!(nodes[0].0, outcome == WorkflowOutcome::Completed);
    let terminal = events
        .iter()
        .find_map(|event| match event {
            Ok(WorkflowEvent::NodeFinished {
                terminal_json: Some(json),
                ..
            }) => Some(serde_json::from_str::<NodeTerminalV1>(json).unwrap()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        terminal.primary,
        match outcome {
            WorkflowOutcome::Completed => NodePrimaryDispositionV1::Completed,
            WorkflowOutcome::Canceled => NodePrimaryDispositionV1::CanceledWorkflow,
            _ if cause == "AgentTimedOut" => NodePrimaryDispositionV1::TimedOut,
            _ => NodePrimaryDispositionV1::Failed,
        }
    );
    assert!(nodes[0].1.contains(cause), "{events:?}");
    assert!(
        events.iter().any(
            |e| matches!(e, Ok(WorkflowEvent::Terminal { outcome: actual, output })
        if *actual == outcome && output.contains(cause))
        ),
        "{events:?}"
    );
    if cause != "fixture.rejected" {
        assert!(!nodes[0].1.contains("fixture.rejected"), "{events:?}");
    }
}

fn precedence_cause(
    events: &[Result<WorkflowEvent, bridge_core::translator::AccountingFailureV1>],
    expected: Option<&BridgeError>,
) {
    let terminal = events
        .iter()
        .find_map(|event| match event {
            Ok(WorkflowEvent::NodeFinished {
                terminal_json: Some(json),
                ..
            }) => Some(serde_json::from_str::<NodeTerminalV1>(json).unwrap()),
            _ => None,
        })
        .expect("typed node terminal");
    assert_eq!(terminal.cause, expected.map(NodeCauseV1::from_bridge_error));
}

fn precedence_error(
    events: &[Result<WorkflowEvent, bridge_core::translator::AccountingFailureV1>],
    original: BridgeError,
    persistence: Option<BridgeError>,
) {
    let failure = events
        .iter()
        .find_map(|e| e.as_ref().err())
        .expect("error exit");
    assert_eq!(failure.original, original, "{events:?}");
    assert_eq!(failure.persistence, persistence);
    assert_eq!(failure.surfaced_error(), persistence.unwrap_or(original));
    assert!(
        !events.iter().any(|e| matches!(
            e,
            Ok(WorkflowEvent::NodeFinished { .. } | WorkflowEvent::Terminal { .. })
        )),
        "{events:?}"
    );
}

async fn precedence_preparation(
    resolution: bool,
    error: BridgeError,
    failures: usize,
    retries: u32,
    expected: &str,
) {
    let (h, admitted, accounting, executor) = precedence_fixture("final-only", retries).await;
    let prior = h.f.rows();
    let progress = checked_progress(&h).await;
    let errors = if resolution {
        &h.calls[1].resolution_errors
    } else {
        &h.calls[1].setup_errors
    };
    *errors.lock().unwrap() = vec![error.clone(); failures];
    let events = drive_restored_accounted(&h, &executor, admitted, Some(accounting), None).await;
    let success = expected == "answer";
    let primary = BridgeError::agent_failure(
        *progress.visits[0]
            .rejection
            .as_ref()
            .unwrap()
            .diagnostic
            .clone(),
    );
    precedence_cause(
        &events,
        if success {
            None
        } else if expected == "fixture.rejected" {
            Some(&primary)
        } else {
            Some(&error)
        },
    );
    precedence_output(
        &events,
        expected,
        if success {
            WorkflowOutcome::Completed
        } else {
            WorkflowOutcome::Failed
        },
    );
    assert_eq!(
        h.f.providers[1].prompts.load(Ordering::SeqCst),
        usize::from(success)
    );
    let attempts = if success { failures + 1 } else { failures };
    let mut states = vec![(false, true); attempts];
    if success {
        states[attempts - 1] = (true, true);
    }
    precedence_rows(&h, &prior, &states);
    let after = checked_progress(&h).await;
    assert_eq!(after.visits[..progress.visits.len()], progress.visits);
    assert_eq!(after.visits.len(), progress.visits.len() + attempts);
    assert_eq!(after.retries_consumed, (attempts - 1) as u32);
}

#[tokio::test]
async fn restored_precedence_model_resolution() {
    precedence_preparation(
        true,
        BridgeError::ModelNotAvailable,
        1,
        2,
        "fixture.rejected",
    )
    .await;
}
#[tokio::test]
async fn restored_precedence_model_configuration() {
    precedence_preparation(
        false,
        BridgeError::ModelNotAvailable,
        1,
        2,
        "fixture.rejected",
    )
    .await;
}
#[tokio::test]
async fn restored_precedence_timeout_exhausted() {
    for resolution in [false, true] {
        precedence_preparation(
            resolution,
            BridgeError::AgentTimedOut,
            2,
            2,
            "fixture.rejected",
        )
        .await;
    }
}
async fn precedence_preparation_cleanup(error: BridgeError, failures: usize, cleanup_fails: bool) {
    let (h, admitted, accounting, executor) = precedence_fixture("final-only", 2).await;
    let prior = h.f.rows();
    let progress = checked_progress(&h).await;
    *h.calls[1].setup_errors.lock().unwrap() = vec![error.clone(); failures];
    // Results are popped: only the exhausted attempt fails cleanup.
    let mut cleanup = vec![Ok(BackendCleanupDispositionV1::Complete); failures];
    if cleanup_fails {
        cleanup[0] = Err(BridgeError::StoreFailure);
    }
    *h.calls[1].cleanup_results.lock().unwrap() = cleanup;
    let events = drive_restored_accounted(&h, &executor, admitted, Some(accounting), None).await;
    let primary = BridgeError::agent_failure(
        *progress.visits[0]
            .rejection
            .as_ref()
            .unwrap()
            .diagnostic
            .clone(),
    );
    precedence_cause(&events, Some(if cleanup_fails { &error } else { &primary }));
    precedence_output(
        &events,
        &if cleanup_fails {
            format!("{error:?}")
        } else {
            "fixture.rejected".into()
        },
        WorkflowOutcome::Failed,
    );
    let ordinary: Vec<_> = events
        .into_iter()
        .map(|e| e.map_err(|f| f.surfaced_error()))
        .collect();
    assert_eq!(
        node_terminal(&ordinary).cleanup.disposition,
        if cleanup_fails {
            NodeCleanupDispositionV1::Failed
        } else {
            NodeCleanupDispositionV1::Complete
        },
    );
    assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 0);
    assert_eq!(h.calls[1].setups.load(Ordering::SeqCst), failures);
    assert_eq!(h.calls[1].cleaned.lock().unwrap().len(), failures);
    precedence_rows(&h, &prior, &vec![(false, true); failures]);
    let after = checked_progress(&h).await;
    assert_eq!(after.visits[..progress.visits.len()], progress.visits);
    assert_eq!(after.visits.len(), progress.visits.len() + failures);
    assert_eq!(after.retries_consumed, (failures - 1) as u32);
}

#[tokio::test]
async fn restored_precedence_model_cleanup_failure() {
    precedence_preparation_cleanup(BridgeError::ModelNotAvailable, 1, true).await;
}

#[tokio::test]
async fn restored_precedence_timeout_cleanup_failure() {
    precedence_preparation_cleanup(BridgeError::AgentTimedOut, 2, true).await;
}

#[tokio::test]
async fn restored_precedence_preparation_cleanup_success() {
    precedence_preparation_cleanup(BridgeError::ModelNotAvailable, 1, false).await;
    precedence_preparation_cleanup(BridgeError::AgentTimedOut, 2, false).await;
}

#[tokio::test]
async fn restored_precedence_timeout_retry_success() {
    for resolution in [false, true] {
        precedence_preparation(resolution, BridgeError::AgentTimedOut, 1, 2, "answer").await;
    }
}
#[tokio::test]
async fn restored_precedence_unknown_configuration() {
    for error in [
        BridgeError::FrameError,
        BridgeError::ResourceFlightUnsupported,
        BridgeError::IdentityUnavailable,
        BridgeError::AgentOverloaded,
        BridgeError::ConfigMismatch {
            field: "adapter_setup",
        },
        BridgeError::AgentNotAuthenticated,
    ] {
        let cause = format!("{error:?}");
        precedence_preparation(false, error, 1, 1, &cause).await;
    }
}
#[tokio::test]
async fn restored_precedence_unknown_resolution() {
    for error in [
        BridgeError::FrameError,
        BridgeError::ResourceFlightUnsupported,
        BridgeError::IdentityUnavailable,
        BridgeError::AgentOverloaded,
        BridgeError::ConfigMismatch {
            field: "adapter_resolution",
        },
        BridgeError::AgentNotAuthenticated,
    ] {
        let cause = format!("{error:?}");
        precedence_preparation(true, error, 1, 1, &cause).await;
    }
}

#[tokio::test]
async fn restored_precedence_setup_settlement_failure() {
    for original in [BridgeError::ModelNotAvailable, BridgeError::FrameError] {
        let (h, admitted, accounting, executor) = precedence_fixture("final-only", 1).await;
        let prior = h.f.rows();
        h.calls[1]
            .setup_errors
            .lock()
            .unwrap()
            .push(original.clone());
        rusqlite::Connection::open(h.f.dir.path().join("tasks.sqlite.usage-v1.sqlite")).unwrap()
            .execute_batch("PRAGMA journal_mode=PERSIST; CREATE TRIGGER fail_settlement BEFORE UPDATE ON usage_attempts_v1 WHEN json_extract(NEW.record_json,'$.attribution.agent')='target' AND json_extract(NEW.record_json,'$.finalized')=1 BEGIN SELECT RAISE(ABORT,'settlement fault'); END;").unwrap();
        let events =
            drive_restored_accounted(&h, &executor, admitted, Some(accounting), None).await;
        precedence_error(&events, original, Some(BridgeError::StoreFailure));
        precedence_rows(&h, &prior, &[(false, false)]);
        assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 0);
    }
}

async fn precedence_persistence_boundary(boundary: &str) {
    let (h, admitted, accounting, executor) = precedence_fixture("final-only", 1).await;
    let prior = h.f.rows();
    let (path, sql) = if boundary == "publication" {
        ("tasks.sqlite", "CREATE TRIGGER fail_publication BEFORE UPDATE ON task_candidate_progress_v1 BEGIN SELECT RAISE(ABORT,'publication fault'); END;")
    } else {
        (
            "tasks.sqlite.usage-v1.sqlite",
            if boundary == "reservation" {
                "CREATE TRIGGER fail_reservation BEFORE INSERT ON usage_attempts_v1 BEGIN SELECT RAISE(ABORT,'reservation fault'); END;"
            } else {
                "CREATE TRIGGER fail_pre_poll BEFORE UPDATE ON usage_attempts_v1 WHEN json_extract(NEW.record_json,'$.attribution.agent')='target' AND json_extract(NEW.record_json,'$.dispatched')=0 AND json_extract(NEW.record_json,'$.finalized')=0 BEGIN SELECT RAISE(ABORT,'pre-poll fault'); END;"
            },
        )
    };
    rusqlite::Connection::open(h.f.dir.path().join(path))
        .unwrap()
        .execute_batch(&format!("PRAGMA journal_mode=PERSIST; {sql}"))
        .unwrap();
    let events = drive_restored_accounted(&h, &executor, admitted, Some(accounting), None).await;
    precedence_error(
        &events,
        BridgeError::StoreFailure,
        Some(BridgeError::StoreFailure),
    );
    precedence_rows(
        &h,
        &prior,
        if boundary == "reservation" {
            &[]
        } else {
            &[(false, true)]
        },
    );
    assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn restored_precedence_reservation_failure() {
    precedence_persistence_boundary("reservation").await;
}
#[tokio::test]
async fn restored_precedence_publication_failure() {
    precedence_persistence_boundary("publication").await;
}
#[tokio::test]
async fn restored_precedence_pre_poll_write_failure() {
    precedence_persistence_boundary("pre-poll").await;
}

#[tokio::test]
async fn restored_precedence_scope_dispatch_barrier() {
    let (h, admitted, accounting, executor) = precedence_fixture("final-only", 1).await;
    let prior = h.f.rows();
    let scope = h.f.dir.path().join("scope");
    let barrier: PromptDispatchBarrier = Arc::new(move || {
        let scope = scope.clone();
        Box::pin(async move {
            std::fs::rename(&scope, scope.with_file_name("old-scope")).unwrap();
            std::fs::create_dir(scope).unwrap();
        })
    });
    let events = drive_restored_controlled(
        &h,
        &executor,
        admitted,
        Some(accounting),
        None,
        CancellationToken::new(),
        Some(barrier),
    )
    .await;
    precedence_output(
        &events,
        "ConfigMismatch { field: \"fallback_scope\" }",
        WorkflowOutcome::Failed,
    );
    precedence_cause(
        &events,
        Some(&BridgeError::ConfigMismatch {
            field: "fallback_scope",
        }),
    );
    precedence_rows(&h, &prior, &[(false, true)]);
    assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 0);
}

async fn precedence_prompt_outcome(target: &'static str) {
    let (h, admitted, accounting, executor) = precedence_fixture(target, 1).await;
    let prior = h.f.rows();
    // Distinguish B's safe rejection from A's checked diagnostic.
    let mut rejection = checked_progress(&h).await.visits[0]
        .rejection
        .as_ref()
        .unwrap()
        .diagnostic
        .as_ref()
        .clone();
    let mut json = serde_json::to_value(&rejection).unwrap();
    json["code"] = "fixture.target".into();
    rejection = serde_json::from_value(json).unwrap();
    if matches!(target, "reject" | "forget-unknown" | "accepted") {
        if target == "accepted" {
            let mut json = serde_json::to_value(&rejection).unwrap();
            json["prompt_may_have_been_accepted"] = true.into();
            rejection = serde_json::from_value(json).unwrap();
        }
        *h.calls[1].prompt_error.lock().unwrap() =
            Some(BridgeError::agent_failure(rejection.clone()));
    }
    let events = drive_restored_accounted(&h, &executor, admitted, Some(accounting), None).await;
    let (cause, finalized, outcome) = match target {
        "final-only" => ("answer", true, WorkflowOutcome::Completed),
        "parity-error" => ("PermissionDenied", false, WorkflowOutcome::Failed),
        "accepted" => ("fixture.target", false, WorkflowOutcome::Failed),
        "missing" => ("AgentCrashed", false, WorkflowOutcome::Failed),
        "local-cancel" => ("", true, WorkflowOutcome::Failed),
        "reject" => ("fixture.target", true, WorkflowOutcome::Failed),
        _ => ("fixture.target", false, WorkflowOutcome::Failed),
    };
    precedence_output(&events, cause, outcome);
    let typed = match target {
        "final-only" | "local-cancel" => None,
        "parity-error" => Some(BridgeError::PermissionDenied),
        "missing" => Some(BridgeError::AgentCrashed {
            reason: "backend stream ended before terminal Done".into(),
        }),
        _ => Some(BridgeError::agent_failure(rejection.clone())),
    };
    precedence_cause(&events, typed.as_ref());
    precedence_rows(&h, &prior, &[(true, finalized)]);
    assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 1);
    if matches!(target, "reject" | "forget-unknown") {
        assert_eq!(
            *checked_progress(&h)
                .await
                .visits
                .last()
                .unwrap()
                .rejection
                .as_ref()
                .unwrap()
                .diagnostic,
            rejection
        );
    }
}

#[tokio::test]
async fn restored_precedence_success() {
    precedence_prompt_outcome("final-only").await;
}
#[tokio::test]
async fn restored_precedence_terminal_failure() {
    precedence_prompt_outcome("parity-error").await;
}
#[tokio::test]
async fn restored_precedence_uncertain_acceptance() {
    precedence_prompt_outcome("accepted").await;
}
#[tokio::test]
async fn restored_precedence_incomplete_terminal() {
    precedence_prompt_outcome("missing").await;
}
#[tokio::test]
async fn restored_precedence_terminal_provider_failure() {
    precedence_prompt_outcome("local-cancel").await;
}
#[tokio::test]
async fn restored_precedence_safe_rejection() {
    precedence_prompt_outcome("reject").await;
}
#[tokio::test]
async fn restored_precedence_incomplete_cleanup() {
    precedence_prompt_outcome("forget-unknown").await;
}

async fn precedence_cancellation(target: &'static str) {
    let (h, admitted, accounting, executor) = precedence_fixture(target, 1).await;
    let prior = h.f.rows();
    let cancel = CancellationToken::new();
    let before = target == "final-only";
    let barrier = before.then(|| {
        let cancel = cancel.clone();
        Arc::new(move || {
            let cancel = cancel.clone();
            Box::pin(async move {
                cancel.cancel();
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        }) as PromptDispatchBarrier
    });
    let drive = drive_restored_controlled(
        &h,
        &executor,
        admitted,
        Some(accounting),
        None,
        cancel.clone(),
        barrier,
    );
    let stop = async {
        if !before {
            h.f.providers[1].configure.0.notified().await;
            cancel.cancel();
        }
    };
    let (events, ()) = tokio::join!(drive, stop);
    let cause = match target {
        "parity-cancel-error" => "PermissionDenied",
        "parity-cancel-cleanup-error" => "StoreFailure",
        _ => "cancel",
    };
    precedence_output(&events, cause, WorkflowOutcome::Canceled);
    precedence_rows(&h, &prior, &[(!before, before)]);
    assert_eq!(
        h.f.providers[1].prompts.load(Ordering::SeqCst),
        usize::from(!before)
    );
}
#[tokio::test]
async fn restored_precedence_cancel_before_prompt() {
    precedence_cancellation("final-only").await;
}
#[tokio::test]
async fn restored_precedence_cancel_after_prompt() {
    precedence_cancellation("parity-cancel").await;
}
#[tokio::test]
async fn restored_precedence_cancel_error() {
    precedence_cancellation("parity-cancel-error").await;
}
#[tokio::test]
async fn restored_precedence_cancel_cleanup_error() {
    precedence_cancellation("parity-cancel-cleanup-error").await;
}

#[tokio::test]
async fn restored_precedence_refusal_before_reservation() {
    for fault in ["owner", "authority", "scope", "accounting"] {
        let (h, admitted, accounting, executor) = precedence_fixture("final-only", 1).await;
        let prior = h.f.rows();
        let progress = checked_progress(&h).await;
        if fault == "owner" {
            restored_owner(&h, &admitted, 1).await;
        }
        if fault == "authority" {
            let mut snapshot = h.f.snapshot();
            snapshot.entries[1].args.push("drift".into());
            h.registry.apply(snapshot).await.unwrap();
        }
        if fault == "scope" {
            let scope = h.f.dir.path().join("scope");
            std::fs::rename(&scope, scope.with_file_name("old-scope")).unwrap();
            std::fs::create_dir(scope).unwrap();
        }
        let events = drive_restored_accounted(
            &h,
            &executor,
            admitted,
            (fault != "accounting").then_some(accounting),
            None,
        )
        .await;
        precedence_error(
            &events,
            if fault == "authority" {
                BridgeError::ConfigMismatch {
                    field: "fallback_authority",
                }
            } else {
                BridgeError::InvalidStateTransition
            },
            None,
        );
        precedence_rows(&h, &prior, &[]);
        assert_eq!(checked_progress(&h).await, progress);
        assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 0);
        assert!(h.f.spawns.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn restored_precedence_accounting_refusal_after_reservation() {
    let (h, admitted, accounting, executor) = precedence_fixture("final-only", 1).await;
    let prior = h.f.rows();
    let replay = restored_executor(&h, &admitted, &accounting).await.unwrap();
    let events = drive_restored_accounted(
        &h,
        &executor,
        admitted.clone(),
        Some(accounting.clone()),
        None,
    )
    .await;
    precedence_output(&events, "answer", WorkflowOutcome::Completed);
    precedence_rows(&h, &prior, &[(true, true)]);
    let prior = h.f.rows();
    let progress = checked_progress(&h).await;
    let events = drive_restored_accounted(&h, &replay, admitted, Some(accounting), None).await;
    precedence_error(
        &events,
        BridgeError::InvalidStateTransition,
        Some(BridgeError::InvalidStateTransition),
    );
    precedence_rows(&h, &prior, &[(false, true)]);
    assert_eq!(checked_progress(&h).await, progress);
    assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn restored_precedence_preparation_canceled() {
    let (h, admitted, accounting, executor) = precedence_fixture("final-only", 1).await;
    let prior = h.f.rows();
    let cancel = CancellationToken::new();
    let cancel_setup = cancel.clone();
    *h.calls[1].entry_check.lock().unwrap() = Some(Box::new(move || cancel_setup.cancel()));
    h.calls[1]
        .setup_errors
        .lock()
        .unwrap()
        .push(BridgeError::ModelNotAvailable);
    let events = drive_restored_controlled(
        &h,
        &executor,
        admitted,
        Some(accounting),
        None,
        cancel,
        None,
    )
    .await;
    precedence_output(&events, "ModelNotAvailable", WorkflowOutcome::Canceled);
    precedence_cause(&events, Some(&BridgeError::ModelNotAvailable));
    precedence_rows(&h, &prior, &[(false, true)]);
    assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn restored_precedence_allowlisted_variant_at_prompt_keeps_own_cause() {
    for original in [BridgeError::ModelNotAvailable, BridgeError::AgentTimedOut] {
        let (h, admitted, accounting, executor) = precedence_fixture("final-only", 1).await;
        let prior = h.f.rows();
        *h.calls[1].prompt_error.lock().unwrap() = Some(original.clone());
        let events =
            drive_restored_accounted(&h, &executor, admitted, Some(accounting), None).await;
        precedence_output(&events, &format!("{original:?}"), WorkflowOutcome::Failed);
        precedence_cause(&events, Some(&original));
        precedence_rows(&h, &prior, &[(true, false)]);
        assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn restored_precedence_later_preparation_keeps_own_cause() {
    let (h, initial) = recovery_candidate_fixture_target("continuation", "reject").await;
    let prior_rows = h.f.rows();
    let prior = checked_progress(&h).await;
    let (admitted, accounting) = restored_owner(&h, &initial, 1).await;
    let executor = restored_executor(&h, &admitted, &accounting).await.unwrap();
    h.calls[2]
        .setup_errors
        .lock()
        .unwrap()
        .push(BridgeError::FrameError);
    let events = drive_restored_accounted(&h, &executor, admitted, Some(accounting), None).await;
    precedence_output(&events, "FrameError", WorkflowOutcome::Failed);
    precedence_cause(&events, Some(&BridgeError::FrameError));
    let rows = h.f.rows();
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows.iter().find(|row| row.id == prior_rows[0].id),
        Some(&prior_rows[0])
    );
    for (agent, dispatched) in [("target", true), ("last", false)] {
        let row = rows
            .iter()
            .find(|row| row.attribution.agent == agent)
            .unwrap();
        assert_eq!(row.attribution.effective_model.as_deref(), Some(agent));
        assert_eq!((row.dispatched, row.finalized), (dispatched, true));
    }
    assert_eq!(h.f.providers[0].prompts.load(Ordering::SeqCst), 0);
    assert_eq!(h.f.providers[1].prompts.load(Ordering::SeqCst), 1);
    assert_eq!(h.f.providers[2].prompts.load(Ordering::SeqCst), 0);
    assert_eq!(
        checked_progress(&h).await.visits[..prior.visits.len()],
        prior.visits
    );
}
