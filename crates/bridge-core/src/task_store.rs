//! Durable task control-plane port: persists a detached workflow run's status
//! and final result. Separate from `SessionStore` (ephemeral routing state) by
//! responsibility. Timestamps are passed IN — the core forbids `Date::now`.

use crate::error::BridgeError;
use crate::ids::TaskId;

/// Durable status of a detached task. `Interrupted` is distinct from `Failed`
/// (a crash mid-run, swept on the next boot) so triage can tell them apart; the
/// A2A wire collapses it to `failed` (see the inbound server).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TaskRecordStatus {
    Working,
    Completed,
    Failed,
    Canceled,
    Interrupted,
}

impl TaskRecordStatus {
    /// Lowercase wire/storage token.
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskRecordStatus::Working => "working",
            TaskRecordStatus::Completed => "completed",
            TaskRecordStatus::Failed => "failed",
            TaskRecordStatus::Canceled => "canceled",
            TaskRecordStatus::Interrupted => "interrupted",
        }
    }
    /// Parse a stored token; unknown → None.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "working" => Some(TaskRecordStatus::Working),
            "completed" => Some(TaskRecordStatus::Completed),
            "failed" => Some(TaskRecordStatus::Failed),
            "canceled" => Some(TaskRecordStatus::Canceled),
            "interrupted" => Some(TaskRecordStatus::Interrupted),
            _ => None,
        }
    }
    pub fn is_terminal(&self) -> bool {
        !matches!(self, TaskRecordStatus::Working)
    }
}

/// One durable task row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskRecord {
    pub id: TaskId,
    pub workflow: String,
    pub status: TaskRecordStatus,
    pub result: Option<String>,
    pub error: Option<String>,
    pub created_ms: i64,
    pub updated_ms: i64,
}

#[async_trait::async_trait]
pub trait TaskStore: Send + Sync {
    /// Non-clobbering INSERT. A duplicate id MUST return an error (NOT upsert),
    /// so a resubmit/colliding id can never overwrite a terminal result.
    async fn create(&self, rec: &TaskRecord) -> Result<(), BridgeError>;
    /// Set the terminal status + result/error on an existing row.
    async fn set_terminal(
        &self,
        id: &TaskId,
        status: TaskRecordStatus,
        result: Option<&str>,
        error: Option<&str>,
        updated_ms: i64,
    ) -> Result<(), BridgeError>;
    async fn get(&self, id: &TaskId) -> Result<Option<TaskRecord>, BridgeError>;
    /// Newest-first, capped at `limit`.
    async fn list(&self, limit: usize) -> Result<Vec<TaskRecord>, BridgeError>;
    /// Flip every `Working` row to `Interrupted`; returns the count flipped.
    async fn sweep_interrupted(&self, updated_ms: i64) -> Result<u64, BridgeError>;
}

use std::collections::HashMap;
use std::sync::Mutex;

/// In-memory `TaskStore` (the default when no DB path is configured). Production
/// use, not just a test fake — lives in `bridge-core` so `bridge-a2a-inbound`
/// can default to it WITHOUT depending on `bridge-store`.
#[derive(Default)]
pub struct MemoryTaskStore {
    inner: Mutex<HashMap<String, TaskRecord>>,
}

impl MemoryTaskStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl TaskStore for MemoryTaskStore {
    async fn create(&self, rec: &TaskRecord) -> Result<(), BridgeError> {
        let mut g = self.inner.lock().unwrap();
        if g.contains_key(rec.id.as_str()) {
            return Err(BridgeError::StoreFailure);
        }
        g.insert(rec.id.as_str().to_string(), rec.clone());
        Ok(())
    }
    async fn set_terminal(
        &self,
        id: &TaskId,
        status: TaskRecordStatus,
        result: Option<&str>,
        error: Option<&str>,
        updated_ms: i64,
    ) -> Result<(), BridgeError> {
        let mut g = self.inner.lock().unwrap();
        let row = g.get_mut(id.as_str()).ok_or(BridgeError::StoreFailure)?;
        row.status = status;
        row.result = result.map(|s| s.to_string());
        row.error = error.map(|s| s.to_string());
        row.updated_ms = updated_ms;
        Ok(())
    }
    async fn get(&self, id: &TaskId) -> Result<Option<TaskRecord>, BridgeError> {
        Ok(self.inner.lock().unwrap().get(id.as_str()).cloned())
    }
    async fn list(&self, limit: usize) -> Result<Vec<TaskRecord>, BridgeError> {
        let g = self.inner.lock().unwrap();
        let mut v: Vec<TaskRecord> = g.values().cloned().collect();
        v.sort_by(|a, b| b.updated_ms.cmp(&a.updated_ms));
        v.truncate(limit);
        Ok(v)
    }
    async fn sweep_interrupted(&self, updated_ms: i64) -> Result<u64, BridgeError> {
        let mut g = self.inner.lock().unwrap();
        let mut n = 0;
        for row in g.values_mut() {
            if row.status == TaskRecordStatus::Working {
                row.status = TaskRecordStatus::Interrupted;
                row.error = Some("interrupted (serve restarted)".into());
                row.updated_ms = updated_ms;
                n += 1;
            }
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str, ms: i64) -> TaskRecord {
        TaskRecord {
            id: TaskId::parse(id).unwrap(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: ms,
            updated_ms: ms,
        }
    }

    #[tokio::test]
    async fn create_get_roundtrip_then_set_terminal() {
        let s = MemoryTaskStore::new();
        let id = TaskId::parse("t-a").unwrap();
        s.create(&rec("t-a", 10)).await.unwrap();
        assert_eq!(
            s.get(&id).await.unwrap().unwrap().status,
            TaskRecordStatus::Working
        );
        s.set_terminal(&id, TaskRecordStatus::Completed, Some("OUT"), None, 20)
            .await
            .unwrap();
        let got = s.get(&id).await.unwrap().unwrap();
        assert_eq!(got.status, TaskRecordStatus::Completed);
        assert_eq!(got.result.as_deref(), Some("OUT"));
        assert_eq!(got.updated_ms, 20);
    }

    #[tokio::test]
    async fn create_is_non_clobbering() {
        let s = MemoryTaskStore::new();
        s.create(&rec("dup", 1)).await.unwrap();
        assert!(s.create(&rec("dup", 2)).await.is_err());
    }

    #[tokio::test]
    async fn list_is_newest_first_and_limited() {
        let s = MemoryTaskStore::new();
        s.create(&rec("old", 1)).await.unwrap();
        s.create(&rec("new", 5)).await.unwrap();
        let got = s.list(1).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id.as_str(), "new");
    }

    #[tokio::test]
    async fn sweep_flips_only_working() {
        let s = MemoryTaskStore::new();
        s.create(&rec("w", 1)).await.unwrap();
        let done = TaskId::parse("done").unwrap();
        s.create(&rec("done", 1)).await.unwrap();
        s.set_terminal(&done, TaskRecordStatus::Completed, Some("x"), None, 2)
            .await
            .unwrap();
        let n = s.sweep_interrupted(99).await.unwrap();
        assert_eq!(n, 1);
        assert_eq!(
            s.get(&TaskId::parse("w").unwrap())
                .await
                .unwrap()
                .unwrap()
                .status,
            TaskRecordStatus::Interrupted
        );
        assert_eq!(
            s.get(&done).await.unwrap().unwrap().status,
            TaskRecordStatus::Completed
        );
    }
}
