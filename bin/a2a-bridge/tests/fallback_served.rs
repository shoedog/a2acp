#[path = "fallback_direct.rs"]
mod direct;
use bridge_core::{ids::*, task_store::TaskStore, workflow_history::WorkflowHistoryStore};
use direct::Fixture;
use std::sync::atomic::Ordering;

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
