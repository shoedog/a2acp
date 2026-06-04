# Streaming Reattach — Design

**Date:** 2026-06-03
**Status:** Draft rev2 (dual-designed + dual-reviewed; folds the Codex+Claude spec-review findings; pending plan)

**Goal:** A client that submitted a workflow **detached** (durable; `message/send`→workflow returns a task id with no stream — W3a/W3b) can later **re-subscribe to its live progress over SSE** via the A2A-standard `SubscribeToTask` method — after the original client disconnected, or from a different client — instead of only polling `tasks/get`.

**Branch:** `feat/reattach` off `main`.

**Provenance:** brainstormed (catch-up semantics, cursor, wire surface settled with the user), **dual-independent-designed** (a firewalled clean-room codex design converged on the spine + sharpened the snapshot↔live seam), then **dual-reviewed** (Codex soundness + Claude architecture). rev2 folds: the wire surface → the existing standard `SubscribeToTask` (Claude F1); the sink owns the sequenced terminal write + the finalizer cleans up the hub (both); `record_node_started` is idempotent for resume (Codex); the snapshot is seq-ordered/row-driven (both); cursor-beyond-seq, the NULL-seq reconciliation, a `SnapshotComplete` sentinel, and detached-workflow-only scope (Claude F3/F4/F5/F7).

---

## Settled product decisions
1. **Wire:** the **existing A2A-standard `SubscribeToTask`** method (`a2a-lf 0.3.0` exposes it as a valid *streaming* method, jsonrpc.rs:143; the bridge already routes it at server.rs:538 — currently lumped with `SendStreamingMessage`→`stream_message`). reattach **splits `SubscribeToTask` out of that arm** into the reattach handler. Params `{taskId}`, cursor via the `Last-Event-ID` header, returns SSE. This is the dedicated subscribe verb (NOT a `message/stream`/`SendStreamingMessage` overload — those start a new run from a message; `SubscribeToTask` subscribes to an *existing* task). **Scope: detached-workflow tasks only** — a `SubscribeToTask` for a non-workflow / sync / unknown task id → not-found (or a single terminal frame if the task is a non-workflow with a stored result); see §states.
2. **Catch-up = STATE SNAPSHOT + LIVE DELTAS**, not raw-event replay. Consumers are LLMs/humans; a re-sent raw "node finished" misleads an LLM (reads as a second completion). A snapshot is idempotent-by-construction (re-send = "here's now again"). Granularity is **node-level** (`NodeStarted`/`NodeFinished{output}`/`Terminal`) — not token-level, not a full exact-event-history log.
3. **Client-held cursor, two modes:** cursor-less → full snapshot + all deltas ("send everything" — dashboard/fresh client/handoff); cursor `K` → only `seq > K` + live deltas ("exclude what I've read" — LLM context economy). The bridge keeps NO per-consumer server-side read-state.
4. **Three states:** in-flight (Working) → snapshot + live-tail to terminal; terminal → snapshot + terminal, close; not-found → JSON-RPC error.
5. **Restart-safe:** after a serve restart, W3b resumes (re-runs only pending nodes; done nodes seeded, emit no new events; new run_id, stable task id). Reattach = snapshot from the durable store + live-tail the resumed run; the cursor spans the durable→live boundary AND the restart.
6. **Single-serve;** multiple concurrent reattachers supported.

## Architecture
The **durable `TaskStore` is the source of truth** for catch-up (so reattach is restart-safe and lag-recoverable); an **in-memory per-task broadcast** is the live tail. **Durable-first:** every event is persisted (allocating its `seq`) BEFORE it is published live, so every SSE `id` corresponds to already-committed state.

