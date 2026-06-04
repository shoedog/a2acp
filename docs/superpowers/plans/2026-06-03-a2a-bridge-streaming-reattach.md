# Streaming Reattach Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** A detached workflow's live progress is re-subscribable over SSE via the A2A-standard `SubscribeToTask` method — a state **snapshot** (from the durable store) + live **deltas** (from a per-task broadcast hub), with a `Last-Event-ID` cursor (cursor-less = full, cursor = only-new).

**Architecture:** Durable-first. Every node event is persisted (allocating a per-task `seq` in one SQLite transaction) BEFORE it is published to an in-memory `tokio::broadcast` hub — so every SSE `id` is already-committed state. `SubscribeToTask` reads a `progress_snapshot` from the store (catch-up, restart-safe), emits a `SnapshotComplete` sentinel, then live-tails the hub. The cursor (`Last-Event-ID`) filters both. The executor is untouched; the change rides the existing `WorkflowSink`/`drain_workflow` + `TaskStore` ports.

**Tech Stack:** Rust, tokio (`sync::broadcast`), rusqlite (`unchecked_transaction`), axum SSE, the existing `WorkflowSink`/`TaskStore`/detached-runner from W3a/W3b.

**Spec:** `docs/superpowers/specs/2026-06-03-a2a-bridge-streaming-reattach-design.md` (rev2, dual-designed + dual-reviewed).
**Branch:** `feat/reattach` off `main`.

**Grounding facts (confirmed against the code + the reviews):**
- `WorkflowSink` trait (`bridge-a2a-inbound/src/workflow_sink.rs:12`): `node_started(&mut,node)->Result`, `node_finished(&mut,node,ok,output)->Result`, `terminal(&mut,outcome,output)->Result`, `error(&mut,err)->Result` — all `pub(crate)`. `drain_workflow(stream,&mut sink)->Result<bool,_>` calls each, returns `Ok(terminal_seen)` / `Err` on first sink error.
- The detached runner `spawn_detached_workflow` (server.rs ~1170): drains over `TaskStoreSink`; **after** drain it writes terminal — `Ok(true)`→`sink.take()`→`set_terminal(captured)`; `Ok(false)`→`set_terminal(Failed,"workflow ended without terminal")`; `Err`→`set_terminal(Failed,"checkpoint write failed")`; then `fin.done=true; workflow_cancels.remove`. `Finalizer{store,task,cancels,done}` Drop (workflow_sink.rs:125) spawns `set_terminal(Failed,"runner ended without terminal")` + `cancels.remove`.
- Dispatch (server.rs:538): `m == methods::SEND_STREAMING_MESSAGE || m == methods::SUBSCRIBE_TO_TASK => stream_message(...)`. `stream_message(srv,headers,id,params)->Response` returns an axum SSE `Response` (gate→produce events→`sse_event_stream`). `methods::SUBSCRIBE_TO_TASK == "SubscribeToTask"`, valid + streaming in a2a-lf 0.3.0.
- `TaskStore` (bridge-core/task_store.rs): `put_node_checkpoint`/`node_checkpoints`/`claim_resume_attempt` (the latter uses `unchecked_transaction` under the `Arc<Mutex<Connection>>`, sqlite.rs ~484 — the model for the new transactional methods). `task_node_checkpoints(task_id,node_id,output,ok,ts)` PK `(task_id,node_id)`. W3b additive migration = `migrate_tasks_columns` (sqlite.rs ~127, pragma table_info-guarded ALTER).
- W3b resume: `resume_working_tasks` spawns `spawn_detached_workflow(...ctx)`; the executor re-emits `NodeStarted` for unseeded nodes (executor.rs ~305). Tests: `-p bridge-a2a-inbound` (workflow_producer.rs); `-p a2a-bridge` has no `--lib`. Coverage floors 85/90/90 after `cargo llvm-cov clean --workspace`. `cargo check --workspace` after EVERY task.
- **THREE `TaskStore` impls exist** (verified `grep 'impl .*TaskStore for'`): `MemoryTaskStore` (task_store.rs:160), `SqliteStore` (sqlite.rs:327), and the test fake **`FailingCheckpointStore`** (`crates/bridge-a2a-inbound/tests/workflow_producer.rs:1750`) whose `put_node_checkpoint`→`Err(StoreFailure)` is the DoD-3 write-failure oracle (test `detached_runner_checkpoint_write_failure_fails_task`, :1897). `WorkflowOutcome` (executor.rs:34) derives only `Debug,Clone,PartialEq,Eq` — **NOT `Serialize`**. The a2a-lf 0.3.0 wire request is `SubscribeToTaskRequest { pub id: TaskId, .. }` (types.rs:571) — the task id field is **`id`**, NOT `taskId`.

---

