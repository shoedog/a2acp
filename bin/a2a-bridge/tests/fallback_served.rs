#[path = "fallback_direct.rs"]
mod direct;
use bridge_core::{ids::*, task_store::TaskStore, workflow_history::WorkflowHistoryStore};
use direct::Fixture;
use std::sync::atomic::Ordering;

async fn terminal(
    f: &Fixture,
    owner: &AttemptIdentity,
) -> bridge_core::workflow_history::AttemptTerminal {
    f.store
        .attempt(&owner.attempt_id)
        .await
        .unwrap()
        .unwrap()
        .terminal
        .unwrap()
}

#[tokio::test]
async fn served_safe_rejection_routes() {
    for method in ["SendMessage", "SendStreamingMessage"] {
        served_safe_rejection_routes_on(method).await;
    }
}

async fn served_safe_rejection_routes_on(method: &str) {
    for source in ["reject", "region", "rate", "billing-match"] {
        let f = Fixture::new(source, "success", true);
        let owner = AttemptIdentity::initial().unwrap();
        let body = f.served(method, &owner, None).await;
        if method == "SendMessage" {
            let reply: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(
                reply["result"]["artifact"]["text"], "answer",
                "{source}: {body}"
            );
            assert_eq!(reply["result"]["task"]["id"], owner.execution_id.as_str());
            assert_eq!(reply["result"]["task"]["state"], "TASK_STATE_COMPLETED");
        } else {
            assert_sse_success(&body, &owner);
        }
        assert_eq!(*f.spawns.lock().unwrap(), ["source", "target"]);
        assert_eq!(
            *f.providers[0].inputs.lock().unwrap(),
            *f.providers[1].inputs.lock().unwrap()
        );
        let rows = f.rows();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.dispatched
            && r.attribution.run_attempt.as_deref() == Some(owner.attempt_id.as_str())));
        let terminal = f
            .coordinator
            .attempt_status(&owner.attempt_id)
            .await
            .unwrap()
            .terminal
            .unwrap();
        assert_eq!(terminal.terminal_evidence_counts.reached, 2);
    }
}

#[tokio::test]
async fn served_replay_refusals_and_lazy_controls() {
    for method in ["SendMessage", "SendStreamingMessage"] {
        served_replay_refusals_and_lazy_controls_on(method).await;
    }
}

async fn served_replay_refusals_and_lazy_controls_on(method: &str) {
    for (source, chain, context) in [
        ("accepted", true, None),
        ("text", true, None),
        ("tool", true, None),
        ("permission", true, None),
        ("cleanup", true, None),
        ("missing", true, None),
        ("success", true, None),
        ("reject", false, None),
        ("billing", true, None),
        ("reject", true, Some("existing")),
    ] {
        let f = Fixture::new(source, "setup", chain);
        let owner = AttemptIdentity::initial().unwrap();
        let body = f.served(method, &owner, context).await;
        assert_eq!(
            body.contains("answer"),
            source == "success",
            "{source}: {body}"
        );
        assert_eq!(*f.spawns.lock().unwrap(), ["source"]);
        assert_eq!(f.rows().len(), 1);
    }
}

fn assert_sse_success(body: &str, owner: &AttemptIdentity) {
    assert!(body.contains("answer"), "{body}");
    assert!(!body.contains("event: error"), "{body}");
    let frames: Vec<serde_json::Value> = body
        .lines()
        .filter_map(|line| {
            line.strip_prefix("data: ").map(|data| {
                let value: serde_json::Value = serde_json::from_str(data).unwrap();
                value["statusUpdate"]
                    .as_object()
                    .map(|_| value["statusUpdate"].clone())
                    .unwrap_or_else(|| value["artifactUpdate"].clone())
            })
        })
        .collect();
    assert!(!frames.is_empty());
    for frame in &frames {
        assert_eq!(frame["taskId"], owner.execution_id.as_str(), "{body}");
        assert_eq!(frame["contextId"], owner.execution_id.as_str(), "{body}");
    }
    assert_eq!(
        frames
            .iter()
            .filter(|f| f["status"]["state"] == "TASK_STATE_COMPLETED")
            .count(),
        1,
        "{body}"
    );
    assert!(body.contains("TASK_STATE_COMPLETED"), "{body}");
}

