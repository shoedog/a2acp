//! Durable task control-plane port: persists a detached workflow run's status
//! and final result. Separate from `SessionStore` (ephemeral routing state) by
//! responsibility. Timestamps are passed IN — the core forbids `Date::now`.

use crate::error::BridgeError;
use crate::ids::{NodeId, TaskId};

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
    /// Return all node checkpoints for a task as `(node_id, output, ok)` tuples.
    async fn node_checkpoints(
        &self,
        task: &TaskId,
    ) -> Result<Vec<(NodeId, String, bool)>, BridgeError>;
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

    // ── Seq-bearing progress methods (Phase A: streaming reattach substrate) ──

    /// Record that a node has started executing. Returns the allocated seq.
    /// Re-starting the same node (resume re-emit) MUST NOT return an error —
    /// it upserts a fresh seq and ts, overwriting the previous start row.
    async fn record_node_started(
        &self,
        task: &TaskId,
        node: &NodeId,
        ts: i64,
    ) -> Result<i64, BridgeError>;

    /// Persist a node output checkpoint WITH a monotonic seq. Returns the seq.
    /// Removes the node's start row (the node is no longer "in progress").
    ///
    /// WRITE-ONCE per `(task,node)`, like `put_node_checkpoint` (W3b semantics):
    /// a node finishes once; on resume, already-finished nodes are seeded and do
    /// NOT re-checkpoint, so the durable (SQLite) impl uses a plain INSERT.
    async fn put_node_checkpoint_sequenced(
        &self,
        task: &TaskId,
        node: &NodeId,
        output: &str,
        ok: bool,
        ts: i64,
    ) -> Result<i64, BridgeError>;

    /// Set the terminal status + result/error, recording a seq for the event.
    /// Returns the seq.
    async fn set_terminal_sequenced(
        &self,
        task: &TaskId,
        status: TaskRecordStatus,
        result: Option<&str>,
        error: Option<&str>,
        ts: i64,
    ) -> Result<i64, BridgeError>;

    /// Reconstruct the current progress state for streaming reattach.
    /// `checkpoints` is ordered by seq (ascending).
    ///
    /// Under the single-writer-per-task model (one detached runner writes; the
    /// handler reads) this returns a consistent point-in-time view; `cut_seq` is
    /// guaranteed >= every included seq.
    async fn progress_snapshot(&self, task: &TaskId) -> Result<TaskProgressSnapshot, BridgeError>;
}

use std::collections::HashMap;
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

/// `(output, ok, ts, seq)` stored per `(task_id, node_id)` checkpoint key.
type CheckpointValue = (String, bool, i64, i64);

/// In-memory `TaskStore` (the default when no DB path is configured). Production
/// use, not just a test fake — lives in `bridge-core` so `bridge-a2a-inbound`
/// can default to it WITHOUT depending on `bridge-store`.
#[derive(Default)]
pub struct MemoryTaskStore {
    inner: Mutex<HashMap<String, TaskRecord>>,
    /// Key: (task_id, node_id) → (output, ok, ts, seq)
    checkpoints: Mutex<HashMap<(String, String), CheckpointValue>>,
    /// Per-task monotonic seq counter. Key: task_id.
    seq_counters: Mutex<HashMap<String, i64>>,
    /// Per-task terminal seq. Key: task_id.
    terminal_seqs: Mutex<HashMap<String, i64>>,
    /// In-progress node starts. Key: (task_id, node_id) → (seq, ts).
    starts: Mutex<HashMap<(String, String), (i64, i64)>>,
}

