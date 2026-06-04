# Streaming Reattach — Design

**Date:** 2026-06-03
**Status:** Draft (dual-designed: Claude + a firewalled independent codex design, merged; pending user review → dual review → plan)

**Goal:** A client that submitted a workflow **detached** (durable; `message/send`→workflow returns a task id with no stream — W3a/W3b) can later **re-subscribe to its live progress over SSE** via a dedicated `tasks/resubscribe` method — after the original client disconnected, or from a different client — instead of only polling `tasks/get`.

**Branch:** `feat/reattach` off `main`.

**Provenance:** brainstormed (catch-up semantics, cursor, wire surface settled with the user), then **dual-independent-designed** — a firewalled clean-room codex design converged on the spine and sharpened the snapshot↔live seam; this spec is the merge.

---

## Settled product decisions
1. **Wire:** a dedicated **`tasks/resubscribe`** method (A2A-spec-named; a2a-lf 0.3.0 doesn't validate it — the bridge routes its own methods via a local constant), params `{taskId}`, cursor via the `Last-Event-ID` header, returns SSE. NOT a `message/stream` overload.
2. **Catch-up = STATE SNAPSHOT + LIVE DELTAS**, not raw-event replay. Consumers are LLMs/humans; a re-sent raw "node finished" misleads an LLM (reads as a second completion). A snapshot is idempotent-by-construction (re-send = "here's now again"). Granularity is **node-level** (`NodeStarted`/`NodeFinished{output}`/`Terminal`) — not token-level, not a full exact-event-history log.
3. **Client-held cursor, two modes:** cursor-less → full snapshot + all deltas ("send everything" — dashboard/fresh client/handoff); cursor `K` → only `seq > K` + live deltas ("exclude what I've read" — LLM context economy). The bridge keeps NO per-consumer server-side read-state.
4. **Three states:** in-flight (Working) → snapshot + live-tail to terminal; terminal → snapshot + terminal, close; not-found → JSON-RPC error.
5. **Restart-safe:** after a serve restart, W3b resumes (re-runs only pending nodes; done nodes seeded, emit no new events; new run_id, stable task id). Reattach = snapshot from the durable store + live-tail the resumed run; the cursor spans the durable→live boundary AND the restart.
6. **Single-serve;** multiple concurrent reattachers supported.

## Architecture
The **durable `TaskStore` is the source of truth** for catch-up (so reattach is restart-safe and lag-recoverable); an **in-memory per-task broadcast** is the live tail. **Durable-first:** every event is persisted (allocating its `seq`) BEFORE it is published live, so every SSE `id` corresponds to already-committed state.

### Components
1. **`TaskProgressHub`** (new `crates/bridge-a2a-inbound/src/reattach.rs`) — owns a bounded `tokio::sync::broadcast::Sender<WorkflowProgressFrame>`. `InboundServer` gains `progress_hubs: Arc<Mutex<HashMap<TaskId, Arc<TaskProgressHub>>>>`. A per-task hub is **inserted BEFORE the detached runner is spawned** — for both fresh submit (`unary_message` `RouteTarget::Workflow`) and W3b resume (`resume_working_tasks`) — and **removed after the terminal is durably written + broadcast**. `broadcast` (not per-subscriber channels) supports multiple late subscribers with no server-side read-state.
2. **`WorkflowProgressFrame`** (versioned, in inbound — a transport concern, NOT in `bridge-core`; the executor/store stay reattach-unaware):
```
WorkflowProgressFrame { v: 1, seq: i64, phase: Snapshot | Live,
  kind: NodeStarted { node } | NodeFinished { node, ok, output } | Terminal { outcome, output } }
```
   We do NOT reuse `bridge_core::translator::Event` (it lacks node ids, output-on-finish, and seq).
3. **`DetachedProgressSink`** (a `WorkflowSink`, replacing the bare `TaskStoreSink` in the detached runner). Per `WorkflowEvent`: (1) persist + **allocate the `seq`** through the `TaskStore` (the durable-first write — checkpoint/terminal/start); (2) ONLY after that write succeeds, publish the `WorkflowProgressFrame{seq}` to the task's hub; (3) no receivers → ignore; (4) **durable write fails → return `Err`** so `drain_workflow` aborts and the runner marks the task `Failed` (the W3b contract is preserved exactly). The sync/streaming `SseSink` is unchanged.
4. **Per-task `seq` (transactional, in the store).** `TaskStore` gains seq-allocating methods, each running in ONE transaction (increment the per-task counter, write the state, commit, return the seq):
   - `record_node_started(task, node, ts) -> seq` (+ a `task_node_starts(task_id, node_id, seq, ts)` row)
   - `put_node_checkpoint_sequenced(task, node, output, ok, ts) -> seq` (sets `task_node_checkpoints.seq`; clears the node's start row)
   - `set_terminal_sequenced(task, status, result, error, ts) -> seq` (sets `tasks.terminal_seq`; clears all of the task's start rows)
   - `progress_snapshot(task) -> TaskProgressSnapshot` (the consistent read: task row + checkpoints ordered by seq + current starts + `terminal_seq`)
   SQLite (additive `migrate_tasks_columns` pattern): `tasks.last_event_seq INTEGER NOT NULL DEFAULT 0`, `tasks.terminal_seq INTEGER`, `task_node_checkpoints.seq INTEGER`, new `task_node_starts(task_id, node_id, seq, ts, PRIMARY KEY(task_id,node_id), FK→tasks ON DELETE CASCADE)`.
   **Why persist `NodeStarted`:** a currently-running node must appear in the snapshot ("synth is running now" — the point of *live* progress), and the seq a start consumed must be durable so the cursor has no phantom gap. `task_node_starts` is bounded *current* state (one row per started-unfinished node, cleared on finish/terminal), NOT raw history.
5. **`tasks/resubscribe` handler** (routed in the method dispatch, NOT through `stream_message`). Auth + A2A-version check only — it does NOT call `gate()` (there is no message/routing metadata). Requires `params.taskId` (no `"task-1"` fallback). Cursor `K` = `Last-Event-ID` parsed as `u64` (absent = full).

### Data flow — `tasks/resubscribe(taskId, K?)`
1. `TaskStore.get(taskId)` → **None → JSON-RPC error**.
2. **Terminal** task: read `progress_snapshot`; emit snapshot frames (`seq > K`) in seq order; if `terminal_seq > K` emit `Terminal`; **close** (no hub).
3. **Working** task — the exactly-once boundary (subscribe-before-snapshot):
   1. Get the hub; **`subscribe()` FIRST** (so no event fired between read and subscribe is lost).
   2. Read `progress_snapshot` with `cut_seq = tasks.last_event_seq`.
   3. Emit snapshot frames with `seq > K` (`phase: Snapshot`): for each graph node — checkpoint→`NodeFinished{output}` if `checkpoint.seq > K`; else current-start→`NodeStarted` if `start.seq > K`; pending (neither) → omit. If the snapshot is already terminal (`terminal_seq` set), emit `Terminal` (if `> K`) and close.
   4. **Live-tail** the receiver, dropping frames with `seq <= max(K, cut_seq)` (dedup the snapshot↔live overlap), until a `Terminal` frame, then close.
4. Every SSE frame sets `id: <seq>`; the client's `Last-Event-ID` auto-advances; reconnect resumes from there. Snapshot semantics: a node that started AND finished since `K` is represented ONCE as finished-with-output (not two events).

### Restart / W3b-resume
`seq` is persisted (`last_event_seq`/checkpoint.seq/terminal_seq), so it survives. After a restart the resumed runner re-runs only pending nodes (done nodes seeded, no live frames) and registers a NEW hub for the stable task id, continuing the seq from `last_event_seq`. A post-restart resubscribe = snapshot of done nodes (checkpoints) + any current starts + live-tail the resumed run; the cursor still selects only-new across the boundary.

### Error handling
- **Broadcast lag** (`RecvError::Lagged`): emit a **retryable SSE error frame carrying the last-sent seq**, then close. The client reconnects with `Last-Event-ID`; the durable snapshot repairs state (the cursor is the recovery path).
- **Hub publish failure / no receivers:** best-effort `let _ =`; never affects the durable run.
- **Hub missing but task Working** (should be impossible — hub inserted before spawn; only a torn restart could race it): re-read; terminal → terminal path; still Working → a retryable error BEFORE committing the SSE response.
- **Terminal during snapshot:** handled by subscribe-before-snapshot + `cut_seq` — if terminal is in the snapshot, close after it; if it lands after `cut_seq`, it arrives via the receiver.

## Hexagonal boundaries
`TaskProgressHub`, `WorkflowProgressFrame`, the `DetachedProgressSink`, and the handler live in `bridge-a2a-inbound` (transport). The seq + the snapshot read are `TaskStore` port concerns (the new sequenced methods). The `WorkflowExecutor` is UNTOUCHED (it already emits `WorkflowEvent`s; the sink consumes them). The frame is NOT in `bridge-core`.

## Definition of Done (each → a falsifiable test)
1. `tasks/resubscribe` routed (auth+version, no `gate()`, requires `taskId`); not-found → JSON-RPC error.
2. Per-task `seq`: transactional allocation (`last_event_seq`), monotonic; `checkpoint.seq`/`terminal_seq`/`task_node_starts` persisted via additive migration; continues across resume. Tests: monotonic; migration on an old DB; resume continuation.
3. `DetachedProgressSink` PERSISTS (W3b behavior intact incl. write-failure→`Failed`) AND PUBLISHES per event; a hub publish failure / no-receivers does NOT fail the run. Test both halves.
4. `progress_snapshot` reconstruction (done→NodeFinished, running→NodeStarted, terminal→Terminal; cursor `seq>K`), seq-ordered, frames carry `id`. Unit test.
5. In-flight resubscribe → snapshot + live deltas to terminal (subscribe-before-snapshot, `cut_seq` dedup; no dup, no gap). Terminal resubscribe → snapshot + terminal, closes. Two concurrent resubscribers both get the full stream.
6. Cursor → only-new (snapshot + deltas filtered by `seq>K`); a node started+finished since K appears once as finished. Test.
7. Restart-then-resubscribe reconstructs done (snapshot) + running/pending (snapshot start + live) coherently. Test (seed checkpoints/starts + a resumed runner).
8. Broadcast-lag → retryable error frame + close; reconnect re-snapshots from the cursor, no lost state. Test (force a lagged receiver).
9. `task watch <id>` CLI (reqwest SSE client honoring `Last-Event-ID`). fmt/clippy/coverage (floors 85/90/90); gated live (real codex+claude detached run → resubscribe mid-flight from a second client → live to Completed; kill+restart serve mid-run → resubscribe reconstructs done+resumed). ADR-0015.

## Slice boundary
- **Slice 1 (this):** the hub + `DetachedProgressSink` + the transactional `seq` (incl. `task_node_starts`) + `tasks/resubscribe` + snapshot/deltas + cursor + the 3 states + restart-safety + the lag fallback + `task watch` CLI.
- **Deferred:** token-level/fine-grained streaming; a full exact-event-history log; cross-serve reattach (N/A — single-serve); per-consumer server-side read-state (rejected — client holds the cursor).

## One-way doors (lock the contract now)
The public **cursor contract** — clients treating `Last-Event-ID` as a per-task numeric `seq` — is hard to change later. Lock: frame `v: 1`; `seq` per-task, monotonic, **gaps allowed**; and document explicitly that `tasks/resubscribe` is **state-catch-up, not event-history replay**.

## Firewall
Designed from the bridge's own ports (`WorkflowSink`, `TaskStore`, the SSE producer) + A2A task-lifecycle/resubscribe semantics; the `a2a-local-bridge` PoC did not inform it.
