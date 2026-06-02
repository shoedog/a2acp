# W3a — Durable Detached Submit (design spec)

**Date:** 2026-06-02
**Status:** Draft (pre dual-review)
**Builds on:** ADR-0009 (workflow-DAG orchestration / W1), the live-validated `code-review` / `spec-review` / `plan-review` workflows, and the existing `bridge-store` SQLite layer.

---

## 1. Context & problem

The bridge's review workflows are live-validated but run **synchronously / streaming-only**: `message/stream` emits live SSE and `run-workflow` blocks in the foreground. A workflow's result is **never persisted** — when the stream ends, the output is gone. `get_task` is a stub: it reports `WORKING` if a `task↔session` mapping exists, else `SUBMITTED`, and returns no result.

The self-hosting goal (ADR-0008 re-trigger #5: replace the `~/code/a2a-local-bridge` PoC) requires the bridge's defining missing capability: **submit a long review detached, let it run in the background, and retrieve the result later** — surviving the submitting client's exit.

This spec is **W3a**: the first, ruthlessly-scoped slice — *result-durable* detached submit. Per-event history and crash-resumption are explicitly deferred to **W3b** (see §13).

## 2. Goal

Run a workflow **detached**: submit it, get a task id immediately, and retrieve its **persisted** status + result later (including after the submitting client exits, and across a `serve` restart **for already-terminal tasks**). Surfaced via the **A2A task lifecycle** and a **thin CLI** that is a client of `serve`.

## 3. Architecture — one engine, two faces

`serve` is the engine. There is **no second run path**: the CLI is a thin A2A client of `serve`.

```
  a2a-bridge submit code-review --input diff.txt
        │  POST message/send {skill, message}
        ▼
  serve (long-lived)
        │  mint TaskId, TaskStore.create(working), return {task: working} immediately
        │  ── spawn background runner ──▶ WorkflowExecutor.run(...)
        │                                     │ on Terminal: TaskStore.set_terminal(status, artifact|error)
        ▼                                     ▼
  TaskStore (SQLite, file-backed) ◀───────────┘
        ▲
  a2a-bridge task get <id>   ─ POST tasks/get   ─▶ serve reads TaskStore ─▶ state (+ artifact when terminal)
  a2a-bridge task list       ─ POST tasks/get?…  (list)
  a2a-bridge task cancel <id>─ POST tasks/cancel ─▶ fire workflow_cancels token; runner writes Canceled
```

The synchronous `run-workflow` CLI is **unchanged** and remains for foreground one-offs. `message/stream` for a workflow is **unchanged** (live SSE, W1). The new detached path is the **`message/send`** path for a workflow.

## 4. Components & responsibilities

### 4.1 `bridge-core` — new `TaskStore` port
A new trait, **separate from `SessionStore`** (different responsibility; both implemented by the same SQLite type).

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
    async fn create(&self, rec: &TaskRecord) -> Result<(), BridgeError>;        // status=Working
    async fn set_terminal(&self, id: &TaskId, status: TaskRecordStatus,
                          result: Option<&str>, error: Option<&str>, updated_ms: i64)
                          -> Result<(), BridgeError>;
    async fn get(&self, id: &TaskId) -> Result<Option<TaskRecord>, BridgeError>;
    async fn list(&self, limit: usize) -> Result<Vec<TaskRecord>, BridgeError>; // newest-first
    async fn sweep_interrupted(&self, updated_ms: i64) -> Result<u64, BridgeError>; // Working -> Interrupted, returns count
}
```

Timestamps are **passed in** (the core forbids `Date::now`); callers (server/CLI) stamp them.

### 4.2 `bridge-store::sqlite` — file-backed + `TaskStore` impl
- Add `SqliteStore::open(path: &Path) -> Result<Self, BridgeError>` (file-backed; creates/opens the DB, runs `CREATE TABLE IF NOT EXISTS`). Keep `open_in_memory()` for tests and the default (unset-path) serve.
- Implement `TaskStore` on `SqliteStore` (it already implements `SessionStore`). New table:

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

**Forward-compatibility (W3b):** node-level history goes in a *separate* future table (`task_node_outputs(task_id, node_id, seq, output, ts)`); the `tasks` table above does not change when W3b lands. No migration of `tasks` later.

### 4.3 `bridge-a2a-inbound::server`
- `InboundServer` gains `task_store: Arc<dyn TaskStore>` (always present; in-memory by default), wired via a `.with_task_store(...)` builder (existing `new` call sites untouched, mirroring `with_workflows`).
- **`message/send` (unary) on a workflow skill** — today rejects `InvalidRequest`; now → **`spawn_detached_workflow`**:
  1. obtain the `TaskId` via the existing `task_id_from_params` (client-supplied id, else generated);
  2. extract input text from the message parts (same extraction the streaming workflow producer uses);
  3. `task_store.create({Working, workflow, created=updated=now})`;
  4. register a `CancellationToken` in `workflow_cancels[task]`;
  5. `tokio::spawn` the **background runner** (§4.4);
  6. return `{ "task": { "id": …, "state": "TASK_STATE_WORKING" } }` **immediately** (non-blocking).
- **`tasks/get`** — consult `task_store.get(id)` first:
  - found → map `TaskRecordStatus` → A2A state (§7) and include the artifact (from `result`) when terminal;
  - not found → **current behavior** (session-mapping heuristic) preserved for non-workflow tasks.
- **`tasks/cancel`** — unchanged W1 behavior (fire the `workflow_cancels` token); the **runner** is the sole writer of the terminal `Canceled` row (single-writer; no cancel-vs-runner race).
- **boot** — after opening the store, `task_store.sweep_interrupted(now)` flips any stale `Working` rows to `Interrupted` (a prior `serve` died mid-run).

### 4.4 The background runner (`spawn_detached_workflow`'s task)
Owns one detached workflow run. It is the **sole writer** of that task's terminal row.
1. `WorkflowExecutor::run(graph, input, run_id = task_id, cancel_token)` → `WorkflowStream`.
2. Drain the stream. W3a **ignores** intermediate `NodeStarted`/`NodeFinished` (no history) and captures the single `Terminal { outcome, output }`.
3. On terminal: `task_store.set_terminal(task, map(outcome), result|error, now)`:
   - `Completed` → status `Completed`, `result = output`;
   - `Failed` → status `Failed`, `error = output` (the degraded/marker text);
   - `Canceled` → status `Canceled`.
4. If the stream ends without a terminal (defensive): `Failed`, error `"workflow ended without terminal"`.
5. Always remove the `workflow_cancels[task]` token on exit.

### 4.5 `bin/a2a-bridge`
- **Config:** optional `[store] path = "…"`. Set → `serve` opens file-backed; unset → `open_in_memory()` (existing ephemeral behavior; tests unaffected). `serve` builds **one** `SqliteStore`, shares it as both `Arc<dyn SessionStore>` and `Arc<dyn TaskStore>` (the type implements both), and runs the boot sweep.
- **New CLI verbs** (thin `reqwest` JSON-RPC clients of a `serve` URL; default `http://127.0.0.1:8080`, override `--url`):
  - `submit <skill> --input <file> [--url <u>]` → POST `message/send`; print the task id.
  - `task get <id> [--url <u>]` → POST `tasks/get`; print status and (when terminal) the result artifact.
  - `task list [--url <u>] [--limit N]` → print recent tasks (id, workflow, status, updated).
  - `task cancel <id> [--url <u>]` → POST `tasks/cancel`.
  - (`task list` rides a `tasks/get`-family method; see §7.)

