# ADR-0011 — Crash-Resume for Detached Workflows (W3b)

**Date:** 2026-06-03
**Status:** Accepted

**Builds on:** ADR-0010 (durable detached submit / W3a), ADR-0009 (workflow-DAG orchestration / W1), and the live-validated `code-review`/`spec-review`/`plan-review` workflows. Second increment of the **W3** durable-task program; the substrate that turns a `serve` restart from "give up on in-flight work" into "pick up where it left off."

---

## Context

W3a made a detached workflow's *result* durable: `message/send` on a workflow persists a `Working` row and a finalizer-guarded background runner writes the terminal result. But an in-flight workflow that was `Working` when `serve` restarted was **swept to `Interrupted`** (`sweep_interrupted`) — all the work its fan-out legs had already done was thrown away, and a long fan-out review had to start from scratch. For the self-hosting goal (replacing the `a2a-local-bridge` PoC), a restart during a multi-reviewer run must not discard the reviewers that already finished.

W3b adds **crash-resume**: on boot, an in-flight detached workflow re-runs **only its not-yet-finished nodes**, reusing the outputs of the nodes that already completed. The substrate it needs is **per-node output history** captured durably as the run progresses.

## Decision

**Auto-resume in-flight detached workflows on `serve` boot, from per-node checkpoints.** Five parts:

1. **Completion-driven executor scheduling.** Replace the executor's `join_all` ready-batch loop with `FuturesUnordered`, so each fan-out leg's `NodeFinished` (and thus its checkpoint) lands the instant that leg finishes — not when its slowest sibling does. (Adopted in v1 as an isolated first commit; see *Dual-review*.)
2. **Additive `NodeFinished{output}` over a fallible `WorkflowSink`.** The executor event carries the node's output text; `WorkflowSink` methods return `Result`, and `drain_workflow` aborts the run on a sink-write failure (so a checkpoint-write failure fails the task rather than silently losing history).
3. **A pure `run_from(seed)` resume entry.** `WorkflowExecutor::run_from(graph, input, run_id, cancel, seed)` pre-loads already-completed node outputs as plain data; `run()` is `run_from` with an empty seed. The executor stays pure (no store/clock/IO).
4. **A lean `task_node_checkpoints` table + a versioned graph snapshot.** A `(task_id, node_id)` write-once checkpoint table (FK→`tasks` `ON DELETE CASCADE`), plus four additive `tasks` columns (`input`, `workflow_spec_json`, `resume_attempts`, `last_resume_ms`) added by an idempotent migration. Submit persists the request `input` and a versioned `{"v":1,"graph":<resolved graph>}` snapshot.
5. **A boot resume routine on `InboundServer` with an attempt cap.** `resume_working_tasks(&srv, cap)` runs before the listener binds and **replaces** `sweep_interrupted` for the durable path.

## Components

- **`bridge-workflow`:** `WorkflowEvent::NodeFinished` widened to `{node, ok, output}`; `run()` rewritten to completion-driven scheduling over `FuturesUnordered` with a `stop_scheduling` **drain-on-cancel** (every in-flight leg still runs its `backend.cancel()`/`forget_session()` cleanup); `run_from(seed)` + seed validation (unknown-node + downward-closure); serde derives on `WorkflowGraph`/`WorkflowNode` (for the snapshot).
- **`bridge-core`:** `TaskStore` gains `put_node_checkpoint` (write-once), `node_checkpoints`, `claim_resume_attempt(cap) → ResumeClaim{Resumable{attempt}|Exhausted}` (atomic increment-or-exhaust), and `working_tasks() → Vec<TaskRecord>`; `TaskRecord` gains `input`/`workflow_spec_json`/`resume_attempts`; `MemoryTaskStore` implements them, rejecting a checkpoint for an unknown task (matching the SQLite FK).
- **`bridge-store`:** `task_node_checkpoints` table + cascade FK; `PRAGMA foreign_keys=ON` per connection; an idempotent `migrate_tasks_columns` (`PRAGMA table_info`-guarded `ALTER`) that upgrades a W3a (7-column) DB in place; plain-`INSERT` (write-once) checkpoints; an atomic `claim_resume_attempt` transaction.
- **`bridge-a2a-inbound`:** `TaskStoreSink` writes a checkpoint per finished node (a write failure aborts the drain → task `Failed`, with the underlying error logged); the detached arm persists `input` + the versioned snapshot at submit; `spawn_detached_workflow` takes its **final shape** (injected `graph`, `run_id`, `seed`) so resume and fresh-submit share one path; `resume_working_tasks` implements the per-task branch ladder and logs each decision.
- **`bin/a2a-bridge`:** `[store] resume_attempt_cap` (default 3); `serve` boot order is open store → build `InboundServer` → **resume → bind** (`sweep_interrupted` removed from the durable path).

## Key design decisions (and why)

