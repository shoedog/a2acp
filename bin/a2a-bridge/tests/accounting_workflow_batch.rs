//! Batch children and boot recovery exercise the same coordinator as served RPCs.
use served::common;
#[path = "accounting_workflow_served.rs"]
mod served;
use bridge_coordinator::batch::BatchParams;
use bridge_core::{accounting::UsageQueryV1, ids::*, task_store::*};
use served::{coordinator, op, settled, support};

// Model a crash after capture but before checkpoint/terminal projection. Keep
// accounting intact, including the already captured charge for the lost checkpoint.
fn crash(f: &support::Fixture, task: &TaskId, incomplete: bool) {
    let db = rusqlite::Connection::open(f.dir.path().join("tasks.sqlite")).unwrap();
    db.execute("UPDATE tasks SET status='working', result=NULL, error=NULL, terminal_seq=NULL, workflow_outcome=NULL, terminal_projection_ready=1, terminal_projection_attempt_id=NULL, terminal_projection_json=NULL WHERE id=?1", [task.as_str()]).unwrap();
    if incomplete {
        db.execute(
            "DELETE FROM task_node_checkpoints WHERE task_id=?1 AND node_id='review'",
            [task.as_str()],
        )
        .unwrap();
    }
}
async fn count(f: &support::Fixture, query: UsageQueryV1) -> u64 {
    let db = bridge_store::sqlite::SqliteStore::open_history_read_only(
        &f.dir.path().join("tasks.sqlite"),
    )
    .unwrap();
    db.usage_summary_v1(&query)
        .await
        .unwrap()
        .buckets
        .iter()
        .map(|b| b.attempted)
        .sum()
}
async fn completed_batch(
    f: &support::Fixture,
    c: &bridge_coordinator::Coordinator,
) -> (BatchId, Vec<TaskRecord>) {
    let bid = c
        .run_batch(BatchParams {
            workflow: "flow".into(),
            concurrency: Some(2),
            items: ["a", "b"]
                .into_iter()
                .map(|id| BatchItem {
                    item_id: id.into(),
                    input: op().input,
                    session_cwd: None,
                })
                .collect(),
        })
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while c.batch_status(&bid).await.unwrap().status == BatchStatus::Working {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let children = f.store.batch_children(&bid).await.unwrap();
    (bid, children)
}
#[tokio::test]
async fn batch_child_filters_and_resumed_checkpoint_ownership() {
    child_ownership(false).await;
}
#[tokio::test]
async fn bound_batch_child_resumes_with_structured_checkpoint_ownership() {
    child_ownership(true).await;
}
async fn child_ownership(bound: bool) {
    let f = support::Fixture::new();
    let c = coordinator(&f, true, bound);
    let (bid, children) = completed_batch(&f, &c).await;
    assert_eq!(children.len(), 2);
    assert_eq!(f.backend.prompts.lock().unwrap().len(), 4);
    let mut owners = Vec::new();
    for child in &children {
        assert_eq!(
            settled(&f, &c, &child.id).await.status,
            TaskRecordStatus::Completed
        );
        let owner = f
            .store
            .get_attempt_locator(&child.id)
            .await
            .unwrap()
            .unwrap()
            .identity;
        support::assert_usage(&f.reopened(Some(&owner)).await, 2, 0);
        for node in ["draft", "review"] {
            assert_eq!(
                count(
                    &f,
                    UsageQueryV1 {
                        execution: Some(owner.execution_id.as_str().into()),
                        node: Some(node.into()),
                        ..Default::default()
                    }
                )
                .await,
                1
            );
            assert_eq!(
                count(
                    &f,
                    UsageQueryV1 {
                        run_attempt: Some(owner.attempt_id.as_str().into()),
                        node: Some(node.into()),
                        ..Default::default()
                    }
                )
                .await,
                1
            );
        }
        owners.push(owner);
    }
    assert_ne!(owners[0].execution_id, owners[1].execution_id);
    assert_eq!(
        count(
            &f,
            UsageQueryV1 {
                execution: Some(owners[0].execution_id.as_str().into()),
                run_attempt: Some(owners[1].attempt_id.as_str().into()),
                ..Default::default()
            }
        )
        .await,
        0
    );
    let before = f.rows();
    crash(&f, &children[0].id, true);
    crash(&f, &children[1].id, false);
    let db = rusqlite::Connection::open(f.dir.path().join("tasks.sqlite")).unwrap();
    db.execute(
        "UPDATE batch SET status='working' WHERE id=?1",
        [bid.as_str()],
    )
    .unwrap();
    c.resume().await.unwrap();
    for child in &children {
        assert_eq!(
            settled(&f, &c, &child.id).await.status,
            TaskRecordStatus::Completed
        );
    }
    let resumed = f
        .store
        .get_attempt_locator(&children[0].id)
        .await
        .unwrap()
        .unwrap()
        .identity;
    assert_eq!(resumed.execution_id, owners[0].execution_id);
    assert_ne!(resumed.attempt_id, owners[0].attempt_id);
    assert_eq!(resumed.ordinal, 1);
    assert_eq!(
        resumed.parent_attempt_id.as_ref(),
        Some(&owners[0].attempt_id)
    );
    support::assert_usage(&f.reopened(Some(&resumed)).await, 1, 0);
    assert_eq!(f.backend.prompts.lock().unwrap().len(), 5);
    let after = f.rows();
    for row in before {
        assert_eq!(after.iter().find(|r| r.id == row.id), Some(&row));
    }
    assert_eq!(after.len(), 5);
    let fresh = after
        .iter()
        .find(|r| r.attribution.run_attempt.as_deref() == Some(resumed.attempt_id.as_str()))
        .unwrap();
    assert_eq!(fresh.attribution.node.as_deref(), Some("review"));
    assert_eq!(fresh.attribution.turn.as_ref(), Some(&fresh.id));
    assert_eq!(
        f.store
            .get_attempt_locator(&children[1].id)
            .await
            .unwrap()
            .unwrap()
            .identity,
        owners[1]
    );
}
#[tokio::test]
async fn non_batch_recovery_forwards_owner_with_and_without_batch_runtime() {
    for batch in [false, true] {
        let f = support::Fixture::new();
        let c = coordinator(&f, batch, false);
        let task = c.run_workflow(op()).await.unwrap();
        settled(&f, &c, &task).await;
        let prior = f
            .store
            .get_attempt_locator(&task)
            .await
            .unwrap()
            .unwrap()
            .identity;
        let before = f.rows();
        crash(&f, &task, true);
        c.resume().await.unwrap();
        assert_eq!(
            settled(&f, &c, &task).await.status,
            TaskRecordStatus::Completed
        );
        let owner = f
            .store
            .get_attempt_locator(&task)
            .await
            .unwrap()
            .unwrap()
            .identity;
        assert_eq!(owner.execution_id, prior.execution_id);
        assert_ne!(owner.attempt_id, prior.attempt_id);
        support::assert_usage(&f.reopened(Some(&owner)).await, 1, 0);
        let after = f.rows();
        for row in before {
            assert_eq!(after.iter().find(|r| r.id == row.id), Some(&row));
        }
        assert_eq!(f.backend.prompts.lock().unwrap().len(), 3);
    }
}

#[tokio::test]
async fn bound_batch_refuses_missing_corrupt_and_committed_resume_evidence() {
    use bridge_core::execution_policy::{
        ControlEventIdV1, FanOutPolicyNameV1, PolicyNodeRefV1, PolicyTriggerV1,
    };
    for fault in [
        "missing-terminal",
        "corrupt-terminal",
        "corrupt-trigger",
        "committed-trigger",
    ] {
        let f = support::Fixture::new();
        let c = coordinator(&f, true, true);
        let (bid, children) = completed_batch(&f, &c).await;
        let task = &children[0].id;
        settled(&f, &c, task).await;
        let prior = f.store.get_attempt_locator(task).await.unwrap().unwrap();
        let before = f.rows();
        crash(&f, task, true);
        let db = rusqlite::Connection::open(f.dir.path().join("tasks.sqlite")).unwrap();
        db.execute(
            "UPDATE batch SET status='working' WHERE id=?1",
            [bid.as_str()],
        )
        .unwrap();
        let trigger = String::from_utf8(
            PolicyTriggerV1 {
                schema_version: 1,
                id: ControlEventIdV1::for_attempt(&prior.identity.attempt_id, 0),
                node: PolicyNodeRefV1::from_node_id(0, "draft"),
                policy: FanOutPolicyNameV1::FailFast,
                grace_ms: None,
            }
            .encode_canonical()
            .unwrap(),
        )
        .unwrap();
        match fault {
            "missing-terminal" => {
                db.execute("UPDATE task_node_checkpoints SET terminal_json=NULL WHERE task_id=?1 AND node_id='draft'", [task.as_str()]).unwrap();
            }
            "corrupt-terminal" => {
                db.execute("UPDATE task_node_checkpoints SET terminal_json='invalid' WHERE task_id=?1 AND node_id='draft'", [task.as_str()]).unwrap();
            }
            _ => {
                db.execute(
                    "UPDATE tasks SET policy_trigger_json=?2 WHERE id=?1",
                    rusqlite::params![
                        task.as_str(),
                        if fault == "committed-trigger" {
                            &trigger
                        } else {
                            "invalid"
                        }
                    ],
                )
                .unwrap();
            }
        }
        f.backend.prompts.lock().unwrap().clear();
        c.resume().await.unwrap();
        let record = settled(&f, &c, task).await;
        assert_eq!(
            record.status,
            TaskRecordStatus::Interrupted,
            "{fault}: {:?}",
            record.error
        );
        assert_eq!(record.resume_attempts, 0, "{fault}");
        assert!(f.backend.prompts.lock().unwrap().is_empty(), "{fault}");
        assert_eq!(f.rows(), before, "{fault}");
        assert_eq!(
            f.store
                .get_attempt_locator(task)
                .await
                .unwrap()
                .unwrap()
                .identity,
            prior.identity,
            "{fault}"
        );
        if fault == "committed-trigger" {
            assert_eq!(
                f.store
                    .workflow_task_evidence(task)
                    .await
                    .unwrap()
                    .policy_trigger_json
                    .as_deref(),
                Some(trigger.as_str())
            );
        }
    }
}
