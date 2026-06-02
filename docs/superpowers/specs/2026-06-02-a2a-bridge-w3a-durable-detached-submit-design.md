# W3a — Durable Detached Submit (design spec)

**Date:** 2026-06-02
**Status:** rev2 — dual-reviewed (Codex gpt-5.5 correctness + Claude opus-4.8 architecture); blockers + refinements folded in.
**Builds on:** ADR-0009 (workflow-DAG orchestration / W1), the live-validated `code-review` / `spec-review` / `plan-review` workflows, and the existing `bridge-store` SQLite layer.

---

## 1. Context & problem

The bridge's review workflows are live-validated but run **synchronously / streaming-only**: `message/stream` emits live SSE and `run-workflow` blocks in the foreground. A workflow's result is **never persisted** — when the stream ends, the output is gone. `get_task` is a stub: it reports `WORKING` if a `task↔session` mapping exists, else `SUBMITTED`, and returns no result.

The self-hosting goal (ADR-0008 re-trigger #5: replace the `~/code/a2a-local-bridge` PoC) requires the bridge's defining missing capability: **submit a long review detached, let it run in the background, and retrieve the result later** — surviving the submitting client's exit.

This spec is **W3a**: the first, ruthlessly-scoped slice — *result-durable* detached submit. Per-event history and crash-resumption are explicitly deferred to **W3b** (see §13).

## 2. Goal

Run a workflow **detached**: submit it, get a task id immediately, and retrieve its **persisted** status + result later (including after the submitting client exits, and across a `serve` restart **for already-terminal tasks**). Surfaced via the **A2A task lifecycle** and a **thin CLI** that is a client of `serve`.

## 3. Architecture — the durable/detached path

`serve` is the engine for detached work. W3a adds **one new durable/detached execution path**; it does **not** claim to be the only executor site — `run-workflow` (in-process, foreground) and the streaming `spawn_workflow_producer` already drive `WorkflowExecutor`. There are therefore **three executor call sites** after W3a; they share a small **executor-build helper** and a single **drain-over-`Sink`** (see §4.4) so terminal-mapping and token lifecycle live once.

```
  a2a-bridge submit code-review --input diff.txt
        │  POST message/send {skill, message}
        ▼
  serve (long-lived)
        │  mint unique TaskId, TaskStore.create(Working)  [non-clobbering INSERT]
        │  return canonical a2a::Task{status.state=working} immediately
        │  ── spawn background runner (finalizer-guarded) ──▶ WorkflowExecutor.run(...)
        │                                     │ drain over a TaskStore Sink
        ▼                                     ▼ on Terminal: set_terminal(status, result|error)
  TaskStore (SQLite, file-backed, single-serve-locked) ◀────┘
        ▲
  a2a-bridge task get <id>   ─ POST tasks/get   ─▶ serve reads TaskStore ─▶ canonical a2a::Task
  a2a-bridge task list       ─ POST (SDK ListTasks)─▶ recent TaskRecords
  a2a-bridge task cancel <id>─ POST tasks/cancel ─▶ TaskStore-aware cancel (see §4.3)
```

The synchronous `run-workflow` CLI is **unchanged**. `message/stream` for a workflow is **unchanged** (live SSE, W1). The new detached path is the **`message/send`** path for a workflow skill.

## 4. Components & responsibilities

### 4.1 `bridge-core` — new `TaskStore` port + in-memory impl
A new trait, **separate from `SessionStore`** (different responsibility: `SessionStore` = ephemeral routing state; `TaskStore` = durable control-plane receipts). Both are implemented by the same SQLite type.

```rust
pub struct TaskRecord {
    pub id: TaskId,
    pub workflow: String,            // workflow id this task is running
    pub status: TaskRecordStatus,
    pub result: Option<String>,      // final artifact text (terminal success)
    pub error: Option<String>,       // failure/interruption reason
    pub created_ms: i64,             // unix ms; passed in by caller (no Date::now in core)
    pub updated_ms: i64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TaskRecordStatus { Working, Completed, Failed, Canceled, Interrupted }

#[async_trait]
pub trait TaskStore: Send + Sync {
    /// Non-clobbering INSERT. A duplicate id MUST error (NOT upsert), so a
    /// resubmit/colliding id can never overwrite a terminal result. (Deliberately
    /// unlike SessionStore::put, which upserts.) This is what makes single-writer hold.
    async fn create(&self, rec: &TaskRecord) -> Result<(), BridgeError>;        // status=Working
    async fn set_terminal(&self, id: &TaskId, status: TaskRecordStatus,
                          result: Option<&str>, error: Option<&str>, updated_ms: i64)
                          -> Result<(), BridgeError>;
    async fn get(&self, id: &TaskId) -> Result<Option<TaskRecord>, BridgeError>;
    async fn list(&self, limit: usize) -> Result<Vec<TaskRecord>, BridgeError>; // newest-first
    async fn sweep_interrupted(&self, updated_ms: i64) -> Result<u64, BridgeError>; // Working -> Interrupted, returns count
}
```

**`MemoryTaskStore` (also in `bridge-core`):** a `Mutex<HashMap<TaskId, TaskRecord>>` impl of `TaskStore`, mirroring the existing in-`ports.rs` test fakes. **Rationale (review blocker):** `bridge-a2a-inbound` does **not** depend on `bridge-store` (where `SqliteStore` lives), so `InboundServer`'s default `TaskStore` must come from `bridge-core` — not pull in `bridge-store`. `serve` injects the file-backed `SqliteStore` at the composition root (§4.5).

Timestamps are **passed in** (the core forbids `Date::now`); callers (server) stamp them.

### 4.2 `bridge-store::sqlite` — file-backed + `TaskStore` impl + single-serve lock
- Add `SqliteStore::open(path: &Path) -> Result<Self, BridgeError>` (file-backed; creates/opens the DB, runs `CREATE TABLE IF NOT EXISTS`). Keep `open_in_memory()` for tests.
- **Single-serve-per-DB lock:** `open(path)` acquires an OS advisory lock (a `<path>.lock` file with the serve PID, or `flock`). If another process holds it, `open` **fails loud** (`StoreFailure` with a clear message). This is required so the boot `sweep_interrupted` (§4.3) can never flip a *live* serve's `Working` rows. Released on drop / process exit.
- Implement `TaskStore` on `SqliteStore` (it already implements `SessionStore`). `create` uses `INSERT` (no `ON CONFLICT`) so a duplicate id returns an error. New table:

```sql
CREATE TABLE IF NOT EXISTS tasks (
    id         TEXT PRIMARY KEY,
    workflow   TEXT NOT NULL,
    status     TEXT NOT NULL,          -- 'working'|'completed'|'failed'|'canceled'|'interrupted'
    result     TEXT,                   -- final artifact (nullable)
    error      TEXT,                   -- failure/interrupt reason (nullable)
    created_ms INTEGER NOT NULL,
    updated_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_tasks_updated ON tasks(updated_ms);
```

**Forward-compatibility (W3b):** node-level history goes in a *separate* future table (`task_node_outputs(task_id, node_id, seq, output, ts)`); the `tasks` table above does not change when W3b lands. No migration of `tasks` later. (Note: W3b *capture* also needs an additive executor change — see §13.)

### 4.3 `bridge-a2a-inbound::server`
- `InboundServer` gains `task_store: Arc<dyn TaskStore>`, **defaulting to `MemoryTaskStore`** (no `bridge-store` dep), wired via a `.with_task_store(...)` builder (existing `new` call sites untouched, mirroring `with_workflows`).
- **`message/send` (unary) on a workflow skill** — today rejects `InvalidRequest`; now → **`spawn_detached_workflow`**:
  1. **mint a fresh unique `TaskId` via `a2a::new_task_id()`.** (The existing `task_id_from_params` returns a *fixed* `"task-1"` for no-id sends — unusable as a unique PK. If the client *supplies* a task id, honor it; `create`'s non-clobbering INSERT rejects a collision.)
  2. extract input text from the message parts (the same extraction the streaming workflow producer uses; factored into the shared helper);
  3. `task_store.create({Working, workflow, created=updated=now})` — fails loud on collision/store error (no spawn on failure);
  4. register a `CancellationToken` in `workflow_cancels[task]`;
  5. `tokio::spawn` the **finalizer-guarded** background runner (§4.4);
  6. return a **canonical `a2a::Task`** with `status.state = working` and the task id, **immediately** (non-blocking).
- **`tasks/get`** — consult `task_store.get(id)` first:
  - found → return a **canonical `a2a::Task`** (`status.state`, `contextId`, `artifacts`) — reuse the existing unary `a2a::Task` builder (server.rs ~1360). Terminal success → one text artifact (`result`); failed/interrupted → status message carries the `error`.
  - not found → **current behavior** (session-mapping heuristic) preserved for non-workflow tasks.
- **`tasks/cancel`** — **TaskStore-aware** (review blocker — it cannot stay "unchanged"). For a task that is a known workflow row:
  - **terminal** (Completed/Failed/Canceled/Interrupted) → return its true persisted state; do **not** re-cancel or touch the default backend;
  - **Working with a live token** → fire the `workflow_cancels` token; the runner observes `Terminal{Canceled}` and writes `Canceled` (single-writer);
  - **Working without a token** (e.g. a row a previous serve owned but the sweep hasn't run, or a lost token) → write `Canceled` directly and return canceled.
  - Non-workflow tasks keep the existing fanout/delegate/local cancel fallthrough.
- **boot** — after `open(path)` (which holds the single-serve lock), `task_store.sweep_interrupted(now)` flips any stale `Working` rows to `Interrupted` (a prior `serve` died mid-run). Safe precisely because the lock guarantees no other live serve owns those rows.

### 4.4 The background runner — one drain over a `Sink`
The detached runner and `spawn_workflow_producer` differ **only in their sink** (an SSE `tx` vs the `TaskStore`). W3a factors **one** drain function over a small `WorkflowSink` abstraction so the stream-drain, `WorkflowOutcome→terminal` mapping, the no-terminal defensive guard, and the token register/remove lifecycle are defined **once** (this is also the reviewer's "materially simpler alternative"). The streaming producer keeps an SSE sink; the detached runner uses a `TaskStore` sink.

The detached runner is the **sole writer** of its task's terminal row, and is **finalizer-guarded**:
1. `WorkflowExecutor::run(graph, input, run_id = task_id, cancel_token)` → `WorkflowStream`.
2. Drain over the `TaskStore` sink. W3a **ignores** intermediate `NodeStarted`/`NodeFinished` (no history) and captures the single `Terminal { outcome, output }`.
3. On terminal: `set_terminal(map(outcome), result|error, now)` — `Completed`→`result=output`; `Failed`→`error=output`; `Canceled`→`Canceled`.
4. **Finalizer (drop guard):** the runner holds a guard that, on **any** exit where no terminal was written (stream ended without terminal, internal error, **panic**), writes `set_terminal(Failed, "runner ended without terminal")`, and **always** removes the `workflow_cancels[task]` token. A `Working` row can therefore never be permanently orphaned within a serve lifetime (the boot sweep is only the cross-restart backstop).

### 4.5 `bin/a2a-bridge`
- **Config:** optional `[store] path = "…"`. Set → `serve` opens file-backed (acquiring the single-serve lock); unset → `MemoryTaskStore` (ephemeral; existing behavior; tests unaffected). When a path is set, `serve` builds **one** `SqliteStore`, shares it as both `Arc<dyn SessionStore>` and `Arc<dyn TaskStore>` (the type implements both), and runs the boot sweep. A shared **executor-build helper** is used by `serve` and `run-workflow` so the three executor sites construct the registry/executor identically.
- **New CLI verbs** (thin `reqwest` JSON-RPC clients of a `serve` URL; default `http://127.0.0.1:8080`, override `--url`):
  - `submit <skill> --input <file> [--url <u>]` → POST `message/send`; print the task id.
  - `task get <id> [--url <u>]` → POST `tasks/get`; print status and (when terminal) the result artifact.
  - `task list [--url <u>] [--limit N]` → POST the SDK list method (§7); print recent tasks.
  - `task cancel <id> [--url <u>]` → POST `tasks/cancel`.
  - **Serve-down UX:** a `reqwest` connection-refused maps to an actionable error: `"cannot reach serve at <url> — is `a2a-bridge serve` running?"`.

## 5. Data flow (detached submit, end to end)
1. `a2a-bridge submit code-review --input diff.txt` → `message/send { skill:"code-review", message:{parts:[diff]} }`.
2. `serve` routes `RouteTarget::Workflow("code-review")` → `spawn_detached_workflow`: mint id → `create(Working)` → spawn finalizer-guarded runner → return `a2a::Task{working}`.
3. runner drains; on `Terminal{Completed, synth}` → `set_terminal(Completed, result=synth)`.
4. `a2a-bridge task get <id>` → `tasks/get` → `task_store.get` → `a2a::Task{completed, artifact=synth}`.

## 6. Decisions & rationale
- **Detached = workflows only.** Single-agent/`Local`/`Delegate` `message/send` stay synchronous. `message/stream` stays live (W1). Net split: **stream = live, send = detached** (workflow skills only).
- **`message/send` overload vs `blocking` flag.** Detached-for-workflows is A2A-conformant (returning a `working` Task is valid). We **keep** it, **advertise detached submit in the Agent Card** so the bifurcation is discoverable, and document A2A `MessageSendConfiguration.blocking` as the clean future disambiguator (optionally honored later). Not a hard door (single client today).
- **Unique id generation; non-clobbering `create`.** Real `a2a::new_task_id()` (not `task-1`); `create` errors on a duplicate id so a resubmit can't overwrite a terminal result.
- **Single-writer terminal rows, finalizer-guarded.** The runner alone writes a task's terminal status, and a drop-guard guarantees finalization even on panic; `cancel` is TaskStore-aware (§4.3) and never re-cancels a persisted terminal task.
- **One engine, CLI = A2A client** (the durable/detached path). `run-workflow` remains the separate in-process foreground path.
- **Single-serve-per-DB lock** so the boot sweep is safe.
- **`Interrupted`** is a distinct **durable** status (preserving crash-vs-failure for triage), collapsed to A2A `failed` + reason only at the wire.
- **DB path unset → in-memory.** Durability is opt-in; existing runs/tests unaffected until a path is set.
- **Retention:** keep all rows; `list` returns the most recent N (default 50). No auto-purge (YAGNI).

## 7. A2A surface semantics
- **`message/send`** on a workflow skill: **non-blocking**, returns a canonical `a2a::Task` with `status.state = working`. (Behavior change from W1's `InvalidRequest` reject — intended async semantics; the existing test asserting that reject is rewritten, see §9.)
- **`tasks/get`**: returns the canonical **`a2a::Task`** (`status.state`, `contextId`, `artifacts`) via the existing unary builder — **not** an ad-hoc `{task:{id,state}}`. Status mapping: `Working→working`, `Completed→completed`, `Failed→failed`, `Canceled→canceled`, `Interrupted→failed` (reason in the status message). Unknown id → current session-heuristic fallback.
- **`tasks/cancel`**: TaskStore-aware per §4.3.
- **list**: use the **canonical SDK list method** the `a2a` crate already provides (`ListTasks` / `tasks/list`) — do **not** invent a new method constant. Returns recent `TaskRecord`s projected into the SDK list shape; `limit` default 50, newest-first. The server is the single store-access path; the CLI is a thin consumer.

## 8. Error handling
- `create` fails (store error or **id collision**) → `message/send` returns a JSON-RPC error; no runner is spawned (fail-loud, no orphan).
- Runner internal error / panic → the finalizer writes `set_terminal(Failed, …)` and removes the token; `serve` stays up (§4.4).
- **Known gap (named):** if `set_terminal` itself fails to write after a *successful* run, that `Working` row is swept to `Interrupted`→`failed` on the next boot and the result text is lost — a success reported as failure. Acceptable for W3a; mitigation (write-retry) deferred. Logged at error.
- `task get` on unknown id → session-heuristic state (today's behavior), not a hard error.

## 9. Testing strategy
- **TaskStore unit tests** (in-memory + `MemoryTaskStore`): create→get round-trip; **duplicate `create` errors** (non-clobbering); set_terminal transitions; list ordering+limit; sweep flips only `Working`.
- **File-backed durability test:** `open(path)`, create+set_terminal, **drop** (releases lock), `open(path)` again, `get` returns the terminal record. Plus: a second `open(path)` while the first is held **fails** (single-serve lock).
- **Server tests** (over fakes) — made **deterministic** with a test-gated fake backend (gate held → assert `working`; release → await completion → assert terminal), **not** a sleep race; expose the runner's completion `Notify`/`JoinHandle` for the test to await:
  - `message/send` on a workflow returns a `working` id while the fake is gated (return-before-terminal);
  - after release, `tasks/get` returns `completed` + artifact;
  - `tasks/cancel` on a gated task → `Canceled`; cancel on an already-terminal task → returns its true state, default backend untouched;
  - runner **panic** → finalizer writes `Failed` (no orphan `Working`);
  - boot `sweep_interrupted` flips a pre-seeded `Working` row to `Interrupted`, and `tasks/get` reports `failed` + reason.
- **Ripple:** the existing test asserting unary-workflow → `InvalidRequest` (`workflow_producer.rs:529`) is **rewritten** to assert the detached `working` return; any test depending on the synthesized `task-1` id is updated for real id-gen.
- **CLI smoke:** `submit` prints an id; `task get` reads it back; serve-down → actionable error.
- **Gated live DoD** (real agents): submit a real `code-review` detached, poll `task get` until `completed`, retrieve the synth.

## 10. Definition of Done (numbered, falsifiable)
1. `bridge-core::TaskStore` trait + `TaskRecord`/`TaskRecordStatus` + **`MemoryTaskStore`** compile and are documented; `bridge-a2a-inbound` gains **no** `bridge-store` dependency.
2. `SqliteStore::open(path)` exists; `tasks` table idempotent; `open_in_memory` retained; second concurrent `open(path)` **fails** (single-serve lock).
3. `SqliteStore` implements `TaskStore`; `create` is **non-clobbering** (duplicate id errors); unit tests for create/get/set_terminal/list/sweep pass.
4. File-backed reopen test: a terminal record survives `drop`+`open` of the same path.
5. `InboundServer` carries `task_store` (defaulting to `MemoryTaskStore`) via `.with_task_store`; existing `new` call sites unchanged; build green.
6. `message/send` on a workflow returns a `working` **canonical `a2a::Task`** id **without blocking** — asserted **deterministically** via a gated fake (return-before-terminal), not a sleep.
7. Background runner writes the correct terminal record for Completed / Failed / Canceled; a **runner panic** is finalized to `Failed` (no orphan `Working` row).
8. `tasks/get` returns a **canonical `a2a::Task`** (state + artifact) for a completed detached task; non-workflow `tasks/get` unchanged.
9. `tasks/cancel` is TaskStore-aware: gated Working → `Canceled`; already-terminal → returns true state with the default backend untouched.
10. Boot `sweep_interrupted` flips stale `Working`→`Interrupted`; `tasks/get` then reports `failed` + reason.
11. CLI `submit` / `task get` / `task list` (SDK list method) / `task cancel` work against a running `serve`; serve-down → actionable error.
12. `[store] path`: set → durable across restart (terminal tasks retrievable after `serve` restart); unset → `MemoryTaskStore` (unchanged behavior).
13. The existing unary-workflow-`InvalidRequest` test is rewritten; no other test left red by id-gen.
14. **Gated live:** a real `code-review` submitted detached completes and its synth is retrievable via `task get`.
15. `cargo fmt`/`clippy -D warnings` clean; full suite green; coverage floors held (workspace ≥85, bridge-core ≥90 incl. `TaskStore`/`MemoryTaskStore`, `bridge-store` `TaskStore` impl unit-tested, all others unchanged).

## 11. Firewall
Designed from the bridge's own ports (`SessionStore` pattern, `RouteTarget::Workflow`, `WorkflowExecutor`, `workflow_cancels`) and **A2A's own task-lifecycle semantics** (`message/send` non-blocking → `tasks/get` polling → canonical `a2a::Task` / SDK `ListTasks`). The `~/code/a2a-local-bridge` PoC's submit/task **schema and methodology do not inform this design** (black-box only). `~/code/agent-knowledge` is readable.

## 12. Out of scope (explicit non-goals for W3a)
No per-event/node history (W3b / durability option 2); no crash-resume of in-flight work (W3b / option 3 — in-flight at restart → `Interrupted`); no retention/cleanup policy; no auth/authz; no detached submit for non-workflow skills; no `set_terminal` write-retry (named gap, §8).

## 13. Follow-ons
- **W3b:** node-output checkpointing (`task_node_outputs` table) + resume-on-restart. The resume *seam* is clean (the executor's `while done.len() < nodes.len()` loop absorbs a pre-seeded done/outputs set), but **capture is not free**: `WorkflowEvent::NodeFinished{node, ok}` carries no node output today, so W3b needs an **additive executor event change** (e.g. `NodeFinished { node, ok, output }`). Pairs naturally with W4's long pipelines.
- **W2:** structured/typed review output (independent of W3).
- The benign `usage_update` ACP SDK gap (SDK bump).
- Possible later: honor A2A `MessageSendConfiguration.blocking` for an explicit sync/async choice; `set_terminal` write-retry.
