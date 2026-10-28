#[path = "fallback_direct.rs"]
mod direct;
use bridge_core::{
    domain::*, error::BridgeError, ids::*, ports::*, task_store::TaskStore, translator::Event,
    workflow_history::WorkflowHistoryStore,
};
use direct::Fixture;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[derive(Default)]
struct Peer {
    fail: bool,
    hold: bool,
    late: bool,
    pending: bool,
    opened: tokio::sync::Notify,
    delegated: tokio::sync::Notify,
    finished: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    prompts: AtomicUsize,
    cancels: AtomicUsize,
}
#[async_trait::async_trait]
impl DelegationPort for Peer {
    async fn delegate(
        &self,
        _: &AuthContext,
        _: &TaskId,
        parts: Vec<Part>,
    ) -> Result<Delegation, BridgeError> {
        self.prompts.fetch_add(1, Ordering::SeqCst);
        assert_eq!(parts[0].text, "original input");
        if self.pending {
            self.opened.notify_one();
            self.delegated.notified().await;
        }
        let (peer_tx, peer_task) =
            tokio::sync::watch::channel((!self.late).then(|| PeerTaskId("peer-task".into())));
        Ok(Delegation {
            peer_task,
            events: {
                let fail = self.fail;
                let hold = self.hold;
                let release = self.release.clone();
                let finished = self.finished.clone();
                use futures::StreamExt;
                Box::pin(
                    futures::stream::iter([
                        Ok(Event::status("peer first")),
                        Ok(Event::status("peer second")),
                    ])
                    .chain(futures::stream::once(async move {
                        if hold {
                            release.notified().await;
                        }
                        let _ = peer_tx.send(Some(PeerTaskId("peer-task".into())));
                        finished.notify_one();
                        if fail {
                            Err(BridgeError::UpstreamA2aError)
                        } else {
                            Ok(Event::artifact("peer answer"))
                        }
                    })),
                )
            },
        })
    }
    async fn cancel(&self, _: &PeerTaskId) -> Result<(), BridgeError> {
        self.cancels.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
#[tokio::test]
async fn fanout_safe_rejection_routes_local_only() {
    for (method, cause) in ["SendMessage", "SendStreamingMessage"]
        .into_iter()
        .flat_map(|method| {
            ["reject", "region", "rate", "billing-match"].map(move |cause| (method, cause))
        })
    {
        let f = Fixture::new(cause, "success", true);
        let peer = Arc::new(Peer::default());
        let owner = AttemptIdentity::initial().unwrap();
        let body = Fixture::served_on(
            f.served_server(true, peer.clone()).router(),
            method,
            &owner,
            None,
        )
        .await;
        let reply = normalized(&body, method);
        assert_eq!(
            reply["result"]["status"]["state"], "TASK_STATE_COMPLETED",
            "{body}"
        );
        assert!(
            !body.contains("agent crashed"),
            "recovered failure leaked: {body}"
        );
        let arts = reply["result"]["artifacts"].as_array().unwrap();
        assert!(
            arts.iter()
                .any(|a| a["name"] == "source" && a["parts"][0]["text"] == "answer"),
            "{body}"
        );
        assert_eq!(arts.len(), 2);
        assert_eq!(peer.prompts.load(Ordering::SeqCst), 1);
        assert_eq!(*f.spawns.lock().unwrap(), ["source", "target"]);
        assert_eq!(
            *f.providers[0].inputs.lock().unwrap(),
            *f.providers[1].inputs.lock().unwrap()
        );
        let rows = f.rows();
        assert_eq!(rows.len(), 2);
        assert_ne!(rows[0].id, rows[1].id);
        assert_eq!(rows[0].attribution.node, rows[1].attribution.node);
        assert_eq!(rows[0].attribution.node.as_deref(), Some("source"));
        assert!(rows.iter().all(|r| r.dispatched
            && r.attribution.run_attempt.as_deref() == Some(owner.attempt_id.as_str())));
        let terminal = f
            .store
            .attempt(&owner.attempt_id)
            .await
            .unwrap()
            .unwrap()
            .terminal
            .unwrap();
        assert_eq!(terminal.terminal_evidence_counts.reached, 3);
    }
}

fn normalized(body: &str, method: &str) -> serde_json::Value {
    if method == "SendMessage" {
        return serde_json::from_str(body).unwrap();
    }
    let frames: Vec<serde_json::Value> = body
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(|line| serde_json::from_str(line.trim()).unwrap())
        .collect();
    let terminals: Vec<_> = frames
        .iter()
        .filter(|v| {
            v["statusUpdate"]["status"]["state"]
                .as_str()
                .is_some_and(|s| s != "TASK_STATE_WORKING")
        })
        .collect();
    assert_eq!(terminals.len(), 1, "{body}");
    let artifacts: Vec<_> = frames
        .iter()
        .filter_map(|v| v["artifactUpdate"].get("artifact").cloned())
        .collect();
    serde_json::json!({"result":{"status":terminals[0]["statusUpdate"]["status"],"artifacts":artifacts}})
}

async fn open_sse(router: axum::Router, owner: &AttemptIdentity) -> axum::body::Body {
    use tower::ServiceExt;
    router.oneshot(axum::http::Request::builder().method("POST").uri("/").header("content-type","application/json").header("A2A-Version","1.0").body(axum::body::Body::from(serde_json::json!({"jsonrpc":"2.0","id":1,"method":"SendStreamingMessage","params":{"message":{"text":"original input","taskId":owner.execution_id.as_str(),"metadata":{"a2a-bridge.execution_id":owner.execution_id.as_str(),"a2a-bridge.attempt_id":owner.attempt_id.as_str()}}}}).to_string())).unwrap()).await.unwrap().into_body()
}

#[tokio::test]
async fn fanout_exposure_is_per_leg() {
    use futures::StreamExt;
    for local_exposed in [false, true] {
        let f = Fixture::new(
            if local_exposed {
                "status-reject"
            } else {
                "blocked-cleanup"
            },
            "success",
            true,
        );
        let peer = Arc::new(Peer::default());
        let owner = AttemptIdentity::initial().unwrap();
        let mut stream = open_sse(f.served_server(true, peer.clone()).router(), &owner)
            .await
            .into_data_stream();
        let mut body = String::new();
        while !body.contains("peer answer") || (local_exposed && !body.contains("source-status")) {
            body.push_str(std::str::from_utf8(&stream.next().await.unwrap().unwrap()).unwrap());
        }
        // The client consumed the publication before permitting the safe rejection.
        f.providers[0].cleanup.1.notify_one();
        while let Some(bytes) = stream.next().await {
            body.push_str(std::str::from_utf8(&bytes.unwrap()).unwrap());
        }
        let reply = normalized(&body, "SendStreamingMessage");
        assert_eq!(reply["result"]["status"]["state"], "TASK_STATE_COMPLETED");
        assert_eq!(body.matches("peer answer").count(), 1, "{body}");
        assert!(body.find("peer first").unwrap() < body.find("peer second").unwrap());
        assert!(body.find("peer second").unwrap() < body.find("peer answer").unwrap());
        assert_eq!(
            body.contains("agent crashed"),
            local_exposed,
            "original failure: {body}"
        );
        assert_eq!(
            body.contains("\"text\":\"answer\""),
            !local_exposed,
            "{body}"
        );
        assert_eq!(
            f.providers[1].prompts.load(Ordering::SeqCst),
            usize::from(!local_exposed)
        );
        assert_eq!(
            f.providers[1].setup.load(Ordering::SeqCst),
            usize::from(!local_exposed)
        );
        assert_eq!(f.rows().len(), if local_exposed { 1 } else { 2 });
        assert_eq!(peer.prompts.load(Ordering::SeqCst), 1);
    }
}

async fn call(f: &Fixture, peer: Arc<Peer>, owner: &AttemptIdentity) -> serde_json::Value {
    serde_json::from_str(
        &Fixture::served_on(
            f.served_server(true, peer).router(),
            "SendMessage",
            owner,
            None,
        )
        .await,
    )
    .unwrap()
}
#[tokio::test]
async fn fanout_chain_is_finite() {
    for target in ["reject", "billing", "exhaust"] {
        let f = Fixture::new("reject", target, true);
        let body = call(
            &f,
            Arc::new(Peer::default()),
            &AttemptIdentity::initial().unwrap(),
        )
        .await;
        assert_eq!(body["result"]["status"]["state"], "TASK_STATE_COMPLETED");
        assert_eq!(
            *f.spawns.lock().unwrap(),
            if target != "billing" {
                vec!["source", "target", "last"]
            } else {
                vec!["source", "target"]
            }
        );
        assert_eq!(
            f.providers[2].prompts.load(Ordering::SeqCst),
            usize::from(target == "exhaust")
        );
        if target == "exhaust" {
            assert!(f.providers[..3]
                .iter()
                .all(|p| p.prompts.load(Ordering::SeqCst) == 1));
            assert!(f.rows().iter().all(|r| r.dispatched));
        }
    }
}
#[tokio::test]
async fn fanout_replay_and_authority_refusals() {
    for (behavior, chain) in [
        ("accepted", true),
        ("text", true),
        ("tool", true),
        ("permission", true),
        ("cleanup", true),
        ("missing", true),
        ("overflow", true),
        ("billing", true),
        ("success", true),
        ("reject", false),
    ] {
        let f = Fixture::new(behavior, "success", chain);
        call(
            &f,
            Arc::new(Peer::default()),
            &AttemptIdentity::initial().unwrap(),
        )
        .await;
        assert_eq!(*f.spawns.lock().unwrap(), ["source"], "{behavior}");
        assert_eq!(f.rows().len(), 1);
    }
    for effect in [false, true] {
        let f = Fixture::new(
            if effect {
                "cleanup-effect"
            } else {
                "blocked-cleanup"
            },
            "success",
            true,
        );
        let owner = AttemptIdentity::initial().unwrap();
        let call = call(&f, Arc::new(Peer::default()), &owner);
        tokio::pin!(call);
        tokio::select! { _ = f.providers[0].cleanup.0.notified() => {}, body = &mut call => panic!("premature aggregate: {body}") }
        if !effect {
            let mut snapshot = f.snapshot();
            snapshot.entries[1].mode = Some("write".into());
            f.coordinator.registry().apply(snapshot).await.unwrap();
        }
        f.providers[0].cleanup.1.notify_one();
        call.await;
        assert_eq!(f.providers[1].setup.load(Ordering::SeqCst), 0);
    }
}
#[tokio::test]
async fn fanout_governed_never_routes() {
    let f = Fixture::with_options("reject", "success", true, false, true);
    let peer = Arc::new(Peer::default());
    let body = call(&f, peer.clone(), &AttemptIdentity::initial().unwrap()).await;
    assert!(body["error"].is_object(), "{body}");
    assert!(f.spawns.lock().unwrap().is_empty());
    assert_eq!(peer.prompts.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn fanout_aggregate_waits_for_logical_legs() {
    for (method, source, target, peer_fails, succeeds) in ["SendMessage", "SendStreamingMessage"]
        .into_iter()
        .flat_map(|method| {
            [
                ("blocked-cleanup", "success", true, true),
                ("reject", "blocked-resolve", false, true),
                ("reject", "reject", false, true),
                ("reject", "reject", true, false),
            ]
            .map(move |(source, target, peer_fails, succeeds)| {
                (method, source, target, peer_fails, succeeds)
            })
        })
    {
        let f = Fixture::new(source, target, true);
        let peer = Arc::new(Peer {
            fail: peer_fails,
            ..Default::default()
        });
        let owner = AttemptIdentity::initial().unwrap();
        let call = Fixture::served_on(
            f.served_server(true, peer.clone()).router(),
            method,
            &owner,
            None,
        );
        tokio::pin!(call);
        if source == "blocked-cleanup" || target == "blocked-resolve" {
            tokio::select! { _ = async { if source == "blocked-cleanup" { f.providers[0].cleanup.0.notified().await; } else { f.providers[1].configure.0.notified().await; } } => {}, body = &mut call => panic!("premature aggregate: {body}") }
            tokio::select! { _ = peer.finished.notified() => {}, body = &mut call => panic!("aggregate before local leg settled: {body}") }
            f.providers[0].cleanup.1.notify_one();
            f.providers[1].configure.1.notify_one();
        }
        let raw = call.await;
        let body = normalized(&raw, method);
        assert_eq!(
            body["result"]["status"]["state"],
            if succeeds {
                "TASK_STATE_COMPLETED"
            } else {
                "TASK_STATE_FAILED"
            },
            "{body}"
        );
        assert_eq!(peer.prompts.load(Ordering::SeqCst), 1);
    }
}
#[tokio::test]
async fn fanout_candidate_accounting_and_evidence() {
    for (target, dispatch_failure) in [
        ("setup", false),
        ("usage", false),
        ("success", true),
        ("evidence-missing", false),
        ("evidence-malformed", false),
        ("evidence-valid", false),
    ] {
        let f = Fixture::with_dispatch_failure("reject", target, true, dispatch_failure);
        if target == "usage" {
            f.store.usage_summary_v1(&Default::default()).await.unwrap();
            let db = rusqlite::Connection::open(f.dir.path().join("tasks.sqlite.usage-v1.sqlite"))
                .unwrap();
            db.execute_batch("PRAGMA journal_mode=PERSIST; CREATE TRIGGER refuse_target BEFORE UPDATE ON usage_attempts_v1 WHEN json_extract(NEW.record_json, '$.attribution.agent') = 'target' BEGIN SELECT RAISE(ABORT, 'dispatch refused'); END;").unwrap();
        }
        let owner = AttemptIdentity::initial().unwrap();
        let body = call(
            &f,
            Arc::new(Peer {
                fail: true,
                ..Default::default()
            }),
            &owner,
        )
        .await;
        assert_eq!(
            body["result"]["status"]["state"],
            if target == "evidence-valid" {
                "TASK_STATE_COMPLETED"
            } else {
                "TASK_STATE_FAILED"
            },
            "{body}"
        );
        let rows = f.rows();
        assert_eq!(rows.len(), 2, "{target}: {body}");
        let dispatched = target.starts_with("evidence-");
        assert_eq!(
            rows.iter()
                .find(|r| r.attribution.agent == "target")
                .unwrap()
                .dispatched,
            dispatched
        );
        assert_eq!(
            f.providers[1].prompts.load(Ordering::SeqCst),
            usize::from(dispatched)
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
            if dispatched { 3 } else { 2 }
        );
    }
}

async fn cancel(router: &axum::Router, owner: &AttemptIdentity) {
    use tower::ServiceExt;
    let response = router.clone().oneshot(axum::http::Request::builder().method("POST").uri("/").header("content-type","application/json").header("A2A-Version","1.0").body(axum::body::Body::from(serde_json::json!({"jsonrpc":"2.0","id":2,"method":"CancelTask","params":{"taskId":owner.execution_id.as_str()}}).to_string())).unwrap()).await.unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("TASK_STATE_CANCELED"));
}
#[tokio::test]
async fn fanout_cancel_before_peer_dispatch_and_positive_control() {
    for canceled in [false, true] {
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            let f = Fixture::new("reject", "first-gate", true);
            let peer = Arc::new(Peer::default());
            let router = f.served_server(true, peer.clone()).router();
            let owner = AttemptIdentity::initial().unwrap();
            let call = Fixture::served_on(router.clone(), "SendMessage", &owner, None);
            tokio::pin!(call);
            tokio::select! {
                _ = f.dispatch_gate.0.notified() => {},
                body = &mut call => panic!("initial barrier not held: {body}"),
            }
            if canceled {
                cancel(&router, &owner).await;
            }
            f.dispatch_gate.1.notify_one();
            let body = call.await;
            assert_eq!(
                peer.prompts.load(Ordering::SeqCst),
                usize::from(!canceled),
                "peer dispatch after acknowledged cancellation"
            );
            assert_eq!(
                f.providers[0].prompts.load(Ordering::SeqCst),
                usize::from(!canceled)
            );
            assert_eq!(
                f.providers[1].prompts.load(Ordering::SeqCst),
                usize::from(!canceled)
            );
            assert!(
                body.contains(if canceled {
                    "TASK_STATE_CANCELED"
                } else {
                    "TASK_STATE_COMPLETED"
                }),
                "{body}"
            );
            let terminal = f
                .store
                .attempt(&owner.attempt_id)
                .await
                .unwrap()
                .unwrap()
                .terminal
                .unwrap();
            assert_eq!(
                terminal.outcome,
                if canceled { "canceled" } else { "completed" }
            );
            assert_eq!(
                terminal.terminal_evidence_counts.reached,
                if canceled { 0 } else { 3 }
            );
            assert_eq!(peer.cancels.load(Ordering::SeqCst), 0);
            if canceled {
                assert!(f.spawns.lock().unwrap().is_empty());
            }
        })
        .await
        .expect("initial barrier cancellation did not settle");
    }
}
#[tokio::test]
async fn fanout_cancellation_tracks_current_candidate() {
    for (method, disconnect, boundary) in [
        ("SendMessage", false),
        ("SendStreamingMessage", false),
        ("SendStreamingMessage", true),
    ]
    .into_iter()
    .flat_map(|(method, disconnect)| {
        ["cleanup", "blocked-resolve", "blocked-setup", "ledger-gate"]
            .map(move |boundary| (method, disconnect, boundary))
    }) {
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            let f = Fixture::new(if boundary == "cleanup" {"blocked-cleanup"} else {"reject"}, if boundary == "cleanup" {"success"} else {boundary}, true);
            let peer = Arc::new(Peer { hold:boundary != "cleanup", fail: boundary == "cleanup", late: boundary == "blocked-resolve", ..Default::default() });
            let router = f.served_server(true, peer.clone()).router();
            let owner = AttemptIdentity::initial().unwrap();
            let (drop_tx, drop_rx) = tokio::sync::oneshot::channel::<()>();
            let call = async {
                if disconnect {
                    let body = open_sse(router.clone(), &owner).await;
                    drop_rx.await.unwrap();
                    drop(body);
                    String::new()
                } else { Fixture::served_on(router.clone(), method, &owner, None).await }
            };
            tokio::pin!(call);
            tokio::select! {
                _ = async { match boundary { "cleanup" => f.providers[0].cleanup.0.notified().await, "ledger-gate" => f.dispatch_gate.0.notified().await, _ => f.providers[1].configure.0.notified().await } } => {},
                body = &mut call => panic!("early {boundary}: {body}"),
            }
            if disconnect {
                drop_tx.send(()).unwrap();
                // Drive the client drop before unblocking any successor work.
                (&mut call).await;
                while !f.store.cancel_requested(&TaskId::parse(owner.execution_id.as_str()).unwrap()).await.unwrap() { tokio::task::yield_now().await; }
            } else { cancel(&router, &owner).await; }
            if peer.late { peer.release.notify_one(); }
            f.providers[0].cleanup.1.notify_one();
            f.providers[1].configure.1.notify_one();
            f.dispatch_gate.1.notify_one();
            let body = if disconnect { String::new() } else { call.await };
            while f.store.attempt(&owner.attempt_id).await.unwrap().and_then(|a| a.terminal).is_none() { tokio::task::yield_now().await; }
            assert!(disconnect || body.contains("TASK_STATE_CANCELED"), "{boundary}: {body}");
            assert_eq!(f.providers[1].prompts.load(Ordering::SeqCst), 0);
            assert_eq!(peer.cancels.load(Ordering::SeqCst), usize::from(boundary != "cleanup"));
            assert_eq!(f.providers[0].cancels.load(Ordering::SeqCst), usize::from(boundary == "cleanup"));
            assert_eq!(f.providers[1].cancels.load(Ordering::SeqCst), usize::from(matches!(boundary, "blocked-setup"|"ledger-gate")));
            assert_eq!(f.providers[1].setup.load(Ordering::SeqCst), usize::from(matches!(boundary,"blocked-setup"|"ledger-gate")));
            let rows = f.rows();
            let has_target_row = boundary != "cleanup";
            assert_eq!(rows.len(), if has_target_row { 2 } else { 1 }, "{boundary}: {rows:?}");
            for (i, agent) in ["source", "target", "last"].into_iter().enumerate() {
                let polled = f.providers[i].prompts.load(Ordering::SeqCst);
                let row = rows.iter().find(|r| r.attribution.agent == agent);
                if i == 0 || (i == 1 && has_target_row) {
                    let row = row.unwrap();
                    assert_eq!(row.attribution.run_attempt.as_deref(), Some(owner.attempt_id.as_str()));
                    assert_eq!(row.attribution.node.as_deref(), Some("source"));
                    assert_eq!(row.dispatched, polled == 1, "{boundary}/{agent}: dispatch must match provider poll");
                    // Rejected prompts lack terminal usage; canceled configuration or
                    // admission also stays pending. Failed resolution settles preparation.
                    assert_eq!(row.finalized, i == 1 && boundary == "blocked-resolve", "{boundary}/{agent}: {row:?}");
                    assert_eq!(row.attribution.execution.as_deref(), Some(owner.execution_id.as_str()));
                    assert!(row.usage.used.is_none() && row.usage.cost.is_none());
                } else {
                    assert!(row.is_none(), "{boundary}/{agent}: unexpected usage row");
                    assert_eq!(polled, 0);
                }
            }
            let terminal = f.store.attempt(&owner.attempt_id).await.unwrap().unwrap().terminal.unwrap();
            assert_eq!(terminal.outcome, "canceled");
            assert_eq!(terminal.terminal_evidence_counts.reached as usize,
                f.providers.iter().map(|p| p.prompts.load(Ordering::SeqCst)).sum::<usize>() + peer.prompts.load(Ordering::SeqCst),
                "{boundary}: reached must match provider polls");
            assert!(f.coordinator.bindings_ref().lock().await.is_empty());
        }).await.unwrap_or_else(|_| panic!("cancel stuck at {boundary}"));
    }
}
#[tokio::test]
async fn fanout_bounds_follow_local_candidates() {
    use bridge_core::turn_liveness::TurnLivenessValueV1;
    use tower::ServiceExt;
    for (method, watching) in ["SendMessage", "SendStreamingMessage"]
        .into_iter()
        .flat_map(|method| [false, true].map(move |watching| (method, watching)))
    {
        let f = Fixture::new("reject", "success", true);
        let owner = AttemptIdentity::initial().unwrap();
        let router = f.served_server(true, Arc::new(Peer::default())).router();
        let response = router.oneshot(axum::http::Request::builder().method("POST").uri("/").header("content-type","application/json").header("A2A-Version","1.0").body(axum::body::Body::from(serde_json::json!({"jsonrpc":"2.0","id":1,"method":method,"params":{"message":{"text":"original input","taskId":owner.execution_id.as_str(),"metadata":{"a2a-bridge.execution_id":owner.execution_id.as_str(),"a2a-bridge.attempt_id":owner.attempt_id.as_str(),"a2a-bridge.silence_cutoff_secs":"17","a2a-bridge.work_cutoff_secs":"23","a2a-bridge.client_watching":watching.to_string()}}}}).to_string())).unwrap()).await.unwrap();
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let source = f.providers[0].liveness.lock().unwrap();
        let target = f.providers[1].liveness.lock().unwrap();
        assert_eq!(*source, *target);
        assert_eq!(source.len(), 1);
        assert_eq!(source[0].silence, TurnLivenessValueV1::Ms(17000));
        assert_eq!(source[0].work_cutoff, TurnLivenessValueV1::Ms(23000));
        assert_eq!(
            source[0].client_watching,
            watching && method == "SendMessage"
        );
        assert_eq!(
            source[0].observer_key.as_deref(),
            Some(owner.execution_id.as_str())
        );
    }
}
#[tokio::test]
async fn fanout_local_cancel_leaves_peer_running() {
    for peer_fails in [false, true] {
        let f = Fixture::new("local-cancel", "success", true);
        let peer = Arc::new(Peer {
            hold: true,
            fail: peer_fails,
            ..Default::default()
        });
        let owner = AttemptIdentity::initial().unwrap();
        let call = call(&f, peer.clone(), &owner);
        tokio::pin!(call);
        tokio::select! {
            _ = async { while f.providers[0].prompts.load(Ordering::SeqCst) == 0 { tokio::task::yield_now().await; } } => {},
            body = &mut call => panic!("peer did not stay running: {body}"),
        }
        peer.release.notify_one();
        let body = call.await;
        assert_eq!(
            body["result"]["status"]["state"],
            if peer_fails {
                "TASK_STATE_FAILED"
            } else {
                "TASK_STATE_COMPLETED"
            }
        );
        let terminal = f
            .store
            .attempt(&owner.attempt_id)
            .await
            .unwrap()
            .unwrap()
            .terminal
            .unwrap();
        assert_eq!(
            terminal.outcome,
            if peer_fails { "failed" } else { "completed" }
        );
        assert_eq!(peer.cancels.load(Ordering::SeqCst), 0);
        assert_eq!(f.providers[1].setup.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn fanout_peer_requires_durable_acceptance() {
    for source in ["setup", "success"] {
        let f = Fixture::new(source, "success", true);
        let peer = Arc::new(Peer::default());
        let owner = AttemptIdentity::initial().unwrap();
        f.store.attempt(&owner.attempt_id).await.unwrap();
        let db = rusqlite::Connection::open(f.dir.path().join("tasks.sqlite")).unwrap();
        db.execute_batch("CREATE TRIGGER refuse_first_prompt BEFORE UPDATE OF prompt_acceptance ON workflow_attempt_summaries WHEN NEW.prompt_acceptance = 'dispatch_uncertain' BEGIN SELECT RAISE(ABORT, 'first barrier refused'); END;").unwrap();
        let body = call(&f, peer.clone(), &owner).await;
        assert!(body["error"].is_object(), "{body}");
        assert_eq!(peer.prompts.load(Ordering::SeqCst), 0);
        assert!(f.spawns.lock().unwrap().is_empty());
        let terminal = f
            .store
            .attempt(&owner.attempt_id)
            .await
            .unwrap()
            .unwrap()
            .terminal
            .unwrap();
        assert_eq!(terminal.terminal_reason, "prompt_barrier_failed");
        assert_eq!(terminal.terminal_evidence_counts.reached, 0);
    }
    let f = Fixture::new("setup", "success", true);
    let owner = AttemptIdentity::initial().unwrap();
    let body = call(&f, Arc::new(Peer::default()), &owner).await;
    assert_eq!(body["result"]["status"]["state"], "TASK_STATE_COMPLETED");
    let terminal = f
        .store
        .attempt(&owner.attempt_id)
        .await
        .unwrap()
        .unwrap()
        .terminal
        .unwrap();
    assert_eq!(terminal.prompt_acceptance, "dispatch_uncertain");
    assert_eq!(terminal.terminal_evidence_counts.reached, 1);
    assert!(!f.rows()[0].dispatched);
}

#[tokio::test]
async fn fanout_default_reload_keeps_frozen_billing_agent() {
    struct Billing;
    impl bridge_core::accounting::BillingIdentityProviderV1 for Billing {
        fn identity(
            &self,
            agent: &str,
            model: Option<&str>,
        ) -> Option<bridge_core::accounting::BillingIdentityV1> {
            assert_eq!(model, Some("shared"));
            Some(bridge_core::accounting::BillingIdentityV1 {
                account_group: agent.into(),
                billing_mode: "metered".into(),
            })
        }
    }
    let f = Fixture::with_billing(
        "shared-reject",
        "first-gate",
        true,
        false,
        false,
        Arc::new(Billing),
    );
    let owner = AttemptIdentity::initial().unwrap();
    let call = call(&f, Arc::new(Peer::default()), &owner);
    tokio::pin!(call);
    tokio::select! { _ = f.dispatch_gate.0.notified() => {}, body = &mut call => panic!("missing initial barrier gate: {body}") }
    assert!(f.spawns.lock().unwrap().is_empty());
    let mut snapshot = f.snapshot();
    snapshot.default = AgentId::parse("target").unwrap();
    f.coordinator.registry().apply(snapshot).await.unwrap();
    f.dispatch_gate.1.notify_one();
    f.providers[0].cleanup.1.notify_one();
    let body = call.await;
    assert_eq!(body["result"]["status"]["state"], "TASK_STATE_COMPLETED");
    assert_eq!(*f.spawns.lock().unwrap(), ["source", "target"]);
    let rows = f.rows();
    assert_eq!(rows.len(), 2);
    for agent in ["source", "target"] {
        let row = rows.iter().find(|r| r.attribution.agent == agent).unwrap();
        assert_eq!(row.attribution.effective_model.as_deref(), Some("shared"));
        assert_eq!(
            row.attribution.billing.as_ref().unwrap().account_group,
            agent
        );
        assert!(row.dispatched);
    }
}

#[tokio::test(start_paused = true)]
async fn fanout_local_cutoff_does_not_cancel_peer_or_route() {
    use tower::ServiceExt;
    for peer_fails in [false, true] {
        let f = Fixture::new("cutoff", "success", true);
        let peer = Arc::new(Peer {
            hold: true,
            fail: peer_fails,
            ..Default::default()
        });
        let owner = AttemptIdentity::initial().unwrap();
        let router = f.served_server(true, peer.clone()).router();
        let call = router.oneshot(axum::http::Request::builder().method("POST").uri("/").header("content-type", "application/json").header("A2A-Version", "1.0").body(axum::body::Body::from(serde_json::json!({"jsonrpc":"2.0","id":1,"method":"SendMessage","params":{"message":{"text":"original input","taskId":owner.execution_id.as_str(),"metadata":{"a2a-bridge.execution_id":owner.execution_id.as_str(),"a2a-bridge.attempt_id":owner.attempt_id.as_str(),"a2a-bridge.work_cutoff_secs":"1","a2a-bridge.silence_cutoff_secs":"off"}}}}).to_string())).unwrap());
        tokio::pin!(call);
        tokio::select! { _ = f.providers[0].configure.0.notified() => {}, _ = &mut call => panic!("cutoff not armed") }
        tokio::time::advance(std::time::Duration::from_secs(1)).await;
        tokio::select! { _ = f.providers[0].cleanup.0.notified() => {}, _ = &mut call => panic!("peer did not survive local cutoff") }
        peer.release.notify_one();
        let response = call.await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            body["result"]["status"]["state"],
            if peer_fails {
                "TASK_STATE_FAILED"
            } else {
                "TASK_STATE_COMPLETED"
            }
        );
        assert_eq!(peer.cancels.load(Ordering::SeqCst), 0);
        assert_eq!(f.providers[1].setup.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn fanout_pending_delegation_progress_and_late_cancel() {
    for (method, canceled) in ["SendMessage", "SendStreamingMessage"]
        .into_iter()
        .flat_map(|m| [false, true].map(move |c| (m, c)))
    {
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            let f = Fixture::new("blocked-cleanup", "setup", true);
            let peer = Arc::new(Peer { pending: true, late: canceled, hold: canceled, ..Default::default() });
            let router = f.served_server(true, peer.clone()).router();
            let owner = AttemptIdentity::initial().unwrap();
            let call = Fixture::served_on(router.clone(), method, &owner, None);
            tokio::pin!(call);
            tokio::select! { _ = peer.opened.notified() => {}, body = &mut call => panic!("delegation not opened: {body}") }
            tokio::select! { _ = f.providers[0].cleanup.0.notified() => {}, body = &mut call => panic!("local did not progress: {body}") }
            assert_eq!(f.providers[0].prompts.load(Ordering::SeqCst), 1);
            if canceled { cancel(&router, &owner).await; }
            f.providers[0].cleanup.1.notify_one();
            peer.delegated.notify_one();
            peer.release.notify_one();
            let raw = call.await;
            let body = normalized(&raw, method);
            assert_eq!(body["result"]["status"]["state"], if canceled {"TASK_STATE_CANCELED"} else {"TASK_STATE_COMPLETED"});
            assert_eq!(peer.prompts.load(Ordering::SeqCst), 1);
            assert_eq!(peer.cancels.load(Ordering::SeqCst), usize::from(canceled));
            assert_eq!(f.providers[1].setup.load(Ordering::SeqCst), usize::from(!canceled));
            assert_eq!(f.providers[1].prompts.load(Ordering::SeqCst), 0);
            if !canceled {
                assert_eq!(raw.matches("peer answer").count(), 1);
                if method == "SendStreamingMessage" { assert!(raw.contains("model not available"), "{raw}"); }
                assert_eq!(f.rows().len(), 2);
            }
            let terminal = f.store.attempt(&owner.attempt_id).await.unwrap().unwrap().terminal.unwrap();
            assert_eq!(terminal.terminal_evidence_counts.reached, 2);
        }).await.expect("pending delegation failed to settle");
    }
}
