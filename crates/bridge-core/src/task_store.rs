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
