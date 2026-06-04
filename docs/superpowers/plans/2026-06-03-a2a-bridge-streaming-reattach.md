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
- [ ] **Step 5: Run → green.** `cargo test -p bridge-core`, `cargo build --workspace 2>&1 | head` (surface any other `impl TaskStore` — the API backend doesn't impl TaskStore, only stores do; confirm), `cargo clippy --workspace --all-targets -- -D warnings`, `cargo check --workspace`.
- [ ] **Step 6: Commit** (NO trailer): `git commit -m "feat(core): TaskStore seq methods + TaskProgressSnapshot (MemoryTaskStore; sqlite stubs)"`

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
```
   (Reuse the `trec` helper from the W3b sqlite tests. Extend `migration_on_old_schema_db_*` to assert the new columns migrate + `task_node_starts` exists.)
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
- [ ] **Step 3: Implement** `reattach.rs`:
```rust
use tokio::sync::broadcast;

#[derive(Clone, Debug)]
pub(crate) enum Phase { Snapshot, Live }

#[derive(Clone, Debug)]
pub(crate) enum FrameKind {
    NodeStarted { node: String },
    NodeFinished { node: String, ok: bool, output: String },
    SnapshotComplete,
    Terminal { outcome: bridge_workflow::executor::WorkflowOutcome, output: String },
}

#[derive(Clone, Debug)]
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
  - `terminal(outcome, output)`: `let seq = self.store.set_terminal_sequenced(&self.task, status_of(outcome), result_of(outcome,&output), error_of(outcome,&output), now_ms()).await?;` then publish `Terminal{outcome,output}`. (Map outcome→status/result/error exactly as the W3b `TaskStoreSink::terminal`/the runner did — Completed→result, Failed→error, Canceled.)
  - Publish is AFTER the durable write (durable-first); a write `Err` propagates (aborts the drain). `NodeId::parse` error → `BridgeError`.
- [ ] **Step 4: Run → green.** `cargo test -p bridge-a2a-inbound`, clippy, check.
- [ ] **Step 5: Commit** `git commit -m "feat(inbound): DetachedProgressSink — persist-via-sequenced then publish (durable-first)"`

### Task 5: wire the sink into the runner + terminal-ownership restructure + hub-before-spawn

**Files:** `crates/bridge-a2a-inbound/src/server.rs`.

- [ ] **Step 1: Failing test.** A detached submit (the W3b harness) → after completion, a subscriber that attached to the task's hub (via `srv.progress_hubs`) saw a `Terminal` frame; AND the W3b assertions (status Completed, checkpoints present) still hold. Plus: assert the hub is REMOVED from `progress_hubs` after terminal.
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement.**
  - In `spawn_detached_workflow`: build the sink as `DetachedProgressSink::new(srv.task_store.clone(), task.clone(), hub.clone())` (the hub is passed in — see below). The runner's post-drain logic CHANGES: the sink's `terminal()` already wrote the sequenced terminal + published, so `Ok(true)` → do NOT write terminal again; just remove the hub. `Ok(false)` (no terminal) → `set_terminal_sequenced(Failed,"workflow ended without terminal")` + publish a `Terminal{Failed}` frame on the hub + remove the hub. `Err` → `set_terminal_sequenced(Failed,"checkpoint write failed")` + publish `Terminal{Failed}` + remove hub. Then `fin.done=true; workflow_cancels.remove`. (Delete the old `sink.take()`→`set_terminal` capture path.)
  - **Hub inserted BEFORE spawn:** the CALLERS create the hub + insert it into `srv.progress_hubs` BEFORE calling `spawn_detached_workflow`, and pass it in. Update the signature `spawn_detached_workflow(..., hub: Arc<TaskProgressHub>)`. Caller sites: the `unary_message` `RouteTarget::Workflow` arm (fresh submit) and `resume_working_tasks` (resume) both `let hub = Arc::new(TaskProgressHub::new()); srv.progress_hubs.lock().await.insert(task.clone(), hub.clone());` then pass `hub`. The test seams pass a fresh hub.
  - A small helper `remove_hub(srv, &task)` = `srv.progress_hubs.lock().await.remove(&task);`.
- [ ] **Step 4: Run → green.** `cargo test -p bridge-a2a-inbound` (ALL W3b detached/resume/cancel/panic tests green — the terminal restructure must not regress them), clippy, `cargo check --workspace`.
- [ ] **Step 5: Commit** `git commit -m "feat(inbound): detached runner uses DetachedProgressSink; sink owns sequenced terminal; hub before spawn"`

### Task 6: `Finalizer` hub cleanup (+ sequenced Failed + broadcast)

**Files:** `crates/bridge-a2a-inbound/src/workflow_sink.rs`, `server.rs`.

- [ ] **Step 1: Failing test.** Extend the W3b `runner_panic_finalizes_failed_no_orphan`: a subscriber attached to the hub also receives a `Terminal{Failed}` frame on a runner panic, AND the hub is removed from `progress_hubs`.
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement.** Add to `Finalizer` the fields `progress_hubs: Arc<Mutex<HashMap<TaskId,Arc<TaskProgressHub>>>>` and `hub: Arc<TaskProgressHub>`. In `Drop` (the spawned async): use `set_terminal_sequenced(Failed,"runner ended without terminal")` (not the old `set_terminal`), then `hub.publish(Terminal{Failed})`, then `progress_hubs.lock().await.remove(&task)`, then `cancels.remove`. Update the `Finalizer{}` construction in `spawn_detached_workflow` to pass `progress_hubs` + `hub`.
- [ ] **Step 4: Run → green.** `cargo test -p bridge-a2a-inbound`, clippy, check.
- [ ] **Step 5: Commit** `git commit -m "feat(inbound): Finalizer writes sequenced Failed + broadcasts terminal + removes hub on panic"`

---

## Phase D — `SubscribeToTask` handler

### Task 7: split the dispatch + the handler skeleton (auth+version, taskId, not-found)

**Files:** `crates/bridge-a2a-inbound/src/server.rs`.

- [ ] **Step 1: Failing test.** A `SubscribeToTask` JSON-RPC with a missing taskId → JSON-RPC error; with an unknown taskId → JSON-RPC error (not-found). `SendStreamingMessage` still routes to `stream_message` (unchanged).
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement.** Split the dispatch arm (server.rs:538): `m if m == methods::SEND_STREAMING_MESSAGE => stream_message(...)`, and a new arm `m if m == methods::SUBSCRIBE_TO_TASK => subscribe_to_task(srv, headers, id, params).await`. Add `async fn subscribe_to_task(srv, headers, id, params) -> Response`: do the auth + A2A-version check (mirror the start of `stream_message`/`unary_message`, but do NOT call `gate()`); extract `params["taskId"]` (required — return `bridge_err_to_jsonrpc(id, &BridgeError::InvalidRequest{field:"taskId"})` if absent); parse the cursor `K` from the `Last-Event-ID` header (`u64`, absent→0); `srv.task_store.get(&taskId)` → `None` → JSON-RPC error. (The snapshot/SSE body is Tasks 8-9 — for now, after the get, return a minimal empty SSE or a stub the next tasks replace.)
- [ ] **Step 4: Run → green.** `cargo test -p bridge-a2a-inbound`, clippy, check.
- [ ] **Step 5: Commit** `git commit -m "feat(inbound): route SubscribeToTask to a reattach handler (auth+version, taskId, not-found)"`

### Task 8: the snapshot builder (row-driven, seq-ordered, cursor) + terminal-state flow

**Files:** `crates/bridge-a2a-inbound/src/server.rs`.

- [ ] **Step 1: Failing test.** Seed a TERMINAL task with checkpoints (seqs) + a `terminal_seq` via the sequenced store methods; `SubscribeToTask` (no cursor) → the SSE body is the snapshot `NodeFinished` frames in SEQ order + `SnapshotComplete` + `Terminal`, then closes. With a cursor `K` between two checkpoint seqs → only `seq>K` frames. (Drive the handler + collect the SSE frames; assert the delivered `(seq,kind)` set.)
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement.** A `fn snapshot_frames(snap: &TaskProgressSnapshot, k: i64) -> Vec<WorkflowProgressFrame>`: from `snap.checkpoints` (each `(node,output,ok,seq)`) → `WorkflowProgressFrame{v:1, seq, phase: Phase::Snapshot, kind: FrameKind::NodeFinished{node,ok,output}}` if `seq>k`; from `snap.starts` → `…kind: FrameKind::NodeStarted{node}` (phase `Snapshot`) if `seq>k`; collect, **sort by `frame.seq`**. (Snapshot-path frames carry `phase: Snapshot`; the sink's live frames carry `phase: Live` — same kind, different delivery path.) In `subscribe_to_task` for a terminal `snap.status`: emit `snapshot_frames(snap,k)`, then the sentinel `WorkflowProgressFrame{v:1, seq: <max snapshot frame seq, else k>, phase: Phase::Snapshot, kind: FrameKind::SnapshotComplete}` (`seq` lives on the frame; `FrameKind::SnapshotComplete` is fieldless — Task 3), then (if `terminal_seq>k`) `WorkflowProgressFrame{v:1, seq: terminal_seq, phase: Phase::Live, kind: FrameKind::Terminal{outcome,output}}`, then close. Convert each frame to an SSE event with `id: <frame.seq>` + a JSON data body (derive `Serialize` on `Phase`/`FrameKind`/`WorkflowProgressFrame` in Task 3, or hand-serialize); reuse `stream_message`'s SSE `Response` builder (`Sse::new(stream)` / `sse_event_stream`).
- [ ] **Step 4: Run → green.** `cargo test -p bridge-a2a-inbound`, clippy, check.
- [ ] **Step 5: Commit** `git commit -m "feat(inbound): SubscribeToTask snapshot (row-driven, seq-ordered, cursor) + terminal-state flow"`

### Task 9: the working-state flow (subscribe-first + cut_seq dedup + live-tail) + cursor-beyond-seq

**Files:** `crates/bridge-a2a-inbound/src/server.rs`.

- [ ] **Step 1: Failing tests.** (a) In-flight: a Working task with a live hub; subscribe → snapshot + `SnapshotComplete` + the live deltas (publish a few frames after) to a `Terminal` → close; assert the delivered `(seq,kind)` set = no dup, no gap. (b) Two concurrent subscribers both get the full set. (c) Empty snapshot (no nodes finished) still emits `SnapshotComplete` then live. (d) Cursor `K >= cut_seq` on a terminal task → `SnapshotComplete` then immediate close (no hang). Drive via a hand-built sequence through the sink+hub (deterministic; assert on the delivered set, not arrival phase).
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement.** In `subscribe_to_task` for a Working `snap.status`: get the task's hub from `srv.progress_hubs` (if absent — re-`get`; terminal→terminal path; still Working→ retryable JSON-RPC error); **`rx = hub.subscribe()` FIRST**; read `progress_snapshot` → `cut_seq`; build the SSE stream as: `snapshot_frames(snap, k)` ++ `SnapshotComplete` ++ a live stream from `rx` that drops frames with `seq <= max(k, cut_seq)` and ends after a `Terminal`. Cursor-beyond-seq: if `k >= snap.cut_seq` the snapshot_frames is empty → still emit `SnapshotComplete`; terminal→close; working→tail (handled by the same path). Use `async_stream` / a `futures::stream` that yields the snapshot vec then bridges the broadcast `rx` (mapping `RecvError::Lagged` → a retryable SSE error frame + close; `Closed` → close).
- [ ] **Step 4: Run → green.** `cargo test -p bridge-a2a-inbound`, clippy, `cargo check --workspace`.
- [ ] **Step 5: Commit** `git commit -m "feat(inbound): SubscribeToTask working-state — subscribe-first + cut_seq dedup + live-tail; cursor-beyond-seq + lag"`

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
| 1 (SubscribeToTask routed; not-found; SendStreamingMessage unchanged; scope) | 7 |
| 2 (transactional seq; migration; resume continuation; upsert idempotent) | 1, 2 |
| 3 (DetachedProgressSink persist+publish; W3b write-failure intact; sink terminal) | 4, 5 |
| 4 (snapshot row-driven + seq-ordered; NULL-seq=0; cursor) | 2, 8 |
| 5 (in-flight snapshot+SnapshotComplete+deltas; no dup/gap) | 9 |
| 6 (terminal snapshot+terminal close; two subscribers) | 8, 9 |
| 7 (empty snapshot sentinel; cursor>=seq no hang) | 9 |
| 8 (cursor only-new) | 8, 9 |
| 9 (restart-then-resubscribe; finalizer hub cleanup) | 5, 6, 12 |
| 10 (lag → retryable + reconnect) | 9 |
| 11 (CLI; coverage; gated-live smoke; ADR) | 10, 11, 12, 13 |

## Notes for the implementer
- **Riskiest = Task 5** (the terminal-ownership restructure). The sink now OWNS the sequenced terminal write; the runner's old `sink.take()`→`set_terminal` path is DELETED; the runner only handles no-terminal / `Err` / hub-removal. Every W3b detached/resume/cancel/panic test is the regression oracle — keep them green.
- **Durable-first** is the invariant: persist (allocate seq) THEN publish, everywhere. Every SSE `id` is committed state.
- **Test on the delivered `(seq,kind)` SET vs the cursor** (no dup/no gap), NOT phase-of-arrival; drive a hand-built `WorkflowEvent` stream through the sink+hub (mirror `drain_awaits_node_finished_before_next`) for determinism.
- **Hub before spawn** (fresh + resume); **Finalizer removes the hub** (Task 6) so a panic never leaks it.
- `record_node_started` is an **upsert** (resume re-emits NodeStarted). `progress_snapshot` treats **NULL-seq checkpoints as seq 0**.
- `run cargo check --workspace` after every task. Firewall: design from bridge ports + A2A `SubscribeToTask` semantics; a2a-local-bridge did not inform it. Controller docs (this plan, ADR-0015) carry the `Co-Authored-By: Claude Opus 4.8 (1M context)` trailer; task commits do NOT. Coverage after `cargo llvm-cov clean --workspace`.
