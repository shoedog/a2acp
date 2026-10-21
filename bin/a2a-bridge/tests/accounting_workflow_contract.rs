//! Foundation-only contract: no workflow executor or served caller is activated.
mod common;
// Reuse slice a's durable row/reopen and usage helpers, including their direct-path fixture.
#[allow(dead_code)]
#[path = "accounting/support.rs"]
mod support;
#[path = "accounting/workflow_support.rs"]
mod workflow_support;

use bridge_core::{
    accounting::{AccountingV1, UsageAttributionV1},
    domain::{EffectiveConfig, Effort, Part, SessionSpec},
    error::BridgeError,
    ids::{AgentId, AttemptIdentity, NodeId, SessionId, WorkflowId},
    ports::{AgentBackend, BackendCleanupDispositionV1, Update},
    routing::ReplayEvidenceV1,
    task_store::TaskStore,
};
use bridge_workflow::accounting::WorkflowAccountingInvocationV1;
use std::sync::{atomic::Ordering, Arc};
use support::{assert_usage, Fixture};
use workflow_support::{dispatch, WorkflowRecording};

fn invocation(
    f: &Fixture,
    owner: &AttemptIdentity,
    workflow: &str,
) -> WorkflowAccountingInvocationV1 {
    WorkflowAccountingInvocationV1::new(
        AccountingV1::new(f.store.clone()),
        owner.clone(),
        WorkflowId::parse(workflow).unwrap(),
    )
}

fn config(model: &str, effort: Effort, mode: &str) -> EffectiveConfig {
    EffectiveConfig {
        model: Some(model.into()),
        effort: Some(effort),
        mode: Some(mode.into()),
    }
}

fn payload(text: &str) -> Vec<Part> {
    vec![Part { text: text.into() }]
}

fn assert_owner(a: &UsageAttributionV1, owner: &AttemptIdentity, workflow: &str) {
    assert_eq!(a.execution.as_deref(), Some(owner.execution_id.as_str()));
    assert_eq!(a.run_attempt.as_deref(), Some(owner.attempt_id.as_str()));
    assert_eq!(a.workflow.as_deref(), Some(workflow));
}

#[tokio::test]
async fn selections_are_effect_free_then_capture_distinct_turns_under_one_owner() {
    let f = Fixture::new();
    let backend = Arc::new(WorkflowRecording::default());
    // Exercise a resumed identity rather than minting a new run for each selection.
    let owner = AttemptIdentity::initial().unwrap().resume().unwrap();
    let invocation = invocation(&f, &owner, "contract");
    let agent = AgentId::parse("candidate").unwrap();
    let configs = [
        config("model-a", Effort::Xhigh, "read-only"),
        config("model-b", Effort::Low, "plan"),
    ];
    let selections: Vec<_> = [
        (&invocation, "draft", "node"),
        (&invocation.clone(), "review", "compaction"),
    ]
    .into_iter()
    .zip(&configs)
    .map(|((invocation, node, kind), config)| {
        invocation
            .select(&NodeId::parse(node).unwrap(), &agent, kind, config)
            .unwrap()
    })
    .collect();
    let ids: Vec<_> = selections
        .iter()
        .map(|s| s.selection().concrete.clone())
        .collect();
    assert_ne!(ids[0], ids[1]);
    for (i, selection) in selections.iter().enumerate() {
        let s = selection.selection();
        assert_eq!(s.owner, owner);
        assert_ne!(s.concrete, owner.attempt_id);
        assert_eq!(s.attribution.turn.as_deref(), Some(ids[i].as_str()));
        assert_eq!(s.attribution.workflow.as_deref(), Some("contract"));
        assert_eq!(s.attribution.effective_model, configs[i].model);
    }
    // A fresh read-only reopen proves inspection did not admit any attempt.
    let reopened = bridge_store::sqlite::SqliteStore::open_history_read_only(
        &f.dir.path().join("tasks.sqlite"),
    )
    .unwrap();
    assert!(reopened
        .usage_summary_v1(&Default::default())
        .await
        .unwrap()
        .buckets
        .is_empty());
    assert!(backend.prompts.lock().unwrap().is_empty());
    assert!(backend.applied.lock().unwrap().is_empty());
    assert!(backend.usage.lock().unwrap().is_empty());
    drop(invocation); // Selections retain their accounting service and ownership.

    for (i, selection) in selections.into_iter().enumerate() {
        let prepared = selection.prepare().await.unwrap();
        assert_owner(prepared.attribution(), &owner, "contract");
        assert!(prepared.matches_config(
            &configs[i].model,
            &Some(if i == 0 { "xhigh" } else { "low" }.into()),
            &configs[i].mode,
        ));
        let session = SessionId::parse(format!("session-{i}")).unwrap();
        backend
            .configure_session(&session, &SessionSpec::from_config(configs[i].clone()))
            .await
            .unwrap();
        let updates = dispatch(
            backend.clone(),
            prepared,
            &session,
            payload(&format!("prompt-{i}")),
        )
        .await
        .unwrap();
        assert!(matches!(updates.last(), Some(Update::Done { .. })));
    }
    let rows = f.rows();
    assert_eq!(rows.len(), 2);
    for (i, id) in ids.iter().enumerate() {
        let row = rows.iter().find(|r| r.id == id.as_str()).unwrap();
        assert_owner(&row.attribution, &owner, "contract");
        assert_eq!(
            row.attribution.node.as_deref(),
            Some(if i == 0 { "draft" } else { "review" })
        );
        assert_eq!(
            row.attribution.attempt_kind,
            if i == 0 { "node" } else { "compaction" }
        );
        assert_eq!(row.attribution.agent, "candidate");
        assert_eq!(row.attribution.turn.as_deref(), Some(id.as_str()));
        assert_eq!(row.attribution.effective_model, configs[i].model);
        assert_eq!(
            row.attribution.effective_effort.as_deref(),
            Some(if i == 0 { "xhigh" } else { "low" })
        );
        assert_eq!(row.attribution.effective_mode, configs[i].mode);
        assert!(row.dispatched && row.finalized);
        let session = SessionId::parse(format!("session-{i}")).unwrap();
        assert_eq!(
            backend.prompts.lock().unwrap()[i],
            (session.clone(), payload(&format!("prompt-{i}")))
        );
        assert_eq!(
            backend.applied.lock().unwrap()[i],
            (session.clone(), configs[i].clone())
        );
        assert_eq!(
            backend.usage.lock().unwrap()[i],
            (session, support::usage())
        );
    }
    assert_eq!(backend.prompts.lock().unwrap().len(), 2);
    assert_eq!(backend.applied.lock().unwrap().len(), 2);
    assert_eq!(backend.usage.lock().unwrap().len(), 2);
    assert_usage(&f.reopened(Some(&owner)).await, 2, 0);
}

