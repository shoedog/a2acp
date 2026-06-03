# W3b — Node checkpoint history + crash-resume (design spec)

**Date:** 2026-06-03
**Status:** rev1 — produced by a **dual independent clean-room design** (Claude + Codex/gpt-5.5, firewalled from each other), merged, then **dual-reviewed** (Codex correctness + Claude architecture); all blockers + the completion-driven-scheduling decision folded in. Ready for spec review → plan.
**Builds on:** ADR-0010 / W3a (durable detached submit), ADR-0009 / W1 (workflow-DAG orchestration).

---

## 1. Context & problem

W3a made detached workflows durable but **not resumable**: a workflow that is `Working` when `serve` restarts (crash / kill / deploy) is swept to `Interrupted` and its in-flight work is lost. The bridge's flagship workflows are **fan-out** (`code-review`/`spec-review`/`plan-review` = two lenses in parallel → a synth rollup), and the two reviewers run in parallel for the bulk of the runtime — the most likely crash window. W3b makes such a task **resume** from per-node checkpoints — re-running only nodes that had not finished, reusing the durable outputs of finished nodes — and ships the per-node durable history resume needs.

## 2. Goal

A detached workflow that is `Working` at `serve` restart **resumes automatically**: re-run only the not-yet-finished nodes, reuse finished nodes' outputs, reach a terminal state. Bounded by a resume-attempt cap (poison-task protection). Node-granularity checkpoint (an in-flight node re-runs from scratch — agents are non-idempotent).

## 3. Architecture (five parts; executor stays PURE)

The `WorkflowExecutor` remains registry-only (no store/clock/IO); persistence + resume orchestration live in the adapter (`bridge-a2a-inbound`) + composition root. Five parts, landed in the order below (part A is an isolated first commit):