impl MemoryTaskStore {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            checkpoints: Mutex::new(HashMap::new()),
            seq_counters: Mutex::new(HashMap::new()),
            terminal_seqs: Mutex::new(HashMap::new()),
            starts: Mutex::new(HashMap::new()),
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
    async fn cancel_if_working(&self, id: &TaskId, updated_ms: i64) -> Result<bool, BridgeError> {
        let mut g = self.inner.lock().unwrap();
        match g.get_mut(id.as_str()) {
            Some(row) if row.status == TaskRecordStatus::Working => {
                row.status = TaskRecordStatus::Canceled;
                row.updated_ms = updated_ms;
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
        // The task must exist — matches the SQLite FK (foreign_keys=ON) contract.
        {
            let inner = self.inner.lock().unwrap();
            if !inner.contains_key(task.as_str()) {
                return Err(BridgeError::StoreFailure);
            }
        } // drop inner guard before locking checkpoints to avoid lock-order deadlock
        let mut g = self.checkpoints.lock().unwrap();
        let key = (task.as_str().to_string(), node.as_str().to_string());
        if g.contains_key(&key) {
            return Err(BridgeError::StoreFailure);
        }
        g.insert(key, (output.to_string(), ok, ts, 0));
        Ok(())
    }
    async fn node_checkpoints(
        &self,
        task: &TaskId,
    ) -> Result<Vec<(NodeId, String, bool)>, BridgeError> {
        let g = self.checkpoints.lock().unwrap();
        let mut out = Vec::new();
        for ((tid, nid), (output, ok, _ts, _seq)) in g.iter() {
            if tid == task.as_str() {
                let node = NodeId::parse(nid).map_err(|_| BridgeError::StoreFailure)?;
                out.push((node, output.clone(), *ok));
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
        row.updated_ms = now_ms; // last_resume_ms is folded into updated_ms for the in-memory store; the SQLite store has the dedicated column.
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

    async fn record_node_started(
        &self,
        task: &TaskId,
        node: &NodeId,
        ts: i64,
    ) -> Result<i64, BridgeError> {
        // Task must exist.
        {
            let inner = self.inner.lock().unwrap();
            if !inner.contains_key(task.as_str()) {
                return Err(BridgeError::StoreFailure);
            }
        }
        let seq = self.next_seq(task.as_str());
        let mut g = self.starts.lock().unwrap();
        let key = (task.as_str().to_string(), node.as_str().to_string());
        // Upsert: re-starting the same node is allowed (resume re-emit).
        g.insert(key, (seq, ts));
        Ok(seq)
    }

    async fn put_node_checkpoint_sequenced(
        &self,
        task: &TaskId,
        node: &NodeId,
        output: &str,
        ok: bool,
        ts: i64,
    ) -> Result<i64, BridgeError> {
        // Task must exist.
        {
            let inner = self.inner.lock().unwrap();
            if !inner.contains_key(task.as_str()) {
                return Err(BridgeError::StoreFailure);
            }
        }
        let seq = self.next_seq(task.as_str());
        // Remove start row for this node (it is no longer in progress).
        {
            let mut sg = self.starts.lock().unwrap();
            sg.remove(&(task.as_str().to_string(), node.as_str().to_string()));
        }
        let mut g = self.checkpoints.lock().unwrap();
        let key = (task.as_str().to_string(), node.as_str().to_string());
        g.insert(key, (output.to_string(), ok, ts, seq));
        Ok(seq)
    }

    async fn set_terminal_sequenced(
        &self,
        task: &TaskId,
        status: TaskRecordStatus,
        result: Option<&str>,
        error: Option<&str>,
        ts: i64,
    ) -> Result<i64, BridgeError> {
        // Task must exist — check BEFORE allocating a seq (mirrors record_node_started /
        // put_node_checkpoint_sequenced order to avoid leaking a counter increment on
        // a non-existent task).
        {
            let inner = self.inner.lock().unwrap();
            if !inner.contains_key(task.as_str()) {
                return Err(BridgeError::StoreFailure);
            }
        }
        let seq = self.next_seq(task.as_str());
        {
            let mut g = self.inner.lock().unwrap();
            let row = g.get_mut(task.as_str()).ok_or(BridgeError::StoreFailure)?;
            row.status = status;
            row.result = result.map(|s| s.to_string());
            row.error = error.map(|s| s.to_string());
            row.updated_ms = ts;
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
        Ok(seq)
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
            for ((tid, nid), (output, ok, _ts, seq)) in g.iter() {
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
    use crate::ids::NodeId;

    fn rec(id: &str, ms: i64) -> TaskRecord {
        TaskRecord {
            id: TaskId::parse(id).unwrap(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: ms,
            updated_ms: ms,
            input: String::new(),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: None,
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
            input: "DIFF".into(),
            workflow_spec_json: Some("{\"v\":1}".into()),
            resume_attempts: 0,
            session_cwd: None,
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
        let s1 = s
            .record_node_started(&t, &NodeId::parse("a").unwrap(), 1)
            .await
            .unwrap();
        let s2 = s
            .put_node_checkpoint_sequenced(&t, &NodeId::parse("a").unwrap(), "OUT", true, 2)
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
            .set_terminal_sequenced(&t, TaskRecordStatus::Completed, Some("R"), None, 3)
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
        let a = s
            .record_node_started(&t2, &NodeId::parse("x").unwrap(), 1)
            .await
            .unwrap();
        let b = s
            .record_node_started(&t2, &NodeId::parse("x").unwrap(), 2)
            .await
            .unwrap();
        assert!(b > a, "re-start upserts a fresh seq, no PK error");
        // Fix 4b: mid-flight check — record_node_started on a fresh task, then snapshot.
        s.create(&rec("t3", 1)).await.unwrap();
        let t3 = TaskId::parse("t3").unwrap();
        let start_seq = s
            .record_node_started(&t3, &NodeId::parse("a").unwrap(), 1)
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
            input: "DIFF".into(),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: Some("/req".to_string()),
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