#[tokio::test]
async fn admitted_rejections_preserve_typed_cause_and_settlement_outcome_after_reopen() {
    for fail_settlement in [false, true] {
        let f = Fixture::new();
        let backend = Arc::new(WorkflowRecording::default());
        let owner = AttemptIdentity::initial().unwrap();
        let invocation = invocation(&f, &owner, "rejection");
        let selection = invocation
            .select(
                &NodeId::parse("rejected").unwrap(),
                &AgentId::parse("candidate").unwrap(),
                "node",
                &Default::default(),
            )
            .unwrap();
        let id = selection.selection().concrete.clone();
        let prepared = selection.prepare().await.unwrap();
        let original = BridgeError::ConfigMismatch { field: "mode" };
        *backend.reject_config.lock().unwrap() = Some(original.clone());
        if fail_settlement {
            let conn = rusqlite::Connection::open_with_flags(
                f.dir.path().join("tasks.sqlite.usage-v1.sqlite"),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
            )
            .unwrap();
            conn.execute_batch("CREATE TRIGGER refuse_settlement BEFORE UPDATE ON usage_attempts_v1 BEGIN SELECT RAISE(ABORT, 'settlement refused'); END;").unwrap();
        }
        let error = backend
            .configure_session(
                &SessionId::parse("rejected-session").unwrap(),
                &SessionSpec::from_config(Default::default()),
            )
            .await
            .unwrap_err();
        let failure = prepared.reject_preparation(error).await;
        assert_eq!(failure.original, original);
        assert_eq!(
            failure.persistence,
            fail_settlement.then_some(BridgeError::StoreFailure)
        );
        assert_eq!(failure.replay, ReplayEvidenceV1::default());
        assert_eq!(failure.cleanup, BackendCleanupDispositionV1::Unknown);
        assert!(backend.prompts.lock().unwrap().is_empty());
        assert!(backend.applied.lock().unwrap().is_empty());
        assert!(backend.usage.lock().unwrap().is_empty());
        let rows = f.rows();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.id, id.as_str());
        assert_owner(&row.attribution, &owner, "rejection");
        assert!(!row.dispatched);
        assert_eq!(row.finalized, !fail_settlement);
        assert_eq!(row.usage, Default::default());
        assert_eq!(row.attribution.effective_model, None);
        assert_eq!(row.attribution.effective_effort, None);
        assert_eq!(row.attribution.effective_mode, None);
        let b = f.reopened(Some(&owner)).await;
        assert_eq!(
            (b.attempted, b.dispatched, b.pending),
            (1, 0, u64::from(fail_settlement))
        );
        assert_eq!(b.total_tokens.reported_sum, 0);
        assert_eq!(b.total_tokens.missing_count, u64::from(!fail_settlement));
        assert!(b.cost_by_currency.is_empty());
    }
}

