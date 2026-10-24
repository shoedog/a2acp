mod common;
use bridge_core::{
    accounting::{AccountingV1, BillingIdentityProviderV1, BillingIdentityV1, UsageAttemptV1},
    domain::*,
    error::BridgeError,
    ids::*,
    ports::*,
    routing::*,
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
struct Provider {
    behavior: &'static str,
    setup: AtomicUsize,
    prompts: AtomicUsize,
    cleaned: Arc<AtomicUsize>,
    inputs: Mutex<Vec<Vec<Part>>>,
}
#[async_trait::async_trait]
impl AgentBackend for Provider {
    fn resource_flight_v1(&self) -> Result<BackendResourceFlightV1, BridgeError> {
        Ok(BackendResourceFlightV1::LegacyV2)
    }
    async fn configure_session(&self, _: &SessionId, _: &SessionSpec) -> Result<(), BridgeError> {
        self.setup.fetch_add(1, Ordering::SeqCst);
        if self.behavior == "setup" {
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
        let error = rejected(
            self.behavior == "accepted",
            if self.behavior == "billing" {
                "billing_failure"
            } else {
                "quota_exhausted"
            },
        );
        match self.behavior {
            "reject" | "accepted" | "billing" | "cleanup" => Err(error),
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
            "permission" => Ok(Box::pin(futures::stream::iter([
                Ok(Update::Permission(PermissionRequest::read())),
                Err(error),
            ]))),
            _ => Ok(Box::pin(futures::stream::iter([
                Ok(Update::Text("answer".into())),
                Ok(Update::done("end_turn")),
            ]))),
        }
    }
    async fn release_session_checked(
        &self,
        _: &SessionId,
    ) -> Result<BackendCleanupDispositionV1, BridgeError> {
        self.cleaned.fetch_add(1, Ordering::SeqCst);
        Ok(if self.behavior == "cleanup" {
            BackendCleanupDispositionV1::Unknown
        } else {
            BackendCleanupDispositionV1::Complete
        })
    }
    async fn cancel(&self, _: &SessionId) -> Result<(), BridgeError> {
        Ok(())
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
struct Fixture {
    dir: tempfile::TempDir,
    coordinator: Arc<bridge_coordinator::Coordinator>,
    providers: Vec<Arc<Provider>>,
    spawns: Arc<Mutex<Vec<String>>>,
}
impl Fixture {
    fn new(source: &'static str, target: &'static str, chain: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(
            bridge_store::sqlite::SqliteStore::open_shared_history(
                &dir.path().join("tasks.sqlite"),
            )
            .unwrap(),
        );
        let cleaned = Arc::new(AtomicUsize::new(0));
        let providers: Vec<_> = [source, target, "setup"]
            .into_iter()
            .map(|behavior| {
                Arc::new(Provider {
                    behavior,
                    setup: AtomicUsize::new(0),
                    prompts: AtomicUsize::new(0),
                    cleaned: cleaned.clone(),
                    inputs: Mutex::new(vec![]),
                })
            })
            .collect();
        let seed = common::single_agent_registry("source", providers[0].clone());
        let entries: Vec<_> = ["source", "target", "last"]
            .into_iter()
            .map(|name| {
                let mut entry = (*seed
                    .entry_snapshot(&AgentId::parse("source").unwrap())
                    .unwrap())
                .clone();
                entry.id = AgentId::parse(name).unwrap();
                entry.model = Some(name.into());
                entry.cwd = Some(dir.path().display().to_string());
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
            let index = ["source", "target", "last"]
                .iter()
                .position(|x| *x == name)
                .unwrap();
            if index > 0 {
                assert!(
                    cleaned.load(Ordering::SeqCst) > 0,
                    "successor preceded checked cleanup"
                );
            }
            log.lock().unwrap().push(name.into());
            let b = backends[index].clone();
            Box::pin(async move { Ok(b as Arc<dyn AgentBackend>) })
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
        .with_workflow_history(Ok(store.clone()))
        .with_accounting(AccountingV1 {
            billing: Some(Arc::new(Billing)),
            ..AccountingV1::new(store)
        })
        .with_mcp_fallback(if chain {
            vec![FrozenFallbackChainV1 {
                id: "capacity".into(),
                source: candidates[0].clone(),
                causes: [FallbackCauseV1::QuotaExhausted].into(),
                steps: candidates[1..].to_vec(),
            }]
        } else {
            vec![]
        });
        Self {
            dir,
            coordinator: Arc::new(coordinator),
            providers,
            spawns,
        }
    }
    async fn call(&self, tool: &str, context: Option<&str>) -> serde_json::Value {
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
    fn rows(&self) -> Vec<UsageAttemptV1> {
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
#[tokio::test]
async fn accounted_rejection_routes_after_cleanup_with_candidate_billing_and_exact_input() {
    let f = Fixture::new("reject", "success", true);
    let reply = f.call("run", None).await;
    assert_eq!(reply["result"]["isError"], false, "{reply}");
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
}
#[tokio::test]
async fn selected_setup_failure_is_accounted_undispatched_and_preserves_original() {
    let f = Fixture::new("reject", "setup", true);
    let reply = f.call("run", None).await;
    assert!(reply.to_string().contains("original rejection"), "{reply}");
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
            reply["result"]["isError"],
            source != "success",
            "{source}: {reply}"
        );
        assert_eq!(*f.spawns.lock().unwrap(), ["source"]);
        assert_eq!(f.rows().len(), 1);
        assert_eq!(f.providers[1].setup.load(Ordering::SeqCst), 0);
        assert_eq!(f.providers[2].setup.load(Ordering::SeqCst), 0);
    }
}