#[tokio::test]
async fn served_governed_never_routes() {
    for method in ["SendMessage", "SendStreamingMessage"] {
        let f = Fixture::with_options("reject", "success", true, false, true);
        let owner = AttemptIdentity::initial().unwrap();
        let body = f.served(method, &owner, None).await;
        assert!(
            body.contains(if method == "SendMessage" {
                "quota_exhausted"
            } else {
                "event: error"
            }),
            "{body}"
        );
        assert_eq!(*f.spawns.lock().unwrap(), ["source"]);
        assert_eq!(f.rows().len(), 1);
    }
}

#[tokio::test]
async fn served_target_setup_failure_and_provider_poll_authority() {
    for (target, fail) in [("setup", false), ("success", true), ("usage", false)] {
        let f = Fixture::with_options("reject", target, true, fail, false);
        if target == "usage" {
            f.store.usage_summary_v1(&Default::default()).await.unwrap();
            let db = rusqlite::Connection::open(f.dir.path().join("tasks.sqlite.usage-v1.sqlite"))
                .unwrap();
            db.execute_batch("PRAGMA journal_mode=PERSIST; CREATE TRIGGER refuse_target BEFORE UPDATE ON usage_attempts_v1 WHEN json_extract(NEW.record_json, '$.attribution.agent') = 'target' BEGIN SELECT RAISE(ABORT, 'candidate refused'); END;").unwrap();
        }
        let owner = AttemptIdentity::initial().unwrap();
        let body = f.served("SendMessage", &owner, None).await;
        assert!(!body.contains("answer"), "{body}");
        let rows = f.rows();
        assert_eq!(rows.len(), 2);
        let target = rows
            .iter()
            .find(|r| r.attribution.agent == "target")
            .unwrap();
        assert!(!target.dispatched);
        assert_eq!(f.providers[1].setup.load(Ordering::SeqCst), 1);
        assert_eq!(f.providers[1].prompts.load(Ordering::SeqCst), 0);
        assert_eq!(f.providers[2].setup.load(Ordering::SeqCst), 0);
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
}

#[tokio::test]
async fn served_chain_is_frozen_and_finite() {
    for method in ["SendMessage", "SendStreamingMessage"] {
        served_chain_is_frozen_and_finite_on(method).await;
    }
}

async fn served_chain_is_frozen_and_finite_on(method: &str) {
    let f = Fixture::new("reject", "reject", true);
    let owner = AttemptIdentity::initial().unwrap();
    let body = f.served(method, &owner, None).await;
    assert!(body.contains("model not available"), "{body}");
    assert_eq!(*f.spawns.lock().unwrap(), ["source", "target", "last"]);
    let rows = f.rows();
    assert_eq!(rows.len(), 3);
    let last = rows.iter().find(|r| r.attribution.agent == "last").unwrap();
    assert!(!last.dispatched);
    assert_eq!(f.providers[0].prompts.load(Ordering::SeqCst), 1);
    assert_eq!(f.providers[1].prompts.load(Ordering::SeqCst), 1);
}

async fn rpc(
    router: &axum::Router,
    method: &str,
    params: serde_json::Value,
) -> axum::response::Response {
    use tower::ServiceExt;
    router
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/")
                .header("content-type", "application/json")
                .header("A2A-Version", "1.0")
                .body(axum::body::Body::from(
                    serde_json::json!({"jsonrpc":"2.0","id":2,"method":method,"params":params})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
}
async fn response_text(response: axum::response::Response) -> String {
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    String::from_utf8(
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test]
async fn served_cleanup_activity_is_resampled() {
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
        let router = f.server().router();
        let owner = AttemptIdentity::initial().unwrap();
        let mut call = Box::pin(Fixture::served_on(router, "SendMessage", &owner, None));
        tokio::select! {
            _ = f.providers[0].cleanup.0.notified() => {},
            body = &mut call => panic!("returned before cleanup: {body}"),
        }
        assert_eq!(f.providers[1].setup.load(Ordering::SeqCst), 0);
        f.providers[0].cleanup.1.notify_one();
        let body = call.await;
        assert_eq!(
            body.contains("answer"),
            !effect,
            "late cleanup effect must forbid replay: {body}"
        );
        assert_eq!(
            f.providers[1].prompts.load(Ordering::SeqCst),
            usize::from(!effect)
        );
        assert_eq!(f.rows().len(), if effect { 1 } else { 2 });
        if effect {
            assert!(
                body.contains("quota_exhausted"),
                "original failure must stand: {body}"
            );
            assert_eq!(*f.spawns.lock().unwrap(), ["source"]);
        }
    }
}

async fn lifecycle(boundary: &str, action: &str, method: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let f = Fixture::new("blocked-cleanup", if boundary == "configure" { "blocked-setup" } else { "success" }, true);
        let router = f.server().router();
        let owner = AttemptIdentity::initial().unwrap();
        let task = TaskId::parse(owner.execution_id.as_str()).unwrap();
        let response = if method == "SendStreamingMessage" {
            Some(rpc(&router, method, serde_json::json!({"message":{
                "text":"original input", "taskId":owner.execution_id.as_str(),
                "metadata":{"a2a-bridge.execution_id":owner.execution_id.as_str(), "a2a-bridge.attempt_id":owner.attempt_id.as_str()}
            }})).await)
        } else { None };
        let mut call = Box::pin(async {
            if let Some(response) = response { response_text(response).await }
            else { Fixture::served_on(router.clone(), method, &owner, None).await }
        });
        tokio::select! {
            _ = f.providers[0].cleanup.0.notified() => {},
            body = &mut call => panic!("returned before cleanup: {body}"),
        }
        assert_eq!(f.providers[1].setup.load(Ordering::SeqCst), 0, "target prepared before source cleanup");
        if boundary == "configure" {
            f.providers[0].cleanup.1.notify_one();
            tokio::select! {
                _ = f.providers[1].configure.0.notified() => {},
                body = &mut call => panic!("returned before configure: {body}"),
            }
            assert_ne!(*f.providers[0].sessions.lock().unwrap(), *f.providers[1].sessions.lock().unwrap());
        }
        // One router owns pending preparations, cancellation and unrelated dispatch.
        let unrelated = AttemptIdentity::initial().unwrap();
        let response = rpc(&router, "SendStreamingMessage", serde_json::json!({"message":{
            "text":"independent", "taskId":unrelated.execution_id.as_str(),
            "metadata":{"a2a-bridge.agent":"other", "a2a-bridge.execution_id":unrelated.execution_id.as_str(), "a2a-bridge.attempt_id":unrelated.attempt_id.as_str()}
        }})).await;
        while f.providers[3].prompts.load(Ordering::SeqCst) == 0 { tokio::task::yield_now().await; }
        let cancel = response_text(rpc(&router, "CancelTask", serde_json::json!({"taskId":unrelated.execution_id.as_str()})).await).await;
        assert!(cancel.contains("TASK_STATE_CANCELED"), "unrelated cancel: {cancel}");
        assert_eq!(f.providers[3].cancels.load(Ordering::SeqCst), 1);
        drop(response);
        assert_eq!(f.providers[1].prompts.load(Ordering::SeqCst), 0);
        if action == "cancel" {
            let cancel = response_text(rpc(&router, "CancelTask", serde_json::json!({"taskId":owner.execution_id.as_str()})).await).await;
            assert!(cancel.contains("TASK_STATE_CANCELED"), "{cancel}");
            assert_eq!(f.providers[usize::from(boundary == "configure")].cancels.load(Ordering::SeqCst), 1);
        }
        if action == "disconnect" {
            // Drop the HTTP future (unary) or response body (SSE) to disconnect.
            drop(call);
        } else {
            f.providers[0].cleanup.1.notify_one();
            f.providers[1].configure.1.notify_one();
            let body = call.await;
            assert_eq!(body.contains("answer"), action == "release", "{boundary}/{action}: {body}");
        }
        f.providers[0].cleanup.1.notify_one();
        f.providers[1].configure.1.notify_one();
        while f.coordinator.bindings_ref().lock().await.contains_key(&task) { tokio::task::yield_now().await; }
        assert_eq!(f.providers[1].prompts.load(Ordering::SeqCst), usize::from(action == "release"));
        if boundary == "cleanup" && action != "release" {
            assert_eq!(f.providers[1].setup.load(Ordering::SeqCst), 0);
        }
        if action == "disconnect" && boundary == "configure" && method == "SendStreamingMessage" {
            // The SSE producer owns the pending configure future; wait for its exact
            // lease to drop before admitting the new incarnation on this task.
            while f.providers[1].forgets.load(Ordering::SeqCst) == 0 { tokio::task::yield_now().await; }
        }
        // Reuse the exact task on this server with a fresh attempt. A leaked pending-cold
        // entry would refuse configuration; a late old guard must leave this binding alive.
        let next = AttemptId::mint().unwrap();
        let response = rpc(&router, "SendStreamingMessage", serde_json::json!({"message":{
            "text":"new incarnation", "taskId":owner.execution_id.as_str(),
            "metadata":{"a2a-bridge.agent":"other", "a2a-bridge.execution_id":owner.execution_id.as_str(), "a2a-bridge.attempt_id":next.as_str()}
        }})).await;
        while f.providers[3].prompts.load(Ordering::SeqCst) != 2 { tokio::task::yield_now().await; }
        let forgotten = f.providers[0].forgets.load(Ordering::SeqCst);
        drop(bridge_coordinator::dispatch::BindingGuard {
            bindings: f.coordinator.bindings_ref().clone(), task: task.clone(),
            backend: f.providers[0].clone(), session: f.providers[0].sessions.lock().unwrap()[0].clone(),
        });
        while f.providers[0].forgets.load(Ordering::SeqCst) == forgotten { tokio::task::yield_now().await; }
        let bindings = f.coordinator.bindings_ref().lock().await;
        assert_eq!(bindings[&task].agent.as_str(), "other", "old guard removed new incarnation");
        drop(bindings);
        let cancel = response_text(rpc(&router, "CancelTask", serde_json::json!({"taskId":owner.execution_id.as_str()})).await).await;
        assert!(cancel.contains("TASK_STATE_CANCELED"), "{cancel}");
        assert_eq!(f.providers[3].cancels.load(Ordering::SeqCst), 2);
        drop(response);
    }).await.unwrap_or_else(|_| panic!("{method}/{boundary}/{action}: lifecycle did not settle"));
}

#[tokio::test]
async fn served_cleanup_cancel_and_binding() {
    for boundary in ["cleanup", "configure"] {
        for method in ["SendMessage", "SendStreamingMessage"] {
            lifecycle(boundary, "cancel", method).await;
        }
    }
}
#[tokio::test]
async fn served_disconnect_and_pending_settlement() {
    for boundary in ["cleanup", "configure"] {
        for method in ["SendMessage", "SendStreamingMessage"] {
            lifecycle(boundary, "disconnect", method).await;
        }
    }
}
#[tokio::test]
async fn served_release_and_transition_controls() {
    for boundary in ["cleanup", "configure"] {
        for method in ["SendMessage", "SendStreamingMessage"] {
            lifecycle(boundary, "release", method).await;
        }
    }
}

#[tokio::test]
async fn served_no_state_migration_or_persistence_replay() {
    for method in ["SendMessage", "SendStreamingMessage"] {
        served_no_state_migration_or_persistence_replay_on(method).await;
    }
}

async fn served_no_state_migration_or_persistence_replay_on(method: &str) {
    use bridge_core::ports::SessionStore;
    for retained in [true, false] {
        let f = Fixture::new("reject", "success", true);
        let owner = AttemptIdentity::initial().unwrap();
        if retained {
            f.store
                .put(
                    &TaskId::parse(owner.execution_id.as_str()).unwrap(),
                    &SessionId::parse("retained").unwrap(),
                )
                .await
                .unwrap();
        } else {
            f.store.usage_summary_v1(&Default::default()).await.unwrap();
            let db = rusqlite::Connection::open(f.dir.path().join("tasks.sqlite.usage-v1.sqlite"))
                .unwrap();
            db.execute_batch("CREATE TRIGGER refuse_completion BEFORE UPDATE ON usage_attempts_v1 BEGIN SELECT RAISE(ABORT, 'persist refused'); END;").unwrap();
        }
        let body = f.served(method, &owner, None).await;
        assert!(!body.contains("answer"), "{body}");
        if !retained {
            assert!(body.contains("store failure"), "{body}");
        }
        assert!(f.spawns.lock().unwrap().iter().all(|name| name == "source"));
        assert!(f.rows().len() <= 1);
    }
}

#[tokio::test]
async fn served_successor_terminal_contract_is_resolved_before_aggregation() {
    for (target, reason, valid, missing, invalid) in [
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
        let f = Fixture::new("reject", target, true);
        let owner = AttemptIdentity::initial().unwrap();
        let reply: serde_json::Value =
            serde_json::from_str(&f.served("SendMessage", &owner, None).await).unwrap();
        assert_eq!(
            reply["result"]["task"]["state"],
            if valid == 1 {
                "TASK_STATE_COMPLETED"
            } else {
                "TASK_STATE_FAILED"
            }
        );
        let terminal = terminal(&f, &owner).await;
        assert_eq!(
            terminal.outcome,
            if valid == 1 { "completed" } else { "failed" }
        );
        assert_eq!(terminal.terminal_reason, reason);
        let counts = terminal.terminal_evidence_counts;
        assert_eq!(
            (counts.reached, counts.valid, counts.missing, counts.invalid),
            (2, valid, missing, invalid)
        );
    }
}

#[tokio::test]
async fn served_retains_admitted_entry_and_scope_until_provider_poll() {
    for boundary in ["blocked-resolve", "blocked-setup", "scope"] {
        let f = Fixture::new(
            "reject",
            if boundary == "scope" {
                "ledger-gate"
            } else {
                boundary
            },
            true,
        );
        let owner = AttemptIdentity::initial().unwrap();
        let call = f.served("SendMessage", &owner, None);
        tokio::pin!(call);
        tokio::select! {
            _ = async { if boundary == "scope" { f.dispatch_gate.0.notified().await; } else { f.providers[1].configure.0.notified().await; } } => {},
            body = &mut call => panic!("returned before admitted target gate: {body}"),
        }
        if boundary == "scope" {
            let path = f.dir.path().join("scope");
            std::fs::rename(&path, f.dir.path().join("old-scope")).unwrap();
            std::fs::create_dir(path).unwrap();
        } else {
            let mut snapshot = f.snapshot();
            snapshot.entries[1].mode = Some("write".into());
            f.coordinator.registry().apply(snapshot).await.unwrap();
        }
        if boundary == "scope" {
            f.dispatch_gate.1.notify_one();
        } else {
            f.providers[1].configure.1.notify_one();
        }
        let body = call.await;
        assert!(body.contains("quota_exhausted"), "{boundary}: {body}");
        assert_eq!(f.providers[1].prompts.load(Ordering::SeqCst), 0);
        assert_eq!(f.providers[2].setup.load(Ordering::SeqCst), 0);
        let rows = f.rows();
        assert_eq!(rows.len(), 2);
        assert!(
            !rows
                .iter()
                .find(|r| r.attribution.agent == "target")
                .unwrap()
                .dispatched
        );
        let terminal = terminal(&f, &owner).await;
        assert_eq!(terminal.terminal_evidence_counts.reached, 1);
    }
}

#[tokio::test]
async fn real_cancel_during_successor_dispatch_barrier_prevents_provider_poll() {
    let f = Fixture::new("reject", "ledger-gate", true);
    let router = f.server().router();
    let owner = AttemptIdentity::initial().unwrap();
    let call = Fixture::served_on(router.clone(), "SendMessage", &owner, None);
    tokio::pin!(call);
    tokio::select! {
        _ = f.dispatch_gate.0.notified() => {},
        body = &mut call => panic!("returned before successor ledger barrier: {body}"),
    }
    let body = response_text(
        rpc(
            &router,
            "CancelTask",
            serde_json::json!({"taskId":owner.execution_id.as_str()}),
        )
        .await,
    )
    .await;
    assert!(body.contains("TASK_STATE_CANCELED"), "{body}");
    f.dispatch_gate.1.notify_one();
    let body = call.await;
    assert!(body.contains("TASK_STATE_CANCELED"), "{body}");
    assert_eq!(f.providers[1].setup.load(Ordering::SeqCst), 1);
    assert_eq!(f.providers[1].prompts.load(Ordering::SeqCst), 0);
    let rows = f.rows();
    assert_eq!(rows.len(), 2);
    assert!(
        !rows
            .iter()
            .find(|r| r.attribution.agent == "target")
            .unwrap()
            .dispatched
    );
    let terminal = terminal(&f, &owner).await;
    assert_eq!(terminal.terminal_evidence_counts.reached, 1);
}

#[tokio::test]
async fn sse_source_event_closes_transition() {
    use futures::StreamExt;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let f = Fixture::new("status-reject", "success", true);
        let owner = AttemptIdentity::initial().unwrap();
        let response = rpc(&f.server().router(), "SendStreamingMessage", serde_json::json!({"message":{
            "text":"input", "taskId":owner.execution_id.as_str(),
            "metadata":{"a2a-bridge.execution_id":owner.execution_id.as_str(), "a2a-bridge.attempt_id":owner.attempt_id.as_str()}
        }})).await;
        let mut stream = response.into_body().into_data_stream();
        let first = stream.next().await.unwrap().unwrap();
        let first = String::from_utf8(first.to_vec()).unwrap();
        assert!(first.contains("source-status") && first.contains("TASK_STATE_WORKING"), "{first}");
        // The client consumed the status before the provider is allowed to reject.
        f.providers[0].cleanup.1.notify_one();
        let mut rest = String::new();
        while let Some(chunk) = stream.next().await { rest.push_str(std::str::from_utf8(&chunk.unwrap()).unwrap()); }
        assert!(rest.contains("event: error"), "{rest}");
        assert_eq!(rest.matches("TASK_STATE_FAILED").count(), 1, "{rest}");
        assert!(!rest.contains("answer"), "{rest}");
        assert_eq!(*f.spawns.lock().unwrap(), ["source"]);
        assert_eq!(f.rows().len(), 1);
        assert_eq!(f.providers[1].setup.load(Ordering::SeqCst), 0);
    }).await.expect("SSE must forward before the gated rejection");
}