#[tokio::test]
async fn shared_backend_keeps_owners_and_bound_services_isolated() {
    for separate_stores in [false, true] {
        let first = Fixture::new();
        let second = Fixture::new();
        let ledgers = [&first, if separate_stores { &second } else { &first }];
        let backend = Arc::new(WorkflowRecording::default());
        let owners = [
            AttemptIdentity::initial().unwrap(),
            AttemptIdentity::initial().unwrap(),
        ];
        let invocations = [
            invocation(ledgers[0], &owners[0], "first"),
            invocation(ledgers[1], &owners[1], "second"),
        ];
        let turns = invocations.iter().enumerate().map(|(i, invocation)| {
            let backend = backend.clone();
            async move {
                let session = SessionId::parse(format!("shared-{i}")).unwrap();
                let config = config(&format!("model-{i}"), Effort::High, "read-only");
                let prepared = invocation
                    .select(
                        &NodeId::parse("same-node").unwrap(),
                        &AgentId::parse("same-agent").unwrap(),
                        "node",
                        &config,
                    )
                    .unwrap()
                    .prepare()
                    .await
                    .unwrap();
                backend
                    .configure_session(&session, &SessionSpec::from_config(config))
                    .await
                    .unwrap();
                dispatch(backend, prepared, &session, payload(&format!("owner-{i}")))
                    .await
                    .unwrap();
            }
        });
        futures::future::join_all(turns).await;
        for i in 0..2 {
            assert_usage(&ledgers[i].reopened(Some(&owners[i])).await, 1, 0);
            let rows = ledgers[i].rows();
            assert_eq!(rows.len(), if separate_stores { 1 } else { 2 });
            let row = rows
                .iter()
                .find(|r| {
                    r.attribution.execution.as_deref() == Some(owners[i].execution_id.as_str())
                })
                .unwrap();
            assert_owner(
                &row.attribution,
                &owners[i],
                if i == 0 { "first" } else { "second" },
            );
            assert_eq!(row.attribution.effective_model, Some(format!("model-{i}")));
            let session = SessionId::parse(format!("shared-{i}")).unwrap();
            assert!(backend
                .prompts
                .lock()
                .unwrap()
                .contains(&(session.clone(), payload(&format!("owner-{i}")),)));
            assert!(backend.applied.lock().unwrap().contains(&(
                session.clone(),
                config(&format!("model-{i}"), Effort::High, "read-only"),
            )));
            assert!(backend
                .usage
                .lock()
                .unwrap()
                .contains(&(session, support::usage())));
        }
        assert_eq!(backend.prompts.lock().unwrap().len(), 2);
        assert_eq!(backend.applied.lock().unwrap().len(), 2);
        assert_eq!(backend.usage.lock().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn interrupted_capture_and_dropped_preparation_remain_pending() {
    let f = Fixture::new();
    let backend = Arc::new(WorkflowRecording::default());
    backend.interrupt.store(true, Ordering::SeqCst);
    let owner = AttemptIdentity::initial().unwrap();
    let invocation = invocation(&f, &owner, "interruption");
    for dispatched in [false, true] {
        let prepared = invocation
            .select(
                &NodeId::parse("node").unwrap(),
                &AgentId::parse("agent").unwrap(),
                "node",
                &Default::default(),
            )
            .unwrap()
            .prepare()
            .await
            .unwrap();
        if dispatched {
            assert!(matches!(
                dispatch(
                    backend.clone(),
                    prepared,
                    &SessionId::parse("interrupted").unwrap(),
                    payload("interrupted")
                )
                .await,
                Err(BridgeError::MissingTerminal)
            ));
        } else {
            drop(prepared);
        }
    }
    assert_eq!(backend.prompts.lock().unwrap().len(), 1);
    assert_eq!(backend.usage.lock().unwrap().len(), 1);
    let rows = f.rows();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| !row.finalized));
    assert_eq!(rows.iter().filter(|row| row.dispatched).count(), 1);
    let b = f.reopened(Some(&owner)).await;
    assert_eq!((b.attempted, b.dispatched, b.pending), (2, 1, 2));
    assert_eq!(b.total_tokens.reported_sum, 15);
}
