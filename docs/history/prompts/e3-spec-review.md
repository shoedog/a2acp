You are doing a rigorous, adversarial SPEC REVIEW (read-only) of "E3 — Parallel Batch Dispatch" for the a2a-bridge
(a Rust A2A↔ACP bridge + multi-agent workflow orchestrator). READ-ONLY: read the spec + the real code with read-only
tools; do NOT edit/build/test. Be terse and end with a bounded STOP.

The spec: `docs/superpowers/specs/2026-06-26-e3-batch.md`. E3 lets `a2a-bridge run-batch <workflow> --manifest <file>`
submit N INDEPENDENT workflow runs to a running `serve`, run them concurrently under a SERVE-WIDE concurrency cap,
durably. Each child is a normal detached task (reuses `spawn_detached_workflow` → W3b-resumable, E1-worktree-isolated,
cancelable). A durable `batch` parent record gives one-query status, cancel-the-batch, and crash-safe tail-resume. The
serve-wide cap is ONE shared `Semaphore` gating batch children only (global across batches); non-batch single runs stay
immediate (deferred).

Binding context — VERIFY every anchor against the real code (the spec cites these `file:line`s; confirm or correct):
- `crates/bridge-coordinator/src/coordinator.rs:359` — `run_workflow(OpParams) -> TaskId` (the per-job path E3
  reuses N times): mints `new_detached_task_id`, builds a `TaskRecord` (status Working, `workflow_spec_json`), calls
  `task_store.create`, registers a `TaskProgressHub` + a `workflow_cancels` `CancellationToken`, then
  `spawn_detached_workflow(...)`. `Coordinator::new` (`:127`), `cancel_task` (`:470`), `status` (`:424`), `resume`
  (`:499` → `resume_working_tasks`).
- `crates/bridge-coordinator/src/detached.rs` — `new_detached_task_id` (`:1120`), `spawn_detached_workflow`,
  `resume_working_tasks` (`:1423`, the W3b boot routine: completion-driven `FuturesUnordered`, `run_from(seed)`,
  per-node checkpoints, `claim_resume_attempt` poison cap), `encode_workflow_spec`.
- `crates/bridge-core/src/task_store.rs` — `TaskRecord` (`:60`: id, workflow, status, result, error, created_ms,
  updated_ms, input, workflow_spec_json, resume_attempts, session_cwd), `TaskStore` trait (`:95`: create,
  set_terminal, get, list, sweep_interrupted, cancel_if_working, put_node_checkpoint, node_checkpoints,
  claim_resume_attempt, working_tasks, record_node_started, set_terminal_sequenced, progress_snapshot). E3 ADDS
  `BatchRecord`/`BatchStatus`/`BatchSummary` + `batch_id`/`item_id` on `TaskRecord` + ~6 trait methods.
- `crates/bridge-core/src/ids.rs:5` — the `id_newtype!` macro (`TaskId` etc.); E3 adds `BatchId`.
- `crates/bridge-a2a-inbound/src/server.rs:713` — the `match method` JSON-RPC dispatch (the `Session*` string arms at
  `:720-726`); E3 adds `RunBatch`/`BatchStatus`/`BatchList`/`CancelBatch`.
- `bin/a2a-bridge/src/config.rs:46` — `ServerConfig`; `RegistryConfig` (`:118`, has `server` + `worktrees`); E3 adds
  `[batch]`. `run-workflow --serve` POST path in `main.rs` (`~:2542`).
- W3b crash-resume + E1 worktree-per-session + Slice-8 (Coordinator IS the service; A2A/CLI/MCP thin adapters) are
  the load-bearing precedents the spec leans on.

{{input}}

GROUND every finding in a real `file:line`. Pressure-test:
1. **The admission loop (the core).** Can `run_batch_admission` (a `FuturesUnordered` of in-flight child
   `JoinHandle`s + a shared `Semaphore` + a pending work-list) actually be built from the existing
   `spawn_detached_workflow` + W3b drain shape WITHOUT breaking the per-child hub/cancel/checkpoint wiring? Is the
   `OwnedSemaphorePermit`-on-the-JoinHandle release-on-completion correct? Any deadlock (acquire-while-holding a
   store lock; the loop awaiting a child that itself blocks on a permit)? Does the loop correctly terminate (work-list
   empty AND FuturesUnordered drained → settle)?