#[tokio::test]
async fn sse_frozen_source_drift_refuses_before_preparation() {
    for field in ["model", "mode"] {
        let f = Fixture::new("success", "admission-gate", true);
        f.store.usage_summary_v1(&Default::default()).await.unwrap();
        let owner = AttemptIdentity::initial().unwrap();
        let call = f.served("SendStreamingMessage", &owner, None);
        tokio::pin!(call);
        tokio::select! {
            _ = f.dispatch_gate.0.notified() => {},
            body = &mut call => panic!("returned before frozen admission: {body}"),
        }
        let mut snapshot = f.snapshot();
        if field == "model" {
            snapshot.entries[0].model = Some("replacement".into());
        } else {
            snapshot.entries[0].mode = Some("write".into());
        }
        f.coordinator.registry().apply(snapshot).await.unwrap();
        f.dispatch_gate.1.notify_one();
        let body = call.await;
        assert!(
            body.contains("event: error") && body.contains("TASK_STATE_FAILED"),
            "{body}"
        );
        assert!(
            f.spawns.lock().unwrap().is_empty(),
            "replacement source was resolved"
        );
        assert_eq!(f.providers[0].setup.load(Ordering::SeqCst), 0);
        assert_eq!(f.providers[0].prompts.load(Ordering::SeqCst), 0);
        assert!(f.rows().is_empty());
    }
}