### Components
1. **`TaskProgressHub`** (new `crates/bridge-a2a-inbound/src/reattach.rs`) — owns a bounded `tokio::sync::broadcast::Sender<WorkflowProgressFrame>`. `InboundServer` gains `progress_hubs: Arc<Mutex<HashMap<TaskId, Arc<TaskProgressHub>>>>`. A per-task hub is **inserted BEFORE the detached runner is spawned** — for both fresh submit (`unary_message` `RouteTarget::Workflow`) and W3b resume (`resume_working_tasks`) — and **removed after the terminal is durably written + broadcast**. `broadcast` (not per-subscriber channels) supports multiple late subscribers with no server-side read-state. **The `Finalizer` drop-guard owns hub cleanup** (rev2, both reviewers): on any non-terminal exit (incl. panic) it writes a **sequenced** `Failed` (`set_terminal_sequenced`), **broadcasts that terminal frame so live subscribers close**, and **removes the hub** — so a panic never leaks a hub or leaves subscribers without a terminal.
2. **`WorkflowProgressFrame`** (versioned, in inbound — a transport concern, NOT in `bridge-core`; the executor/store stay reattach-unaware):
```
WorkflowProgressFrame { v: 1, seq: i64, phase: Snapshot | Live,
  kind: NodeStarted { node } | NodeFinished { node, ok, output } | SnapshotComplete | Terminal { outcome, output } }
```
   `SnapshotComplete` (rev2, Claude F5) is a sentinel the handler emits **after the snapshot, before any live deltas** (always, even when the snapshot is empty) so a consumer — especially an LLM — knows "catch-up is done; the rest is live." It is handler-emitted only (never persisted, never on the hub). We do NOT reuse `bridge_core::translator::Event` (it lacks node ids, output-on-finish, and seq — Claude confirmed `SseSink` flattens the node id into a status string).
