use std::collections::HashMap;
use std::sync::Arc;

use bridge_core::ids::BatchId;
use bridge_core::session_cwd::SessionCwd;
use bridge_core::task_store::{
    BatchRecord, BatchStatus, BatchSummary, TaskRecord, TaskRecordStatus,
};
use tokio::sync::{Mutex, Semaphore};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct BatchRuntime {
    pub semaphore: Arc<Semaphore>,
    pub default_concurrency: u32,
    pub max_concurrent: u32,
    pub batch_cancels: Arc<Mutex<HashMap<BatchId, CancellationToken>>>,
}

impl BatchRuntime {
    pub fn new(max_concurrent: u32, default_concurrency: u32) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrent as usize)),
            default_concurrency,
            max_concurrent,
            batch_cancels: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[derive(Clone)]
pub struct BatchDeps {
    pub detached: crate::detached::DetachedDeps,
    pub runtime: BatchRuntime,
    pub allowed_cwd_root: Option<SessionCwd>,
}

/// Pure roll-up over the durable plan (rec.total) + the child rows. The SINGLE owner
/// of the bucket math (RR-FIX-7) -- never re-implemented in a store impl.
pub fn summarize_batch(rec: &BatchRecord, children: &[TaskRecord]) -> BatchSummary {
    let mut ok = 0;
    let mut failed = 0;
    let mut canceled = 0;
    let mut running = 0;
    let mut kids = Vec::with_capacity(children.len());

    for c in children {
        match c.status {
            TaskRecordStatus::Completed => ok += 1,
            TaskRecordStatus::Failed | TaskRecordStatus::Interrupted => failed += 1,
            TaskRecordStatus::Canceled => canceled += 1,
            TaskRecordStatus::Working => running += 1,
        }
        kids.push((c.item_id.clone().unwrap_or_default(), c.id.clone(), c.status));
    }

    let pending = rec.total.saturating_sub(children.len() as u32);
    BatchSummary {
        id: rec.id.clone(),
        workflow: rec.workflow.clone(),
        status: rec.status,
        total: rec.total,
        ok,
        failed,
        canceled,
        running,
        pending,
        children: kids,
    }
}

/// The SINGLE settle predicate (RR-FIX-8): Working drained -> Completed; Canceling with
/// no running child -> Canceled. Returns the terminal to CAS into, or None.
pub fn is_settleable(s: &BatchSummary) -> Option<BatchStatus> {
    match s.status {
        BatchStatus::Working if s.pending == 0 && s.running == 0 => Some(BatchStatus::Completed),
        BatchStatus::Canceling if s.running == 0 => Some(BatchStatus::Canceled),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::{
        ids::{BatchId, TaskId},
        task_store::{BatchRecord, BatchStatus, TaskRecord, TaskRecordStatus},
    };

    fn br(status: BatchStatus, total: u32) -> BatchRecord {
        BatchRecord {
            id: BatchId::parse("batch-1").unwrap(),
            workflow: "code-review".into(),
            concurrency: 2,
            total,
            status,
            items_json: r#"{"v":1,"items":[]}"#.into(),
            error: None,
            created_ms: 10,
            updated_ms: 10,
        }
    }

    fn child(item: &str, status: TaskRecordStatus) -> TaskRecord {
        TaskRecord {
            id: TaskId::parse(format!("task-{item}")).unwrap(),
            workflow: "code-review".into(),
            status,
            result: None,
            error: None,
            created_ms: 10,
            updated_ms: 10,
            input: format!("input-{item}"),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: None,
            batch_id: Some(BatchId::parse("batch-1").unwrap()),
            item_id: Some(item.into()),
        }
    }

    #[test]
    fn summary_buckets_every_task_status() {
        let rec = br(BatchStatus::Working, 5);
        let kids = vec![
            child("a", TaskRecordStatus::Completed),
            child("b", TaskRecordStatus::Failed),
            child("c", TaskRecordStatus::Canceled),
            child("d", TaskRecordStatus::Interrupted),
            child("e", TaskRecordStatus::Working),
        ];
        let s = summarize_batch(&rec, &kids);
        assert_eq!(
            (s.ok, s.failed, s.canceled, s.running, s.pending),
            (1, 2, 1, 1, 0)
        );
    }

    #[test]
    fn pending_is_total_minus_rows() {
        let rec = br(BatchStatus::Working, 4);
        let kids = vec![child("a", TaskRecordStatus::Completed)];
        assert_eq!(summarize_batch(&rec, &kids).pending, 3);
    }

    #[test]
    fn is_settleable_working_completes_only_when_drained() {
        let rec = br(BatchStatus::Working, 2);
        let done = vec![
            child("a", TaskRecordStatus::Completed),
            child("b", TaskRecordStatus::Failed),
        ];
        assert_eq!(
            is_settleable(&summarize_batch(&rec, &done)),
            Some(BatchStatus::Completed)
        );
        let running = vec![
            child("a", TaskRecordStatus::Working),
            child("b", TaskRecordStatus::Completed),
        ];
        assert_eq!(is_settleable(&summarize_batch(&rec, &running)), None);
    }

    #[test]
    fn is_settleable_canceling_settles_when_no_running() {
        let rec = br(BatchStatus::Canceling, 3);
        let kids = vec![
            child("a", TaskRecordStatus::Canceled),
            child("b", TaskRecordStatus::Completed),
        ];
        assert_eq!(
            is_settleable(&summarize_batch(&rec, &kids)),
            Some(BatchStatus::Canceled)
        );
    }
}
