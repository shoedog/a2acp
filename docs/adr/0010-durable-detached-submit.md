# ADR-0010 — Durable Detached Submit (W3a)

**Date:** 2026-06-02
**Status:** Accepted

**Builds on:** ADR-0009 (workflow-DAG orchestration / W1), the live-validated `code-review`/`spec-review`/`plan-review` workflows, and the existing `bridge-store` SQLite layer. First increment of the **W3** durable-task program; the keystone toward retiring the `~/code/a2a-local-bridge` PoC for the real dual-review workflow.

---

## Context

After W1, the bridge's review workflows ran **synchronously / streaming-only**: `message/stream` emitted live SSE and `run-workflow` blocked in the foreground; a workflow's result was **never persisted** and `get_task` was a stub. The self-hosting goal (ADR-0008 re-trigger #5) needs the PoC's defining capability: **submit a long review detached, let it run in the background, retrieve the result later** — surviving the submitting client's exit. W3a is the first, ruthlessly-scoped slice: *result-durable* detached submit (per-event history + crash-resume deferred to W3b).

## Decision

**Add server-backed durable detached submit.** `serve` is the engine; the CLI is a thin A2A client of it. `message/send` on a **workflow** skill becomes **non-blocking**: it mints a unique task id, persists a `Working` row, spawns a finalizer-guarded background runner, and returns a canonical `a2a::Task{working}` immediately. `tasks/get`/`tasks/cancel`/`ListTasks` read the durable store; a `reqwest` CLI (`submit` / `task get|list|cancel`) wraps them. Non-workflow `message/send` stays synchronous; `message/stream` stays live. Net split: **stream = live, send = detached** (workflow skills only).

## Components

- **`bridge-core`:** new `TaskStore` port (`TaskRecord` + `TaskRecordStatus{Working,Completed,Failed,Canceled,Interrupted}`; timestamps passed in — core forbids `Date::now`) + an in-memory `MemoryTaskStore`. The in-memory default lives in `bridge-core` so `bridge-a2a-inbound` gains **no `bridge-store` dependency**.
- **`bridge-store`:** file-backed `SqliteStore::open(path)` with a single-serve advisory lock (`<path>.lock` via `fs2`); `impl TaskStore` over a new `tasks` table; `create` is a **non-clobbering INSERT** (duplicate id errors — never an upsert, unlike `SessionStore::put`).
- **`bridge-a2a-inbound`:** `task_store` field + `.with_task_store`; a **`WorkflowSink`** abstraction with one `drain_workflow` shared by the streaming producer (SSE sink) and the new **detached runner** (TaskStore sink); a **`Finalizer`** drop-guard (writes `Failed` on any non-terminal exit incl. panic, removes the cancel token); `message/send` workflow → detached; `tasks/get` returns a canonical `a2a::Task` (+ result/error as artifact); `tasks/cancel` is TaskStore-aware (terminal → true state, never re-cancel a backend); a `ListTasks` handler; the Agent Card advertises detached submit on workflow skills.
- **`bin/a2a-bridge`:** `[store] path` config; `serve` opens the file-backed store (lock + boot `sweep_interrupted`) when set, else `MemoryTaskStore`; the `submit`/`task` CLI subcommands.

## Key design decisions (and why)

- **One engine, two faces.** No second run/persistence path; any A2A client gets the same lifecycle. `run-workflow` remains a separate in-process foreground path (three executor call sites total; the streaming and detached sinks share the drain).
- **Single-writer terminal rows, finalizer-guarded.** The runner alone writes a task's terminal status; `tasks/cancel` only fires the token (the runner observes `Terminal{Canceled}`). `Finalizer.done = true` is set immediately after the terminal write with no intervening `.await`, so the guard and a successful write can never both fire. A panic mid-run is finalized to `Failed` (validated by a live-panic test).
- **Unique ids, non-clobbering create.** `a2a::new_task_id()` (UUIDv7) — NOT `task_id_from_params`, which returns a fixed `"task-1"`. `create` errors on a colliding id so a resubmit can't overwrite a terminal result — this is what makes single-writer actually hold.
- **Single-serve-per-DB lock** makes the boot sweep safe: `sweep_interrupted` flips stale `Working`→`Interrupted` on every `serve` start, which is only correct because the exclusive lock guarantees no other live serve owns those rows.
- **`Interrupted` is a distinct *durable* status** (preserving crash-vs-failure for triage), collapsed to A2A `failed` + reason only at the wire (via the artifact).
- **Durability is opt-in:** DB path unset → `MemoryTaskStore` (ephemeral), so existing runs/tests are byte-for-byte unaffected until `[store] path` is set.

## Dual-review corrections (spec rev2 + plan rev2)

The detached Codex+Claude reviews of the spec and plan caught real issues, including correcting two of the controller's own grounding assumptions:
- **`a2a::new_task_id()` and `a2a::methods::LIST_TASKS` DO exist** in `a2a-lf 0.3.0` (the in-repo Explore only checked *usage*; the reviewers read the crate). → no `uuid` dep; no invented `tasks/list` constant; the CLL and server share the SDK `methods::*` + `SVC_PARAM_VERSION` constants on **both** ends (a CLI using slash-style strings / `X-A2A-Version` would 404 every verb).
- `a2a::TaskStatus.message` is `Option<a2a::Message>` (not `Option<String>`) → `get_task` keeps it `None` and surfaces the error via an artifact.
- The unary-workflow→`InvalidRequest` test was rewritten to the detached `working` return in the **same atomic commit** as the server change.
- The gated submit test uses an `AtomicBool` flag (not bare `notify_waiters()`, which would hang the late-parking `synth` node).
- `tasks/cancel` made TaskStore-aware (the old fall-through would cancel the default backend and report `CANCELED` while the durable row disagreed); the `MemoryTaskStore`-in-`bridge-core` crate-boundary fix; the finalizer-guard design; the missing Agent-Card advertisement task.

## Live-gate results (real agents, through `serve`)

`serve` (with `[store] path`) was driven end-to-end against real `codex-acp` + `claude-agent-acp`:
- `submit code-review` → a UUIDv7 task id immediately; `task get` polled `WORKING`→`COMPLETED` in ~64s; the synth artifact (a high-quality merged review) was retrievable via `tasks/get` as a canonical `a2a::Task`.
- **Durable across restart:** after killing + restarting `serve`, the completed task's result was still retrievable from the SQLite file.
- **Interrupted:** a task killed mid-run was swept to `TASK_STATE_FAILED` with reason `"interrupted (serve restarted)"` on the next boot.
- `task list` showed both rows; the restart re-acquired the single-serve lock.

## Consequences

- **The dual-review loop is now self-hostable *detached*** — the keystone toward replacing the a2a-local-bridge PoC for the real workflow.
- **Coverage held:** workspace 90.35% lines, `bridge-core` 96.45% (incl. the new `task_store` module), `bridge-workflow` 91.68%, all floors green; 39 test binaries pass; `cargo fmt`/`clippy -D warnings` clean.
- **Firewall clean:** designed from the bridge's own ports + A2A's task-lifecycle semantics; the PoC's submit/task schema did not inform the design.

## Follow-ons

- **W3b:** node-output history (`task_node_outputs` table) + resume-on-restart. The resume *seam* is clean (the executor's `while done.len() < nodes.len()` loop), but **history capture needs an additive executor event change** — `WorkflowEvent::NodeFinished` carries no node output today. Pairs with W4's long pipelines.
- The named **§8 gap**: a `set_terminal` write-failure on the happy path orphans a `Working` row until the next boot sweep (mitigation: write-retry, deferred).
- Optionally honor `MessageSendConfiguration.blocking` for an explicit sync/async choice instead of the route-target overload.
- **W2:** structured/typed review output (independent of W3).