2. **The serve-wide cap (the key new invariant).** Is ONE `Arc<Semaphore>` on the Coordinator the right realization
   of "global across batches"? Does it actually bound concurrency across TWO simultaneous batches (not just within
   one)? Is `min(per-batch concurrency, max_concurrent)` correct? Starvation/fairness risk (Q5)? Does the cap hold ON
   BOOT (D6/Q-cap-on-boot — see #4)?
3. **D4 — durable plan model.** Is `items_json` on the `BatchRecord` + create-child-on-admit sound, vs the rejected
   pre-create-`Pending`-children alternative? Does keeping children = only-admitted tasks (no new `TaskRecordStatus`)
   actually leave `resume_working_tasks`/`sweep_interrupted`/`list` semantics UNTOUCHED? Is the `(batch_id, item_id)`
   idempotency key enough to prevent double-spawn on a crash between create-row and spawn?
4. **D6 — resume division (the subtle correctness point).** MUST batch children skip the plain `resume_working_tasks`
   path (filter `batch_id IS NULL`) so `resume_working_batches` re-admits them THROUGH the semaphore — else the global
   cap breaks on boot (N>cap Working children all respawn at once)? Is the proposed split (`working_tasks()` returns
   non-batch only, vs a call-site filter — D6) right? Does `resume_working_batches` correctly recompute the
   un-submitted tail (items minus existing child item_ids) AND re-seed still-Working children via `run_from` with
   their checkpoints AND register fresh `workflow_cancels` tokens (Q3)?
5. **Cancel (Q2/Q6).** Does `cancel_batch` (stop admitting + cancel in-flight via `workflow_cancels`) compose with a
   child waiting on `acquire_owned()` (must abort the acquire, NOT create a row — Q2)? Does canceling an INDIVIDUAL
   child via the existing `cancel_task` wedge the loop (Q6)? Is the `Cancelling → Cancelled` settle race-free?
6. **Terminal-status ownership (Q1).** The loop sets `Completed`; if the process dies after the last child finished
   but before the write, the batch is stuck `Working` until boot re-settles. Is boot-settle sufficient, or is a lazy
   `batch_status` settle needed? Any way the batch is reported `Completed` while a child is still mid-flight?
7. **Plumbing + durability.** Additive SQLite migration (new `batch` table + 2 nullable `tasks` cols) — idempotent
   per the W3a/W3b pattern? Do the ~6 new `TaskStore` methods have a clean home on BOTH `SqliteTaskStore` +
   `MemoryTaskStore` + every test `impl TaskStore` (FailingCheckpointStore et al.)? Does the inbound `RunBatch` param
   shape (CLI resolves input_file→input + item_id defaults before sending) round-trip? Is `[batch]` opt-in (absent →
   reject) the right default?
8. **Scope + missing pieces.** Is the MVP cut (serve-only, manifest-only, continue-others, poll-not-SSE, batch-
   children-only cap) right? Are the deferrals (global-over-non-batch traffic, --fail-fast, glob, nested batches,
   MCP adapter, batch-retry) the correct cut-line, or does one hide a real gap? Walk D1–D8: any decision wrong or
   under-justified? Walk Q1–Q7: any that MUST be decided before planning (vs at-plan)? Any wrong `file:line`. Any
   spike needed (e.g. does a shared `Semaphore` + `FuturesUnordered` cleanly drive two concurrent fake-agent batches
   under a cap of 3)?

OUTPUT: a numbered findings list, each `BLOCKER | MAJOR | MINOR | NIT` + a real `file:line` + a concrete fix. Explicitly
rule on D1–D8 (agree/disagree+why) and answer Q1–Q7. End with `SPEC VERDICT: ready-to-plan | needs-revision |
needs-spike`. Then STOP.