- **Resume re-runs only un-checkpointed nodes.** `run_from` seeds `done`/`outputs` from the checkpoints; the completion-driven scheduler then naturally skips seeded nodes and runs the rest. A checkpoint set is always **downward-closed** (a node only checkpoints after the write-ahead barrier has made its inputs' checkpoints durable), so the seed-closure validation never rejects a legitimate resume.
- **Write-ahead barrier.** The executor yields `NodeFinished` before scheduling dependents, and `drain_workflow` awaits the sink (the checkpoint write) between yields — so a downstream node is never polled until its upstream's checkpoint is durable. Crash-safety follows: a node that ran is either fully checkpointed or re-run from scratch, never half-applied.
- **Checkpoints are write-once.** `(task, node)` is written exactly once in every correct path; a duplicate is a resume-seeding bug we *want* surfaced (plain `INSERT` errors; `MemoryTaskStore` matches). The trait doc and both impls were aligned to this after a reviewer initially read the looser W3a doc as upsert intent.
- **Resume-attempt cap = poison-pill guard.** A workflow whose resumed run reliably crashes the server would otherwise resume→crash forever (an unbootable server). `claim_resume_attempt` increments a durable counter each boot; at `cap` the task is marked `Interrupted`. Deterministically proven to terminate.
- **Terminal-checkpoint short-circuit narrows the ADR-0010 §8 gap.** If a crashed task's *terminal* node already has a checkpoint (its output was produced but the row wasn't flipped due to a `set_terminal` write failure), the next boot finalizes it directly (`ok`→Completed / `!ok`→Failed) **without prompting and without consuming an attempt** — recovering the orphaned `Working` row the W3a sweep would have called `Interrupted`.
- **Executor stays pure / snapshot stays opaque.** `run_from`'s seed is plain data and the snapshot is opaque `TEXT` in the `TaskStore` port — no `bridge-workflow` dependency leaks into the store, preserving the hexagonal boundary.
- **Versioned snapshot envelope = forward-compat door.** `{"v":1,...}`; an unknown version (or any unparseable snapshot) → `Interrupted`, never a panic on boot.

## Dual-design + dual-review provenance

W3b's design used a **dual independent architecture** experiment: Claude and Codex each produced a firewalled clean-room design from the brainstorm; the two converged on the spine (validating it), and the best of both was merged before the usual dual review.

- **Completion-driven-as-v1 decision.** The merged-design review split: Codex recommended *deferring* completion-driven scheduling to a later slice; Claude recommended *adopting* it in v1. Claude's argument was decisive — the flagship workflows are fan-out (parallel reviewers), so a `join_all`-resume would re-run *all* parallel legs on a mid-fan-out crash, the most likely crash window. Adopted in v1 as an isolated first commit (later split 1a/1b so the additive field and the scheduler rewrite land separately).
- **Plan-review blockers folded (rev2).** The Codex+Claude plan review caught, among others: the **cancel `break` regression** — under `FuturesUnordered`, breaking on cancel drops in-flight siblings *mid-cleanup*, stranding ACP sessions; fixed with a `stop_scheduling` **drain** + a 2-leg cancel test that fails under `break` and passes under drain. Also: the missing **write-ahead-barrier test** (DoD-1); a green-per-task break where Task 4's trait methods broke the SQLite impl (fixed with compile-stubs); a missed `bin/.../main.rs` `NodeFinished` consumer (+ `cargo check --workspace` per task); the `spawn_detached_workflow` signature settled once (input/graph/run_id/seed); plain `INSERT` over `INSERT OR REPLACE`; the redundant `WorkingTask` struct dropped (`working_tasks() → Vec<TaskRecord>`).
- **Firewall clean:** designed from the bridge's own ports + the executor's topo model; the `a2a-local-bridge` PoC's checkpoint schema did not inform the design.

## Live-gate results (real agents, through `serve`)

`serve` (file-backed `[store]`) was driven end-to-end against real `codex-acp` + `claude-agent-acp` with the `code-review` workflow (`codex`,`claude`→`synth`):

- `submit code-review` → a UUIDv7 task id immediately; both reviewers ran in parallel and **checkpointed** (`codex`, `claude` rows in `task_node_checkpoints`), `synth` still pending, task `Working`.
- **Crash simulated:** `serve` was killed with `synth` not yet run; the row stayed `Working`, `resume_attempts=0`.
- **Restart resumed (not swept):** the boot log emitted `resume scan: resumed from checkpoints` (`attempt=1`, `run_id=…-resume-1`).
- **Checkpointed reviewers were NOT re-prompted:** the `codex` and `claude` checkpoint timestamps were **byte-identical** across the restart; only `synth` got a **new** checkpoint — it re-ran using the seeded reviewer outputs (its merged review cited a blocker the reviewers found).
- **Completed:** `tasks/get` over the wire returned `TASK_STATE_COMPLETED` with the result payload; `resume_attempts=1`.

## Consequences

- **A `serve` restart mid-review now resumes** instead of discarding finished reviewers — the durable-task program is restart-survivable, not just result-durable.
- **Coverage held:** workspace **90.32%** lines, `bridge-core` **98.14%**, `bridge-workflow` **92.86%** — all floors (85/90/90) green; full workspace test suite passes; `cargo fmt`/`clippy -D warnings` clean.
- **Backward compatible:** the migration upgrades a W3a DB in place (old rows intact, defaulted); durability stays opt-in via `[store] path`.

## Follow-ons

- **Streaming reattach:** a client that submitted detached cannot yet re-subscribe to a resumed run's live SSE (only poll `tasks/get`).
- **Retry policy:** a *failed* (not crashed) node is not retried; resume only re-runs nodes that never finished. Per-node retry is a separate policy.
- **Node-history query API:** `node_checkpoints` is internal; there is no wire verb to inspect a task's per-node outputs.
- **Retention/prune:** checkpoints are retained, not pruned; a retention/prune policy is deferred (cascade-on-task-delete is in place).
- The named ADR-0010 **§8 gap** is *narrowed* (terminal-checkpoint short-circuit) but not closed for the non-terminal write-failure case (still relies on the cap + Interrupted).