## Dual-review rev2 — folded findings (Codex gpt-5.5 + Claude opus-4-8)
Both reviews **endorsed the architecture spine** (subscribe-before-snapshot verified race-free; transactional seq mirrors `claim_resume_attempt`; upsert idempotency justified by executor.rs:306/321; dispatch-split safe; all 4 `spawn_detached_workflow` call sites covered). All findings are **hardening**, folded below:
- **B1 (blocker, Task 1):** `FailingCheckpointStore` is a 3rd `TaskStore` impl compiled by `clippy --all-targets` → the 4 new methods MUST be added in Task 1 (both reviewers).
- **I1 (Task 1+5):** the write-failure injection MOVES to `put_node_checkpoint_sequenced` (the sink no longer calls `put_node_checkpoint`), else the DoD-3 oracle silently goes green-for-wrong-reason.
- **I2 (Task 7+8):** cursor-less must be **distinct from `K=0`** (`Option<i64>`/sentinel `-1`) so a NULL-seq(=0) legacy checkpoint is INCLUDED in a cursor-less snapshot (DoD 4); add a delivery test.
- **I3 (Task 7):** read the wire param **`id`** (alias `taskId`), then `bridge_core::TaskId::parse` — `taskId` alone breaks A2A-standard interop (the spec's stated rationale).
- **I4 (Tasks 8/9):** assert an **ordered `Vec<(seq,kind)>` equality**, NOT a `HashSet` — a set silently absorbs a duplicate (the false-positive class this project has been bitten by). Supersedes the spec's "delivered SET" wording.
- **I5 (Task 9):** terminal-during-snapshot — after subscribe-first, if the post-subscribe snapshot is **already terminal**, take the terminal branch (snapshot + `SnapshotComplete` + `Terminal` if `>K` + close); do NOT rely on `rx` `Closed` (spec §4.3). Deterministic test.
- **I6 (Task 5):** convert **ALL** detached terminal transitions (sink-owned + no-terminal/Err arms + no-executor `server.rs:~1191` + resume short-circuit `~1467` + unknown-workflow `~1823`) to sequenced terminal (+ broadcast + hub-removal where a hub was inserted), else `terminal_seq` stays NULL and Task 8 omits `Terminal`; the no-executor arm also leaks the hub.
- **I7 (Task 9):** add the DoD-10 **broadcast-lag** deterministic test (retryable frame + close → reconnect re-snapshots).
- **M1 (Task 5/6):** Finalizer-clobber window — set `fin.done=true` BEFORE the `remove_hub` await AND make the Finalizer terminal write **conditional on still-Working** (cancel_if_working-style).
- **M2 (Task 5/6):** durable-first in the failure arms — publish `Terminal` only AFTER the sequenced write succeeds, using its returned seq.
- **M3 (Task 3):** the frame can't `derive(Serialize)` holding `WorkflowOutcome` → carry a local `TerminalOutcome` (Serialize) mapped from it; decide the wire shape in Task 3.
- **M4 (Task 2):** migration test must pre-create a POPULATED legacy 5-col `task_node_checkpoints` (exercises ALTER-add-seq-on-data + feeds the NULL-seq inclusion test); add a resume-continuation seq unit test (DoD 2).
- **M5 (Task 7):** add a falsifiable non-workflow/sync-task → not-found test.

---

## File Structure
- **Modify** `crates/bridge-core/src/task_store.rs` — `TaskProgressSnapshot` type + 4 trait methods + `MemoryTaskStore`.
- **Modify** `crates/bridge-store/src/sqlite.rs` — migration (cols + `task_node_starts`) + the 4 real impls (transactional seq).
- **Create** `crates/bridge-a2a-inbound/src/reattach.rs` — `TaskProgressHub`, `WorkflowProgressFrame`.
- **Modify** `crates/bridge-a2a-inbound/src/lib.rs` — `mod reattach;`.
- **Modify** `crates/bridge-a2a-inbound/src/workflow_sink.rs` — `DetachedProgressSink`; extend `Finalizer` (hub cleanup).
- **Modify** `crates/bridge-a2a-inbound/src/server.rs` — `progress_hubs` field + builder; hub-before-spawn; runner terminal restructure; split `SubscribeToTask` dispatch + the `subscribe_to_task` handler.
- **Modify** `bin/a2a-bridge/src/main.rs` — `task watch <id>` CLI.
- **Create** `docs/adr/0015-streaming-reattach.md`.

---

## Phase A — Durable seq substrate (store first)

### Task 1: `TaskStore` seq methods + `TaskProgressSnapshot` + `MemoryTaskStore` + `SqliteStore` stubs

**Files:** `crates/bridge-core/src/task_store.rs`; `crates/bridge-store/src/sqlite.rs` (compile-stubs only — real impls Task 2).

- [ ] **Step 1: Failing test (bridge-core).** Append to the task_store tests:
```rust
    #[tokio::test]
    async fn seq_methods_roundtrip_memory() {
        let s = MemoryTaskStore::new();
        let t = TaskId::parse("t").unwrap();
        s.create(&working_record(&t)).await.unwrap(); // a helper that builds a Working TaskRecord (reuse the existing test helper `rec`/build one)
        let s1 = s.record_node_started(&t, &NodeId::parse("a").unwrap(), 1).await.unwrap();
        let s2 = s.put_node_checkpoint_sequenced(&t, &NodeId::parse("a").unwrap(), "OUT", true, 2).await.unwrap();
        assert!(s2 > s1, "seq is monotonic");
        let s3 = s.set_terminal_sequenced(&t, TaskRecordStatus::Completed, Some("R"), None, 3).await.unwrap();
        assert!(s3 > s2);
        let snap = s.progress_snapshot(&t).await.unwrap();
        assert_eq!(snap.cut_seq, s3);
        assert_eq!(snap.terminal_seq, Some(s3));
        assert_eq!(snap.checkpoints.len(), 1);
        assert!(snap.starts.is_empty(), "the start row was cleared on finish");
        // idempotent re-start (resume re-emit): no error, new seq
        s.create(&working_record(&TaskId::parse("t2").unwrap())).await.unwrap();
        let t2 = TaskId::parse("t2").unwrap();
        let a = s.record_node_started(&t2, &NodeId::parse("x").unwrap(), 1).await.unwrap();
        let b = s.record_node_started(&t2, &NodeId::parse("x").unwrap(), 2).await.unwrap();
        assert!(b > a, "re-start upserts a fresh seq, no PK error");
    }
```
   (Confirm the existing test-helper for building a `Working` `TaskRecord` — reuse it; `working_record` above is a stand-in for that helper.)
- [ ] **Step 2: Run → fails.** `cargo test -p bridge-core seq_methods_roundtrip_memory`
- [ ] **Step 3: Implement (bridge-core).**
  - Add `pub struct TaskProgressSnapshot { pub status: TaskRecordStatus, pub result: Option<String>, pub error: Option<String>, pub checkpoints: Vec<(NodeId, String, bool, i64)>, pub starts: Vec<(NodeId, i64)>, pub terminal_seq: Option<i64>, pub cut_seq: i64 }` (`checkpoints` ordered by seq; tuple = `(node, output, ok, seq)`).
  - Add to the `TaskStore` trait (async, `Result<_, BridgeError>`): `record_node_started(&self, task: &TaskId, node: &NodeId, ts: i64) -> Result<i64, BridgeError>`; `put_node_checkpoint_sequenced(&self, task: &TaskId, node: &NodeId, output: &str, ok: bool, ts: i64) -> Result<i64, BridgeError>`; `set_terminal_sequenced(&self, task: &TaskId, status: TaskRecordStatus, result: Option<&str>, error: Option<&str>, ts: i64) -> Result<i64, BridgeError>`; `progress_snapshot(&self, task: &TaskId) -> Result<TaskProgressSnapshot, BridgeError>`.
  - Implement on `MemoryTaskStore`: a per-task `last_event_seq` counter (a field in the row or a sibling map); `record_node_started` upserts a `starts: HashMap<(task,node), (seq,ts)>` (replace on re-start); `put_node_checkpoint_sequenced` writes the checkpoint with the seq + removes the start row; `set_terminal_sequenced` sets status/result/error + a `terminal_seq` + clears the task's starts; `progress_snapshot` reads it all (checkpoints sorted by seq, cut_seq = last_event_seq). (Extend the existing checkpoint map to store the seq.)
- [ ] **Step 4: SqliteStore compile-stubs.** Add the 4 methods to `impl TaskStore for SqliteStore`, each body `Err(BridgeError::StoreFailure) // TODO(Task 2)`. Add `progress_snapshot` returning the same. (Keeps the workspace green; Task 2 fills them.)
- [ ] **Step 4b: Update the THIRD impl — `FailingCheckpointStore` (B1/I1, `crates/bridge-a2a-inbound/tests/workflow_producer.rs:1750`).** This test fake "behaves like memory EXCEPT `put_node_checkpoint`→`Err`" and is compiled by `clippy --all-targets` (a plain `cargo check --workspace` would NOT catch it). Add the 4 new methods, **delegating to its inner memory store EXCEPT `put_node_checkpoint_sequenced` which returns `Err(BridgeError::StoreFailure)`** — this MOVES the write-failure injection to the sequenced method the sink will call (Task 5), so the DoD-3 oracle `detached_runner_checkpoint_write_failure_fails_task` (:1897) keeps asserting `Failed` for the right reason: `record_node_started`/`set_terminal_sequenced`/`progress_snapshot` → inner; `put_node_checkpoint_sequenced` → `Err`. (Confirm there are exactly 3 impls: `grep -rn 'impl .*TaskStore for' crates/ bin/`.)
- [ ] **Step 5: Run → green.** `cargo test -p bridge-core`, `cargo clippy --workspace --all-targets -- -D warnings` (compiles the test target → catches a missing impl), `cargo check --workspace`.
- [ ] **Step 6: Commit** (NO trailer): `git commit -m "feat(core): TaskStore seq methods + TaskProgressSnapshot (Memory + sqlite stubs + FailingCheckpointStore)"`

### Task 2: `SqliteStore` migration + real seq impls

**Files:** `crates/bridge-store/src/sqlite.rs`.

- [ ] **Step 1: Failing tests.** Append:
```rust
    #[tokio::test]
    async fn sqlite_seq_and_snapshot() {
        let s = SqliteStore::open_in_memory().unwrap();
        let t = TaskId::parse("t").unwrap();
        s.create(&trec(&t)).await.unwrap();
        let s1 = s.record_node_started(&t, &NodeId::parse("a").unwrap(), 1).await.unwrap();
        let s2 = s.put_node_checkpoint_sequenced(&t, &NodeId::parse("a").unwrap(), "OUT", true, 2).await.unwrap();
        assert!(s2 > s1);
        let snap = s.progress_snapshot(&t).await.unwrap();
        assert_eq!(snap.checkpoints[0].3, s2); // seq carried
        assert!(snap.starts.is_empty());        // start cleared on finish
        // re-start upsert (resume): no PK error
        let r1 = s.record_node_started(&t, &NodeId::parse("b").unwrap(), 3).await.unwrap();
        let r2 = s.record_node_started(&t, &NodeId::parse("b").unwrap(), 4).await.unwrap();
        assert!(r2 > r1);
        let term = s.set_terminal_sequenced(&t, TaskRecordStatus::Completed, Some("R"), None, 5).await.unwrap();
        assert_eq!(s.progress_snapshot(&t).await.unwrap().terminal_seq, Some(term));
    }

    #[tokio::test]
    async fn null_seq_legacy_checkpoint_is_seq_zero() {
        // a checkpoint written by the LEGACY put_node_checkpoint (NULL seq) appears as seq 0 in the snapshot.
        let s = SqliteStore::open_in_memory().unwrap();
        let t = TaskId::parse("t").unwrap();
        s.create(&trec(&t)).await.unwrap();
        s.put_node_checkpoint(&t, &NodeId::parse("old").unwrap(), "O", true, 1).await.unwrap(); // legacy, no seq
        let snap = s.progress_snapshot(&t).await.unwrap();
        assert_eq!(snap.checkpoints.iter().find(|c| c.0.as_str()=="old").unwrap().3, 0);
    }

    #[tokio::test]
    async fn seq_continues_across_resume_seed() {
        // DoD-2: a new event after seeded checkpoints continues the seq (no unit test existed — only manual smoke).
        let s = SqliteStore::open_in_memory().unwrap();
        let t = TaskId::parse("t").unwrap();
        s.create(&trec(&t)).await.unwrap();
        let a = s.put_node_checkpoint_sequenced(&t, &NodeId::parse("a").unwrap(), "A", true, 1).await.unwrap();
        // simulate a fresh runner instance (same store/task) continuing
        let b = s.record_node_started(&t, &NodeId::parse("b").unwrap(), 2).await.unwrap();
        assert!(b > a, "seq continues across a resumed run, not reset");
    }
```
   (Reuse the `trec` helper from the W3b sqlite tests. **M4:** extend `migration_on_old_schema_db_*` so it (a) PRE-CREATES a populated legacy 5-column `task_node_checkpoints` (`task_id,node_id,output,ok,ts`) with one row BEFORE opening the store — exercising the ALTER-add-`seq`-on-a-populated-table path, not just `CREATE … IF NOT EXISTS` fresh — then (b) asserts the migration adds `tasks.last_event_seq`/`tasks.terminal_seq`/`task_node_checkpoints.seq` + creates `task_node_starts`, and (c) `progress_snapshot` returns that pre-existing row at `seq 0`.)
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement.**
  - Migration: in `migrate_tasks_columns` add `tasks.last_event_seq INTEGER NOT NULL DEFAULT 0`, `tasks.terminal_seq INTEGER`; add a SECOND guarded ALTER for `task_node_checkpoints.seq INTEGER` (pragma table_info on `task_node_checkpoints`); in `create_schema` add `CREATE TABLE IF NOT EXISTS task_node_starts(task_id TEXT NOT NULL, node_id TEXT NOT NULL, seq INTEGER NOT NULL, ts INTEGER NOT NULL, PRIMARY KEY(task_id,node_id), FOREIGN KEY(task_id) REFERENCES tasks(id) ON DELETE CASCADE)`.
  - A private `fn alloc_seq(conn) -> rusqlite::Result<i64>` is NOT needed separately — each method runs ONE `conn.unchecked_transaction()`: `UPDATE tasks SET last_event_seq = last_event_seq + 1 WHERE id=?1; SELECT last_event_seq FROM tasks WHERE id=?1` (the new seq), then the state write, then `tx.commit()`, return the seq.
  - `record_node_started`: in the txn, alloc seq, `INSERT INTO task_node_starts(task_id,node_id,seq,ts) VALUES(?,?,?,?) ON CONFLICT(task_id,node_id) DO UPDATE SET seq=excluded.seq, ts=excluded.ts`, commit, return seq.
  - `put_node_checkpoint_sequenced`: alloc seq, `INSERT INTO task_node_checkpoints(task_id,node_id,output,ok,ts,seq) VALUES(?,?,?,?,?,?)` (write-once plain INSERT, per W3b), `DELETE FROM task_node_starts WHERE task_id=? AND node_id=?`, commit, return seq.
  - `set_terminal_sequenced`: alloc seq, `UPDATE tasks SET status=?, result=?, error=?, updated_ms=?, terminal_seq=<seq> WHERE id=?`, `DELETE FROM task_node_starts WHERE task_id=?`, commit, return seq.
  - `progress_snapshot`: read the task row (status/result/error/terminal_seq/last_event_seq), `SELECT node_id, output, ok, COALESCE(seq,0) FROM task_node_checkpoints WHERE task_id=? ORDER BY COALESCE(seq,0)`, `SELECT node_id, seq FROM task_node_starts WHERE task_id=? ORDER BY seq`; build `TaskProgressSnapshot{ cut_seq=last_event_seq, ... }`.
- [ ] **Step 4: Run → green.** `cargo test -p bridge-store`, clippy, `cargo check --workspace`.
- [ ] **Step 5: Commit** `git commit -m "feat(store): transactional per-task seq + task_node_starts + progress_snapshot (NULL-seq=0)"`

---

## Phase B — Hub + frame (inbound; no behavior change)

### Task 3: `TaskProgressHub` + `WorkflowProgressFrame` + `progress_hubs` field

**Files:** Create `crates/bridge-a2a-inbound/src/reattach.rs`; modify `lib.rs`, `server.rs`.

- [ ] **Step 1: Failing test** (in `reattach.rs`):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn hub_broadcasts_to_late_subscriber() {
        let hub = TaskProgressHub::new();
        let mut rx = hub.subscribe();
        hub.publish(WorkflowProgressFrame { v: 1, seq: 5, phase: Phase::Live,
            kind: FrameKind::NodeFinished { node: "a".into(), ok: true, output: "o".into() } });
        let f = rx.recv().await.unwrap();
        assert_eq!(f.seq, 5);
    }
}
```
- [ ] **Step 2: Run → fails.** `cargo test -p bridge-a2a-inbound hub_broadcasts`
- [ ] **Step 3: Implement** `reattach.rs`. **M3:** `WorkflowOutcome` is NOT `Serialize` (executor.rs:34 derives only `Debug,Clone,PartialEq,Eq`), so the frame canNOT hold it and `derive(Serialize)`. Carry a **local `TerminalOutcome`** (Serialize, lowercase wire tag) mapped from `WorkflowOutcome`, and derive `Serialize` on the frame types so Task 8 serializes by `serde_json` (no hand-rolling). Lock the wire shape here:
```rust
use serde::Serialize;
use tokio::sync::broadcast;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase { Snapshot, Live }