#[tokio::test]
async fn served_cancel_during_successor_resolution_stops_configuration() {
    for method in ["SendMessage", "SendStreamingMessage"] {
        let f = Fixture::new("reject", "blocked-resolve", true);
        let router = f.server().router();
        let owner = AttemptIdentity::initial().unwrap();
        let call = Fixture::served_on(router.clone(), method, &owner, None);
        tokio::pin!(call);
        tokio::select! {
            _ = f.providers[1].configure.0.notified() => {},
            body = &mut call => panic!("returned before resolution: {body}"),
        }
        let cancel = response_text(
            rpc(
                &router,
                "CancelTask",
                serde_json::json!({"taskId":owner.execution_id.as_str()}),
            )
            .await,
        )
        .await;
        assert!(cancel.contains("TASK_STATE_CANCELED"), "{cancel}");
        f.providers[1].configure.1.notify_one();
        let body = call.await;
        assert!(!body.contains("answer"), "{body}");
        assert_eq!(f.providers[1].setup.load(Ordering::SeqCst), 0);
        assert_eq!(f.providers[1].prompts.load(Ordering::SeqCst), 0);
        assert_eq!(f.providers[2].setup.load(Ordering::SeqCst), 0);
        assert_eq!(
            terminal(&f, &owner).await.terminal_evidence_counts.reached,
            1
        );
    }
}

