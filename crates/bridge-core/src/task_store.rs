//! Durable task control-plane port: persists a detached workflow run's status
//! and final result. Separate from `SessionStore` (ephemeral routing state) by
//! responsibility. Timestamps are passed IN — the core forbids `Date::now`.

use crate::error::BridgeError;
use crate::ids::{BatchId, ContextId, NodeId, OperationId, TaskId, TurnId};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const RETENTION_NEVER_ELIGIBLE_MS: i64 = i64::MAX;

pub type PersistenceClock = Arc<dyn Fn() -> i64 + Send + Sync>;

pub fn system_wall_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(RETENTION_NEVER_ELIGIBLE_MS)
}

pub fn valid_retention_wall_ms(ms: i64) -> bool {
    ms > 0 && ms < RETENTION_NEVER_ELIGIBLE_MS
}

pub fn durable_retention_ms(ms: i64) -> i64 {
    if valid_retention_wall_ms(ms) {
        ms
    } else {
        RETENTION_NEVER_ELIGIBLE_MS
    }
}

/// Durable status of a detached task. `Interrupted` is distinct from `Failed`
/// (a crash mid-run, swept on the next boot) so triage can tell them apart; the
/// A2A wire collapses it to `failed` (see the inbound server).
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchStatus {
    Working,
    Completed,
    Canceling,
    Canceled,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BatchItem {
    pub item_id: String,
    pub input: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_cwd: Option<String>,
}

#[derive(Clone, Debug)]
pub struct BatchRecord {
    pub id: crate::ids::BatchId,
    pub workflow: String,
    pub concurrency: u32,
    pub total: u32,
    pub status: BatchStatus,
    pub items_json: String,
    pub error: Option<String>,
    pub created_ms: i64,
    pub updated_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct BatchSummary {
    pub id: crate::ids::BatchId,
    pub workflow: String,
    pub status: BatchStatus,
    pub total: u32,
    pub ok: u32,
    pub failed: u32,
    pub canceled: u32,
    pub running: u32,
    pub pending: u32,
    pub children: Vec<(String, crate::ids::TaskId, TaskRecordStatus)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChildClaim {
    Created,
    ExistingWorking,
    ExistingTerminal,
}

pub fn terminal_status_from_record(s: &TaskRecordStatus) -> crate::orch::TerminalStatus {
    use crate::orch::TerminalStatus;
    match s {
        TaskRecordStatus::Completed => TerminalStatus::Completed,
        TaskRecordStatus::Canceled => TerminalStatus::Canceled,
        other => TerminalStatus::Failed {
            reason: other.as_str().to_string(),
        },
    }
}

pub fn turn_log_outcome_strings(
    outcome: &crate::ports::TurnOutcome,
) -> (&'static str, Option<&'static str>) {
    use crate::ports::{FailureClass, TurnOutcome};
    match outcome {
        TurnOutcome::Success => ("success", None),
        TurnOutcome::Canceled => ("canceled", None),
        TurnOutcome::Failed(FailureClass::AgentCrashed) => ("failed", Some("agent_crashed")),
        TurnOutcome::Failed(FailureClass::TimedOut) => ("failed", Some("timed_out")),
        TurnOutcome::Failed(FailureClass::Overloaded) => ("failed", Some("overloaded")),
        TurnOutcome::Failed(FailureClass::Config) => ("failed", Some("config")),
        TurnOutcome::Failed(FailureClass::Transport) => ("failed", Some("transport")),
        TurnOutcome::Failed(FailureClass::Other) => ("failed", Some("other")),
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
    pub last_artifact_ms: Option<i64>,
    /// The raw input text submitted with this task (needed for crash-resume replay).
    pub input: String,
    /// JSON-serialized workflow spec snapshot at submit time (crash-resume needs the
    /// exact graph that was running, not the live on-disk spec which may have changed).
    pub workflow_spec_json: Option<String>,
    /// Number of resume attempts consumed so far; used by `claim_resume_attempt`.
    pub resume_attempts: u32,
    /// Per-request working directory at detached-submit time (Task 8/9 of the
    /// session_cwd increment). `None` means no cwd override was requested. Stored
    /// in its OWN `tasks` column (NOT in the `workflow_spec_json` envelope) so it
    /// is independently accessible without deserializing the snapshot.
    pub session_cwd: Option<String>,
    pub batch_id: Option<crate::ids::BatchId>,
    pub item_id: Option<String>,
    pub artifacts_purged_at: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct TurnLogFinished {
    pub ctx: crate::ports::TurnContext,
    pub started_ms: i64,
    pub completed_ms: i64,
    pub latency: Duration,
    pub ttft: Option<Duration>,
    pub outcome: crate::ports::TurnOutcome,
}

#[derive(Clone, Debug)]
pub struct TurnLogUsage {
    pub ctx: crate::ports::TurnContext,
    pub usage: crate::orch::UsageSnapshot,
}

#[derive(Clone, Debug)]
pub struct TurnLogFinalized {
    pub ctx: crate::ports::TurnContext,
    pub finalization: TurnUsageFinalization,
}

#[derive(Clone, Debug)]
pub enum TurnUsageFinalization {
    Usage(crate::orch::UsageSnapshot),
    NoUsage,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TurnLogRow {
    pub turn_id: TurnId,
    pub session_id: ContextId,
    pub task_id: Option<TaskId>,
    pub workflow: Option<String>,
    pub node: Option<String>,
    pub attempt: u32,
    pub agent: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub mode: Option<String>,
    pub prompt_id: Option<String>,
    pub started_ms: Option<i64>,
    pub completed_ms: Option<i64>,
    pub latency_ms: Option<u64>,
    pub ttft_ms: Option<u64>,
    pub outcome: Option<String>,
    pub failure_class: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub thought_tokens: Option<u64>,
    pub cached_read_tokens: Option<u64>,
    pub cached_write_tokens: Option<u64>,
    pub cost_amount: Option<f64>,
    pub cost_currency: Option<String>,
    pub traceparent: Option<crate::ports::TraceParent>,
    pub usage_finalized_ms: Option<i64>,
    pub usage_finalization_kind: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeArtifactMeta {
    pub node: NodeId,
    pub finished: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NodeCheckpointOutput {
    Found {
        output: String,
        ok: bool,
        usage: Option<crate::orch::UsageSnapshot>,
        bytes: u64,
    },
    TooLarge {
        bytes: u64,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct TaskUsageAgg {
    pub rows: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub thought_tokens: Option<u64>,
    pub cached_read_tokens: Option<u64>,
    pub cached_write_tokens: Option<u64>,
    pub cost: Option<crate::orch::UsageCost>,
    pub at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JournalRead {
    Body {
        jsonl: String,
        events: u64,
        bytes: u64,
    },
    TooLarge {
        events: u64,
        bytes: u64,
    },
}

/// Outcome of a `claim_resume_attempt` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeClaim {
    /// The attempt was granted; `attempt` is the new (incremented) count.
    Resumable { attempt: u32 },
    /// The cap has been reached; this task must be marked `Interrupted` instead.
    Exhausted,
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
    /// Atomically cancel a row ONLY if it is still `Working`; returns `true` if it
    /// flipped. This is the single-writer guard for `tasks/cancel`'s no-token path:
    /// the runner removes its cancel token only AFTER writing the terminal row, so a
    /// "no token" cancel must NOT unconditionally overwrite a terminal the runner
    /// just wrote — this conditional update no-ops on an already-terminal row.
    async fn cancel_if_working(&self, id: &TaskId, updated_ms: i64) -> Result<bool, BridgeError>;
    /// Persist a per-node output checkpoint for crash-resume.
    /// Each (task, node) is written at most once: a second write for the same
    /// pair is an error. The SQLite impl enforces this via the (task_id, node_id)
    /// primary key; the in-memory impl matches. The task id must already exist;
    /// writing a checkpoint for an unknown task returns an error.
    async fn put_node_checkpoint(
        &self,
        task: &TaskId,
        node: &NodeId,
        output: &str,
        ok: bool,
        ts: i64,
    ) -> Result<(), BridgeError>;
    /// Return all node checkpoints for a task as `(node_id, output, ok, usage)` tuples.
    async fn node_checkpoints(
        &self,
        task: &TaskId,
    ) -> Result<Vec<(NodeId, String, bool, Option<crate::orch::UsageSnapshot>)>, BridgeError>;
    /// Atomic poison-pill guard: if `resume_attempts < cap`, increments
    /// `resume_attempts` and returns `Resumable { attempt: new_count }`.
    /// If already `>= cap`, returns `Exhausted` without modifying the row.
    /// Missing task id returns an error.
    async fn claim_resume_attempt(
        &self,
        task: &TaskId,
        cap: u32,
        now_ms: i64,
    ) -> Result<ResumeClaim, BridgeError>;
    /// Return all rows whose status is `Working` (for the boot-time resume scan).
    async fn working_tasks(&self) -> Result<Vec<TaskRecord>, BridgeError>;

    async fn upsert_turn_finished(&self, _row: &TurnLogFinished) -> Result<(), BridgeError> {
        Err(BridgeError::StoreFailure)
    }

    async fn finalize_turn_usage(&self, _row: &TurnLogFinalized) -> Result<(), BridgeError> {
        Err(BridgeError::StoreFailure)
    }

    async fn update_turn_usage(&self, row: &TurnLogUsage) -> Result<(), BridgeError> {
        self.finalize_turn_usage(&TurnLogFinalized {
            ctx: row.ctx.clone(),
            finalization: TurnUsageFinalization::Usage(row.usage.clone()),
        })
        .await
    }

    async fn turn_log_rows(&self) -> Result<Vec<TurnLogRow>, BridgeError> {
        Err(BridgeError::StoreFailure)
    }

    async fn turn_log_row(&self, _turn_id: &TurnId) -> Result<Option<TurnLogRow>, BridgeError> {
        Err(BridgeError::StoreFailure)
    }

    async fn turn_log_rows_for_task(
        &self,
        _task: &TaskId,
        _limit: usize,
    ) -> Result<Vec<TurnLogRow>, BridgeError> {
        Err(BridgeError::StoreFailure)
    }

    async fn turn_log_usage_for_task(
        &self,
        _task: &TaskId,
    ) -> Result<Option<TaskUsageAgg>, BridgeError> {
        Err(BridgeError::StoreFailure)
    }

    async fn latest_turn_log_row_for_session(
        &self,
        _session: &ContextId,
    ) -> Result<Option<TurnLogRow>, BridgeError> {
        Err(BridgeError::StoreFailure)
    }

    async fn journal_jsonl_bounded(
        &self,
        _task: &TaskId,
        _max_events: usize,
        _max_bytes: usize,
    ) -> Result<JournalRead, BridgeError> {
        Err(BridgeError::StoreFailure)
    }

    async fn node_checkpoint_nodes(&self, _task: &TaskId) -> Result<Vec<NodeId>, BridgeError> {
        Err(BridgeError::StoreFailure)
    }

    async fn node_checkpoint_output(
        &self,
        _task: &TaskId,
        _node: &NodeId,
        _max_bytes: usize,
    ) -> Result<Option<NodeCheckpointOutput>, BridgeError> {
        Err(BridgeError::StoreFailure)
    }

    async fn create_batch(&self, _rec: &BatchRecord) -> Result<(), BridgeError> {
        Err(BridgeError::StoreFailure)
    }
    async fn get_batch(&self, _id: &BatchId) -> Result<Option<BatchRecord>, BridgeError> {
        Err(BridgeError::StoreFailure)
    }
    async fn list_batches(&self, _limit: usize) -> Result<Vec<BatchRecord>, BridgeError> {
        Ok(vec![])
    }
    /// Return batches that still need boot/resume ownership: `Working` or `Canceling`.
    async fn active_batches(&self) -> Result<Vec<BatchRecord>, BridgeError> {
        Ok(vec![])
    }
    async fn batch_children(&self, _id: &BatchId) -> Result<Vec<TaskRecord>, BridgeError> {
        Ok(vec![])
    }
    /// Atomic insert-or-observe on `(batch_id, item_id)`. Spawn only on `Created`.
    async fn claim_batch_child(
        &self,
        _batch: &BatchId,
        _item: &str,
        _rec: &TaskRecord,
    ) -> Result<ChildClaim, BridgeError> {
        Err(BridgeError::StoreFailure)
    }
    /// CAS `Working` -> `Canceling`; false if not currently working.
    async fn cancel_batch_if_working(&self, _id: &BatchId, _ts: i64) -> Result<bool, BridgeError> {
        Err(BridgeError::StoreFailure)
    }
    /// CAS `expect` -> `new`; false if the current status differs.
    async fn settle_batch_if_status(
        &self,
        _id: &BatchId,
        _expect: BatchStatus,
        _new: BatchStatus,
        _ts: i64,
    ) -> Result<bool, BridgeError> {
        Err(BridgeError::StoreFailure)
    }
    /// CAS `expect` -> `Failed`, recording the failure reason.
    async fn fail_batch_if_status(
        &self,
        _id: &BatchId,
        _expect: BatchStatus,
        _error: &str,
        _ts: i64,
    ) -> Result<bool, BridgeError> {
        Err(BridgeError::StoreFailure)
    }

    // ── Seq-bearing progress methods (Phase A: streaming reattach substrate) ──

    /// Record that a node has started executing. Returns the allocated seq.
    /// Re-starting the same node (resume re-emit) MUST NOT return an error —
    /// it upserts a fresh seq and ts, overwriting the previous start row.
    async fn record_node_started(
        &self,
        task: &TaskId,
        node: &NodeId,
        operation_id: &OperationId,
        ts: i64,
    ) -> Result<i64, BridgeError>;

    /// Persist a node output checkpoint WITH a monotonic seq. Returns the seq.
    /// Removes the node's start row (the node is no longer "in progress").
    ///
    /// WRITE-ONCE per `(task,node)`, like `put_node_checkpoint` (W3b semantics):
    /// a node finishes once; on resume, already-finished nodes are seeded and do
    /// NOT re-checkpoint, so the durable (SQLite) impl uses a plain INSERT.
    #[allow(clippy::too_many_arguments)]
    async fn put_node_checkpoint_sequenced(
        &self,
        task: &TaskId,
        node: &NodeId,
        operation_id: &OperationId,
        output: &str,
        ok: bool,
        ts: i64,
        usage: Option<&crate::orch::UsageSnapshot>,
    ) -> Result<i64, BridgeError>;

    /// Set the terminal status + result/error, recording a seq for the event.
    /// Returns the seq.
    async fn set_terminal_sequenced(
        &self,
        task: &TaskId,
        operation_id: &OperationId,
        status: TaskRecordStatus,
        result: Option<&str>,
        error: Option<&str>,
        ts: i64,
    ) -> Result<i64, BridgeError>;

    /// Persist a rich orchestration event with a monotonic seq.
    ///
    /// The default keeps custom test/wrapper stores source-compatible; durable
    /// stores that support the shared journal override it.
    async fn record_event_sequenced(
        &self,
        _task: &TaskId,
        _op: &OperationId,
        _ts: i64,
        _kind: crate::orch::OrchEventKind,
    ) -> Result<i64, BridgeError> {
        Err(BridgeError::StoreFailure)
    }

    async fn journal_from(
        &self,
        task: &TaskId,
        after_seq: i64,
    ) -> Result<Vec<crate::orch::OrchEvent>, BridgeError>;

    async fn journal_fold_inputs(&self, task: &TaskId) -> Result<JournalFoldInputs, BridgeError> {
        let snap = self.progress_snapshot(task).await?;
        let events = self.journal_from(task, -1).await?;
        Ok(JournalFoldInputs {
            complete_from_birth: false,
            scalars: JournalScalars {
                status: snap.status,
                result: snap.result,
                error: snap.error,
                terminal_seq: snap.terminal_seq,
                cut_seq: snap.cut_seq,
            },
            events,
        })
    }

    /// Reconstruct the current progress state for streaming reattach.
    /// `checkpoints` is ordered by seq (ascending).
    ///
    /// Under the single-writer-per-task model (one detached runner writes; the
    /// handler reads) this returns a consistent point-in-time view; `cut_seq` is
    /// guaranteed >= every included seq.
    async fn progress_snapshot(&self, task: &TaskId) -> Result<TaskProgressSnapshot, BridgeError>;
}

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// Snapshot of a task's current progress for streaming reattach.
/// `checkpoints` is ordered by seq (ascending); tuple = `(node, output, ok, seq)`.
/// `starts` holds in-progress nodes that have started but not yet finished.
/// `terminal_seq` is the seq of the terminal event, if any.
/// `cut_seq` is the highest seq allocated so far (equals `terminal_seq` when terminal).
#[derive(Clone, Debug)]
pub struct TaskProgressSnapshot {
    pub status: TaskRecordStatus,
    pub result: Option<String>,
    pub error: Option<String>,
    pub checkpoints: Vec<(NodeId, String, bool, i64)>,
    /// `(node, start_seq)` — the seq when the node's start was recorded; the
    /// timestamp is intentionally not exposed.
    pub starts: Vec<(NodeId, i64)>,
    pub terminal_seq: Option<i64>,
    pub cut_seq: i64,
}

#[derive(Clone, Debug)]
pub struct JournalScalars {
    pub status: TaskRecordStatus,
    pub result: Option<String>,
    pub error: Option<String>,
    pub terminal_seq: Option<i64>,
    pub cut_seq: i64,
}

#[derive(Clone, Debug)]
pub struct JournalFoldInputs {
    pub complete_from_birth: bool,
    pub scalars: JournalScalars,
    pub events: Vec<crate::orch::OrchEvent>,
}

pub fn fold_journal_to_snapshot(
    events: &[crate::orch::OrchEvent],
    scalars: &JournalScalars,
) -> Result<TaskProgressSnapshot, BridgeError> {
    let mut checkpoints = Vec::new();
    let mut starts: Vec<(NodeId, i64)> = Vec::new();

    for event in events {
        match &event.kind {
            crate::orch::OrchEventKind::NodeStarted { node } => {
                let node = NodeId::parse(node)?;
                starts.retain(|(started, _seq)| started != &node);
                starts.push((node, event.seq));
            }
            crate::orch::OrchEventKind::NodeFinished {
                node,
                ok,
                output,
                usage: _,
            } => {
                let node = NodeId::parse(node)?;
                starts.retain(|(started, _seq)| started != &node);
                checkpoints.push((node, output.clone(), *ok, event.seq));
            }
            crate::orch::OrchEventKind::Terminal { .. } => {
                starts.clear();
            }
            crate::orch::OrchEventKind::Progress { .. }
            | crate::orch::OrchEventKind::Usage { .. }
            | crate::orch::OrchEventKind::Plan { .. }
            | crate::orch::OrchEventKind::ToolCall { .. }
            | crate::orch::OrchEventKind::ToolCallUpdate { .. } => {}
        }
    }

    // `cut_seq` is trusted verbatim from the tasks-row scalar (= `last_event_seq`), NOT clamped to
    // the max event seq. This relies on the load-bearing dual-store invariant: every sequenced
    // writer bumps `last_event_seq` THEN inserts its journal row at `seq == last_event_seq` in the
    // SAME transaction (SQLite) / under the SAME guard (Memory), so `cut_seq >= max journal seq`
    // always holds. A future writer that journals WITHOUT bumping `last_event_seq` would break this.
    Ok(TaskProgressSnapshot {
        status: scalars.status,
        result: scalars.result.clone(),
        error: scalars.error.clone(),
        checkpoints,
        starts,
        terminal_seq: scalars.terminal_seq,
        cut_seq: scalars.cut_seq,
    })
}

/// `(output, ok, ts, seq, usage)` stored per `(task_id, node_id)` checkpoint key.
type CheckpointValue = (String, bool, i64, i64, Option<crate::orch::UsageSnapshot>);

/// In-memory `TaskStore` (the default when no DB path is configured). Production
/// use, not just a test fake — lives in `bridge-core` so `bridge-a2a-inbound`
/// can default to it WITHOUT depending on `bridge-store`.
pub struct MemoryTaskStore {
    now_ms: PersistenceClock,
    /// Coarse guard for Memory fold-input consistency across the split maps.
    /// Writers that touch both `inner` and `turn_log` must hold this before either map.
    journal_fold_guard: Mutex<()>,
    inner: Mutex<HashMap<String, TaskRecord>>,
    batches: Mutex<HashMap<BatchId, BatchRecord>>,
    /// Tasks created under S6 code have complete journal coverage from birth.
    birth: Mutex<HashSet<String>>,
    /// Key: (task_id, node_id) → (output, ok, ts, seq, usage)
    checkpoints: Mutex<HashMap<(String, String), CheckpointValue>>,
    /// Per-task monotonic seq counter. Key: task_id.
    seq_counters: Mutex<HashMap<String, i64>>,
    /// Per-task terminal seq. Key: task_id.
    terminal_seqs: Mutex<HashMap<String, i64>>,
    /// In-progress node starts. Key: (task_id, node_id) → (seq, ts).
    starts: Mutex<HashMap<(String, String), (i64, i64)>>,
    turn_log: Mutex<HashMap<String, TurnLogRow>>,
    /// Per-task durable orchestration journal rows. Key: task_id.
    journals: Mutex<HashMap<String, Vec<(i64, crate::orch::OrchEvent)>>>,
}

impl Default for MemoryTaskStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryTaskStore {
    pub fn new() -> Self {
        Self::with_clock(Arc::new(system_wall_now_ms))
    }

    pub fn with_clock(now_ms: PersistenceClock) -> Self {
        Self {
            now_ms,
            journal_fold_guard: Mutex::new(()),
            inner: Mutex::new(HashMap::new()),
            batches: Mutex::new(HashMap::new()),
            birth: Mutex::new(HashSet::new()),
            checkpoints: Mutex::new(HashMap::new()),
            seq_counters: Mutex::new(HashMap::new()),
            terminal_seqs: Mutex::new(HashMap::new()),
            starts: Mutex::new(HashMap::new()),
            turn_log: Mutex::new(HashMap::new()),
            journals: Mutex::new(HashMap::new()),
        }
    }

    fn retention_now_ms(&self) -> i64 {
        durable_retention_ms((self.now_ms)())
    }

    fn bump_last_artifact(row: &mut TaskRecord, artifact_ms: i64) {
        if row
            .last_artifact_ms
            .map(|existing| existing < artifact_ms)
            .unwrap_or(true)
        {
            row.last_artifact_ms = Some(artifact_ms);
        }
    }

    /// Allocate and return the next seq for the given task.
    /// Must be called with NO other locks held (takes seq_counters lock internally).
    fn next_seq(&self, task_id: &str) -> i64 {
        let mut g = self.seq_counters.lock().unwrap();
        let seq = g.entry(task_id.to_string()).or_insert(0);
        *seq += 1;
        *seq
    }
}

#[async_trait::async_trait]
impl TaskStore for MemoryTaskStore {
    async fn create(&self, rec: &TaskRecord) -> Result<(), BridgeError> {
        let _guard = self.journal_fold_guard.lock().unwrap();
        let mut g = self.inner.lock().unwrap();
        if g.contains_key(rec.id.as_str()) {
            return Err(BridgeError::StoreFailure);
        }
        g.insert(rec.id.as_str().to_string(), rec.clone());
        self.birth
            .lock()
            .unwrap()
            .insert(rec.id.as_str().to_string());
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
        let _guard = self.journal_fold_guard.lock().unwrap();
        let mut g = self.inner.lock().unwrap();
        let row = g.get_mut(id.as_str()).ok_or(BridgeError::StoreFailure)?;
        row.status = status;
        row.result = result.map(|s| s.to_string());
        row.error = error.map(|s| s.to_string());
        row.updated_ms = durable_retention_ms(updated_ms);
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
        let _guard = self.journal_fold_guard.lock().unwrap();
        let mut g = self.inner.lock().unwrap();
        let mut n = 0;
        for row in g.values_mut() {
            if row.status == TaskRecordStatus::Working {
                row.status = TaskRecordStatus::Interrupted;
                row.error = Some("interrupted (serve restarted)".into());
                row.updated_ms = durable_retention_ms(updated_ms);
                n += 1;
            }
        }
        Ok(n)
    }
    async fn cancel_if_working(&self, id: &TaskId, updated_ms: i64) -> Result<bool, BridgeError> {
        let _guard = self.journal_fold_guard.lock().unwrap();
        let mut g = self.inner.lock().unwrap();
        match g.get_mut(id.as_str()) {
            Some(row) if row.status == TaskRecordStatus::Working => {
                row.status = TaskRecordStatus::Canceled;
                row.updated_ms = durable_retention_ms(updated_ms);
                Ok(true)
            }
            _ => Ok(false), // missing or already terminal — do not clobber
        }
    }
    async fn put_node_checkpoint(
        &self,
        task: &TaskId,
        node: &NodeId,
        output: &str,
        ok: bool,
        ts: i64,
    ) -> Result<(), BridgeError> {
        let _guard = self.journal_fold_guard.lock().unwrap();
        let key = (task.as_str().to_string(), node.as_str().to_string());
        let mut g = self.checkpoints.lock().unwrap();
        if g.contains_key(&key) {
            return Err(BridgeError::StoreFailure);
        }
        let artifact_ms = self.retention_now_ms();
        {
            let mut inner = self.inner.lock().unwrap();
            let row = inner
                .get_mut(task.as_str())
                .ok_or(BridgeError::StoreFailure)?;
            Self::bump_last_artifact(row, artifact_ms);
        }
        g.insert(key, (output.to_string(), ok, ts, 0, None));
        Ok(())
    }
    async fn node_checkpoints(
        &self,
        task: &TaskId,
    ) -> Result<Vec<(NodeId, String, bool, Option<crate::orch::UsageSnapshot>)>, BridgeError> {
        let g = self.checkpoints.lock().unwrap();
        let mut out = Vec::new();
        for ((tid, nid), (output, ok, _ts, _seq, usage)) in g.iter() {
            if tid == task.as_str() {
                let node = NodeId::parse(nid).map_err(|_| BridgeError::StoreFailure)?;
                out.push((node, output.clone(), *ok, usage.clone()));
            }
        }
        Ok(out)
    }
    async fn claim_resume_attempt(
        &self,
        task: &TaskId,
        cap: u32,
        now_ms: i64,
    ) -> Result<ResumeClaim, BridgeError> {
        let mut g = self.inner.lock().unwrap();
        let row = g.get_mut(task.as_str()).ok_or(BridgeError::StoreFailure)?;
        if row.resume_attempts >= cap {
            return Ok(ResumeClaim::Exhausted);
        }
        row.resume_attempts += 1;
        row.updated_ms = durable_retention_ms(now_ms); // last_resume_ms is folded into updated_ms for the in-memory store; the SQLite store has the dedicated column.
        Ok(ResumeClaim::Resumable {
            attempt: row.resume_attempts,
        })
    }
    async fn working_tasks(&self) -> Result<Vec<TaskRecord>, BridgeError> {
        let g = self.inner.lock().unwrap();
        Ok(g.values()
            .filter(|r| r.status == TaskRecordStatus::Working)
            .cloned()
            .collect())
    }

    async fn upsert_turn_finished(&self, row: &TurnLogFinished) -> Result<(), BridgeError> {
        let _guard = self.journal_fold_guard.lock().unwrap();
        let artifact_ms = self.retention_now_ms();
        if let Some(task) = row.ctx.task_id.as_ref() {
            let mut inner = self.inner.lock().unwrap();
            if let Some(task_row) = inner.get_mut(task.as_str()) {
                Self::bump_last_artifact(task_row, artifact_ms);
            }
        }
        let mut g = self.turn_log.lock().unwrap();
        let entry = g
            .entry(row.ctx.turn_id.as_str().to_string())
            .or_insert_with(|| TurnLogRow {
                turn_id: row.ctx.turn_id.clone(),
                session_id: row.ctx.session_id.clone(),
                task_id: row.ctx.task_id.clone(),
                workflow: row.ctx.workflow.clone(),
                node: row.ctx.node.clone(),
                attempt: row.ctx.attempt,
                agent: row.ctx.agent.clone(),
                model: row.ctx.model.clone(),
                effort: row.ctx.effort.clone(),
                mode: row.ctx.mode.clone(),
                prompt_id: row.ctx.prompt_id.clone(),
                started_ms: None,
                completed_ms: None,
                latency_ms: None,
                ttft_ms: None,
                outcome: None,
                failure_class: None,
                input_tokens: None,
                output_tokens: None,
                thought_tokens: None,
                cached_read_tokens: None,
                cached_write_tokens: None,
                cost_amount: None,
                cost_currency: None,
                traceparent: row.ctx.traceparent.clone(),
                usage_finalized_ms: None,
                usage_finalization_kind: "pending".to_string(),
            });
        entry.session_id = row.ctx.session_id.clone();
        entry.task_id = row.ctx.task_id.clone();
        entry.workflow = row.ctx.workflow.clone();
        entry.node = row.ctx.node.clone();
        entry.attempt = row.ctx.attempt;
        entry.agent = row.ctx.agent.clone();
        entry.model = row.ctx.model.clone();
        entry.effort = row.ctx.effort.clone();
        entry.mode = row.ctx.mode.clone();
        entry.prompt_id = row.ctx.prompt_id.clone();
        entry.traceparent = row.ctx.traceparent.clone();
        entry.started_ms = Some(row.started_ms);
        entry.completed_ms = Some(row.completed_ms);
        entry.latency_ms = Some(row.latency.as_millis() as u64);
        entry.ttft_ms = row.ttft.map(|d| d.as_millis() as u64);
        let (outcome, failure_class) = turn_log_outcome_strings(&row.outcome);
        entry.outcome = Some(outcome.to_string());
        entry.failure_class = failure_class.map(str::to_string);
        Ok(())
    }

    async fn finalize_turn_usage(&self, row: &TurnLogFinalized) -> Result<(), BridgeError> {
        let _guard = self.journal_fold_guard.lock().unwrap();
        let persistence_ms = self.retention_now_ms();
        let mut g = self.turn_log.lock().unwrap();
        let entry = g
            .get_mut(row.ctx.turn_id.as_str())
            .ok_or(BridgeError::StoreFailure)?;
        if entry.completed_ms.is_none() {
            return Err(BridgeError::StoreFailure);
        }
        if entry.usage_finalized_ms.is_some() {
            let expected = match &row.finalization {
                TurnUsageFinalization::Usage(_) => "usage",
                TurnUsageFinalization::NoUsage => "no_usage",
            };
            return if entry.usage_finalization_kind == expected {
                Ok(())
            } else {
                Err(BridgeError::StoreFailure)
            };
        }
        match &row.finalization {
            TurnUsageFinalization::Usage(usage) => {
                if let Some(term) = usage.terminal.as_ref() {
                    entry.input_tokens = Some(term.input_tokens);
                    entry.output_tokens = Some(term.output_tokens);
                    entry.thought_tokens = term.thought_tokens;
                    entry.cached_read_tokens = term.cached_read_tokens;
                    entry.cached_write_tokens = term.cached_write_tokens;
                }
                if let Some(cost) = usage.cost.as_ref() {
                    entry.cost_amount = Some(cost.amount);
                    entry.cost_currency = Some(cost.currency.clone());
                }
                entry.usage_finalization_kind = "usage".to_string();
            }
            TurnUsageFinalization::NoUsage => {
                let has_usage = entry.input_tokens.is_some()
                    || entry.output_tokens.is_some()
                    || entry.thought_tokens.is_some()
                    || entry.cached_read_tokens.is_some()
                    || entry.cached_write_tokens.is_some()
                    || entry.cost_amount.is_some()
                    || entry.cost_currency.is_some();
                if has_usage {
                    return Err(BridgeError::StoreFailure);
                }
                entry.usage_finalization_kind = "no_usage".to_string();
            }
        }
        entry.usage_finalized_ms = Some(persistence_ms);
        if let Some(task) = entry.task_id.as_ref() {
            let mut inner = self.inner.lock().unwrap();
            if let Some(task_row) = inner.get_mut(task.as_str()) {
                Self::bump_last_artifact(task_row, persistence_ms);
            }
        }
        Ok(())
    }

    async fn turn_log_rows(&self) -> Result<Vec<TurnLogRow>, BridgeError> {
        let mut rows: Vec<_> = self.turn_log.lock().unwrap().values().cloned().collect();
        rows.sort_by(|a, b| a.turn_id.as_str().cmp(b.turn_id.as_str()));
        Ok(rows)
    }

    async fn turn_log_row(&self, turn_id: &TurnId) -> Result<Option<TurnLogRow>, BridgeError> {
        Ok(self.turn_log.lock().unwrap().get(turn_id.as_str()).cloned())
    }

    async fn turn_log_rows_for_task(
        &self,
        task: &TaskId,
        limit: usize,
    ) -> Result<Vec<TurnLogRow>, BridgeError> {
        let mut rows: Vec<_> = self
            .turn_log
            .lock()
            .unwrap()
            .values()
            .filter(|row| row.task_id.as_ref().map(|t| t.as_str()) == Some(task.as_str()))
            .cloned()
            .collect();
        rows.sort_by(|a, b| {
            a.completed_ms
                .unwrap_or(i64::MAX)
                .cmp(&b.completed_ms.unwrap_or(i64::MAX))
                .then_with(|| a.turn_id.as_str().cmp(b.turn_id.as_str()))
        });
        rows.truncate(limit);
        Ok(rows)
    }

    async fn turn_log_usage_for_task(
        &self,
        task: &TaskId,
    ) -> Result<Option<TaskUsageAgg>, BridgeError> {
        let rows: Vec<_> = self
            .turn_log
            .lock()
            .unwrap()
            .values()
            .filter(|row| row.task_id.as_ref().map(|t| t.as_str()) == Some(task.as_str()))
            .cloned()
            .collect();

        if rows.is_empty() {
            return Ok(None);
        }

        let mut input_tokens = 0_u64;
        let mut output_tokens = 0_u64;
        let mut thought_tokens = None::<u64>;
        let mut cached_read_tokens = None::<u64>;
        let mut cached_write_tokens = None::<u64>;
        let mut cost_amount = None::<f64>;
        let mut currencies = std::collections::HashSet::new();
        let mut at_ms = 0_i64;

        for row in &rows {
            input_tokens += row.input_tokens.unwrap_or(0);
            output_tokens += row.output_tokens.unwrap_or(0);
            if let Some(v) = row.thought_tokens {
                thought_tokens = Some(thought_tokens.unwrap_or(0) + v);
            }
            if let Some(v) = row.cached_read_tokens {
                cached_read_tokens = Some(cached_read_tokens.unwrap_or(0) + v);
            }
            if let Some(v) = row.cached_write_tokens {
                cached_write_tokens = Some(cached_write_tokens.unwrap_or(0) + v);
            }
            if let (Some(amount), Some(currency)) = (row.cost_amount, row.cost_currency.as_ref()) {
                cost_amount = Some(cost_amount.unwrap_or(0.0) + amount);
                currencies.insert(currency.clone());
            }
            if let Some(ms) = row.completed_ms {
                at_ms = at_ms.max(ms);
            }
        }

        let cost = if currencies.len() == 1 {
            cost_amount.map(|amount| crate::orch::UsageCost {
                amount,
                currency: currencies.into_iter().next().unwrap(),
            })
        } else {
            None
        };

        Ok(Some(TaskUsageAgg {
            rows: rows.len() as u64,
            input_tokens,
            output_tokens,
            thought_tokens,
            cached_read_tokens,
            cached_write_tokens,
            cost,
            at_ms,
        }))
    }

    async fn latest_turn_log_row_for_session(
        &self,
        session: &ContextId,
    ) -> Result<Option<TurnLogRow>, BridgeError> {
        let rows = self.turn_log.lock().unwrap();
        Ok(rows
            .values()
            .filter(|row| row.session_id.as_str() == session.as_str())
            .max_by(|a, b| {
                a.completed_ms
                    .unwrap_or(i64::MIN)
                    .cmp(&b.completed_ms.unwrap_or(i64::MIN))
                    .then_with(|| a.turn_id.as_str().cmp(b.turn_id.as_str()))
            })
            .cloned())
    }

    async fn journal_jsonl_bounded(
        &self,
        task: &TaskId,
        max_events: usize,
        max_bytes: usize,
    ) -> Result<JournalRead, BridgeError> {
        let events = self
            .journals
            .lock()
            .unwrap()
            .get(task.as_str())
            .cloned()
            .unwrap_or_default();

        // Preflight (mirrors the SQLite COUNT/SUM): compute the event count and total
        // JSONL byte size WITHOUT retaining the assembled body, so an over-limit journal
        // is rejected as `TooLarge` before we ever build the response string.
        let events_count = events.len() as u64;
        let mut bytes = 0_u64;
        for (_seq, event) in &events {
            let line = serde_json::to_string(event).map_err(|_| BridgeError::StoreFailure)?;
            bytes += line.len() as u64 + 1;
        }

        if events.len() > max_events || bytes as usize > max_bytes {
            return Ok(JournalRead::TooLarge {
                events: events_count,
                bytes,
            });
        }

        // Under both caps: assemble the bounded body (≤ max_bytes).
        let mut jsonl = String::with_capacity(bytes as usize);
        for (_seq, event) in &events {
            let line = serde_json::to_string(event).map_err(|_| BridgeError::StoreFailure)?;
            jsonl.push_str(&line);
            jsonl.push('\n');
        }
        Ok(JournalRead::Body {
            jsonl,
            events: events_count,
            bytes,
        })
    }

    async fn node_checkpoint_nodes(&self, task: &TaskId) -> Result<Vec<NodeId>, BridgeError> {
        let g = self.checkpoints.lock().unwrap();
        let mut rows = Vec::new();
        for ((tid, nid), (_output, _ok, ts, seq, _usage)) in g.iter() {
            if tid == task.as_str() {
                rows.push((
                    *seq,
                    *ts,
                    NodeId::parse(nid).map_err(|_| BridgeError::StoreFailure)?,
                ));
            }
        }
        rows.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.2.as_str().cmp(b.2.as_str()))
        });
        Ok(rows.into_iter().map(|(_seq, _ts, node)| node).collect())
    }

    async fn node_checkpoint_output(
        &self,
        task: &TaskId,
        node: &NodeId,
        max_bytes: usize,
    ) -> Result<Option<NodeCheckpointOutput>, BridgeError> {
        let g = self.checkpoints.lock().unwrap();
        let Some((output, ok, _ts, _seq, usage)) =
            g.get(&(task.as_str().to_string(), node.as_str().to_string()))
        else {
            return Ok(None);
        };
        let bytes = output.len() as u64;
        if bytes as usize > max_bytes {
            return Ok(Some(NodeCheckpointOutput::TooLarge { bytes }));
        }
        Ok(Some(NodeCheckpointOutput::Found {
            output: output.clone(),
            ok: *ok,
            usage: usage.clone(),
            bytes,
        }))
    }

    async fn create_batch(&self, rec: &BatchRecord) -> Result<(), BridgeError> {
        let mut g = self.batches.lock().unwrap();
        if g.contains_key(&rec.id) {
            return Err(BridgeError::StoreFailure);
        }
        g.insert(rec.id.clone(), rec.clone());
        Ok(())
    }

    async fn get_batch(&self, id: &BatchId) -> Result<Option<BatchRecord>, BridgeError> {
        Ok(self.batches.lock().unwrap().get(id).cloned())
    }

    async fn list_batches(&self, limit: usize) -> Result<Vec<BatchRecord>, BridgeError> {
        let g = self.batches.lock().unwrap();
        let mut v: Vec<BatchRecord> = g.values().cloned().collect();
        v.sort_by(|a, b| b.updated_ms.cmp(&a.updated_ms));
        v.truncate(limit);
        Ok(v)
    }

    async fn active_batches(&self) -> Result<Vec<BatchRecord>, BridgeError> {
        let g = self.batches.lock().unwrap();
        Ok(g.values()
            .filter(|r| matches!(r.status, BatchStatus::Working | BatchStatus::Canceling))
            .cloned()
            .collect())
    }

    async fn batch_children(&self, id: &BatchId) -> Result<Vec<TaskRecord>, BridgeError> {
        let g = self.inner.lock().unwrap();
        Ok(g.values()
            .filter(|r| r.batch_id.as_ref() == Some(id))
            .cloned()
            .collect())
    }

    async fn claim_batch_child(
        &self,
        batch: &BatchId,
        item: &str,
        rec: &TaskRecord,
    ) -> Result<ChildClaim, BridgeError> {
        let _guard = self.journal_fold_guard.lock().unwrap();
        let mut g = self.inner.lock().unwrap();
        if let Some(existing) = g
            .values()
            .find(|r| r.batch_id.as_ref() == Some(batch) && r.item_id.as_deref() == Some(item))
        {
            return Ok(if existing.status == TaskRecordStatus::Working {
                ChildClaim::ExistingWorking
            } else {
                ChildClaim::ExistingTerminal
            });
        }
        if g.contains_key(rec.id.as_str()) {
            return Err(BridgeError::StoreFailure);
        }
        let mut rec = rec.clone();
        rec.batch_id = Some(batch.clone());
        rec.item_id = Some(item.to_string());
        let task_id = rec.id.as_str().to_string();
        g.insert(task_id.clone(), rec);
        self.birth.lock().unwrap().insert(task_id);
        Ok(ChildClaim::Created)
    }

    async fn cancel_batch_if_working(&self, id: &BatchId, ts: i64) -> Result<bool, BridgeError> {
        let mut g = self.batches.lock().unwrap();
        match g.get_mut(id) {
            Some(row) if row.status == BatchStatus::Working => {
                row.status = BatchStatus::Canceling;
                row.updated_ms = durable_retention_ms(ts);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn settle_batch_if_status(
        &self,
        id: &BatchId,
        expect: BatchStatus,
        new: BatchStatus,
        ts: i64,
    ) -> Result<bool, BridgeError> {
        let mut g = self.batches.lock().unwrap();
        match g.get_mut(id) {
            Some(row) if row.status == expect => {
                row.status = new;
                row.updated_ms = durable_retention_ms(ts);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn fail_batch_if_status(
        &self,
        id: &BatchId,
        expect: BatchStatus,
        error: &str,
        ts: i64,
    ) -> Result<bool, BridgeError> {
        let mut g = self.batches.lock().unwrap();
        match g.get_mut(id) {
            Some(row) if row.status == expect => {
                row.status = BatchStatus::Failed;
                row.error = Some(error.to_string());
                row.updated_ms = durable_retention_ms(ts);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn record_node_started(
        &self,
        task: &TaskId,
        node: &NodeId,
        operation_id: &OperationId,
        ts: i64,
    ) -> Result<i64, BridgeError> {
        let _guard = self.journal_fold_guard.lock().unwrap();
        let artifact_ms = self.retention_now_ms();
        {
            let mut inner = self.inner.lock().unwrap();
            let row = inner
                .get_mut(task.as_str())
                .ok_or(BridgeError::StoreFailure)?;
            Self::bump_last_artifact(row, artifact_ms);
        }
        let seq = self.next_seq(task.as_str());
        let mut g = self.starts.lock().unwrap();
        let key = (task.as_str().to_string(), node.as_str().to_string());
        // Upsert: re-starting the same node is allowed (resume re-emit).
        g.insert(key, (seq, ts));
        let event = crate::orch::OrchEvent {
            v: crate::orch::ORCH_V,
            seq,
            ts_ms: ts,
            operation_id: operation_id.clone(),
            session: None,
            source: None,
            kind: crate::orch::OrchEventKind::NodeStarted {
                node: node.as_str().to_string(),
            },
        };
        self.journals
            .lock()
            .unwrap()
            .entry(task.as_str().to_string())
            .or_default()
            .push((seq, event));
        Ok(seq)
    }

    #[allow(clippy::too_many_arguments)]
    async fn put_node_checkpoint_sequenced(
        &self,
        task: &TaskId,
        node: &NodeId,
        operation_id: &OperationId,
        output: &str,
        ok: bool,
        ts: i64,
        usage: Option<&crate::orch::UsageSnapshot>,
    ) -> Result<i64, BridgeError> {
        let _guard = self.journal_fold_guard.lock().unwrap();
        let key = (task.as_str().to_string(), node.as_str().to_string());
        {
            let g = self.checkpoints.lock().unwrap();
            if g.contains_key(&key) {
                return Err(BridgeError::StoreFailure);
            }
        }
        let artifact_ms = self.retention_now_ms();
        {
            let mut inner = self.inner.lock().unwrap();
            let row = inner
                .get_mut(task.as_str())
                .ok_or(BridgeError::StoreFailure)?;
            Self::bump_last_artifact(row, artifact_ms);
        }
        let seq = self.next_seq(task.as_str());
        // Remove start row for this node (it is no longer in progress).
        {
            let mut sg = self.starts.lock().unwrap();
            sg.remove(&key);
        }
        let mut g = self.checkpoints.lock().unwrap();
        g.insert(key, (output.to_string(), ok, ts, seq, usage.cloned()));
        let event = crate::orch::OrchEvent {
            v: crate::orch::ORCH_V,
            seq,
            ts_ms: ts,
            operation_id: operation_id.clone(),
            session: None,
            source: None,
            kind: crate::orch::OrchEventKind::NodeFinished {
                node: node.as_str().to_string(),
                ok,
                output: output.to_string(),
                usage: usage.cloned(),
            },
        };
        self.journals
            .lock()
            .unwrap()
            .entry(task.as_str().to_string())
            .or_default()
            .push((seq, event));
        Ok(seq)
    }

    async fn set_terminal_sequenced(
        &self,
        task: &TaskId,
        operation_id: &OperationId,
        status: TaskRecordStatus,
        result: Option<&str>,
        error: Option<&str>,
        ts: i64,
    ) -> Result<i64, BridgeError> {
        let _guard = self.journal_fold_guard.lock().unwrap();
        let artifact_ms = self.retention_now_ms();
        {
            let mut inner = self.inner.lock().unwrap();
            let row = inner
                .get_mut(task.as_str())
                .ok_or(BridgeError::StoreFailure)?;
            Self::bump_last_artifact(row, artifact_ms);
        }
        let seq = self.next_seq(task.as_str());
        {
            let mut g = self.inner.lock().unwrap();
            let row = g.get_mut(task.as_str()).ok_or(BridgeError::StoreFailure)?;
            row.status = status;
            row.result = result.map(|s| s.to_string());
            row.error = error.map(|s| s.to_string());
            row.updated_ms = durable_retention_ms(ts);
        }
        // Record the terminal seq.
        {
            let mut tg = self.terminal_seqs.lock().unwrap();
            tg.insert(task.as_str().to_string(), seq);
        }
        // Clear all start rows for this task.
        {
            let mut sg = self.starts.lock().unwrap();
            sg.retain(|(tid, _nid), _| tid != task.as_str());
        }
        let event = crate::orch::OrchEvent {
            v: crate::orch::ORCH_V,
            seq,
            ts_ms: ts,
            operation_id: operation_id.clone(),
            session: None,
            source: None,
            kind: crate::orch::OrchEventKind::Terminal {
                status: terminal_status_from_record(&status),
                output: result.or(error).unwrap_or("").to_string(),
            },
        };
        self.journals
            .lock()
            .unwrap()
            .entry(task.as_str().to_string())
            .or_default()
            .push((seq, event));
        Ok(seq)
    }

    async fn record_event_sequenced(
        &self,
        task: &TaskId,
        op: &OperationId,
        ts: i64,
        kind: crate::orch::OrchEventKind,
    ) -> Result<i64, BridgeError> {
        let _guard = self.journal_fold_guard.lock().unwrap();
        let artifact_ms = self.retention_now_ms();
        {
            let mut inner = self.inner.lock().unwrap();
            let row = inner
                .get_mut(task.as_str())
                .ok_or(BridgeError::StoreFailure)?;
            Self::bump_last_artifact(row, artifact_ms);
        }
        let seq = self.next_seq(task.as_str());
        let event = crate::orch::OrchEvent {
            v: crate::orch::ORCH_V,
            seq,
            ts_ms: ts,
            operation_id: op.clone(),
            session: None,
            source: None,
            kind,
        };
        self.journals
            .lock()
            .unwrap()
            .entry(task.as_str().to_string())
            .or_default()
            .push((seq, event));
        Ok(seq)
    }

    async fn journal_from(
        &self,
        task: &TaskId,
        after_seq: i64,
    ) -> Result<Vec<crate::orch::OrchEvent>, BridgeError> {
        let g = self.journals.lock().unwrap();
        let mut out: Vec<crate::orch::OrchEvent> = g
            .get(task.as_str())
            .into_iter()
            .flat_map(|rows| rows.iter())
            .filter(|(seq, _event)| *seq > after_seq)
            .map(|(seq, event)| {
                let mut event = event.clone();
                event.seq = *seq;
                event
            })
            .collect();
        out.sort_by_key(|event| event.seq);
        Ok(out)
    }

    async fn journal_fold_inputs(&self, task: &TaskId) -> Result<JournalFoldInputs, BridgeError> {
        let _guard = self.journal_fold_guard.lock().unwrap();
        let row = {
            let g = self.inner.lock().unwrap();
            g.get(task.as_str())
                .cloned()
                .ok_or(BridgeError::StoreFailure)?
        };
        let cut_seq = {
            let g = self.seq_counters.lock().unwrap();
            *g.get(task.as_str()).unwrap_or(&0)
        };
        let terminal_seq = {
            let g = self.terminal_seqs.lock().unwrap();
            g.get(task.as_str()).copied()
        };
        let complete_from_birth = self.birth.lock().unwrap().contains(task.as_str());
        let mut events: Vec<crate::orch::OrchEvent> = {
            let g = self.journals.lock().unwrap();
            g.get(task.as_str())
                .into_iter()
                .flat_map(|rows| rows.iter())
                .map(|(seq, event)| {
                    let mut event = event.clone();
                    event.seq = *seq;
                    event
                })
                .collect()
        };
        events.sort_by_key(|event| event.seq);
        Ok(JournalFoldInputs {
            complete_from_birth,
            scalars: JournalScalars {
                status: row.status,
                result: row.result,
                error: row.error,
                terminal_seq,
                cut_seq,
            },
            events,
        })
    }

    async fn progress_snapshot(&self, task: &TaskId) -> Result<TaskProgressSnapshot, BridgeError> {
        let row = {
            let g = self.inner.lock().unwrap();
            g.get(task.as_str())
                .cloned()
                .ok_or(BridgeError::StoreFailure)?
        };
        let cut_seq = {
            let g = self.seq_counters.lock().unwrap();
            *g.get(task.as_str()).unwrap_or(&0)
        };
        let terminal_seq = {
            let g = self.terminal_seqs.lock().unwrap();
            g.get(task.as_str()).copied()
        };
        let mut checkpoints: Vec<(NodeId, String, bool, i64)> = {
            let g = self.checkpoints.lock().unwrap();
            let mut out = Vec::new();
            for ((tid, nid), (output, ok, _ts, seq, _usage)) in g.iter() {
                if tid == task.as_str() {
                    let node = NodeId::parse(nid).map_err(|_| BridgeError::StoreFailure)?;
                    out.push((node, output.clone(), *ok, *seq));
                }
            }
            out
        };
        checkpoints.sort_by_key(|(_n, _o, _ok, seq)| *seq);
        let starts: Vec<(NodeId, i64)> = {
            let g = self.starts.lock().unwrap();
            let mut out = Vec::new();
            for ((tid, nid), (seq, _ts)) in g.iter() {
                if tid == task.as_str() {
                    let node = NodeId::parse(nid).map_err(|_| BridgeError::StoreFailure)?;
                    out.push((node, *seq));
                }
            }
            out
        };
        // Ensure cut_seq >= every seq actually included in the snapshot.  Under the
        // single-writer-per-task model this is almost always a no-op, but a concurrent
        // write between the counter read and the data reads could otherwise produce a
        // snapshot where a checkpoint or start seq exceeds the recorded cut_seq.
        let max_included = checkpoints
            .iter()
            .map(|(_, _, _, s)| *s)
            .chain(starts.iter().map(|(_, s)| *s))
            .chain(terminal_seq)
            .max()
            .unwrap_or(0);
        let cut_seq = cut_seq.max(max_included);
        Ok(TaskProgressSnapshot {
            status: row.status,
            result: row.result,
            error: row.error,
            checkpoints,
            starts,
            terminal_seq,
            cut_seq,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{BatchId, ContextId, NodeId, OperationId, TurnId};
    use crate::orch::{OrchEvent, OrchEventKind, TerminalUsage, UsageCost, UsageSnapshot, ORCH_V};
    use crate::ports::{TraceParent, TurnContext, TurnOutcome};
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    fn turn_ctx(turn: &str, session: &str, task: Option<&str>, attempt: u32) -> TurnContext {
        TurnContext {
            turn_id: TurnId::parse(turn).unwrap(),
            session_id: ContextId::parse(session).unwrap(),
            task_id: task.map(|t| TaskId::parse(t).unwrap()),
            workflow: Some("code-review".to_string()),
            node: Some("reviewer".to_string()),
            attempt,
            agent: "codex".to_string(),
            model: Some("gpt-5.5".to_string()),
            effort: Some("high".to_string()),
            mode: Some("default".to_string()),
            prompt_id: Some("prompt/eval".to_string()),
            traceparent: TraceParent::parse_header_value(
                "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            ),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn write_finished_turn(
        store: &MemoryTaskStore,
        turn: &str,
        session: &str,
        task: Option<&str>,
        completed_ms: i64,
        input: u64,
        output: u64,
        cost: Option<(&str, f64)>,
    ) {
        let ctx = turn_ctx(turn, session, task, 0);
        store
            .upsert_turn_finished(&TurnLogFinished {
                ctx: ctx.clone(),
                started_ms: completed_ms - 10,
                completed_ms,
                latency: Duration::from_millis(10),
                ttft: None,
                outcome: TurnOutcome::Success,
            })
            .await
            .unwrap();
        store
            .update_turn_usage(&TurnLogUsage {
                ctx,
                usage: UsageSnapshot {
                    used: None,
                    size: None,
                    cost: cost.map(|(currency, amount)| UsageCost {
                        amount,
                        currency: currency.to_string(),
                    }),
                    terminal: Some(TerminalUsage {
                        total_tokens: input + output + 999,
                        input_tokens: input,
                        output_tokens: output,
                        thought_tokens: Some(3),
                        cached_read_tokens: None,
                        cached_write_tokens: Some(5),
                    }),
                    at_ms: completed_ms,
                },
            })
            .await
            .unwrap();
    }

    fn usage_snapshot(input: u64, output: u64, at_ms: i64) -> UsageSnapshot {
        UsageSnapshot {
            used: None,
            size: None,
            cost: Some(UsageCost {
                amount: 0.42,
                currency: "USD".to_string(),
            }),
            terminal: Some(TerminalUsage {
                total_tokens: input + output,
                input_tokens: input,
                output_tokens: output,
                thought_tokens: Some(1),
                cached_read_tokens: Some(2),
                cached_write_tokens: Some(3),
            }),
            at_ms,
        }
    }

    fn finished_row(ctx: TurnContext, completed_ms: i64) -> TurnLogFinished {
        TurnLogFinished {
            ctx,
            started_ms: completed_ms - 10,
            completed_ms,
            latency: Duration::from_millis(10),
            ttft: None,
            outcome: TurnOutcome::Success,
        }
    }

    fn rec(id: &str, ms: i64) -> TaskRecord {
        TaskRecord {
            id: TaskId::parse(id).unwrap(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: ms,
            updated_ms: ms,
            last_artifact_ms: None,
            input: String::new(),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: None,
            batch_id: None,
            item_id: None,
            artifacts_purged_at: None,
        }
    }

    fn sample_batch(bid: &BatchId, status: BatchStatus, total: u32, ms: i64) -> BatchRecord {
        BatchRecord {
            id: bid.clone(),
            workflow: "code-review".into(),
            concurrency: 2,
            total,
            status,
            items_json: r#"{"v":1,"items":[]}"#.into(),
            error: None,
            created_ms: ms,
            updated_ms: ms,
        }
    }

    fn batch_child_record(tid: &TaskId, bid: &BatchId, item: &str) -> TaskRecord {
        TaskRecord {
            id: tid.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 0,
            updated_ms: 0,
            last_artifact_ms: None,
            input: "DIFF".into(),
            workflow_spec_json: Some(r#"{"v":1,"nodes":[]}"#.into()),
            resume_attempts: 0,
            session_cwd: None,
            batch_id: Some(bid.clone()),
            item_id: Some(item.to_string()),
            artifacts_purged_at: None,
        }
    }

    #[tokio::test]
    async fn usage_finalized_some_updates_usage_and_barrier_atomically() {
        let store = MemoryTaskStore::with_clock(Arc::new(|| 12_345));
        let ctx = turn_ctx("turn-final-usage", "ctx-final-usage", None, 0);
        store.upsert_turn_finished(&finished_row(ctx.clone(), 200)).await.unwrap();

        store
            .finalize_turn_usage(&TurnLogFinalized {
                ctx: ctx.clone(),
                finalization: TurnUsageFinalization::Usage(usage_snapshot(5, 7, 1)),
            })
            .await
            .unwrap();

        let row = store.turn_log_row(&ctx.turn_id).await.unwrap().unwrap();
        assert_eq!(row.input_tokens, Some(5));
        assert_eq!(row.output_tokens, Some(7));
        assert_eq!(row.thought_tokens, Some(1));
        assert_eq!(row.cached_read_tokens, Some(2));
        assert_eq!(row.cached_write_tokens, Some(3));
        assert_eq!(row.cost_amount, Some(0.42));
        assert_eq!(row.cost_currency.as_deref(), Some("USD"));
        assert_eq!(row.usage_finalized_ms, Some(12_345));
        assert_eq!(row.usage_finalization_kind, "usage");
    }

    #[tokio::test]
    async fn usage_finalization_uses_persistence_time_not_old_event_time() {
        let store = MemoryTaskStore::with_clock(Arc::new(|| 86_400_001));
        let ctx = turn_ctx("turn-persist-time", "ctx-persist-time", None, 0);
        store.upsert_turn_finished(&finished_row(ctx.clone(), 200)).await.unwrap();

        store
            .finalize_turn_usage(&TurnLogFinalized {
                ctx: ctx.clone(),
                finalization: TurnUsageFinalization::Usage(usage_snapshot(5, 7, 1)),
            })
            .await
            .unwrap();

        let row = store.turn_log_row(&ctx.turn_id).await.unwrap().unwrap();
        assert_eq!(row.usage_finalized_ms, Some(86_400_001));
        assert_ne!(row.usage_finalized_ms, Some(1));
    }

    #[tokio::test]
    async fn usage_finalized_none_sets_no_usage_barrier() {
        let store = MemoryTaskStore::with_clock(Arc::new(|| 12_346));
        let ctx = turn_ctx("turn-final-none", "ctx-final-none", None, 0);
        store.upsert_turn_finished(&finished_row(ctx.clone(), 200)).await.unwrap();

        store
            .finalize_turn_usage(&TurnLogFinalized {
                ctx: ctx.clone(),
                finalization: TurnUsageFinalization::NoUsage,
            })
            .await
            .unwrap();

        let row = store.turn_log_row(&ctx.turn_id).await.unwrap().unwrap();
        assert_eq!(row.input_tokens, None);
        assert_eq!(row.cost_amount, None);
        assert_eq!(row.usage_finalized_ms, Some(12_346));
        assert_eq!(row.usage_finalization_kind, "no_usage");
    }

    #[tokio::test]
    async fn usage_finalization_invalid_clock_uses_never_eligible_timestamp() {
        let store = MemoryTaskStore::with_clock(Arc::new(|| 0));
        let ctx = turn_ctx("turn-final-zero", "ctx-final-zero", None, 0);
        store.upsert_turn_finished(&finished_row(ctx.clone(), 200)).await.unwrap();

        store
            .finalize_turn_usage(&TurnLogFinalized {
                ctx: ctx.clone(),
                finalization: TurnUsageFinalization::NoUsage,
            })
            .await
            .unwrap();

        let row = store.turn_log_row(&ctx.turn_id).await.unwrap().unwrap();
        assert_eq!(row.usage_finalized_ms, Some(RETENTION_NEVER_ELIGIBLE_MS));
        assert_ne!(row.usage_finalized_ms, Some(0));
    }

    #[tokio::test]
    async fn no_usage_finalization_rejects_existing_usage_columns() {
        let store = MemoryTaskStore::with_clock(Arc::new(|| 12_347));
        let ctx = turn_ctx("turn-final-contradict", "ctx-final-contradict", None, 0);
        store.upsert_turn_finished(&finished_row(ctx.clone(), 200)).await.unwrap();
        store
            .finalize_turn_usage(&TurnLogFinalized {
                ctx: ctx.clone(),
                finalization: TurnUsageFinalization::Usage(usage_snapshot(1, 2, 1)),
            })
            .await
            .unwrap();

        assert!(store
            .finalize_turn_usage(&TurnLogFinalized {
                ctx: ctx.clone(),
                finalization: TurnUsageFinalization::NoUsage,
            })
            .await
            .is_err());
        let row = store.turn_log_row(&ctx.turn_id).await.unwrap().unwrap();
        assert_eq!(row.usage_finalized_ms, Some(12_347));
        assert_eq!(row.usage_finalization_kind, "usage");
    }

    #[tokio::test]
    async fn turn_finished_upsert_does_not_clear_finalization() {
        let store = MemoryTaskStore::with_clock(Arc::new(|| 12_348));
        let ctx = turn_ctx("turn-final-replay", "ctx-final-replay", None, 0);
        let first = finished_row(ctx.clone(), 200);
        store.upsert_turn_finished(&first).await.unwrap();
        store
            .finalize_turn_usage(&TurnLogFinalized {
                ctx: ctx.clone(),
                finalization: TurnUsageFinalization::NoUsage,
            })
            .await
            .unwrap();

        store.upsert_turn_finished(&finished_row(ctx.clone(), 250)).await.unwrap();
        let row = store.turn_log_row(&ctx.turn_id).await.unwrap().unwrap();
        assert_eq!(row.usage_finalized_ms, Some(12_348));
        assert_eq!(row.usage_finalization_kind, "no_usage");
    }

    #[tokio::test]
    async fn turn_finished_task_linked_bumps_artifact_recency() {
        let store = MemoryTaskStore::with_clock(Arc::new(|| 50_000));
        let task = TaskId::parse("task-recency-finish").unwrap();
        store.create(&rec(task.as_str(), 1)).await.unwrap();
        let ctx = turn_ctx("turn-recency-finish", "ctx-recency-finish", Some(task.as_str()), 0);

        store.upsert_turn_finished(&finished_row(ctx, 200)).await.unwrap();

        let row = store.get(&task).await.unwrap().unwrap();
        assert_eq!(row.last_artifact_ms, Some(50_000));
    }

    #[tokio::test]
    async fn finalize_turn_usage_task_linked_bumps_artifact_recency() {
        let clock = Arc::new(AtomicI64::new(60_000));
        let store = MemoryTaskStore::with_clock({
            let clock = Arc::clone(&clock);
            Arc::new(move || clock.load(Ordering::SeqCst))
        });
        let task = TaskId::parse("task-recency-finalize").unwrap();
        store.create(&rec(task.as_str(), 1)).await.unwrap();
        let ctx = turn_ctx("turn-recency-finalize", "ctx-recency-finalize", Some(task.as_str()), 0);
        store.upsert_turn_finished(&finished_row(ctx.clone(), 200)).await.unwrap();
        clock.store(70_000, Ordering::SeqCst);

        store
            .finalize_turn_usage(&TurnLogFinalized {
                ctx,
                finalization: TurnUsageFinalization::NoUsage,
            })
            .await
            .unwrap();

        let row = store.get(&task).await.unwrap().unwrap();
        assert_eq!(row.last_artifact_ms, Some(70_000));
    }

    #[test]
    fn batch_id_parses_nonempty() {
        assert!(crate::ids::BatchId::parse("batch-abc").is_ok());
        assert!(crate::ids::BatchId::parse("").is_err());
    }

    #[test]
    fn batch_status_serde_roundtrip() {
        for s in [
            BatchStatus::Working,
            BatchStatus::Completed,
            BatchStatus::Canceling,
            BatchStatus::Canceled,
            BatchStatus::Failed,
        ] {
            let j = serde_json::to_string(&s).unwrap();
            assert_eq!(serde_json::from_str::<BatchStatus>(&j).unwrap(), s);
        }
    }

    #[test]
    fn task_record_batch_fields_default_none() {
        let rec = rec("task-1", 0);
        assert!(rec.batch_id.is_none() && rec.item_id.is_none());
    }

    #[tokio::test]
    async fn memory_batch_roundtrip_and_claim_and_cas() {
        let s = MemoryTaskStore::new();
        let bid = BatchId::parse("b1").unwrap();
        s.create_batch(&sample_batch(&bid, BatchStatus::Working, 2, 0))
            .await
            .unwrap();
        assert_eq!(
            s.get_batch(&bid).await.unwrap().unwrap().status,
            BatchStatus::Working
        );
        assert_eq!(s.active_batches().await.unwrap().len(), 1);

        let t1 = TaskId::parse("t1").unwrap();
        let rec = batch_child_record(&t1, &bid, "item-a");
        assert_eq!(
            s.claim_batch_child(&bid, "item-a", &rec).await.unwrap(),
            ChildClaim::Created
        );
        let t2 = TaskId::parse("t2").unwrap();
        let rec2 = batch_child_record(&t2, &bid, "item-a");
        assert_eq!(
            s.claim_batch_child(&bid, "item-a", &rec2).await.unwrap(),
            ChildClaim::ExistingWorking
        );
        assert_eq!(s.batch_children(&bid).await.unwrap().len(), 1);

        assert!(s
            .settle_batch_if_status(&bid, BatchStatus::Working, BatchStatus::Completed, 1)
            .await
            .unwrap());
        assert!(!s.cancel_batch_if_working(&bid, 2).await.unwrap());
    }

    #[tokio::test]
    async fn memory_fail_batch_if_status() {
        let s = MemoryTaskStore::new();
        let bid = BatchId::parse("b-fail").unwrap();
        s.create_batch(&sample_batch(&bid, BatchStatus::Working, 1, 0))
            .await
            .unwrap();

        assert!(s
            .fail_batch_if_status(&bid, BatchStatus::Working, "bad plan", 7)
            .await
            .unwrap());
        let got = s.get_batch(&bid).await.unwrap().unwrap();
        assert_eq!(got.status, BatchStatus::Failed);
        assert_eq!(got.error.as_deref(), Some("bad plan"));
        assert_eq!(got.updated_ms, 7);
        assert!(!s
            .fail_batch_if_status(&bid, BatchStatus::Working, "ignored", 8)
            .await
            .unwrap());
        assert_eq!(
            s.get_batch(&bid).await.unwrap().unwrap().error.as_deref(),
            Some("bad plan")
        );
    }

    #[test]
    fn fold_collapses_started_then_finished() {
        let op = OperationId::parse("op-t").unwrap();
        let ev = |seq, kind| OrchEvent {
            v: ORCH_V,
            seq,
            ts_ms: 0,
            operation_id: op.clone(),
            session: None,
            source: None,
            kind,
        };
        let events = vec![
            ev(1, OrchEventKind::NodeStarted { node: "a".into() }),
            ev(
                2,
                OrchEventKind::NodeFinished {
                    node: "a".into(),
                    ok: true,
                    output: "oA".into(),
                    usage: None,
                },
            ),
            ev(3, OrchEventKind::NodeStarted { node: "b".into() }),
        ];
        let scalars = JournalScalars {
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            terminal_seq: None,
            cut_seq: 3,
        };
        let snap = fold_journal_to_snapshot(&events, &scalars).unwrap();
        assert_eq!(
            snap.checkpoints
                .iter()
                .map(|c| (c.0.as_str().to_string(), c.3))
                .collect::<Vec<_>>(),
            vec![("a".into(), 2)]
        );
        assert_eq!(
            snap.starts
                .iter()
                .map(|s| (s.0.as_str().to_string(), s.1))
                .collect::<Vec<_>>(),
            vec![("b".into(), 3)]
        );
        assert_eq!(snap.cut_seq, 3);
    }

    #[test]
    fn fold_repeated_start_keeps_latest_only() {
        let op = OperationId::parse("op-t").unwrap();
        let ev = |seq, kind| OrchEvent {
            v: ORCH_V,
            seq,
            ts_ms: 0,
            operation_id: op.clone(),
            session: None,
            source: None,
            kind,
        };
        let events = vec![
            ev(1, OrchEventKind::NodeStarted { node: "a".into() }),
            ev(4, OrchEventKind::NodeStarted { node: "a".into() }),
        ];
        let scalars = JournalScalars {
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            terminal_seq: None,
            cut_seq: 4,
        };
        let snap = fold_journal_to_snapshot(&events, &scalars).unwrap();
        assert_eq!(
            snap.starts
                .iter()
                .map(|s| (s.0.as_str().to_string(), s.1))
                .collect::<Vec<_>>(),
            vec![("a".into(), 4)]
        );
    }

    #[test]
    fn task_record_status_maps_total() {
        use crate::orch::TerminalStatus;
        assert!(matches!(
            terminal_status_from_record(&TaskRecordStatus::Completed),
            TerminalStatus::Completed
        ));
        assert!(matches!(
            terminal_status_from_record(&TaskRecordStatus::Canceled),
            TerminalStatus::Canceled
        ));
        for s in [
            TaskRecordStatus::Failed,
            TaskRecordStatus::Interrupted,
            TaskRecordStatus::Working,
        ] {
            assert!(matches!(
                terminal_status_from_record(&s),
                TerminalStatus::Failed { .. }
            ));
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

    #[tokio::test]
    async fn cancel_if_working_guards_against_clobber() {
        let s = MemoryTaskStore::new();
        // Working row → flips to Canceled, returns true.
        let w = TaskId::parse("w").unwrap();
        s.create(&rec("w", 1)).await.unwrap();
        assert!(s.cancel_if_working(&w, 7).await.unwrap());
        assert_eq!(
            s.get(&w).await.unwrap().unwrap().status,
            TaskRecordStatus::Canceled
        );
        // Already-terminal row → no-op, returns false, result preserved (the M1 race guard).
        let c = TaskId::parse("c").unwrap();
        s.create(&rec("c", 1)).await.unwrap();
        s.set_terminal(&c, TaskRecordStatus::Completed, Some("KEEP"), None, 2)
            .await
            .unwrap();
        assert!(!s.cancel_if_working(&c, 9).await.unwrap());
        let got = s.get(&c).await.unwrap().unwrap();
        assert_eq!(got.status, TaskRecordStatus::Completed);
        assert_eq!(got.result.as_deref(), Some("KEEP"));
        // Missing row → false.
        assert!(!s
            .cancel_if_working(&TaskId::parse("nope").unwrap(), 9)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn node_checkpoints_roundtrip_and_claim() {
        let s = MemoryTaskStore::new();
        let t = TaskId::parse("t").unwrap();
        s.create(&TaskRecord {
            id: t.clone(),
            workflow: "wf".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
            last_artifact_ms: None,
            input: "DIFF".into(),
            workflow_spec_json: Some("{\"v\":1}".into()),
            resume_attempts: 0,
            session_cwd: None,
            batch_id: None,
            item_id: None,
            artifacts_purged_at: None,
        })
        .await
        .unwrap();
        s.put_node_checkpoint(&t, &NodeId::parse("codex").unwrap(), "OUT", true, 2)
            .await
            .unwrap();
        let cps = s.node_checkpoints(&t).await.unwrap();
        assert_eq!(cps.len(), 1);
        assert_eq!(cps[0].1, "OUT");
        assert!(matches!(
            s.claim_resume_attempt(&t, 2, 9).await.unwrap(),
            ResumeClaim::Resumable { attempt: 1 }
        ));
        assert!(matches!(
            s.claim_resume_attempt(&t, 2, 9).await.unwrap(),
            ResumeClaim::Resumable { attempt: 2 }
        ));
        assert!(matches!(
            s.claim_resume_attempt(&t, 2, 9).await.unwrap(),
            ResumeClaim::Exhausted
        ));
        let wt = s.working_tasks().await.unwrap();
        assert_eq!(wt.len(), 1);
        assert_eq!(wt[0].input, "DIFF");
    }

    #[tokio::test]
    async fn put_checkpoint_duplicate_node_errors() {
        let s = MemoryTaskStore::new();
        let t = TaskId::parse("t-dup").unwrap();
        s.create(&rec("t-dup", 1)).await.unwrap();
        let node = NodeId::parse("codex").unwrap();
        // First write succeeds.
        s.put_node_checkpoint(&t, &node, "OUT1", true, 10)
            .await
            .unwrap();
        // Second write for the same (task, node) must be an error (write-once).
        let res = s.put_node_checkpoint(&t, &node, "OUT2", true, 20).await;
        assert!(
            res.is_err(),
            "expected Err on duplicate (task, node) checkpoint write"
        );
        // The original checkpoint is unchanged.
        let cps = s.node_checkpoints(&t).await.unwrap();
        assert_eq!(cps.len(), 1);
        assert_eq!(cps[0].1, "OUT1");
    }

    #[tokio::test]
    async fn put_checkpoint_unknown_task_errors() {
        let s = MemoryTaskStore::new();
        let unknown = TaskId::parse("does-not-exist").unwrap();
        let result = s
            .put_node_checkpoint(&unknown, &NodeId::parse("node-a").unwrap(), "OUT", true, 1)
            .await;
        assert!(result.is_err(), "expected Err for unknown task id");
    }

    #[tokio::test]
    async fn seq_methods_roundtrip_memory() {
        let s = MemoryTaskStore::new();
        let t = TaskId::parse("t").unwrap();
        s.create(&rec("t", 1)).await.unwrap(); // use the EXISTING helper for a Working TaskRecord
        let op = OperationId::parse("op-t").unwrap();
        let s1 = s
            .record_node_started(&t, &NodeId::parse("a").unwrap(), &op, 1)
            .await
            .unwrap();
        let s2 = s
            .put_node_checkpoint_sequenced(
                &t,
                &NodeId::parse("a").unwrap(),
                &op,
                "OUT",
                true,
                2,
                None,
            )
            .await
            .unwrap();
        assert!(s2 > s1, "seq is monotonic");
        // Fix 4a: checkpoint carries the allocated seq.
        let snap = s.progress_snapshot(&t).await.unwrap();
        assert_eq!(
            snap.checkpoints[0].3, s2,
            "checkpoint carries its allocated seq"
        );
        let s3 = s
            .set_terminal_sequenced(&t, &op, TaskRecordStatus::Completed, Some("R"), None, 3)
            .await
            .unwrap();
        assert!(s3 > s2);
        let snap = s.progress_snapshot(&t).await.unwrap();
        assert_eq!(snap.cut_seq, s3);
        assert_eq!(snap.terminal_seq, Some(s3));
        assert_eq!(snap.checkpoints.len(), 1);
        assert!(
            snap.starts.is_empty(),
            "the start row was cleared on finish"
        );
        // idempotent re-start (resume re-emit): no error, new seq
        s.create(&rec("t2", 1)).await.unwrap();
        let t2 = TaskId::parse("t2").unwrap();
        let op2 = OperationId::parse("op-t2").unwrap();
        let a = s
            .record_node_started(&t2, &NodeId::parse("x").unwrap(), &op2, 1)
            .await
            .unwrap();
        let b = s
            .record_node_started(&t2, &NodeId::parse("x").unwrap(), &op2, 2)
            .await
            .unwrap();
        assert!(b > a, "re-start upserts a fresh seq, no PK error");
        // Fix 4b: mid-flight check — record_node_started on a fresh task, then snapshot.
        s.create(&rec("t3", 1)).await.unwrap();
        let t3 = TaskId::parse("t3").unwrap();
        let op3 = OperationId::parse("op-t3").unwrap();
        let start_seq = s
            .record_node_started(&t3, &NodeId::parse("a").unwrap(), &op3, 1)
            .await
            .unwrap();
        let snap = s.progress_snapshot(&t3).await.unwrap();
        assert_eq!(
            snap.starts.len(),
            1,
            "start must be present before checkpoint"
        );
        assert_eq!(
            snap.starts[0].1, start_seq,
            "start carries the allocated seq"
        );
        assert!(snap.checkpoints.is_empty(), "no checkpoint yet for t3");
    }

    #[tokio::test]
    async fn memory_node_checkpoint_roundtrips_usage_and_journals_it() {
        let s = MemoryTaskStore::new();
        let t = TaskId::parse("t-usage").unwrap();
        s.create(&rec("t-usage", 1)).await.unwrap();
        let op = OperationId::parse("op-t-usage").unwrap();
        let member = NodeId::parse("member").unwrap();
        let usage = UsageSnapshot {
            used: Some(15071),
            size: Some(258400),
            cost: None,
            terminal: None,
            at_ms: 7,
        };

        s.put_node_checkpoint_sequenced(&t, &member, &op, "OUT", true, 7, Some(&usage))
            .await
            .unwrap();
        s.put_node_checkpoint(&t, &NodeId::parse("legacy").unwrap(), "L", true, 8)
            .await
            .unwrap();

        let cps = s.node_checkpoints(&t).await.unwrap();
        let got = cps
            .iter()
            .find(|(node, ..)| node.as_str() == "member")
            .unwrap();
        assert_eq!(got.1, "OUT");
        assert!(got.2);
        assert_eq!(got.3.as_ref(), Some(&usage));
        let legacy = cps
            .iter()
            .find(|(node, ..)| node.as_str() == "legacy")
            .unwrap();
        assert!(legacy.3.is_none(), "legacy checkpoint has no usage");

        let evs = s.journal_from(&t, -1).await.unwrap();
        assert!(matches!(
            &evs[0].kind,
            OrchEventKind::NodeFinished { usage: Some(got), .. } if got == &usage
        ));
    }

    #[tokio::test]
    async fn memory_journal_write() {
        let s = MemoryTaskStore::new();
        let t = TaskId::parse("task-j").unwrap();
        s.create(&rec("task-j", 1)).await.unwrap();
        let a = NodeId::parse("a").unwrap();
        let op = OperationId::parse("op-task-j").unwrap();
        let s1 = s.record_node_started(&t, &a, &op, 1).await.unwrap();
        let s2 = s
            .put_node_checkpoint_sequenced(&t, &a, &op, "oA", true, 2, None)
            .await
            .unwrap();
        let evs = s.journal_from(&t, -1).await.unwrap();
        assert_eq!(evs.len(), 2);
        assert!(matches!(evs[0].kind, OrchEventKind::NodeStarted { .. }) && evs[0].seq == s1);
        assert!(
            matches!(&evs[1].kind, OrchEventKind::NodeFinished { output, .. } if output == "oA")
                && evs[1].seq == s2
        );
        assert_eq!(evs[0].operation_id.as_str(), "op-task-j");
    }

    #[tokio::test]
    async fn create_sets_birth_flag_and_fold_inputs_consistent() {
        let s = MemoryTaskStore::new();
        let t = TaskId::parse("task-b").unwrap();
        s.create(&rec("task-b", 1)).await.unwrap();
        let a = NodeId::parse("a").unwrap();
        let op = OperationId::parse("op-task-b").unwrap();
        s.record_node_started(&t, &a, &op, 1).await.unwrap();
        s.put_node_checkpoint_sequenced(&t, &a, &op, "oA", true, 2, None)
            .await
            .unwrap();

        let fi = s.journal_fold_inputs(&t).await.unwrap();
        assert!(fi.complete_from_birth);
        assert_eq!(fi.events.len(), 2);
        assert_eq!(fi.scalars.cut_seq, 2);
    }

    #[tokio::test]
    async fn memory_turn_log_row_lookup() {
        let store = MemoryTaskStore::new();
        write_finished_turn(
            &store,
            "turn-a",
            "ctx-a",
            Some("task-a"),
            20,
            2,
            4,
            Some(("USD", 0.25)),
        )
        .await;

        let found = store
            .turn_log_row(&TurnId::parse("turn-a").unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.turn_id.as_str(), "turn-a");
        assert_eq!(found.task_id.as_ref().unwrap().as_str(), "task-a");
        assert_eq!(
            found.traceparent.unwrap().to_header_value(),
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
        );

        assert!(store
            .turn_log_row(&TurnId::parse("missing").unwrap())
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn memory_turn_log_rows_for_task_orders_and_limits() {
        let store = MemoryTaskStore::new();
        write_finished_turn(&store, "turn-c", "ctx-a", Some("task-a"), 30, 1, 1, None).await;
        write_finished_turn(&store, "turn-a", "ctx-a", Some("task-a"), 10, 1, 1, None).await;
        write_finished_turn(&store, "turn-b", "ctx-a", Some("task-a"), 20, 1, 1, None).await;
        write_finished_turn(&store, "turn-x", "ctx-a", Some("task-x"), 5, 1, 1, None).await;

        let rows = store
            .turn_log_rows_for_task(&TaskId::parse("task-a").unwrap(), 2)
            .await
            .unwrap();

        assert_eq!(
            rows.iter().map(|r| r.turn_id.as_str()).collect::<Vec<_>>(),
            vec!["turn-a", "turn-b"]
        );
    }

    #[tokio::test]
    async fn memory_turn_log_usage_for_task_is_unbounded_and_single_currency() {
        let store = MemoryTaskStore::new();
        write_finished_turn(
            &store,
            "turn-1",
            "ctx-a",
            Some("task-a"),
            10,
            2,
            3,
            Some(("USD", 0.10)),
        )
        .await;
        write_finished_turn(
            &store,
            "turn-2",
            "ctx-a",
            Some("task-a"),
            20,
            5,
            7,
            Some(("USD", 0.20)),
        )
        .await;
        write_finished_turn(
            &store,
            "turn-3",
            "ctx-a",
            Some("task-a"),
            30,
            11,
            13,
            Some(("USD", 0.30)),
        )
        .await;

        let agg = store
            .turn_log_usage_for_task(&TaskId::parse("task-a").unwrap())
            .await
            .unwrap()
            .unwrap();
        let cost = agg.cost.as_ref().cloned().unwrap_or(UsageCost {
            amount: 0.0,
            currency: String::new(),
        });

        assert_eq!(agg.rows, 3);
        assert_eq!(agg.input_tokens, 18);
        assert_eq!(agg.output_tokens, 23);
        assert_eq!(agg.thought_tokens, Some(9));
        assert_eq!(agg.cached_read_tokens, None);
        assert_eq!(agg.cached_write_tokens, Some(15));
        assert_eq!(cost.currency, "USD");
        assert!((cost.amount - 0.60).abs() < 0.000_001);
        assert_eq!(agg.at_ms, 30);
    }

    #[tokio::test]
    async fn memory_turn_log_usage_for_task_omits_mixed_currency_cost() {
        let store = MemoryTaskStore::new();
        write_finished_turn(
            &store,
            "turn-1",
            "ctx-a",
            Some("task-a"),
            10,
            2,
            3,
            Some(("USD", 0.10)),
        )
        .await;
        write_finished_turn(
            &store,
            "turn-2",
            "ctx-a",
            Some("task-a"),
            20,
            5,
            7,
            Some(("EUR", 0.20)),
        )
        .await;

        let agg = store
            .turn_log_usage_for_task(&TaskId::parse("task-a").unwrap())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(agg.input_tokens, 7);
        assert_eq!(agg.output_tokens, 10);
        assert!(agg.cost.is_none());
    }

    #[tokio::test]
    async fn memory_latest_turn_log_row_for_session_returns_latest() {
        let store = MemoryTaskStore::new();
        write_finished_turn(&store, "turn-old", "ctx-a", None, 10, 1, 1, None).await;
        write_finished_turn(&store, "turn-new", "ctx-a", None, 20, 1, 1, None).await;
        write_finished_turn(&store, "turn-other", "ctx-b", None, 30, 1, 1, None).await;

        let row = store
            .latest_turn_log_row_for_session(&ContextId::parse("ctx-a").unwrap())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(row.turn_id.as_str(), "turn-new");
    }

    #[tokio::test]
    async fn memory_journal_jsonl_bounded_body_counts_and_limits() {
        let store = MemoryTaskStore::new();
        let task = TaskId::parse("task-a").unwrap();
        let op = OperationId::parse("op-a").unwrap();
        store.create(&rec("task-a", 1)).await.unwrap();
        store
            .record_event_sequenced(
                &task,
                &op,
                10,
                OrchEventKind::Progress { text: "one".into() },
            )
            .await
            .unwrap();
        store
            .record_event_sequenced(
                &task,
                &op,
                11,
                OrchEventKind::Progress { text: "two".into() },
            )
            .await
            .unwrap();

        let body = store
            .journal_jsonl_bounded(&task, 10, 10_000)
            .await
            .unwrap();

        match body {
            JournalRead::Body {
                jsonl,
                events,
                bytes,
            } => {
                assert_eq!(events, 2);
                assert_eq!(bytes as usize, jsonl.len());
                assert_eq!(jsonl.lines().count(), 2);
                assert!(jsonl.ends_with('\n'));
            }
            JournalRead::TooLarge { .. } => panic!("journal should fit"),
        }

        assert!(matches!(
            store.journal_jsonl_bounded(&task, 1, 10_000).await.unwrap(),
            JournalRead::TooLarge { events: 2, .. }
        ));
        assert!(matches!(
            store.journal_jsonl_bounded(&task, 10, 1).await.unwrap(),
            JournalRead::TooLarge { events: 2, .. }
        ));
    }

    #[tokio::test]
    async fn memory_node_checkpoint_nodes_and_output_are_bounded() {
        let store = MemoryTaskStore::new();
        let task = TaskId::parse("task-a").unwrap();
        store.create(&rec("task-a", 1)).await.unwrap();
        store
            .put_node_checkpoint(
                &task,
                &NodeId::parse("node-b").unwrap(),
                "large-output",
                true,
                10,
            )
            .await
            .unwrap();
        store
            .put_node_checkpoint(&task, &NodeId::parse("node-a").unwrap(), "small", false, 11)
            .await
            .unwrap();

        let nodes = store.node_checkpoint_nodes(&task).await.unwrap();
        assert_eq!(
            nodes.iter().map(|n| n.as_str()).collect::<Vec<_>>(),
            vec!["node-b", "node-a"]
        );

        let output = store
            .node_checkpoint_output(&task, &NodeId::parse("node-a").unwrap(), 10)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            output,
            NodeCheckpointOutput::Found {
                output: "small".into(),
                ok: false,
                usage: None,
                bytes: 5
            }
        );

        assert_eq!(
            store
                .node_checkpoint_output(&task, &NodeId::parse("node-b").unwrap(), 3)
                .await
                .unwrap(),
            Some(NodeCheckpointOutput::TooLarge { bytes: 12 })
        );
    }

    #[tokio::test]
    async fn session_cwd_roundtrip() {
        // A TaskRecord with session_cwd=Some("/req") must survive create→get intact.
        let s = MemoryTaskStore::new();
        let id = TaskId::parse("cwd-1").unwrap();
        s.create(&TaskRecord {
            id: id.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
            last_artifact_ms: None,
            input: "DIFF".into(),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: Some("/req".to_string()),
            batch_id: None,
            item_id: None,
            artifacts_purged_at: None,
        })
        .await
        .unwrap();
        let got = s.get(&id).await.unwrap().unwrap();
        assert_eq!(
            got.session_cwd.as_deref(),
            Some("/req"),
            "session_cwd must survive create→get"
        );
    }
}