/// Serializable mirror of bridge_workflow::executor::WorkflowOutcome (which is not Serialize).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TerminalOutcome { Completed, Failed, Canceled }

impl TerminalOutcome {
    pub(crate) fn from_workflow(o: &bridge_workflow::executor::WorkflowOutcome) -> Self {
        use bridge_workflow::executor::WorkflowOutcome as W;
        match o { W::Completed => Self::Completed, W::Failed => Self::Failed, W::Canceled => Self::Canceled }
        // NOTE: match the ACTUAL WorkflowOutcome variants at executor.rs:34 — adjust if it carries data.
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum FrameKind {
    NodeStarted { node: String },
    NodeFinished { node: String, ok: bool, output: String },
    SnapshotComplete,
    Terminal { outcome: TerminalOutcome, output: String },
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct WorkflowProgressFrame { pub v: u8, pub seq: i64, pub phase: Phase, pub kind: FrameKind }

pub(crate) struct TaskProgressHub { tx: broadcast::Sender<WorkflowProgressFrame> }

impl TaskProgressHub {
    pub(crate) fn new() -> Self { let (tx, _) = broadcast::channel(256); Self { tx } }
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<WorkflowProgressFrame> { self.tx.subscribe() }
    pub(crate) fn publish(&self, f: WorkflowProgressFrame) { let _ = self.tx.send(f); } // best-effort
}
```
  - `lib.rs`: `mod reattach;`. `server.rs`: add `progress_hubs: Arc<tokio::sync::Mutex<std::collections::HashMap<TaskId, Arc<reattach::TaskProgressHub>>>>` to `InboundServer` (init empty in `new`).
- [ ] **Step 4: Run → green.** `cargo test -p bridge-a2a-inbound`, clippy, check.
- [ ] **Step 5: Commit** `git commit -m "feat(inbound): TaskProgressHub + WorkflowProgressFrame + progress_hubs (no behavior change)"`

---

## Phase C — DetachedProgressSink + terminal ownership + finalizer (RISKIEST)

### Task 4: `DetachedProgressSink` (persist-via-sequenced + publish)

**Files:** `crates/bridge-a2a-inbound/src/workflow_sink.rs`.

- [ ] **Step 1: Failing test.** A test driving a hand-built `WorkflowStream` (NodeStarted a, NodeFinished a, Terminal Completed) through `drain_workflow` with a `DetachedProgressSink` over a `MemoryTaskStore` + a hub; assert the store has the checkpoint (with seq) + `terminal_seq`, AND a subscriber received frames `[NodeStarted, NodeFinished, Terminal]` with monotonic seqs. Also: a store whose sequenced write errors → `drain_workflow` returns `Err` (W3b abort contract).
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement** `DetachedProgressSink { store: Arc<dyn TaskStore>, task: TaskId, hub: Arc<TaskProgressHub> }` implementing `WorkflowSink`:
  - `node_started(node)`: `let seq = self.store.record_node_started(&self.task, &NodeId::parse(node)?, now_ms()).await?;` then `self.hub.publish(frame(seq, Live, NodeStarted{node}))`.
  - `node_finished(node, ok, output)`: `let seq = self.store.put_node_checkpoint_sequenced(&self.task, &NodeId::parse(node)?, output, ok, now_ms()).await?;` then publish `NodeFinished{node,ok,output}`.
  - `terminal(outcome, output)`: `let seq = self.store.set_terminal_sequenced(&self.task, status_of(outcome), result_of(outcome,&output), error_of(outcome,&output), now_ms()).await?;` then publish `Terminal{ outcome: TerminalOutcome::from_workflow(&outcome), output }` **using the returned `seq`** (M2 durable-first — never publish a seq the store didn't commit). (Map outcome→status/result/error exactly as the W3b `TaskStoreSink::terminal`/the runner did — Completed→result, Failed→error, Canceled.)
  - Publish is AFTER the durable write (durable-first), carrying the committed `seq`; a write `Err` propagates (aborts the drain → DoD-3, and via `FailingCheckpointStore::put_node_checkpoint_sequenced`→`Err`). `NodeId::parse` error → `BridgeError`.
- [ ] **Step 4: Run → green.** `cargo test -p bridge-a2a-inbound`, clippy, check.
- [ ] **Step 5: Commit** `git commit -m "feat(inbound): DetachedProgressSink — persist-via-sequenced then publish (durable-first)"`

### Task 5: wire the sink into the runner + terminal-ownership restructure + hub-before-spawn

**Files:** `crates/bridge-a2a-inbound/src/server.rs`.

- [ ] **Step 1: Failing tests.** (a) A detached submit (the W3b harness) → after completion, a subscriber that attached to the task's hub (via `srv.progress_hubs`) saw a `Terminal` frame; AND the W3b assertions (status Completed, checkpoints present) still hold; the hub is REMOVED from `progress_hubs` after terminal. (b) **DoD-3 oracle (I1):** `detached_runner_checkpoint_write_failure_fails_task` STILL goes `Failed` — now because `FailingCheckpointStore::put_node_checkpoint_sequenced`→`Err` (added in Task 1) aborts the drain. (c) **I6:** a detached submit routed to an UNKNOWN workflow id → terminal task has a non-NULL `terminal_seq` (a later `SubscribeToTask` must be able to emit `Terminal`), and the no-executor / resume-short-circuit paths likewise write a sequenced terminal + leave no hub in `progress_hubs`.
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement.**
  - A shared helper `async fn finalize_detached(store, progress_hubs, task, status, result, error, hub: Option<Arc<TaskProgressHub>>)`: `let seq = store.set_terminal_sequenced(task, status, result, error, now_ms()).await?;` then (if `hub`) `hub.publish(Terminal{ outcome: <map status>, output })` with that `seq` (M2 durable-first) + `progress_hubs.lock().await.remove(task)`. Use it for EVERY detached terminal transition.
  - In `spawn_detached_workflow`: build the sink as `DetachedProgressSink::new(srv.task_store.clone(), task.clone(), hub.clone())`. The runner's post-drain logic CHANGES: the sink's `terminal()` already wrote the sequenced terminal + published, so `Ok(true)` → do NOT write terminal again; **set `fin.done=true` FIRST (M1 — before any await), then** `remove_hub`. `Ok(false)` (no terminal) → `finalize_detached(Failed,"workflow ended without terminal", Some(hub))` then `fin.done=true`. `Err` → `finalize_detached(Failed,"checkpoint write failed", Some(hub))` then `fin.done=true`. (Delete the old `sink.take()`→`set_terminal` capture path.)
  - **I6 — convert the OTHER detached terminal paths** to `finalize_detached` (so `terminal_seq` is never NULL going forward, and the hub never leaks): the no-executor early-return (`server.rs:~1188-1204`, `Some(hub)`), the resume short-circuit (`~1467`), the unknown-workflow reject (`~1823`). For a path that rejects BEFORE the hub is inserted (unknown-workflow pre-spawn), pass `hub: None` (no subscribers yet) — it still writes a sequenced terminal.
  - **Hub inserted BEFORE spawn:** the CALLERS create the hub + insert it into `srv.progress_hubs` BEFORE calling `spawn_detached_workflow`, and pass it in. Update the signature `spawn_detached_workflow(..., hub: Arc<TaskProgressHub>)`. Caller sites: the `unary_message` `RouteTarget::Workflow` arm (fresh submit) and `resume_working_tasks` (resume) both `let hub = Arc::new(TaskProgressHub::new()); srv.progress_hubs.lock().await.insert(task.clone(), hub.clone());` then pass `hub`. The test seams pass a fresh hub.
  - A small helper `remove_hub(srv, &task)` = `srv.progress_hubs.lock().await.remove(&task);`.
- [ ] **Step 4: Run → green.** `cargo test -p bridge-a2a-inbound` (ALL W3b detached/resume/cancel/panic tests green — the terminal restructure must not regress them; the write-failure oracle goes green via the sequenced method), clippy, `cargo check --workspace`.
- [ ] **Step 5: Commit** `git commit -m "feat(inbound): detached runner uses DetachedProgressSink; sequenced terminal on ALL detached paths; hub before spawn"`

### Task 6: `Finalizer` hub cleanup (+ sequenced Failed + broadcast)

**Files:** `crates/bridge-a2a-inbound/src/workflow_sink.rs`, `server.rs`.

- [ ] **Step 1: Failing test.** Extend the W3b `runner_panic_finalizes_failed_no_orphan`: a subscriber attached to the hub also receives a `Terminal{Failed}` frame on a runner panic, AND the hub is removed from `progress_hubs`.
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement.** Add to `Finalizer` the fields `progress_hubs: Arc<Mutex<HashMap<TaskId,Arc<TaskProgressHub>>>>` and `hub: Arc<TaskProgressHub>`. In `Drop` (the spawned async): call the shared `finalize_detached(store, progress_hubs, &task, Failed, None, Some("runner ended without terminal"), Some(hub))` (sequenced Failed → publish `Terminal{Failed}` with the committed seq, M2 → remove hub), then `cancels.remove`. Update the `Finalizer{}` construction in `spawn_detached_workflow` to pass `progress_hubs` + `hub`. **M1:** the Finalizer only runs when `!done`, and Task 5 sets `fin.done=true` BEFORE the success-arm `remove_hub` await — so there is no await between the sink's committed terminal and `done=true`, and the Finalizer can no longer clobber a committed `Completed`. (If a conditional "Failed only if still Working" guard is later wanted as belt-and-suspenders, add it as an atomic store method in a follow-up — not required here.)
- [ ] **Step 4: Run → green.** `cargo test -p bridge-a2a-inbound` (the W3b `runner_panic_finalizes_failed_no_orphan` + cancel/resume tests all green), clippy, check.
- [ ] **Step 5: Commit** `git commit -m "feat(inbound): Finalizer writes sequenced Failed + broadcasts terminal + removes hub on panic"`

---

## Phase D — `SubscribeToTask` handler

### Task 7: split the dispatch + the handler skeleton (auth+version, `id` param, cursor, not-found)

**Files:** `crates/bridge-a2a-inbound/src/server.rs`.

- [ ] **Step 1: Failing tests.** (a) A `SubscribeToTask` JSON-RPC with a missing task id → JSON-RPC error; (b) with an unknown id → JSON-RPC error (not-found); (c) **I3 wire-conformance:** a request carrying the standard a2a-lf field `{"id": "<taskid>"}` is ACCEPTED (not rejected as missing) — a regression test that fails if the handler reads `taskId` only; (d) **M5:** a known non-workflow / session-only id that is NOT in the `TaskStore` → not-found (cannot fall through to `gate()` and start a new run). `SendStreamingMessage` still routes to `stream_message` (unchanged).
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement.** Split the dispatch arm (server.rs:538): `m if m == methods::SEND_STREAMING_MESSAGE => stream_message(...)`, and a new arm `m if m == methods::SUBSCRIBE_TO_TASK => subscribe_to_task(srv, headers, id, params).await`. Add `async fn subscribe_to_task(srv, headers, id, params) -> Response`: do the auth + A2A-version check (mirror the start of `stream_message`/`unary_message`, but do NOT call `gate()` — `gate()` would synthesize a `task-1` and start a run). **I3:** extract the task id from `params["id"]` (the a2a-lf `SubscribeToTaskRequest` field; ALSO accept `params["taskId"]` as a lenient alias), required — `None` → `bridge_err_to_jsonrpc(id, &BridgeError::InvalidRequest{field:"id"})`; then `bridge_core::TaskId::parse(...)` (the a2a-lf `TaskId` is a distinct type — parse the string). **I2:** parse the cursor as `let cursor: Option<i64> = headers.get("Last-Event-ID").and_then(|v| v.to_str().ok()?.parse().ok());` — `None` means cursor-less (include everything, incl. `seq 0`); `Some(K)` filters `seq > K`. Do NOT collapse absent→0. `srv.task_store.get(&taskId)` → `None` → JSON-RPC not-found error. (The snapshot/SSE body is Tasks 8-9 — for now, after the get, return a minimal empty SSE or a stub the next tasks replace.)
- [ ] **Step 4: Run → green.** `cargo test -p bridge-a2a-inbound`, clippy, check.
- [ ] **Step 5: Commit** `git commit -m "feat(inbound): route SubscribeToTask to a reattach handler (id param, Option cursor, not-found)"`

### Task 8: the snapshot builder (row-driven, seq-ordered, cursor) + terminal-state flow

**Files:** `crates/bridge-a2a-inbound/src/server.rs`.

- [ ] **Step 1: Failing tests.** Drive the handler + collect the SSE frames into an **ordered `Vec<(i64, &str)>`** of `(seq, kind-tag)` and assert **`Vec` EQUALITY** against the expected ordered vector (I4 — a `HashSet` silently absorbs a duplicate, the false-positive class this project has hit; a `Vec` catches dups by length + content and verifies seq ordering). Cases: (a) a TERMINAL task with checkpoints (seqs 1,2) + `terminal_seq`=3, **no cursor** → `[(1,NodeFinished),(2,NodeFinished),(maxseq,SnapshotComplete),(3,Terminal)]`, then close. (b) cursor `K=1` → only `seq>1` (the `(2,NodeFinished)`, `SnapshotComplete`, `(3,Terminal)`). (c) **I2 NULL-seq delivery:** a terminal task whose only checkpoint was written via legacy `put_node_checkpoint` (NULL seq ⇒ `seq 0`), **no cursor** → the `(0,NodeFinished)` frame IS delivered (cursor-less includes `seq 0`); and a legacy terminal with `terminal_seq=None` still emits a `Terminal` frame.
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement.** A `fn snapshot_frames(snap: &TaskProgressSnapshot, cursor: Option<i64>) -> Vec<WorkflowProgressFrame>`: a predicate `let pass = |seq: i64| cursor.map_or(true, |k| seq > k);` (**I2 — cursor-less passes everything incl. `seq 0`; never collapse to `K=0`**). From `snap.checkpoints` (each `(node,output,ok,seq)`) → `WorkflowProgressFrame{v:1, seq, phase: Phase::Snapshot, kind: FrameKind::NodeFinished{node,ok,output}}` if `pass(seq)`; from `snap.starts` → `…kind: FrameKind::NodeStarted{node}` (phase `Snapshot`) if `pass(seq)`; collect, **sort by `frame.seq`**. (Snapshot-path frames carry `phase: Snapshot`; the sink's live frames carry `phase: Live` — same kind, different delivery path.) In `subscribe_to_task` for a terminal `snap.status`: emit `snapshot_frames(snap, cursor)`, then the sentinel `WorkflowProgressFrame{v:1, seq: <max snapshot frame seq, else snap.cut_seq>, phase: Phase::Snapshot, kind: FrameKind::SnapshotComplete}` (`seq` lives on the frame; `FrameKind::SnapshotComplete` is fieldless — Task 3), then a `Terminal` frame **iff `snap.terminal_seq.map_or(true, |ts| pass(ts))`** (`None` legacy terminal ⇒ always emit; `Some(ts)` ⇒ dedup by cursor) as `WorkflowProgressFrame{v:1, seq: snap.terminal_seq.unwrap_or(0), phase: Phase::Live, kind: FrameKind::Terminal{ outcome: <map snap.status>, output: snap.result/error }}`, then close. Convert each frame to an SSE event with `id: <frame.seq>` + a JSON data body (`serde_json::to_string(&frame)` — Task 3 made them `Serialize`); reuse `stream_message`'s SSE `Response` builder (`Sse::new(stream)` / `sse_event_stream`).
- [ ] **Step 4: Run → green.** `cargo test -p bridge-a2a-inbound`, clippy, check.
- [ ] **Step 5: Commit** `git commit -m "feat(inbound): SubscribeToTask snapshot (row-driven, Option cursor incl seq0, Option terminal_seq) + terminal flow"`

### Task 9: the working-state flow (subscribe-first + cut_seq dedup + live-tail) + cursor-beyond-seq

**Files:** `crates/bridge-a2a-inbound/src/server.rs`.

- [ ] **Step 1: Failing tests.** Collect the SSE frames into an **ordered `Vec<(i64,&str)>`** and assert `Vec` EQUALITY (I4 — not a set). (a) In-flight: a Working task with a live hub; subscribe → snapshot + `SnapshotComplete` + the live deltas (publish a few frames after) to a `Terminal` → close; assert no dup, no gap, correct order. (b) Two concurrent subscribers both get the full ordered vector. (c) Empty snapshot (no nodes finished) still emits `SnapshotComplete` then live. (d) Cursor `K >= cut_seq` on a terminal task → `SnapshotComplete` then immediate close (no hang). (e) **I5 terminal-during-snapshot:** the post-subscribe `progress_snapshot` reads `status == terminal` (the task finished after `get()` but before/at the snapshot, hub may already be gone) → MUST emit snapshot + `SnapshotComplete` + `Terminal` (if it passes the cursor) + close — NOT silently close on `rx` `Closed`. (f) **I7 broadcast-lag (DoD 10):** force a lagged receiver (publish > channel-capacity frames before the consumer drains) → the live stream yields a **retryable error frame + close**; a fresh `SubscribeToTask` with the last-seen cursor re-snapshots with no lost state. Drive via a hand-built sequence through the sink+hub (deterministic; assert on the delivered ordered vector, not arrival phase).
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement.** In `subscribe_to_task`, AFTER `get()` says Working: get the task's hub from `srv.progress_hubs`; if absent → re-`get` (terminal→terminal path; still Working but no hub → retryable JSON-RPC error). **`rx = hub.subscribe()` FIRST**; THEN read `progress_snapshot` → `snap`, `cut_seq`. **I5:** if `snap.status` is now terminal, take the TERMINAL branch (Task 8: snapshot + `SnapshotComplete` + `Terminal` if it passes the cursor + close) — do not rely on `rx` `Closed`. Else (still Working) build the SSE stream as: `snapshot_frames(snap, cursor)` ++ `SnapshotComplete` ++ a live stream from `rx` that **drops frames with `seq <= max(cursor.unwrap_or(-1), cut_seq)`** (I2 — `-1` sentinel so a cursor-less subscriber keeps `seq 0`) and ends after a `Terminal`. Cursor-beyond-seq: `cursor >= snap.cut_seq` ⇒ snapshot_frames empty → still emit `SnapshotComplete`; terminal→close / working→tail (same path; never hang). Use `async_stream` / a `futures::stream` that yields the snapshot vec then bridges the broadcast `rx`, mapping **`RecvError::Lagged` → a retryable SSE error frame + close** (I7; client reconnects with its cursor and re-snapshots), `Closed` → close.
- [ ] **Step 4: Run → green.** `cargo test -p bridge-a2a-inbound`, clippy, `cargo check --workspace`.
- [ ] **Step 5: Commit** `git commit -m "feat(inbound): SubscribeToTask working-state — subscribe-first, terminal-during-snapshot, cursor dedup, lag→retryable"`

---

## Phase E — CLI

### Task 10: `task watch <id>`

**Files:** `bin/a2a-bridge/src/main.rs`.

- [ ] **Step 1: Failing test.** A `flag`/arg-parse unit test that `task watch <id> [--url] [--from <seq>]` parses (mirror the existing `task get` subcommand parse test if present; else a thin parse test).
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement.** Add a `watch` arm to `task_cmd`: open an SSE stream to `<url>` with method `SubscribeToTask` (a streaming POST — reuse the `rpc_call` body shape but read the SSE response line-by-line via `reqwest`'s streaming `bytes_stream`), send `Last-Event-ID: <from>` if `--from`, and print each frame's data (and track the last `id` so a manual reconnect can resume). Keep it simple: print `data:` lines until the stream closes.
- [ ] **Step 4: Run → green.** `cargo test -p a2a-bridge`, `cargo build -p a2a-bridge`, clippy, check.
- [ ] **Step 5: Commit** `git commit -m "feat(bin): task watch <id> — SubscribeToTask SSE client (Last-Event-ID)"`

---

## Phase F — verification, live gate, ADR

### Task 11: full sweep + coverage
- [ ] `cargo fmt --all && cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` → all green.
- [ ] `cargo llvm-cov clean --workspace` then floors `--workspace 85`, `--package bridge-core 90`, `--package bridge-workflow 90`. Top up if dipped (esp. the snapshot builder + the working-state stream). Commit additions.

### Task 12: gated live (smoke — real agents)
- [ ] Build; a `/tmp/reattach-serve/a2a-bridge.toml` (codex+claude, `code-review` workflow, `[store] path`). `serve &`. `submit code-review --input <diff>`. From a SECOND client, `task watch <taskid>` mid-flight → see the snapshot (codex/claude done so far) + `SnapshotComplete` + live deltas (synth) + Terminal → Completed. Then: submit again, `kill` serve after one reviewer's checkpoint, restart, `task watch <taskid>` → snapshot reconstructs the done reviewer + live-tails the resumed run to Completed. Record in the ADR. (Manual smoke gate, not CI.)

### Task 13: ADR-0015
- [ ] Write `docs/adr/0015-streaming-reattach.md`: the decision (SubscribeToTask = snapshot+deltas, durable-first, per-task seq, cursor); the dual-design + dual-review provenance (the SubscribeToTask-not-tasks/resubscribe correction, the terminal-ownership move, the exactly-once boundary); the live-smoke result; follow-ons (token-level streaming, full event-log, cross-serve). Commit with the controller trailer.

---

## DoD §(spec) → tasks
| DoD | Task |
|-----|------|
| 1 (SubscribeToTask routed via `id` param; not-found incl. non-workflow/sync; SendStreamingMessage unchanged) | 7 |
| 2 (transactional seq; migration on POPULATED legacy table; resume-continuation unit test; upsert idempotent) | 1, 2 |
| 3 (DetachedProgressSink persist+publish; W3b write-failure via the SEQUENCED method; sink owns terminal) | 1, 4, 5 |
| 4 (snapshot row-driven + seq-ordered; NULL-seq=0 DELIVERED cursor-less; Option cursor) | 2, 8 |
| 5 (in-flight snapshot+SnapshotComplete+deltas; ordered-Vec no dup/gap; terminal-during-snapshot) | 9 |
| 6 (terminal snapshot+terminal close; two subscribers; Option terminal_seq) | 8, 9 |
| 7 (empty snapshot sentinel; cursor>=seq no hang) | 9 |
| 8 (cursor only-new; absent≠K=0) | 7, 8, 9 |
| 9 (restart-then-resubscribe; ALL detached terminal paths sequenced; finalizer hub cleanup, no clobber) | 5, 6, 12 |
| 10 (lag → retryable error frame + close; reconnect re-snapshots) | 9 |
| 11 (CLI; coverage; gated-live smoke; ADR) | 10, 11, 12, 13 |

## Notes for the implementer
- **Riskiest = Task 5** (terminal-ownership restructure). The sink OWNS the sequenced terminal write; the runner's old `sink.take()`→`set_terminal` path is DELETED. **ALL** detached terminal transitions (sink, no-terminal/Err arms, no-executor, resume-short-circuit, unknown-workflow) go through `finalize_detached` so `terminal_seq` is never NULL and the hub never leaks. Every W3b detached/resume/cancel/panic test is the regression oracle — keep them green; the write-failure oracle now fires via `put_node_checkpoint_sequenced`.
- **Durable-first** is the invariant: persist (allocate seq) THEN publish with that committed seq, everywhere — including the failure arms (M2). Every SSE `id` is committed state.
- **Test on the delivered ORDERED `Vec<(seq,kind)>`** vs the cursor (assert `Vec` equality — a `HashSet` can't falsify a duplicate, the false-positive class this project has hit), NOT phase-of-arrival; drive a hand-built `WorkflowEvent` stream through the sink+hub (mirror `drain_awaits_node_finished_before_next`) for determinism.
- **Cursor is `Option<i64>`**, absent ≠ `K=0`: cursor-less includes `seq 0` (legacy NULL-seq checkpoints); the live dedup uses `cursor.unwrap_or(-1)`.
- **Wire param is `id`** (a2a-lf `SubscribeToTaskRequest`), with `taskId` as a lenient alias; `bridge_core::TaskId::parse`. The frame carries a local `TerminalOutcome` (Serialize), NOT `WorkflowOutcome`.
- **Hub before spawn** (fresh + resume); **Finalizer removes the hub** (Task 6) + `fin.done` is set before the success-arm hub-removal await (no clobber, M1).
- `record_node_started` is an **upsert** (resume re-emits NodeStarted). `progress_snapshot` treats **NULL-seq checkpoints as seq 0** and **`terminal_seq` as `Option`** (None legacy ⇒ always emit Terminal).
- `run cargo check --workspace` after every task; `clippy --workspace --all-targets` (compiles the test target — catches a missing `TaskStore` impl). Firewall: design from bridge ports + A2A `SubscribeToTask` semantics; a2a-local-bridge did not inform it. Controller docs (this plan, ADR-0015) carry the `Co-Authored-By: Claude Opus 4.8 (1M context)` trailer; task commits do NOT. Coverage after `cargo llvm-cov clean --workspace`.
