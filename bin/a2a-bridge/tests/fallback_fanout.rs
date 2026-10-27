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
        let (peer_tx, peer_task) =
            tokio::sync::watch::channel((!self.late).then(|| PeerTaskId("peer-task".into())));
        Ok(Delegation {
            peer_task,
            events: {
                let fail = self.fail;
                let hold = self.hold;
                let release = self.release.clone();
                Box::pin(futures::stream::once(async move {
                    if hold {
                        release.notified().await;
                    }
                    let _ = peer_tx.send(Some(PeerTaskId("peer-task".into())));
                    if fail {
                        Err(BridgeError::UpstreamA2aError)
                    } else {
                        Ok(Event::artifact("peer answer"))
                    }
                }))
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
    for cause in ["reject", "region", "rate", "billing-match"] {
        let f = Fixture::new(cause, "success", true);
        let peer = Arc::new(Peer::default());
        let owner = AttemptIdentity::initial().unwrap();
        let body = Fixture::served_on(
            f.served_server(true, peer.clone()).router(),
            "SendMessage",
            &owner,
            None,
        )
        .await;
        let reply: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            reply["result"]["status"]["state"], "TASK_STATE_COMPLETED",
            "{body}"
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
#[tokio::test]
async fn fanout_sse_keeps_original_failure() {
    let f = Fixture::new("reject", "success", true);
    let body = Fixture::served_on(
        f.served_server(true, Arc::new(Peer::default())).router(),
        "SendStreamingMessage",
        &AttemptIdentity::initial().unwrap(),
        None,
    )
    .await;
    assert!(!body.contains("\"text\":\"answer\""), "{body}");
    assert_eq!(*f.spawns.lock().unwrap(), ["source"]);
    assert_eq!(f.rows().len(), 1);
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
    for target in ["reject", "billing"] {
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
            if target == "reject" {
                vec!["source", "target", "last"]
            } else {
                vec!["source", "target"]
            }
        );
        assert_eq!(f.providers[2].prompts.load(Ordering::SeqCst), 0);
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
    for (source, target, peer_fails, succeeds) in [
        ("blocked-cleanup", "success", true, true),
        ("reject", "reject", false, true),
        ("reject", "reject", true, false),
    ] {
        let f = Fixture::new(source, target, true);
        let peer = Arc::new(Peer {
            fail: peer_fails,
            ..Default::default()
        });
        let owner = AttemptIdentity::initial().unwrap();
        let call = call(&f, peer.clone(), &owner);
        tokio::pin!(call);
        if source == "blocked-cleanup" {
            tokio::select! { _ = f.providers[0].cleanup.0.notified() => {}, body = &mut call => panic!("premature aggregate: {body}") }
            f.providers[0].cleanup.1.notify_one();
        }
        let body = call.await;
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
async fn fanout_cancellation_tracks_current_candidate() {
    for boundary in ["cleanup", "blocked-resolve", "blocked-setup", "ledger-gate"] {
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            let f = Fixture::new(if boundary == "cleanup" {"blocked-cleanup"} else {"reject"}, if boundary == "cleanup" {"success"} else {boundary}, true);
            let peer = Arc::new(Peer { hold:boundary != "cleanup", fail: boundary == "cleanup", late: boundary == "blocked-resolve", ..Default::default() });
            let router = f.served_server(true, peer.clone()).router();
            let owner = AttemptIdentity::initial().unwrap();
            let call = Fixture::served_on(router.clone(), "SendMessage", &owner, None);
            tokio::pin!(call);
            tokio::select! {
                _ = async { match boundary { "cleanup" => f.providers[0].cleanup.0.notified().await, "ledger-gate" => f.dispatch_gate.0.notified().await, _ => f.providers[1].configure.0.notified().await } } => {},
                body = &mut call => panic!("early {boundary}: {body}"),
            }
            cancel(&router, &owner).await;
            if peer.late { peer.release.notify_one(); }
            f.providers[0].cleanup.1.notify_one();
            f.providers[1].configure.1.notify_one();
            f.dispatch_gate.1.notify_one();
            let body = call.await;
            assert!(body.contains("TASK_STATE_CANCELED"), "{boundary}: {body}");
            assert_eq!(f.providers[1].prompts.load(Ordering::SeqCst), 0);
            assert_eq!(peer.cancels.load(Ordering::SeqCst), usize::from(boundary != "cleanup"));
            assert_eq!(f.providers[0].cancels.load(Ordering::SeqCst), usize::from(boundary == "cleanup"));
            assert_eq!(f.providers[1].cancels.load(Ordering::SeqCst), usize::from(matches!(boundary, "blocked-setup"|"ledger-gate")));
            assert_eq!(f.providers[1].setup.load(Ordering::SeqCst), usize::from(matches!(boundary,"blocked-setup"|"ledger-gate")));
            assert!(f.coordinator.bindings_ref().lock().await.is_empty());
        }).await.unwrap_or_else(|_| panic!("cancel stuck at {boundary}"));
    }
}
#[tokio::test]
async fn fanout_bounds_follow_local_candidates() {
    use bridge_core::turn_liveness::TurnLivenessValueV1;
    use tower::ServiceExt;
    for watching in [false, true] {
        let f = Fixture::new("reject", "success", true);
        let owner = AttemptIdentity::initial().unwrap();
        let router = f.served_server(true, Arc::new(Peer::default())).router();
        let response = router.oneshot(axum::http::Request::builder().method("POST").uri("/").header("content-type","application/json").header("A2A-Version","1.0").body(axum::body::Body::from(serde_json::json!({"jsonrpc":"2.0","id":1,"method":"SendMessage","params":{"message":{"text":"original input","taskId":owner.execution_id.as_str(),"metadata":{"a2a-bridge.execution_id":owner.execution_id.as_str(),"a2a-bridge.attempt_id":owner.attempt_id.as_str(),"a2a-bridge.silence_cutoff_secs":"17","a2a-bridge.work_cutoff_secs":"23","a2a-bridge.client_watching":watching.to_string()}}}}).to_string())).unwrap()).await.unwrap();
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let source = f.providers[0].liveness.lock().unwrap();
        let target = f.providers[1].liveness.lock().unwrap();
        assert_eq!(*source, *target);
        assert_eq!(source.len(), 1);
        assert_eq!(source[0].silence, TurnLivenessValueV1::Ms(17000));
        assert_eq!(source[0].work_cutoff, TurnLivenessValueV1::Ms(23000));
        assert_eq!(source[0].client_watching, watching);
        assert_eq!(
            source[0].observer_key.as_deref(),
            Some(owner.execution_id.as_str())
        );
    }
}
#[tokio::test]
async fn fanout_local_cancel_leaves_peer_running() {
    let f = Fixture::new("local-cancel", "success", true);
    let peer = Arc::new(Peer {
        hold: true,
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
    assert_eq!(body["result"]["status"]["state"], "TASK_STATE_COMPLETED");
    assert_eq!(peer.cancels.load(Ordering::SeqCst), 0);
    assert_eq!(f.providers[1].setup.load(Ordering::SeqCst), 0);
}
