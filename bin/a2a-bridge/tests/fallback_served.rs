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
    for source in ["reject", "region", "rate", "billing-match"] {
        let f = Fixture::new(source, "success", true);
        let owner = AttemptIdentity::initial().unwrap();
        let body = f.served("SendMessage", &owner, None).await;
        let reply: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            reply["result"]["artifact"]["text"], "answer",
            "{source}: {body}"
        );
        assert_eq!(reply["result"]["task"]["id"], owner.execution_id.as_str());
        assert_eq!(reply["result"]["task"]["state"], "TASK_STATE_COMPLETED");
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
        let body = f.served("SendMessage", &owner, context).await;
        assert_eq!(
            body.contains("answer"),
            source == "success",
            "{source}: {body}"
        );
        assert_eq!(*f.spawns.lock().unwrap(), ["source"]);
        assert_eq!(f.rows().len(), 1);
    }
}

#[tokio::test]
async fn sse_request_keeps_original_failure() {
    let f = Fixture::new("reject", "success", true);
    let owner = AttemptIdentity::initial().unwrap();
    let body = f.served("SendStreamingMessage", &owner, None).await;
    assert!(!body.contains("answer"), "{body}");
    assert!(body.contains("error"), "{body}");
    assert_eq!(*f.spawns.lock().unwrap(), ["source"]);
    assert_eq!(f.rows().len(), 1);
}

#[tokio::test]
async fn served_governed_never_routes() {
    let f = Fixture::with_options("reject", "success", true, false, true);
    let owner = AttemptIdentity::initial().unwrap();
    let body = f.served("SendMessage", &owner, None).await;
    assert!(body.contains("quota_exhausted"), "{body}");
    assert_eq!(*f.spawns.lock().unwrap(), ["source"]);
    assert_eq!(f.rows().len(), 1);
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
    let f = Fixture::new("reject", "reject", true);
    let owner = AttemptIdentity::initial().unwrap();
    let body = f.served("SendMessage", &owner, None).await;
    assert!(body.contains("model not available"), "{body}");
    assert_eq!(*f.spawns.lock().unwrap(), ["source", "target", "last"]);
    let rows = f.rows();
    assert_eq!(rows.len(), 3);
    let last = rows.iter().find(|r| r.attribution.agent == "last").unwrap();
    assert!(!last.dispatched);
    assert_eq!(f.providers[0].prompts.load(Ordering::SeqCst), 1);
    assert_eq!(f.providers[1].prompts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn served_cleanup_cancel_and_binding() {
    use bridge_core::ports::SessionStore;
    for boundary in ["cleanup", "configure"] {
        let f = Fixture::new(
            "blocked-cleanup",
            if boundary == "configure" {
                "blocked-setup"
            } else {
                "success"
            },
            true,
        );
        let owner = AttemptIdentity::initial().unwrap();
        let call = f.served("SendMessage", &owner, None);
        tokio::pin!(call);
        tokio::select! {
            biased;
            _ = f.providers[0].cleanup.0.notified() => {},
            body = &mut call => panic!("returned before cleanup: {body}"),
        }
        assert_eq!(f.providers[1].setup.load(Ordering::SeqCst), 0);
        if boundary == "configure" {
            f.providers[0].cleanup.1.notify_one();
            tokio::select! {
                biased;
                _ = f.providers[1].configure.0.notified() => {},
                body = &mut call => panic!("returned before configure: {body}"),
            }
            assert_ne!(
                *f.providers[0].sessions.lock().unwrap(),
                *f.providers[1].sessions.lock().unwrap()
            );
        }
        f.store
            .request_cancel(&TaskId::parse(owner.execution_id.as_str()).unwrap())
            .await
            .unwrap();
        f.providers[0].cleanup.1.notify_one();
        f.providers[1].configure.1.notify_one();
        let body = call.await;
        assert!(!body.contains("answer"), "{body}");
        assert_eq!(f.providers[1].prompts.load(Ordering::SeqCst), 0);
        assert_eq!(f.rows().len(), if boundary == "cleanup" { 1 } else { 2 });
    }
}

#[tokio::test]
async fn served_no_state_migration_or_persistence_replay() {
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
        let body = f.served("SendMessage", &owner, None).await;
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
    use tower::ServiceExt;
    let f = Fixture::new("reject", "ledger-gate", true);
    let owner = AttemptIdentity::initial().unwrap();
    let call = f.served("SendMessage", &owner, None);
    tokio::pin!(call);
    tokio::select! {
        _ = f.dispatch_gate.0.notified() => {},
        body = &mut call => panic!("returned before successor ledger barrier: {body}"),
    }
    let response = f.server().router().oneshot(axum::http::Request::builder()
        .method("POST").uri("/").header("content-type", "application/json").header("A2A-Version", "1.0")
        .body(axum::body::Body::from(serde_json::json!({"jsonrpc":"2.0","id":2,"method":"CancelTask","params":{"taskId":owner.execution_id.as_str()}}).to_string())).unwrap()).await.unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
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
