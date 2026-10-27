mod common;
use bridge_core::{
    accounting::{AccountingV1, BillingIdentityProviderV1, BillingIdentityV1, UsageAttemptV1},
    domain::*,
    error::BridgeError,
    governance::*,
    ids::*,
    ports::*,
    routing::*,
    terminal_evidence::*,
    workflow_history::*,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

fn rejected(accepted: bool, class: &str) -> BridgeError {
    BridgeError::AgentFailure {
        diagnostic: Box::new(
            serde_json::from_value(serde_json::json!({
                "schema_version":1, "failed_phase":"prompt_start", "class":class,
                "disposition":"fatal", "code":"fixture.rejected", "summary":"original rejection",
                "causes":[], "stderr_observed":false, "stderr_line_count":0,
                "prompt_may_have_been_accepted":accepted
            }))
            .unwrap(),
        ),
    }
}
pub(crate) struct Provider {
    pub(crate) behavior: &'static str,
    pub(crate) setup: AtomicUsize,
    pub(crate) prompts: AtomicUsize,
    pub(crate) cancels: AtomicUsize,
    pub(crate) forgets: AtomicUsize,
    cleaned: Arc<AtomicUsize>,
    pub(crate) inputs: Mutex<Vec<Vec<Part>>>,
    pub(crate) cleanup: (tokio::sync::Notify, tokio::sync::Notify),
    pub(crate) configure: (tokio::sync::Notify, tokio::sync::Notify),
    pub(crate) sessions: Mutex<Vec<SessionId>>,
    activity: Mutex<Option<Arc<dyn bridge_core::attempt_activity::AttemptRecorder>>>,
}
#[async_trait::async_trait]
impl AgentBackend for Provider {
    fn terminal_evidence_capability(&self) -> EvidenceCapability {
        if self.behavior.starts_with("evidence-") {
            EvidenceCapability::V1
        } else {
            EvidenceCapability::Unsupported
        }
    }
    fn resource_flight_v1(&self) -> Result<BackendResourceFlightV1, BridgeError> {
        Ok(BackendResourceFlightV1::LegacyV2)
    }
    async fn configure_session(
        &self,
        session: &SessionId,
        _: &SessionSpec,
    ) -> Result<(), BridgeError> {
        self.setup.fetch_add(1, Ordering::SeqCst);
        self.sessions.lock().unwrap().push(session.clone());
        if self.behavior == "blocked-setup" {
            self.configure.0.notify_one();
            self.configure.1.notified().await;
        }
        if self.behavior == "checkout-store" {
            Err(BridgeError::StoreFailure)
        } else if self.behavior == "setup" {
            Err(BridgeError::ModelNotAvailable)
        } else {
            Ok(())
        }
    }
    async fn prompt(&self, _: &SessionId, _: Vec<Part>) -> Result<BackendStream, BridgeError> {
        unreachable!("accounted coordinator uses observers")
    }
    async fn prompt_with_observers(
        &self,
        _: &SessionId,
        parts: Vec<Part>,
        obs: BackendObservers,
    ) -> Result<BackendStream, BridgeError> {
        self.prompts.fetch_add(1, Ordering::SeqCst);
        self.inputs.lock().unwrap().push(parts);
        *self.activity.lock().unwrap() = Some(obs.activity.clone());
        if self.behavior == "overflow" {
            obs.activity.mark_overflowed();
        }
        if matches!(self.behavior, "evidence-valid" | "evidence-malformed") {
            let b = obs.terminal_evidence.binding().unwrap();
            let mut envelope = serde_json::to_value(b).unwrap();
            envelope.as_object_mut().unwrap().extend(serde_json::json!({
                "version":TURN_EVIDENCE_VERSION, "native_turn_id":"native", "sequence":1,
                "producer":"completed", "final_presence":"nonempty", "ordered_notifications_drained":true,
                "complete":self.behavior == "evidence-valid"
            }).as_object().unwrap().clone());
            obs.terminal_evidence
                .accept(serde_json::from_value(envelope).unwrap());
            obs.terminal_evidence.record_deliverable_final();
        }
        let error = rejected(
            self.behavior == "accepted",
            if matches!(self.behavior, "billing" | "billing-match") {
                "billing_failure"
            } else if self.behavior == "region" {
                "region_restricted"
            } else if self.behavior == "rate" {
                "rate_limited"
            } else {
                "quota_exhausted"
            },
        );
        match self.behavior {
            "parked" => Ok(Box::pin(futures::stream::pending())),
            "reject" | "shared-reject" | "accepted" | "billing" | "billing-match" | "region"
            | "rate" | "cleanup" | "blocked-cleanup" | "cleanup-effect" => Err(error),
            "tool" => {
                obs.activity.record(
                    bridge_core::attempt_activity::AttemptPhase::Tool,
                    bridge_core::attempt_activity::ActivityReason::ToolTransition,
                    1,
                );
                Err(error)
            }
            "text" => Ok(Box::pin(futures::stream::iter([
                Ok(Update::Text("partial".into())),
                Err(error),
            ]))),
            "missing" => Ok(Box::pin(futures::stream::empty())),
            "permission" => Ok(Box::pin(futures::stream::iter([
                Ok(Update::Permission(PermissionRequest::read())),
                Err(error),
            ]))),
            _ => Ok(Box::pin(futures::stream::iter([
                Ok(Update::Usage(bridge_core::orch::UsageSnapshot {
                    terminal: Some(bridge_core::orch::TerminalUsage {
                        total_tokens: 15,
                        input_tokens: 10,
                        output_tokens: 5,
                        thought_tokens: None,
                        cached_read_tokens: None,
                        cached_write_tokens: None,
                    }),
                    ..Default::default()
                })),
                Ok(Update::Text("answer".into())),
                Ok(Update::done("end_turn")),
            ]))),
        }
    }
    async fn release_session_checked(
        &self,
        _: &SessionId,
    ) -> Result<BackendCleanupDispositionV1, BridgeError> {
        if matches!(
            self.behavior,
            "blocked-cleanup" | "shared-reject" | "cleanup-effect"
        ) {
            self.cleanup.0.notify_one();
            self.cleanup.1.notified().await;
        }
        if self.behavior == "cleanup-effect" {
            self.activity.lock().unwrap().as_ref().unwrap().record(
                bridge_core::attempt_activity::AttemptPhase::Tool,
                bridge_core::attempt_activity::ActivityReason::ToolTransition,
                1,
            );
        }
        self.cleaned.fetch_add(1, Ordering::SeqCst);
        Ok(if self.behavior == "cleanup" {
            BackendCleanupDispositionV1::Unknown
        } else {
            BackendCleanupDispositionV1::Complete
        })
    }
    async fn forget_session(&self, _: &SessionId) {
        self.forgets.fetch_add(1, Ordering::SeqCst);
    }
    async fn cancel(&self, _: &SessionId) -> Result<(), BridgeError> {
        self.cancels.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
struct SecondDispatchFailure(
    Arc<bridge_store::sqlite::SqliteStore>,
    AtomicUsize,
    Option<Arc<(tokio::sync::Notify, tokio::sync::Notify)>>,
);
#[async_trait::async_trait]
impl WorkflowHistoryStore for SecondDispatchFailure {
    async fn reserve(&self, r: &AttemptReservation) -> Result<(), LedgerError> {
        self.0.reserve(r).await
    }
    async fn mark_prompt_acceptance(&self, id: &AttemptId, a: &str) -> Result<(), LedgerError> {
        if self.1.fetch_add(1, Ordering::SeqCst) == 1 {
            if let Some(gate) = &self.2 {
                gate.0.notify_one();
                gate.1.notified().await;
            } else {
                return Err(LedgerError::new(LedgerUnavailableReason::Io));
            }
        }
        self.0.mark_prompt_acceptance(id, a).await
    }
    async fn terminalize(
        &self,
        id: &AttemptId,
        t: &AttemptTerminal,
    ) -> Result<TerminalWrite, LedgerError> {
        self.0.terminalize(id, t).await
    }
    async fn set_pinned(&self, id: &AttemptId, p: bool) -> Result<bool, LedgerError> {
        self.0.set_pinned(id, p).await
    }
    async fn interrupt_active(&self, t: i64) -> Result<u64, LedgerError> {
        self.0.interrupt_active(t).await
    }
    async fn latest_reservation_for_task(
        &self,
        id: &TaskId,
    ) -> Result<Option<AttemptReservation>, LedgerError> {
        self.0.latest_reservation_for_task(id).await
    }
    async fn completed_between(
        &self,
        a: i64,
        b: i64,
    ) -> Result<Vec<CompletedAttempt>, LedgerError> {
        self.0.completed_between(a, b).await
    }
}
struct Billing;
impl BillingIdentityProviderV1 for Billing {
    fn identity(&self, agent: &str, model: Option<&str>) -> Option<BillingIdentityV1> {
        (model == Some(agent)).then(|| BillingIdentityV1 {
            account_group: agent.into(),
            billing_mode: "metered".into(),
        })
    }
}
pub(crate) struct Fixture {
    #[allow(dead_code)] // Read by the served cancellation regression.
    pub(crate) dispatch_gate: Arc<(tokio::sync::Notify, tokio::sync::Notify)>,
    pub(crate) dir: tempfile::TempDir,
    pub(crate) coordinator: Arc<bridge_coordinator::Coordinator>,
    pub(crate) providers: Vec<Arc<Provider>>,
    pub(crate) spawns: Arc<Mutex<Vec<String>>>,
    pub(crate) store: Arc<bridge_store::sqlite::SqliteStore>,
}
impl Fixture {
    pub(crate) fn new(source: &'static str, target: &'static str, chain: bool) -> Self {
        Self::with_dispatch_failure(source, target, chain, false)
    }
    fn with_dispatch_failure(
        source: &'static str,
        target: &'static str,
        chain: bool,
        fail: bool,
    ) -> Self {
        Self::with_options(source, target, chain, fail, false)
    }
    pub(crate) fn with_options(
        source: &'static str,
        target: &'static str,
        chain: bool,
        fail: bool,
        governed: bool,
    ) -> Self {
        Self::with_billing(source, target, chain, fail, governed, Arc::new(Billing))
    }
    pub(crate) fn with_billing(
        source: &'static str,
        target: &'static str,
        chain: bool,
        fail: bool,
        governed: bool,
        billing: Arc<dyn BillingIdentityProviderV1>,
    ) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(
            bridge_store::sqlite::SqliteStore::open_shared_history(
                &dir.path().join("tasks.sqlite"),
            )
            .unwrap(),
        );
        let scope = dir.path().join("scope");
        std::fs::create_dir(&scope).unwrap();
        let dispatch_gate = Arc::<(tokio::sync::Notify, tokio::sync::Notify)>::default();
        let cleaned = Arc::new(AtomicUsize::new(0));
        let providers: Vec<_> = [source, target, "setup", "parked"]
            .into_iter()
            .map(|behavior| {
                Arc::new(Provider {
                    behavior,
                    setup: AtomicUsize::new(0),
                    prompts: AtomicUsize::new(0),
                    cancels: AtomicUsize::new(0),
                    forgets: AtomicUsize::new(0),
                    cleaned: cleaned.clone(),
                    inputs: Mutex::new(vec![]),
                    cleanup: Default::default(),
                    configure: Default::default(),
                    sessions: Default::default(),
                    activity: Default::default(),
                })
            })
            .collect();
        let seed = common::single_agent_registry("source", providers[0].clone());
        let entries: Vec<_> = ["source", "target", "last", "other"]
            .into_iter()
            .map(|name| {
                let mut entry = (*seed
                    .entry_snapshot(&AgentId::parse("source").unwrap())
                    .unwrap())
                .clone();
                entry.id = AgentId::parse(name).unwrap();
                entry.model = Some(
                    if source == "shared-reject" {
                        "shared"
                    } else {
                        name
                    }
                    .into(),
                );
                entry.cwd = Some(scope.display().to_string());
                entry
            })
            .collect();
        let candidates: Vec<_> = entries
            .iter()
            .map(|entry| FallbackCandidateV1 {
                agent: entry.id.clone(),
                model: entry.model.clone().unwrap(),
                posture: CandidatePostureV1 {
                    kind: "acp".into(),
                    cmd: entry.cmd.clone(),
                    cwd: entry.cwd.clone().unwrap(),
                    session_cwd: entry.cwd.clone().unwrap(),
                    mcp_delivery: "acp".into(),
                    ..Default::default()
                },
            })
            .collect();
        let spawns = Arc::new(Mutex::new(vec![]));
        let log = spawns.clone();
        let backends = providers.clone();
        let spawn: bridge_registry::registry::SpawnFn = Arc::new(move |entry| {
            let name = entry.id.as_str();
            let index = ["source", "target", "last", "other"]
                .iter()
                .position(|x| *x == name)
                .unwrap();
            if index > 0 && index < 3 {
                assert!(
                    cleaned.load(Ordering::SeqCst) > 0,
                    "successor preceded checked cleanup"
                );
            }
            log.lock().unwrap().push(name.into());
            let b = backends[index].clone();
            Box::pin(async move {
                if b.behavior == "blocked-resolve" {
                    b.configure.0.notify_one();
                    b.configure.1.notified().await;
                }
                if b.behavior == "registry-init" {
                    return Err(BridgeError::AgentCrashed {
                        reason: "registry initialization".into(),
                    });
                }
                Ok(b as Arc<dyn AgentBackend>)
            })
        });
        let registry = Arc::new(
            bridge_registry::registry::Registry::new(
                RegistrySnapshot {
                    default: candidates[0].agent.clone(),
                    entries,
                    allowed_cmds: vec!["test-cmd".into()],
                    billing_identities: vec![],
                },
                spawn,
            )
            .unwrap(),
        );
        let coordinator = bridge_coordinator::Coordinator::new(
            Arc::new(bridge_coordinator::session_manager::SessionManager::new(
                registry.clone(),
                std::time::Duration::from_secs(60),
            )),
            None,
            Arc::new(Default::default()),
            store.clone(),
            store.clone(),
            Arc::new(bridge_policy::permission::AutoPolicy),
            registry,
            Arc::new(bridge_coordinator::clock::SystemClock),
            None,
            None,
            Arc::new(bridge_observ::NoopObserver),
            3,
        )
        .with_workflow_history(Ok(if fail || target == "ledger-gate" {
            Arc::new(SecondDispatchFailure(
                store.clone(),
                AtomicUsize::new(0),
                (target == "ledger-gate").then(|| dispatch_gate.clone()),
            ))
        } else {
            store.clone()
        }))
        .with_accounting(AccountingV1 {
            billing: Some(billing),
            budget_policy: governed.then(|| {
                Arc::new(
                    BudgetPolicyV1::new(
                        vec![BudgetRuleV1 {
                            id: "ample".into(),
                            selector: BudgetSelectorV1::EachRequest,
                            limits: BudgetLimitsV1 {
                                turns: Some(10),
                                terminal_tokens: None,
                            },
                        }],
                        vec![],
                    )
                    .unwrap(),
                )
            }),
            ..AccountingV1::new(store.clone())
        })
        .with_mcp_fallback(if chain {
            vec![
                FrozenFallbackChainV1 {
                    id: "capacity".into(),
                    source: candidates[0].clone(),
                    causes: if source == "billing-match" {
                        [FallbackCauseV1::BillingFailure].into()
                    } else {
                        [
                            FallbackCauseV1::QuotaExhausted,
                            FallbackCauseV1::RegionRestricted,
                            FallbackCauseV1::RateLimited,
                        ]
                        .into()
                    },
                    steps: candidates[1..3].to_vec(),
                },
                FrozenFallbackChainV1 {
                    id: "never-reroot".into(),
                    source: candidates[1].clone(),
                    causes: [FallbackCauseV1::QuotaExhausted].into(),
                    steps: vec![candidates[0].clone()],
                },
            ]
        } else {
            vec![]
        });
        Self {
            dispatch_gate,
            dir,
            coordinator: Arc::new(coordinator),
            providers,
            spawns,
            store,
        }
    }
    pub(crate) async fn call(&self, tool: &str, context: Option<&str>) -> serde_json::Value {
        let (client, server) = tokio::io::duplex(65536);
        let (read, write) = tokio::io::split(server);
        let serve = bridge_mcp::serve(read, write, self.coordinator.clone());
        let caller = async {
            let (r, mut w) = tokio::io::split(client);
            let owner = AttemptIdentity::initial().unwrap();
            for request in [
                serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}),
                serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":tool,"arguments":{
                    "input":"original input", "context":context, "execution_id":owner.execution_id.as_str(), "attempt_id":owner.attempt_id.as_str()
                }}}),
            ] {
                w.write_all(format!("{request}\n").as_bytes())
                    .await
                    .unwrap();
            }
            let mut lines = tokio::io::BufReader::new(r).lines();
            lines.next_line().await.unwrap().unwrap();
            let reply = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            w.shutdown().await.unwrap();
            reply
        };
        let (served, reply) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(serve, caller)
        })
        .await
        .unwrap();
        served.unwrap();
        reply
    }
    #[allow(dead_code)] // Shared with served and production-composition witnesses.
    pub(crate) fn server(&self) -> Arc<bridge_a2a_inbound::server::InboundServer> {
        struct Route;
        impl RouteDecision for Route {
            fn route(&self, meta: &TaskMeta) -> Result<RouteTarget, BridgeError> {
                Ok(RouteTarget::Local(
                    meta.agent.clone().unwrap_or(AgentId::parse("source")?),
                ))
            }
        }
        Arc::new(bridge_a2a_inbound::server::InboundServer::from_coordinator(
            self.coordinator.clone(),
            Arc::new(Route),
            Arc::new(bridge_policy::auth::AlwaysGrant),
            "http://localhost",
            Arc::new(bridge_a2a_outbound::StubDelegation),
            "source",
        ))
    }
    #[allow(dead_code)]
    pub(crate) async fn served(
        &self,
        method: &str,
        owner: &AttemptIdentity,
        context: Option<&str>,
    ) -> String {
        Self::served_on(self.server().router(), method, owner, context).await
    }
    #[allow(dead_code)]
    pub(crate) async fn served_on(
        router: axum::Router,
        method: &str,
        owner: &AttemptIdentity,
        context: Option<&str>,
    ) -> String {
        use tower::ServiceExt;
        let request = serde_json::json!({"jsonrpc":"2.0", "id":1, "method":method, "params":{
            "message":{"text":"original input", "taskId":owner.execution_id.as_str(), "contextId":context,
                "metadata":{"account_group":"forged", "billing_mode":"subscription", "fallback_chain":"forged",
                    "a2a-bridge.execution_id":owner.execution_id.as_str(), "a2a-bridge.attempt_id":owner.attempt_id.as_str()}}
        }});
        let response = router
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/")
                    .header("content-type", "application/json")
                    .header("A2A-Version", "1.0")
                    .body(axum::body::Body::from(request.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        String::from_utf8(
            axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap()
    }
    #[allow(dead_code)] // Shared by served reload and production composition witnesses.
    pub(crate) fn snapshot(&self) -> RegistrySnapshot {
        let registry = self.coordinator.registry();
        RegistrySnapshot {
            default: registry.default_id(),
            allowed_cmds: vec!["test-cmd".into()],
            entries: ["source", "target", "last"]
                .map(|name| {
                    (*registry
                        .entry_snapshot(&AgentId::parse(name).unwrap())
                        .unwrap())
                    .clone()
                })
                .into(),
            billing_identities: vec![],
        }
    }
    pub(crate) fn rows(&self) -> Vec<UsageAttemptV1> {
        let db = rusqlite::Connection::open(self.dir.path().join("tasks.sqlite.usage-v1.sqlite"))
            .unwrap();
        let rows = db
            .prepare("SELECT record_json FROM usage_attempts_v1")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
            .collect();
        rows
    }
}
fn params() -> bridge_coordinator::params::OpParams {
    bridge_coordinator::params::OpParams::from_mcp_args(
        &serde_json::json!({"input":"original input"}),
    )
    .unwrap()
}
fn content(reply: &serde_json::Value) -> serde_json::Value {
    serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}
#[tokio::test]
async fn accounted_rejection_routes_after_cleanup_with_candidate_billing_and_exact_input() {
    let f = Fixture::new("reject", "success", true);
    let reply = f.call("run", None).await;
    assert_ne!(reply["result"]["isError"], true, "{reply}");
    assert_eq!(*f.spawns.lock().unwrap(), ["source", "target"]);
    let rows = f.rows();
    assert_eq!(rows.len(), 2);
    for name in ["source", "target"] {
        let row = rows.iter().find(|r| r.attribution.agent == name).unwrap();
        assert!(row.dispatched);
        assert_eq!(row.attribution.effective_model.as_deref(), Some(name));
        assert_eq!(
            row.attribution.billing.as_ref().unwrap().account_group,
            name
        );
    }
    assert_eq!(
        *f.providers[0].inputs.lock().unwrap(),
        *f.providers[1].inputs.lock().unwrap()
    );
    assert_eq!(f.providers[2].setup.load(Ordering::SeqCst), 0);
    let output = content(&reply);
    assert_eq!(output["text"], "answer");
    let id = AttemptId::parse(output["attempt_id"].as_str().unwrap()).unwrap();
    let record = f.coordinator.attempt_status(&id).await.unwrap();
    let terminal = record.terminal.unwrap();
    assert_eq!(terminal.outcome, "completed");
    assert_eq!(terminal.terminal_evidence_counts.reached, 2);
    for row in rows {
        assert_eq!(row.attribution.run_attempt.as_deref(), Some(id.as_str()));
    }
    let owner = record.reservation.identity;
    assert!(f
        .coordinator
        .prompt_with_identity_outcome(params(), owner.resume().unwrap())
        .await
        .is_err());
    assert!(f
        .coordinator
        .prompt_with_identity_outcome(params(), owner)
        .await
        .is_err());
    assert_eq!(f.rows().len(), 2);
}
#[tokio::test]
async fn selected_setup_failure_is_accounted_undispatched_and_preserves_original() {
    let f = Fixture::new("reject", "setup", true);
    let reply = f.call("run", None).await;
    assert!(reply.to_string().contains("model not available"), "{reply}");
    let rows = f.rows();
    assert_eq!(rows.len(), 2);
    assert!(
        !rows
            .iter()
            .find(|r| r.attribution.agent == "target")
            .unwrap()
            .dispatched
    );
    assert_eq!(f.providers[1].prompts.load(Ordering::SeqCst), 0);
    assert_eq!(f.providers[2].setup.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn unsafe_effects_success_absent_chain_and_nonmatching_cause_never_prepare_targets() {
    for (source, chain) in [
        ("cleanup", true),
        ("text", true),
        ("tool", true),
        ("permission", true),
        ("accepted", true),
        ("success", true),
        ("reject", false),
        ("billing", true),
    ] {
        let f = Fixture::new(source, "setup", chain);
        let reply = f.call("run", None).await;
        assert_eq!(
            reply["result"]["isError"] == true,
            source != "success",
            "{source}: {reply}"
        );
        assert_eq!(*f.spawns.lock().unwrap(), ["source"]);
        assert_eq!(f.rows().len(), 1);
        assert_eq!(f.providers[1].setup.load(Ordering::SeqCst), 0);
        assert_eq!(f.providers[2].setup.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn candidate_accounting_failure_preserves_source_and_surfaces_persistence() {
    use bridge_core::task_store::TaskStore;
    let f = Fixture::new("reject", "success", true);
    f.store.usage_summary_v1(&Default::default()).await.unwrap();
    let db = rusqlite::Connection::open(f.dir.path().join("tasks.sqlite.usage-v1.sqlite")).unwrap();
    db.execute_batch("PRAGMA journal_mode=PERSIST; CREATE TRIGGER refuse_target BEFORE INSERT ON usage_attempts_v1 WHEN json_extract(NEW.record_json, '$.attribution.agent') = 'target' BEGIN SELECT RAISE(ABORT, 'candidate insert refused'); END;").unwrap();
    let failure = f
        .coordinator
        .prompt_with_identity_outcome(params(), AttemptIdentity::initial().unwrap())
        .await
        .err()
        .unwrap();
    assert_eq!(failure.original, rejected(false, "quota_exhausted"));
    assert_eq!(failure.persistence, Some(BridgeError::StoreFailure));
    assert_eq!(failure.surfaced_error(), BridgeError::StoreFailure);
    assert_eq!(*f.spawns.lock().unwrap(), ["source"]);
    let f = Fixture::new("reject", "setup", true);
    let failure = f
        .coordinator
        .prompt_with_identity_outcome(params(), AttemptIdentity::initial().unwrap())
        .await
        .err()
        .unwrap();
    assert_eq!(failure.original, rejected(false, "quota_exhausted"));
    assert_eq!(failure.persistence, Some(BridgeError::ModelNotAvailable));
}
#[tokio::test]
async fn candidate_accounting_start_failure_preserves_source_with_undispatched_row() {
    use bridge_core::task_store::TaskStore;
    let f = Fixture::new("reject", "success", true);
    f.store.usage_summary_v1(&Default::default()).await.unwrap();
    let db = rusqlite::Connection::open(f.dir.path().join("tasks.sqlite.usage-v1.sqlite")).unwrap();
    // FAIL retains the inserted row while reporting an accounting-start error.
    db.execute_batch("PRAGMA journal_mode=PERSIST; CREATE TRIGGER refuse_target_start AFTER INSERT ON usage_attempts_v1 WHEN json_extract(NEW.record_json, '$.attribution.agent') = 'target' BEGIN SELECT RAISE(FAIL, 'candidate start refused'); END;").unwrap();
    let failure = f
        .coordinator
        .prompt_with_identity_outcome(params(), AttemptIdentity::initial().unwrap())
        .await
        .err()
        .unwrap();
    assert_eq!(failure.original, rejected(false, "quota_exhausted"));
    assert_eq!(failure.persistence, Some(BridgeError::StoreFailure));
    assert_eq!(failure.surfaced_error(), BridgeError::StoreFailure);
    let rows = f.rows();
    assert_eq!(rows.len(), 2);
    let target = rows
        .iter()
        .find(|r| r.attribution.agent == "target")
        .unwrap();
    assert!(!target.dispatched);
    assert!(!target.finalized);
    assert_eq!(*f.spawns.lock().unwrap(), ["source"]);
    assert_eq!(f.providers[1].setup.load(Ordering::SeqCst), 0);
    assert_eq!(f.providers[1].prompts.load(Ordering::SeqCst), 0);
    assert_eq!(f.providers[2].setup.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn caller_cancellation_during_checked_cleanup_prevents_successor() {
    let f = Fixture::new("blocked-cleanup", "success", true);
    let coordinator = f.coordinator.clone();
    let task = tokio::spawn(async move {
        coordinator
            .prompt_with_identity_outcome(params(), AttemptIdentity::initial().unwrap())
            .await
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        f.providers[0].cleanup.0.notified(),
    )
    .await
    .unwrap();
    task.abort();
    assert!(task.await.err().unwrap().is_cancelled());
    f.providers[0].cleanup.1.notify_one();
    assert_eq!(*f.spawns.lock().unwrap(), ["source"]);
    assert_eq!(f.rows().len(), 1);
}
#[tokio::test]
async fn overflow_marker_survives_production_activity_wrapper() {
    use bridge_core::workflow_history::WorkflowHistoryStore;
    for behavior in ["overflow", "success"] {
        let f = Fixture::new(behavior, "setup", false);
        let reply = content(&f.call("run", None).await);
        let id = AttemptId::parse(reply["attempt_id"].as_str().unwrap()).unwrap();
        let tally = f.store.activity_tally(&id).await.unwrap().unwrap();
        assert_eq!(tally.overflowed, behavior == "overflow");
    }
}

#[tokio::test]
async fn fallback_target_terminal_evidence_is_resolved_before_aggregate_projection() {
    for (behavior, reason, valid, missing, invalid) in [
        (
            "evidence-missing",
            "protocol_terminal_evidence_missing",
            0,
            1,
            0,
        ),
        (
            "evidence-malformed",
            "protocol_terminal_evidence_malformed",
            0,
            0,
            1,
        ),
        ("evidence-valid", "completed_final", 1, 0, 0),
    ] {
        let f = Fixture::new("reject", behavior, true);
        let output = content(&f.call("run", None).await);
        assert_eq!(output["stop_reason"], reason, "{behavior}");
        let id = AttemptId::parse(output["attempt_id"].as_str().unwrap()).unwrap();
        let terminal = f
            .coordinator
            .attempt_status(&id)
            .await
            .unwrap()
            .terminal
            .unwrap();
        assert_eq!(
            terminal.outcome,
            if valid == 1 { "completed" } else { "failed" }
        );
        let counts = terminal.terminal_evidence_counts;
        assert_eq!(
            (counts.reached, counts.valid, counts.missing, counts.invalid),
            (2, valid, missing, invalid)
        );
    }
}
#[tokio::test]
async fn second_dispatch_failure_preserves_source_and_counts_only_reached_provider() {
    let f = Fixture::with_dispatch_failure("reject", "success", true, true);
    let owner = AttemptIdentity::initial().unwrap();
    let failure = f
        .coordinator
        .prompt_with_identity_outcome(params(), owner.clone())
        .await
        .err()
        .unwrap();
    assert_eq!(failure.original, rejected(false, "quota_exhausted"));
    assert!(matches!(
        failure.persistence,
        Some(BridgeError::DurableEvidenceUnavailable { reason: "io" })
    ));
    let rows = f.rows();
    assert_eq!(rows.len(), 2);
    assert!(
        !rows
            .iter()
            .find(|r| r.attribution.agent == "target")
            .unwrap()
            .dispatched
    );
    assert_eq!(f.providers[1].setup.load(Ordering::SeqCst), 1);
    assert_eq!(f.providers[1].prompts.load(Ordering::SeqCst), 0);
    let terminal = f
        .store
        .attempt(&owner.attempt_id)
        .await
        .unwrap()
        .unwrap()
        .terminal
        .unwrap();
    assert_eq!(terminal.terminal_evidence_counts.reached, 1);
}

async fn candidate_dispatch_invariant(stage: &str) {
    use bridge_core::task_store::TaskStore;
    let f = Fixture::with_dispatch_failure(
        "reject",
        match stage {
            "checkout" => "checkout-store",
            "checkout-model" => "setup",
            "registry-init" => "registry-init",
            _ => "billing",
        },
        true,
        stage == "history",
    );
    if matches!(stage, "insert" | "start" | "usage-update") {
        f.store.usage_summary_v1(&Default::default()).await.unwrap();
        let db =
            rusqlite::Connection::open(f.dir.path().join("tasks.sqlite.usage-v1.sqlite")).unwrap();
        let trigger = match stage {
            "insert" => "BEFORE INSERT",
            "start" => "AFTER INSERT",
            _ => "BEFORE UPDATE",
        };
        let action = if stage == "start" { "FAIL" } else { "ABORT" };
        db.execute_batch(&format!("PRAGMA journal_mode=PERSIST; CREATE TRIGGER refuse_target {trigger} ON usage_attempts_v1 WHEN json_extract(NEW.record_json, '$.attribution.agent') = 'target' BEGIN SELECT RAISE({action}, 'candidate refused'); END;")).unwrap();
    }
    let owner = AttemptIdentity::initial().unwrap();
    let failure = f
        .coordinator
        .prompt_with_identity_outcome(params(), owner.clone())
        .await
        .err()
        .unwrap();
    let dispatched = stage == "provider";
    assert_eq!(
        failure.original,
        rejected(
            false,
            if dispatched {
                "billing_failure"
            } else {
                "quota_exhausted"
            }
        ),
        "{stage}"
    );
    let persistence = match stage {
        "insert" | "start" | "checkout" | "usage-update" => Some(BridgeError::StoreFailure),
        "history" => Some(BridgeError::DurableEvidenceUnavailable { reason: "io" }),
        "checkout-model" => Some(BridgeError::ModelNotAvailable),
        "registry-init" => Some(BridgeError::AgentCrashed {
            reason: "registry initialization".into(),
        }),
        _ => None,
    };
    assert_eq!(failure.persistence, persistence, "{stage}");
    let target = f
        .rows()
        .into_iter()
        .find(|r| r.attribution.agent == "target");
    if stage == "insert" {
        assert!(target.is_none());
    } else {
        assert_eq!(target.unwrap().dispatched, dispatched, "{stage}");
    }
    assert_eq!(
        f.providers[1].prompts.load(Ordering::SeqCst),
        usize::from(dispatched),
        "{stage}"
    );
    assert_eq!(f.providers[2].setup.load(Ordering::SeqCst), 0, "{stage}");
    assert_eq!(f.providers[2].prompts.load(Ordering::SeqCst), 0, "{stage}");
    assert!(
        !f.spawns.lock().unwrap().iter().any(|s| s == "last"),
        "{stage}"
    );
    if stage == "usage-update" {
        assert_eq!(f.providers[1].setup.load(Ordering::SeqCst), 1);
    }
    let terminal = f
        .store
        .attempt(&owner.attempt_id)
        .await
        .unwrap()
        .unwrap()
        .terminal
        .unwrap();
    assert_eq!(
        terminal.terminal_evidence_counts.reached,
        if dispatched { 2 } else { 1 },
        "{stage}"
    );
}

#[tokio::test]
async fn candidate_usage_dispatch_update_failure_preserves_source_and_reached_count() {
    candidate_dispatch_invariant("usage-update").await;
}

#[tokio::test]
async fn every_candidate_pre_dispatch_failure_preserves_source_and_reachability() {
    for stage in [
        "insert",
        "start",
        "checkout",
        "checkout-model",
        "registry-init",
        "history",
        "usage-update",
        "provider",
    ] {
        candidate_dispatch_invariant(stage).await;
    }
}

#[tokio::test]
async fn source_accounting_start_failure_keeps_base_mcp_projection() {
    use bridge_core::task_store::TaskStore;
    let f = Fixture::new("reject", "success", true);
    f.store.usage_summary_v1(&Default::default()).await.unwrap();
    let db = rusqlite::Connection::open(f.dir.path().join("tasks.sqlite.usage-v1.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER refuse_source BEFORE INSERT ON usage_attempts_v1 WHEN json_extract(NEW.record_json, '$.attribution.agent') = 'source' BEGIN SELECT RAISE(ABORT, 'source refused'); END;").unwrap();
    let failure = f
        .coordinator
        .prompt_with_identity_outcome(params(), AttemptIdentity::initial().unwrap())
        .await
        .err()
        .unwrap();
    assert_eq!(failure.original, BridgeError::StoreFailure);
    assert_eq!(failure.persistence, None);
    // Exact main (44e0601d) MCP response for a source accounting-start refusal.
    assert_eq!(
        f.call("run", None).await,
        serde_json::json!({
            "jsonrpc":"2.0", "id":2, "result":{
                "isError":true, "content":[{"type":"text", "text":"store failure"}]
            }
        })
    );
    assert!(f
        .providers
        .iter()
        .all(|p| p.prompts.load(Ordering::SeqCst) == 0));
    assert!(f.spawns.lock().unwrap().is_empty());
}

// Non-storage failures must retain the candidate error as well as the source rejection.
#[tokio::test]
async fn candidate_non_storage_checkout_failure() {
    candidate_dispatch_invariant("checkout-model").await;
}
#[tokio::test]
async fn candidate_registry_initialization_failure() {
    candidate_dispatch_invariant("registry-init").await;
}

// Baseline witness: run this same test on a git-archive copy of 44e0601d,
// omitting only the fixture's with_mcp_fallback call (absent from main).
#[tokio::test]
async fn source_terminalization_failure_projection() {
    for (source, chain) in [("reject", false), ("billing", true)] {
        for terminalization_fails in [false, true] {
            let f = Fixture::new(source, "success", chain);
            // Initialize history before installing a fault only on terminal writes.
            f.store
                .attempt(&AttemptIdentity::initial().unwrap().attempt_id)
                .await
                .unwrap();
            if terminalization_fails {
                let db = rusqlite::Connection::open(f.dir.path().join("tasks.sqlite")).unwrap();
                db.execute_batch("CREATE TRIGGER refuse_terminal BEFORE UPDATE OF terminal_json ON workflow_attempt_summaries WHEN NEW.terminal_json IS NOT NULL BEGIN SELECT RAISE(ABORT, 'terminal refused'); END;").unwrap();
            }
            let reply = f.call("run", None).await;
            assert_eq!(
                reply,
                serde_json::json!({
                    "jsonrpc":"2.0", "id":2, "result":{
                        "isError":true, "content":[{"type":"text", "text":"agent crashed"}]
                    }
                }),
                "source={source} chain={chain} terminalization_fails={terminalization_fails}"
            );
            assert_eq!(*f.spawns.lock().unwrap(), ["source"]);
            let rows = f.rows();
            assert_eq!(rows.len(), 1);
            assert!(rows[0].dispatched && !rows[0].finalized);
            let db = rusqlite::Connection::open(f.dir.path().join("tasks.sqlite")).unwrap();
            let terminal_count: u64 = db.query_row(
                "SELECT count(*) FROM workflow_attempt_summaries WHERE terminal_json IS NOT NULL",
                [], |r| r.get(0),
            ).unwrap();
            assert_eq!(terminal_count, u64::from(!terminalization_fails));
        }
    }
}

#[tokio::test]
async fn governed_turn_never_falls_back() {
    for governed in [true, false] {
        let f = Fixture::with_options("reject", "success", true, false, governed);
        let owner = AttemptIdentity::initial().unwrap();
        let result = f
            .coordinator
            .prompt_with_identity_outcome(params(), owner.clone())
            .await;
        if governed {
            let failure = result.err().unwrap();
            assert_eq!(failure.original, rejected(false, "quota_exhausted"));
            assert_eq!(failure.persistence, None);
            assert_eq!(failure.cleanup, BackendCleanupDispositionV1::Unknown);
            assert!(!failure.replay.cleanup_completed);
        } else {
            assert_eq!(result.unwrap().text, "answer");
        }
        assert_eq!(
            *f.spawns.lock().unwrap(),
            if governed {
                vec!["source"]
            } else {
                vec!["source", "target"]
            }
        );
        let rows = f.rows();
        assert_eq!(rows.len(), if governed { 1 } else { 2 });
        assert!(rows.iter().all(|r| r.dispatched));
        assert!(
            !rows
                .iter()
                .find(|r| r.attribution.agent == "source")
                .unwrap()
                .finalized
        );
        assert_eq!(f.providers[0].prompts.load(Ordering::SeqCst), 1);
        assert_eq!(
            f.providers[1].setup.load(Ordering::SeqCst),
            usize::from(!governed)
        );
        assert_eq!(
            f.providers[1].prompts.load(Ordering::SeqCst),
            usize::from(!governed)
        );
        assert_eq!(f.providers[2].setup.load(Ordering::SeqCst), 0);
        let terminal = f
            .store
            .attempt(&owner.attempt_id)
            .await
            .unwrap()
            .unwrap()
            .terminal
            .unwrap();
        assert_eq!(
            terminal.terminal_evidence_counts.reached,
            if governed { 1 } else { 2 }
        );
        if governed {
            let db = rusqlite::Connection::open(f.dir.path().join("tasks.sqlite.usage-v1.sqlite"))
                .unwrap();
            let reservations: Vec<serde_json::Value> = db
                .prepare("SELECT record_json FROM governance_reservations_v1")
                .unwrap()
                .query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
                .collect();
            assert_eq!(reservations.len(), 1);
            assert_eq!(
                reservations[0]["reservation"]["candidate"]["agent"],
                "source"
            );
            // Main retains liability after a rejected prompt without a provider Done.
            assert_eq!(
                reservations[0]["liability"],
                serde_json::json!({"Pending": 0})
            );
        }
    }
}

// Deferred: C2b3 production-provider billing witness and the target protocol
// reason in the durable aggregate (diagnostic fidelity).