3. **`DetachedProgressSink`** (a `WorkflowSink`, replacing the bare `TaskStoreSink` in the detached runner). Per `WorkflowEvent`: (1) persist + **allocate the `seq`** through the `TaskStore` (the durable-first write — start/checkpoint/terminal via the sequenced methods); (2) ONLY after that write succeeds, publish the `WorkflowProgressFrame{seq}` to the task's hub; (3) no receivers → ignore; (4) **durable write fails → return `Err`** so `drain_workflow` aborts and the runner marks the task `Failed` (the W3b contract is preserved exactly). **Terminal ownership moves into the sink (rev2, both reviewers):** today `TaskStoreSink::terminal` only *captures* state and the runner writes `set_terminal` afterward (unsequenced, error ignored). `DetachedProgressSink::terminal` instead calls **`set_terminal_sequenced` + publishes the Terminal frame**, and the runner **no longer writes terminal post-drain** (it just observes the drain result + handles the no-terminal/`Err` cases). This makes the terminal write sequenced, ordered-before-publish, and abortable. The sync/streaming `SseSink` is unchanged.
4. **Per-task `seq` (transactional, in the store).** `TaskStore` gains seq-allocating methods, each running in ONE transaction (increment the per-task counter, write the state, commit, return the seq), via the real `Arc<Mutex<Connection>>` + `unchecked_transaction` (Codex confirmed feasible — same as `claim_resume_attempt`):
   - `record_node_started(task, node, ts) -> seq` — **idempotent upsert** (rev2, Codex blocker #1): `ON CONFLICT(task_id,node_id) DO UPDATE SET seq=<new>, ts=<new>`. On W3b resume the executor re-emits `NodeStarted` for unseeded nodes; a naive `INSERT` would hit the `task_node_starts` PK and fail the resumed task. The upsert replaces the stale start with a fresh seq (the old seq becomes a gap — allowed).
   - `put_node_checkpoint_sequenced(task, node, output, ok, ts) -> seq` (sets `task_node_checkpoints.seq`; clears the node's start row). **The detached runner writes checkpoints ONLY via this sequenced method** (rev2, Claude F4 — NULL-seq hazard): the legacy `put_node_checkpoint` (NULL seq) must not be used on the reattach path; `progress_snapshot` treats any NULL-seq checkpoint (a pre-reattach-era row) as `seq = 0` so it is always included in a cursor-less snapshot and never breaks ordering.
   - `set_terminal_sequenced(task, status, result, error, ts) -> seq` (sets `tasks.terminal_seq`; clears all of the task's start rows)
   - `progress_snapshot(task) -> TaskProgressSnapshot` (the consistent read: task row + checkpoints + current starts + `terminal_seq` + `last_event_seq` as `cut_seq`)
   SQLite (additive `migrate_tasks_columns` pattern — covers BOTH `tasks` AND `task_node_checkpoints`, Codex): `tasks.last_event_seq INTEGER NOT NULL DEFAULT 0`, `tasks.terminal_seq INTEGER`, `task_node_checkpoints.seq INTEGER`, new `task_node_starts(task_id, node_id, seq, ts, PRIMARY KEY(task_id,node_id), FK→tasks ON DELETE CASCADE)`.
   **Why persist `NodeStarted`** (Claude confirmed essential to slice 1, not deferrable): a currently-running node must appear in the snapshot ("synth is running now" — the point of *live* progress), and the seq a start consumed must be durable so the cursor can dedup an already-delivered live `NodeStarted` (no phantom gap). `task_node_starts` is bounded *current* state (one row per started-unfinished node, cleared on finish/terminal), NOT raw history.
5. **`SubscribeToTask` handler** (rev2, Claude F1 — the wire surface). Split `SubscribeToTask` out of the shared `SEND_STREAMING_MESSAGE || SUBSCRIBE_TO_TASK` dispatch arm (server.rs:538) into its own reattach handler; `SendStreamingMessage` stays `stream_message`. **Auth + A2A-version check only — it does NOT call `gate()`** (there is no message/routing metadata; `task_id_from_params` would synthesize `"task-1"`, which Claude confirmed would be a real bug — so require `params.taskId` explicitly). Cursor `K` = `Last-Event-ID` parsed as `u64` (absent = full). Returns SSE (reusing `message/stream`'s SSE-response machinery).

### Data flow — `SubscribeToTask(taskId, K?)`
1. `TaskStore.get(taskId)` → **None → JSON-RPC error**.
2. **Build the snapshot frame list (rev2, row-driven + seq-ordered — both reviewers):** materialize candidates from the durable rows — finished checkpoints → `NodeFinished{output}` (seq), current starts → `NodeStarted` (seq), terminal → `Terminal` (`terminal_seq`) — filter to `seq > K`, then **SORT by `seq`** (NOT graph order; an out-of-order emit would let a reconnect with `Last-Event-ID: hi` skip a lower-seq frame). Pending nodes (no checkpoint, no start) are omitted. The graph snapshot (`workflow_spec_json`) supplies node names only; the *frame set* is row-driven.
3. **Terminal** task: emit the snapshot frames; then `SnapshotComplete`; then if `terminal_seq > K` the `Terminal` frame; **close** (no hub).
4. **Working** task — the exactly-once boundary (subscribe-before-snapshot):
   1. Get the hub; **`subscribe()` FIRST** (so no event fired between read and subscribe is lost).
   2. Read `progress_snapshot` with `cut_seq = tasks.last_event_seq`.
   3. Emit the snapshot frames (step 2, `phase: Snapshot`), then a **`SnapshotComplete`** sentinel. If the snapshot read is already terminal, emit `Terminal` (if `> K`) and close.
   4. **Live-tail** the receiver, dropping frames with `seq <= max(K, cut_seq)` (dedup the snapshot↔live overlap), until a `Terminal` frame, then close.
5. **Cursor beyond the current seq (rev2, Claude F3 — lock this door):** if `K >= last_event_seq` the snapshot is empty → emit `SnapshotComplete` immediately; then **terminal task → close at once** (do NOT block waiting for `seq > K` that will never come); **working task → live-tail** (future events have `seq > K`). Never starve/hang.
6. Every SSE frame sets `id: <seq>` (the `SnapshotComplete` sentinel carries the highest snapshot seq, or `K` if empty); the client's `Last-Event-ID` auto-advances; reconnect resumes from there. Snapshot semantics: a node that started AND finished since `K` is represented ONCE as finished-with-output (not two events).

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
**Test approach (rev2, Claude):** assert on the **delivered `(seq, kind)` SET** vs the cursor (no dup / no gap) — NOT the *phase-of-arrival* (timing-dependent; snapshot+cursor makes the delivered set timing-independent — that's the property). Drive a **hand-built `WorkflowEvent` stream through the sink + hub** for determinism (mirror `drain_awaits_node_finished_before_next`). The gated-live kill/restart is a **manual smoke gate, not CI**.

1. `SubscribeToTask` split out of the shared streaming arm + routed (auth+version, no `gate()`, requires `taskId`); not-found → JSON-RPC error; a non-workflow/sync task → not-found (or single terminal frame). `SendStreamingMessage` behavior unchanged.
2. Per-task `seq`: transactional allocation (`last_event_seq`), monotonic, gaps allowed; `checkpoint.seq`/`terminal_seq`/`task_node_starts` persisted via additive migration (covering `tasks` AND `task_node_checkpoints`); continues across resume. Tests: monotonic; migration on an old DB; resume continuation; **`record_node_started` upsert is idempotent on a resume re-emit** (no PK failure).
3. `DetachedProgressSink` PERSISTS (W3b behavior intact incl. checkpoint-write-failure→`Failed`) AND PUBLISHES per event; the sink's `terminal()` writes `set_terminal_sequenced` (the runner does NOT write terminal post-drain); a hub publish failure / no-receivers does NOT fail the run. Test all three.
4. `progress_snapshot` reconstruction is **row-driven + seq-ordered** (done→NodeFinished, running→NodeStarted, terminal→Terminal; cursor `seq>K`); a **NULL-seq legacy checkpoint is treated as `seq=0`** (Claude F4); frames carry `id`. Unit test incl. the NULL-seq case.
5. **In-flight resubscribe** → snapshot + `SnapshotComplete` + live deltas to terminal (subscribe-before-snapshot, `cut_seq` dedup; assert the delivered `(seq,kind)` set = no dup, no gap).
6. **Terminal resubscribe** → snapshot + `SnapshotComplete` + terminal, closes. **Two concurrent subscribers** both get the full delivered set.
7. **Empty snapshot** (reattach before any node finishes) still emits `SnapshotComplete` then live (Claude F5); **cursor `K >= last_event_seq`** → `SnapshotComplete` then (terminal → close immediately / working → tail), never hangs (Claude F3).
8. Cursor → only-new (snapshot + deltas filtered by `seq>K`); a node started+finished since K appears once as finished. Test.
9. Restart-then-resubscribe reconstructs done (snapshot) + running/pending (snapshot start + live) coherently. Test (seed checkpoints/starts + a resumed runner; the `Finalizer` removes the hub + writes sequenced `Failed` + broadcasts on a panic).
10. Broadcast-lag → retryable error frame + close; reconnect re-snapshots from the cursor, no lost state. Test (force a lagged receiver — deterministic).
11. `task watch <id>` CLI (reqwest SSE client honoring `Last-Event-ID`). fmt/clippy/coverage (floors 85/90/90); gated-live smoke (real codex+claude detached run → resubscribe mid-flight from a second client → live to Completed; kill+restart serve mid-run → resubscribe reconstructs done+resumed). ADR-0015.

## Slice boundary
- **Slice 1 (this):** the hub + `DetachedProgressSink` + the transactional `seq` (incl. `task_node_starts`) + `SubscribeToTask` + snapshot/deltas + cursor + the 3 states + restart-safety + the lag fallback + `task watch` CLI.
- **Deferred:** token-level/fine-grained streaming; a full exact-event-history log; cross-serve reattach (N/A — single-serve); per-consumer server-side read-state (rejected — client holds the cursor).

## One-way doors (lock the contract now)
The public **cursor contract** — clients treating `Last-Event-ID` as a per-task numeric `seq` — is hard to change later. Lock: frame `v: 1`; `seq` per-task, monotonic, **gaps allowed**; and document explicitly that `SubscribeToTask` is **state-catch-up, not event-history replay**.

## Firewall
Designed from the bridge's own ports (`WorkflowSink`, `TaskStore`, the SSE producer) + A2A task-lifecycle/resubscribe semantics; the `a2a-local-bridge` PoC did not inform it.