## 5. Data flow (detached submit, end to end)
1. `a2a-bridge submit code-review --input diff.txt` → `message/send { skill:"code-review", message:{parts:[diff]} }` to `serve`.
2. `serve` routes `RouteTarget::Workflow("code-review")` → `spawn_detached_workflow` → `create(Working)` → returns `{task:{id, state:working}}`.
3. background runner drives the executor; on `Terminal{Completed, synth}` → `set_terminal(Completed, result=synth)`.
4. `a2a-bridge task get <id>` → `tasks/get` → `task_store.get` → `{state:completed, artifact:{text:synth}}`.

## 6. Decisions & rationale (the approved defaults)
- **Detached = workflows only.** Single-agent/`Local` and `Delegate` `message/send` stay synchronous (they're short). `message/stream` stays live (W1). Net split: **stream = live, send = detached** (workflow skills only).
- **One engine, CLI = A2A client.** No second run/persistence path; any A2A client gets the same lifecycle. The "`serve` must be running" cost is intrinsic to self-hosting (the bridge is the review service).
- **`Interrupted` → A2A `failed`** with reason `"interrupted (serve restarted)"` (A2A has no "interrupted" state; `failed` + reason is the honest mapping).
- **Single-writer terminal rows.** The runner alone writes a task's terminal status; `cancel` only fires the token. Eliminates cancel-vs-completion races.
- **DB path unset → in-memory.** Durability is opt-in via config; existing runs/tests are byte-for-byte unaffected until a path is set.
- **Retention:** keep all rows; `list` returns the most recent N (default 50). No auto-purge (YAGNI; revisit when volume is real).

## 7. A2A surface semantics
- **`message/send`** on a workflow skill: **non-blocking**, returns a `working` task immediately. (Behavior change from W1's `InvalidRequest` reject — this is the intended async semantics, not a regression.)
- **`tasks/get`**: returns `{ task: { id, state, artifacts? } }`. Terminal records include the result as an A2A artifact (`{ text: result }`); failed/interrupted include the error in the artifact/status message. Status mapping: `Working→TASK_STATE_WORKING`, `Completed→TASK_STATE_COMPLETED`, `Failed→TASK_STATE_FAILED`, `Canceled→TASK_STATE_CANCELED`, `Interrupted→TASK_STATE_FAILED` (reason carried). Unknown id (not in TaskStore) → current session-heuristic fallback.
- **`tasks/cancel`**: unchanged (fires the `workflow_cancels` token).
- **`task list`** (decision): a bridge-specific server method **`tasks/list`** → `{ tasks: [{id, workflow, state, updated_ms}] }`, newest-first, with a `limit` param (default 50). The server is the single source of truth and the CLI is a thin consumer — list is a **server method, not a CLI-direct DB read**, so there is exactly one access path to the store.

## 8. Error handling
- Store unavailable at submit (`create` fails) → `message/send` returns a JSON-RPC error; no task is spawned (fail-loud, no orphan).
- Runner internal error / panic → `set_terminal(Failed, error)`; the token is removed; `serve` stays up.
- `set_terminal` write failure → logged at error; the in-memory run already finished, so the durable row may lag — acceptable for W3a (no history/resume to corrupt); surfaced in logs.
- `task get` on unknown id → `tasks/get` returns the session-heuristic state (today's behavior), i.e. not a hard error.

## 9. Testing strategy
- **TaskStore unit tests** (in-memory): create→get round-trip; set_terminal transitions; list ordering+limit; sweep flips only `Working`.
- **File-backed durability test:** `open(path)`, create+set_terminal, **drop**, `open(path)` again, `get` returns the terminal record (survives reopen).
- **Server tests** (over fakes): `message/send` on a workflow returns a `working` id **without blocking** (assert it returns before the fake workflow's terminal); after the background runner completes, `tasks/get` returns `completed` + artifact; `tasks/cancel` → record `Canceled`; boot `sweep_interrupted` flips a pre-seeded `Working` row to `Interrupted` and `tasks/get` reports `failed`+reason.
- **CLI smoke** (against a test `serve` or unit-level arg parsing + client): `submit` prints an id; `task get` reads it back.
- **Gated live DoD** (real agents): submit a real `code-review` detached, poll `task get` until `completed`, retrieve the synth artifact.

## 10. Definition of Done (numbered, falsifiable)
1. `bridge-core::TaskStore` trait + `TaskRecord`/`TaskRecordStatus` compile and are documented.
2. `SqliteStore::open(path)` exists; `tasks` table created idempotently; `open_in_memory` retained.
3. `SqliteStore` implements `TaskStore`; unit tests for create/get/set_terminal/list/sweep pass.
4. File-backed reopen test: a terminal record survives `drop`+`open` of the same path.
5. `InboundServer` carries `task_store` via `.with_task_store`; existing `new` call sites unchanged; build green.
6. `message/send` on a workflow returns a `working` task id **without blocking** (test asserts return-before-terminal).
7. Background runner writes the correct terminal record for Completed / Failed / Canceled (tests).
8. `tasks/get` returns the persisted state + artifact for a completed detached task; non-workflow `tasks/get` behavior unchanged.
9. Boot `sweep_interrupted` flips stale `Working`→`Interrupted`; `tasks/get` then reports `failed` + reason.
10. CLI `submit` / `task get` / `task list` / `task cancel` work against a running `serve`.
11. `[store] path` config: set → durable across restart (terminal tasks retrievable after `serve` restart); unset → in-memory (unchanged behavior).
12. **Gated live:** a real `code-review` submitted detached completes and its synth is retrievable via `task get`.
13. `cargo fmt`/`clippy -D warnings` clean; full suite green; coverage floors held (workspace ≥85, bridge-core ≥90 incl. the new `TaskStore` trait, the `bridge-store` `TaskStore` impl unit-tested, all other floors unchanged).

## 11. Firewall
Designed from the bridge's own ports (`SessionStore` pattern, `RouteTarget::Workflow`, `WorkflowExecutor`, `workflow_cancels`) and **A2A's own task-lifecycle semantics** (`message/send` non-blocking → `tasks/get` polling). The `~/code/a2a-local-bridge` PoC's submit/task **schema and methodology do not inform this design** (black-box only). `~/code/agent-knowledge` is readable.

## 12. Out of scope (explicit non-goals for W3a)
No per-event/node history (W3b / durability option 2); no crash-resume of in-flight work (W3b / option 3 — in-flight at restart → `Interrupted`); no retention/cleanup policy; no auth/authz on the A2A surface; no detached submit for non-workflow skills.

## 13. Follow-ons
- **W3b:** node-output checkpointing (`task_node_outputs` table) + resume-on-restart (executor `resume(graph, completed-node-outputs)` entry point + boot reschedule) — pairs naturally with W4's genuinely long pipelines.
- **W2:** structured/typed review output (independent of W3).
- The benign `usage_update` ACP SDK gap (SDK bump).