#[tokio::test]
async fn sse_existing_state_keeps_legacy_dispatch() {
    use bridge_core::ports::SessionStore;
    for state in ["retained", "repeated", "bound"] {
        let f = Fixture::new(
            if state == "retained" {
                "evidence-missing"
            } else if state == "bound" {
                "parked"
            } else {
                "success"
            },
            "setup",
            true,
        );
        let router = f.server().router();
        let mut owner = AttemptIdentity::initial().unwrap();
        let mut first = None;
        if state == "retained" {
            f.store
                .put(
                    &TaskId::parse(owner.execution_id.as_str()).unwrap(),
                    &SessionId::parse("retained").unwrap(),
                )
                .await
                .unwrap();
        } else if state == "repeated" {
            assert_sse_success(
                &Fixture::served_on(router.clone(), "SendStreamingMessage", &owner, None).await,
                &owner,
            );
        } else {
            first = Some(rpc(&router, "SendStreamingMessage", serde_json::json!({"message":{"text":"input", "taskId":owner.execution_id.as_str(), "metadata":{"a2a-bridge.execution_id":owner.execution_id.as_str(), "a2a-bridge.attempt_id":owner.attempt_id.as_str()}}})).await);
            while f.providers[0].prompts.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        }
        owner.attempt_id = AttemptId::mint().unwrap();
        if state == "bound" {
            let second = rpc(&router, "SendStreamingMessage", serde_json::json!({"message":{"text":"again", "taskId":owner.execution_id.as_str(), "metadata":{"a2a-bridge.execution_id":owner.execution_id.as_str(), "a2a-bridge.attempt_id":owner.attempt_id.as_str()}}})).await;
            assert_eq!(second.status(), axum::http::StatusCode::OK);
            while f.providers[0].prompts.load(Ordering::SeqCst) != 2 {
                tokio::task::yield_now().await;
            }
            rpc(
                &router,
                "CancelTask",
                serde_json::json!({"taskId":owner.execution_id.as_str()}),
            )
            .await;
            drop(second);
            drop(first);
        } else {
            assert_sse_success(
                &Fixture::served_on(router.clone(), "SendStreamingMessage", &owner, None).await,
                &owner,
            );
        }
        assert!(f
            .spawns
            .lock()
            .unwrap()
            .iter()
            .all(|agent| agent == "source"));
        assert_eq!(f.providers[1].setup.load(Ordering::SeqCst), 0);
    }
}
