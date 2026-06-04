# ADR-0015 — Streaming Reattach (live progress for detached runs)

**Date:** 2026-06-04
**Status:** Accepted

**Builds on:** ADR-0010 (durable detached submit) + ADR-0011 (crash-resume). Those made a detached workflow durable and resumable but **poll-only** (`tasks/get`). This increment makes its live progress **re-subscribable over SSE**.

---

## Context

A workflow submitted **detached** (`message/send`→workflow returns a task id, durable, surviving the submitting client's exit) drained over a `TaskStoreSink` with **no live SSE consumer** — the only way to follow it was polling `tasks/get`. The synchronous path (`message/stream`) had live SSE but starts a *new* run. We wanted: follow an **in-flight** detached run's live progress, **reattachable** after the original client disconnected, from any client, surviving a serve restart.

## Decision

**`SubscribeToTask` delivers a durable state *snapshot* + live *deltas*, durable-first, keyed by a per-task `seq` cursor.**

- **Wire:** the A2A-standard **`SubscribeToTask`** method (split out of the shared `SendStreamingMessage` dispatch arm into its own handler — no `gate()`, so it never starts a run). The task id is the a2a-lf `SubscribeToTaskRequest.id` field (`taskId` accepted as a lenient alias). Cursor = the **`Last-Event-ID`** header, parsed `Option<i64>` (absent ≠ `K=0`).
- **Catch-up = state snapshot, NOT event replay.** Consumers are LLMs/humans; a re-sent raw "node finished" misreads as a second completion. The snapshot is idempotent-by-construction (finished nodes → `NodeFinished{output}`, running nodes → `NodeStarted`, terminal → `Terminal`), **row-driven from the durable store + sorted by `seq`** (never graph order). Granularity is node-level (not token-level, not a full event-history log).
- **Durable-first.** Every node event is **persisted (allocating a per-task `seq` in one SQLite transaction) BEFORE it is published** to an in-memory `tokio::broadcast` hub — so every SSE `id` is already-committed state, and a lagged/restarted subscriber recovers losslessly from the durable snapshot.
- **The detached runner's terminal ownership moved into the sink.** `DetachedProgressSink` (replacing `TaskStoreSink`) persists-via-sequenced-then-publishes each event AND owns the sequenced terminal write. **Every** detached terminal transition (sink, no-terminal/`Err` arms, no-executor, resume short-circuits, unknown-workflow, the `Finalizer` panic guard) routes through one `finalize_detached` helper → so `terminal_seq` is never NULL and the per-task hub never leaks.
- **Exactly-once across the snapshot↔live boundary:** the handler **subscribes to the hub BEFORE reading the snapshot**, then live-tails dropping frames with `seq <= max(cursor.unwrap_or(-1), cut_seq)`. Cursor-less keeps `seq 0` (legacy NULL-seq checkpoints). A post-subscribe snapshot that reads terminal **re-branches to the terminal flow** (never relies on `rx` `Closed`). Broadcast lag → a retryable `event: error` SSE event + close (the client reconnects from its cursor and re-snapshots).
- **One-way-door contract (locked):** frame `v: 1`, `{v, seq, phase, kind, …}` with the `FrameKind` discriminator flattened to the top level; `seq` per-task, monotonic, **gaps allowed**; catch-up is state-snapshot, not history replay.

## Components

- **`bridge-core` (`task_store.rs`):** `TaskProgressSnapshot`; the seq-bearing `TaskStore` methods `record_node_started` (UPSERT — resume re-emits `NodeStarted`), `put_node_checkpoint_sequenced` (write-once, W3b), `set_terminal_sequenced`, `progress_snapshot`. `MemoryTaskStore` impl (`cut_seq` clamped ≥ every included seq for a consistent view).
- **`bridge-store` (`sqlite.rs`):** each seq method = one `unchecked_transaction` (bump `tasks.last_event_seq`, write state, commit, return seq); additive migration of `tasks.last_event_seq`/`terminal_seq` + `task_node_checkpoints.seq` + the new `task_node_starts` table; `progress_snapshot` reads in one transaction (NULL-seq legacy checkpoint ⇒ `seq 0`).
- **`bridge-a2a-inbound` (`reattach.rs`):** `TaskProgressHub` (a bounded `broadcast` wrapper); `WorkflowProgressFrame`/`Phase`/`FrameKind`; `TerminalOutcome` (a Serialize mirror of the non-Serialize `WorkflowOutcome`). **(`workflow_sink.rs`):** `DetachedProgressSink`; the `Finalizer` extended to finalize-via-`finalize_detached` (sequenced + hub cleanup) on panic. **(`server.rs`):** `progress_hubs` map (hub inserted before spawn); `finalize_detached`; the runner restructure (`fin.done=true` before the success-arm hub-removal await — M1); `subscribe_to_task` + `snapshot_frames` + `terminal_sse_response` + the working-state live-tail.
- **`bin/a2a-bridge`:** `task watch <id> [--from <seq>] [--url]` — a `reqwest` SSE client honoring `Last-Event-ID`, tracking the last data `id` for resume.

## Provenance — dual-design + spec dual-review + plan dual-review

Brainstormed (catch-up = snapshot, cursor, wire surface settled with the user), **dual-independent-designed** (a firewalled clean-room codex design converged on the spine + sharpened the snapshot↔live seam), **spec dual-reviewed** (Codex soundness + Claude architecture) → rev2, then the plan was **dual-reviewed** (Codex + Claude) → rev2 + spec rev3. Findings that shaped the build:

- **The wire method is the existing `SubscribeToTask`** (Claude caught it is already a valid streaming method routed at the shared dispatch arm — not an invented `tasks/resubscribe`), and **its param field is `id`, not `taskId`** (Claude, verified against `a2a-lf 0.3.0` `types.rs`).
- **Subscribe-before-snapshot is the exactly-once boundary**; assert on the **delivered ordered `Vec<(seq,kind)>`** (a `HashSet` can't falsify a duplicate — the false-positive class this project has been bitten by).
- **`FailingCheckpointStore` is a third `TaskStore` impl** → the seq methods + the write-failure injection (moved to the sequenced method) land in Task 1 (both reviewers).
- **All detached terminal paths must be sequenced** (Codex), and the snapshot↔live `cursor-less ≠ K=0` distinction (Claude) so legacy `seq 0` is delivered.
- The holistic review confirmed the merged whole: seq numbers line up end-to-end with no dup/gap, no detached path leaves `terminal_seq` NULL or leaks a hub, durable-first holds on every path, and the M1 clobber window is closed.

## Live-gate results (real codex + claude, the `code-review` workflow, durable store)

- **Mid-flight reattach:** submit → `task watch` while codex+claude ran → the snapshot reconstructed both in-progress nodes (`codex`@1, `claude`@2 as `node_started`, `phase:snapshot`) + `snapshot_complete`@2; then the live tail `node_finished` codex@3/claude@4, synth `node_started`@5/`node_finished`@6, `terminal`@7 (`completed`). Monotonic 1→7, no dup/gap.
- **Crash-resume + reattach:** submit → SIGKILL serve after `codex`'s checkpoint (seq 3) → restart (boot `resume_working_tasks` re-ran only the pending nodes) → `task watch` → the snapshot **reconstructed codex@3 across the restart** + the resumed `claude`@4, `snapshot_complete`@4; live tail claude@5, synth@6/7, `terminal`@8 (`completed`). **`seq` continued monotonically across the restart** (3→8); the cursor spanned the durable→restart→live boundary.

## Consequences

- A detached run is now followable live (and re-followable, from any client, across a restart) — not just pollable. The `task watch` CLI is the operator-facing reattach client; the same `SubscribeToTask` SSE serves dashboards / LLM consumers.
- **Coverage held:** workspace 90.92% line, bridge-core ~98%, bridge-workflow ~92% (all floors); full suite + clippy `-D warnings` + fmt clean.
- **Hexagonal boundary respected:** the hub/frame/handler are transport (`bridge-a2a-inbound`); the `seq`/snapshot are `TaskStore`-port concerns; the executor is untouched (it already emits the events the sink consumes).

## Follow-ons

- **Token-level / fine-grained streaming** and a **full exact-event-history log** — deferred (node-level state-snapshot is right for LLM/human consumers; ADR-0012's structuring seam applies if a deterministic consumer appears).
- **Cross-serve reattach** — N/A under the single-serve model; would need a shared bus.
- **The no-token `tasks/cancel` path** (the rare §8-write-failure case) still writes `Canceled` unsequenced (no live `Terminal` frame; a later reattach is still correct via the `Option terminal_seq` path) — route it through `finalize_detached` in a follow-up. The **normal** cancel (live token → the runner emits `Terminal{Canceled}` via the sink) is already sequenced + published.
- A dedicated **transient/unavailable `BridgeError`** variant for the "Working-but-hub-not-yet-registered" race (currently reuses `AgentOverloaded`, which maps to the correct retryable INTERNAL disposition).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