**A. Completion-driven scheduling (isolated, behavior-preserving first step).**
Today the executor schedules a ready-batch and `join_all`s it — `NodeFinished` is emitted only after **every** sibling in the batch finishes. For a fan-out (`codex ∥ claude`), a crash while both run checkpoints **neither** → resume re-runs both. Replace the batch loop with **completion-driven** scheduling (`FuturesUnordered`): as each node future completes, emit its `NodeFinished`, update `done`+`outputs`, and schedule any newly-ready nodes. A finished fan-out leg is thus observable/checkpointable immediately, even while a sibling runs. **Invariant preserved:** the existing **write-ahead barrier** (the runner's `drain_workflow` awaits the sink before the executor is polled again, so a downstream node cannot run before its upstream checkpoint is durable — see §B) must hold under completion-driven scheduling: a newly-ready downstream node is not polled until the just-finished node's `NodeFinished` has been handled by the sink. This step is **behavior-preserving** for the final result + terminal outcome; only the *timing/ordering* of `NodeFinished` changes. The existing W1 streaming tests are the regression guard.

**B. Capture (additive event + a fallible sink).**
- Add `output` to the event: `WorkflowEvent::NodeFinished { node, ok, output }` (the executor already holds the text).
- `WorkflowSink`'s node methods return `Result<(), BridgeError>`, and `drain_workflow` **aborts on a sink error** (returning it). The detached runner's `TaskStoreSink` persists the checkpoint on `NodeFinished`; if that write fails, drain aborts and the runner writes the task `Failed` **without polling the executor again** (no continuation on volatile data). (Today the sink methods return `()` and drain can't stop — this is the required contract change.)
- **Write-ahead:** a node is "checkpointed" only once its row is durable; the barrier (§A) guarantees no downstream node ran from a non-durable upstream output. The unavoidable boundary: a node finishes in the agent, then the process dies before `NodeFinished` is emitted/persisted → that node re-runs on resume.

**C. Resume entry point (one code path).**
`WorkflowExecutor::run_from(graph, input, run_id, cancel, seed: HashMap<NodeId,(String,bool)>)`; `run()` = `run_from(…, empty)`. It seeds the internal `done`+`outputs` from `seed`, then the existing scheduler runs only not-done nodes. **Validate the seed at entry:** every seeded id must be a node in `graph` (reject unknown — a stale checkpoint for a removed node), and every seeded non-root node's `inputs` must all be seeded (closure) — a violation fails the run loudly rather than producing a wrong result. A re-run node renders its template from `input` + upstream outputs (all present) and mints a **fresh** session via a resume-qualified run id `"{task}-resume-{n}"` (under the executor's `workflow-{wf}-{node}-{run_id}` scheme) so re-runs never reuse a stale session.

**D. Durable schema (lean checkpoints + task bookkeeping).**
- **Checkpoint table** (write-once per finished node; the checkpoint AND the retained history):
  ```sql
  CREATE TABLE IF NOT EXISTS task_node_checkpoints (
      task_id     TEXT NOT NULL,
      node_id     TEXT NOT NULL,
      finished_ms INTEGER NOT NULL,
      ok          INTEGER NOT NULL,
      output      TEXT NOT NULL,
      PRIMARY KEY (task_id, node_id),
      FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE CASCADE
  );
  ```
  Continue-from-checkpoint semantics (§7) guarantee a node is never re-checkpointed (resume re-runs only never-finished nodes), so one row per `(task,node)` suffices — no `run_no`, no nullable-finished state machine, no partial index. Retained post-terminal (the "history"); pruning rides `ON DELETE CASCADE` when a future task-prune deletes the task row.
- **`tasks` additive columns** (idempotent `ALTER TABLE … ADD COLUMN`, each guarded by a `pragma table_info` presence check; old rows get the defaults):
  ```sql
  input              TEXT NOT NULL DEFAULT '',  -- the original submit input
  workflow_spec_json TEXT,                      -- versioned graph snapshot (NULL on pre-W3b rows)
  resume_attempts    INTEGER NOT NULL DEFAULT 0,
  last_resume_ms     INTEGER
  ```
- **`PRAGMA foreign_keys = ON`** is set on every `SqliteStore::open*` connection (today it is not) so the cascade is enforced.
- **`workflow_spec_json`** is a **versioned envelope** `{ "v": 1, "graph": <serialized WorkflowGraph> }` persisted at submit. Resume deserializes it; an **unparseable/unknown-version snapshot → mark the task `Interrupted`** (never panic) — a forward-compat guard. Requires `WorkflowGraph`/`WorkflowNode` to derive `serde::{Serialize,Deserialize}` (their ids already do; add the derives + a `serde` dep to `bridge-workflow`).
- **`TaskStore` port additions** (trait + `MemoryTaskStore` + `SqliteStore`): `put_node_checkpoint(task,node,output,ok,ts)`; `node_checkpoints(task) -> Vec<(NodeId,String,bool)>`; `claim_resume_attempt(task, cap, now) -> ResumeClaim{Resumable{attempt}, Exhausted}` (atomic bump-or-give-up, stamps `last_resume_ms`); `working_tasks() -> Vec<{id, workflow, input, spec_json, resume_attempts}>`; `create` persists `input` + `workflow_spec_json`. `MemoryTaskStore` implements all (trait completeness + tests; cross-restart resume is moot in-memory, but capture + the boot routine are unit-tested against it).

**E. Boot resume routine (on `InboundServer`, not `main.rs`).**
Resume orchestration needs the executor + the workflow map + snapshot deserialization + the detached runner — too rich for a composition-root one-liner. It lives as an `InboundServer` method (which already holds `executor`, `workflows`, `task_store`, `workflow_cancels`); `spawn_detached_workflow` is generalized to accept an **injected graph** (the deserialized snapshot) rather than always resolving from the live `workflows` map. `serve` boot: open the store under the `fs2` lock → build `InboundServer` → run the resume routine → bind the listener → serve. For each `Working` task:
1. Load row + `workflow_spec_json` + node checkpoints.
2. `workflow_spec_json IS NULL` (pre-W3b row, or a task with no snapshot) → `set_terminal(Interrupted, "not resumable")`.
3. Snapshot unparseable / unknown version → `set_terminal(Interrupted, "unreadable workflow snapshot")`.
4. The terminal node already has a **valid** checkpoint (`finished_ms`+`ok`+`output` present) → finalize directly from it (`ok=true`→`Completed`+output; `ok=false`→`Failed`+output) **without prompting any agent and without consuming an attempt** — recovers a completed-but-unpersisted terminal (narrows the W3a `set_terminal`-write-failure gap, ADR-0010 §8).
5. Else `claim_resume_attempt(task, cap, now)`: `Exhausted` → `set_terminal(Interrupted, "resume attempt cap exceeded")`; `Resumable{attempt}` → seed from checkpoints, **register the cancel token, then** spawn the finalizer-guarded detached runner via `run_from` with the injected snapshot graph and `run_id = "{task}-resume-{attempt}"`.
`sweep_interrupted` is retired for workflow tasks (a `Working` row is always resumed, finalized, or given-up). Cap default **3**, configurable via `[store] resume_attempt_cap` (today `StoreConfig` has only `path`).

## 4. Data flow (worked resume — `codex ∥ claude → synth`)
Submit → task T `Working`; `tasks.input` + `workflow_spec_json` stored. With completion-driven scheduling, codex finishes → `put_node_checkpoint(T, codex, …, ok)` immediately (claude still running). **serve killed.** Boot: load T (`Working`, attempts 0<3), checkpoints `{codex}`, terminal `synth` not checkpointed → `claim_resume_attempt`→1 → register token → `run_from(snapshot, input, "T-resume-1", seed={codex})` → scheduler seeds codex done, re-runs **claude**, then **synth** (stored codex + new claude + original input) → `Terminal{Completed}` → `set_terminal(Completed)`. Only claude+synth ran. *(Had the crash been after synth's checkpoint but before `set_terminal`, boot step 4 finalizes from synth's checkpoint with zero agent calls.)*

## 5. Decisions & rationale
- **Completion-driven scheduling adopted in v1** (isolated first commit): the flagship workflows are fan-out, so `join_all`-based resume saves nothing during the long parallel phase (the likely crash window) — completion-driven is what makes fan-out resume deliver. Risk mitigated by landing it isolated + behavior-preserving + write-ahead/ordering tests first.
- **Lean checkpoint table** (one finished row per node, retained): continue-from-checkpoint semantics make `run_no`/multi-run history unnecessary; the lean table sidesteps the run-no-identification + start-row-coupling hazards while still satisfying the "history" goal.
- **Snapshot the graph at submit** (versioned, opaque TEXT in the store): an in-flight task resumes with the topology + templates as submitted, immune to a workflow-config edit/deploy between submit and restart. Stored as opaque text → the `TaskStore` port stays type-agnostic (no `bridge-workflow` dependency in the port).
- **Continue-from-checkpoint, not retry:** a checkpointed node is reused whether `ok=true` OR `ok=false` (a failed non-terminal node is a completed-with-error-marker output per W1 degradation); only never-finished nodes re-run. Retrying failed nodes is a separate future feature.
- **Cap consumed before spawn** so an instant re-crash still advances toward give-up; cap is durable on the `tasks` row → survives boots.
- **Resume orchestration on `InboundServer`**; the executor stays pure (seed is plain data).

## 6. Error handling & failure modes
- In-process runner panic → `Failed` (existing finalizer). Whole-process crash → `Working` row resumed next boot.
- Checkpoint-write failure mid-run → runner fails the task (no volatile-data continuation). `set_terminal` write-failure → narrowed by the terminal-checkpoint short-circuit (step 4).
- Seed-closure violation / unknown seed node → the run fails loudly (not a silent wrong result).
- Unknown agent on resume → normal node-failure marker (degradation) → terminal.
- Cancel during/after resume → resumed task is a normal detached run (token registered before spawn) → `tasks/cancel` works; the W3a `cancel_if_working` guard still holds.

## 7. Semantics (completed vs failed vs in-flight on resume)
- **Completed (`ok=true`) checkpoint** → reused.
- **Failed (`ok=false`) checkpoint** → reused (the error-marker output, reproducing the original degraded path).
- **In-flight (no checkpoint row)** → re-run from scratch.

## 8. Testing strategy
- **Executor:** `run_from` with seeded / partially-seeded / terminal-seeded / closure-violating / unknown-node seeds; **completion-driven scheduling** — a fast fan-out leg emits `NodeFinished` before a slow sibling; the **write-ahead barrier** (a downstream node is not polled before its upstream `NodeFinished` is handled); cancel mid-completion; **W1 streaming tests stay green** (final frames/outcome unchanged).
- **Sink/drain:** `WorkflowSink` methods returning `Result`; `drain_workflow` aborts + surfaces a sink error.
- **Store:** the additive migration (open an old-schema DB file → columns/table added, existing rows intact, `foreign_keys` on); `put_node_checkpoint`/`node_checkpoints`; `claim_resume_attempt` increments + `Exhausted`; durable reopen; `working_tasks`; snapshot round-trip + an unparseable snapshot.
- **Runner capture:** drain a multi-node workflow → checkpoint rows appear as nodes finish; a checkpoint-write failure fails the task.
- **Boot resume:** seeded `Working` task → only pending nodes prompted (assert finished nodes are NOT prompted); terminal-checkpoint short-circuit finalizes with zero prompts; cap-exhausted → `Interrupted`; `workflow_spec_json IS NULL` row → `Interrupted`; unparseable snapshot → `Interrupted`.
- **Poison cap (deterministic):** claim a task to the cap without completing → next boot marks `Interrupted` — no loop.
- **Gated live (real agents):** submit a 3-node review with a file-backed store; kill `serve` after exactly one reviewer's checkpoint is logged; restart → assert the checkpointed reviewer is NOT re-prompted (e.g. via `task_node_checkpoints` timestamps / agent-stderr), the other reviewer + synth run, and `tasks/get` reaches `Completed`.
- Coverage floors held (`bridge-core` ≥90, `bridge-workflow` ≥90, workspace ≥85).

## 9. Definition of Done (numbered, falsifiable)
1. Completion-driven scheduling replaces `join_all`; W1 streaming tests pass unchanged; write-ahead barrier + completion-order tests pass.
2. `WorkflowEvent::NodeFinished` carries `output`; `WorkflowSink` methods return `Result`; `drain_workflow` aborts on sink error.
3. `WorkflowExecutor::run_from(seed)` runs only not-done nodes; `run` = empty seed; unknown-node + closure-violation seeds rejected (tests).
4. `WorkflowGraph`/`WorkflowNode` derive serde; snapshot round-trips; an unparseable/unknown-version snapshot is handled (not a panic).
5. `task_node_checkpoints` table + `tasks` additive columns; the migration is idempotent and preserves old rows; `PRAGMA foreign_keys=ON`.
6. `TaskStore`: `put_node_checkpoint`/`node_checkpoints`/`claim_resume_attempt`/`working_tasks` + `create` persists input+snapshot; `MemoryTaskStore` + `SqliteStore` both implement + unit-tested.
7. The detached runner persists a checkpoint per finished node; a checkpoint-write failure fails the task.
8. Boot resume routine on `InboundServer`: resumes a seeded `Working` task running only pending nodes; terminal-checkpoint short-circuit; cap-exhausted/unsnapshotted/unparseable → `Interrupted`; replaces `sweep_interrupted` for workflow tasks; token registered before spawn.
9. `[store] resume_attempt_cap` config (default 3); the poison-cap test proves bounded give-up.
10. **Gated live:** a real fan-out review killed after one reviewer's checkpoint resumes without re-prompting that reviewer and completes.
11. `cargo fmt`/`clippy -D warnings` clean; full suite green; coverage floors held.

## 10. Firewall
Designed from the bridge's own ports (`TaskStore`, the pure `WorkflowExecutor`, the detached runner, the single-serve lock) — and produced via a dual independent clean-room design (see provenance). No task/checkpoint/journal schema or methodology was imported from `~/code/a2a-local-bridge` (black-box only). `~/code/agent-knowledge` is readable.

## 11. Non-goals (deferred)
No retry-of-failed-nodes (resume ≠ retry); no mid-node / streaming resume (in-flight = re-run from scratch); no streaming **reattach** to a live resumed task (completion-driven scheduling makes it possible later); no node-history **query API** (the checkpoint table is the substrate; a `tasks/get` history view is a future increment); no **retention/prune** policy (cascade-on-task-delete only; growth is acknowledged debt); no resume of non-workflow detached tasks; no distributed/multi-serve resume (the single-serve lock is the boundary).

## 12. Follow-ons
- Streaming **reattach** to a resumed/in-flight task (enabled by completion-driven scheduling).
- A node-failure **retry** policy (distinct from resume).
- A node-history **query** surface (`tasks/get` exposing per-node outputs).
- A **retention/prune** policy for `tasks` + cascaded checkpoints.

---

### Provenance (dual independent design + dual review)
Two architects designed this clean-room and firewalled from each other (Claude + Codex/gpt-5.5); the designs were merged, then dual-reviewed (Codex correctness, Claude architecture). **Convergent spine** (both, independently): pure executor; additive `NodeFinished{output}`; `run_from` seed = one code path; persist the original `input`; attempt cap consumed-before-spawn; continue-from-checkpoint semantics; the non-goals. **From Codex:** the graph snapshot; the terminal-checkpoint short-circuit (narrows the W3a write-failure gap); resume-qualified run ids; old-rows-not-resumable; the serde/pragma/config prerequisites + seed-validation; *and the recommendation that drove keeping `join_all` an option*. **From Claude:** the **lean checkpoint table** (over the merge's richer one — sidestepping real blockers); the **adopt-completion-driven** call (the flagship is fan-out); the `InboundServer` placement + injected-snapshot seam; the snapshot version envelope. **Decided by the product owner:** adopt completion-driven scheduling in v1 (isolated first commit).
