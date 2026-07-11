# M4 Slice 3 rev3 — Final Data-Safety Re-Check (codex gpt-5.5 xhigh, repo-verified)

A-status up front:

- A1: CLOSED. Rev3 removes terminal task deletion and session matching. Current repo has no `purge_terminal*`, no retention `DELETE FROM tasks`, and no session `GLOB`; the only `DELETE FROM tasks` I found is a `#[cfg(test)]` cascade helper at [sqlite.rs](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-store/src/sqlite.rs:106).
- A2: CLOSED for the original lost-finalization deletion bug. Rev3 requires explicit `usage|no_usage` barrier columns in the eligibility view, and legacy/pending rows are excluded. I found a new finalization timestamp bug below.
- A3: PARTIAL. The spec has the right central overwrite requirement, but current code at the cited boundary does not yet set `ctx.task_id`; see [detached.rs](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-coordinator/src/detached.rs:1252). Treat closed only after implementation/test.
- A4: CLOSED. Rev3 is DDL-only, and current migration style is additive DDL with no data-row sweep; see [sqlite.rs](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-store/src/sqlite.rs:204).

Ranked findings:

1. WRONG — `usage_finalized_ms` is event/enqueue time, not durable persistence time.

Concrete scenario: a turn completes, `UsageFinalized` is enqueued, then the process/worker is suspended or the store worker is delayed for >24h. When the command finally persists, rev3 stores the old `finalized_ms`, so the row can be immediately eligible and deleted on the next retention pass. That violates rev3’s own “24h past finalization persistence” guard.

Spec evidence: `TurnLogFinalized` carries `finalized_ms` from the observer command, and the mapping sets it before enqueue at [rev3 spec](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev3.md:441). Current observer architecture confirms commands are async/background and independently droppable at [lib.rs](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-observ/src/lib.rs:219) and [lib.rs](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-observ/src/lib.rs:257).

Fix: do not let the producer/observer author the durable barrier timestamp. Stamp `usage_finalized_ms` at store persistence time, ideally inside the SQLite transaction / memory mutation, after acquiring the mutation guard. Producer authors `Usage` vs `NoUsage`; storage authors persistence time.

2. WRONG — task artifact eligibility ignores journal/checkpoint write recency while current stores permit late writes to terminal tasks.

Concrete scenario: task `t` is terminal with `updated_ms` 15 days old. A delayed or buggy internal path writes a new `task_journal` event or checkpoint for `t` now. Current SQLite/memory write methods check task existence, not `status='working'`. Rev3 task eligibility uses only `tasks.updated_ms` plus linked turn times, so retention can delete the newly written journal/checkpoint immediately.

Spec evidence: task eligibility computes `eligible_ms` from `t.updated_ms` and linked `turn_log` only at [rev3 spec](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev3.md:273), then deletes journal/checkpoint rows at [rev3 spec](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev3.md:528). Current SQLite journal/checkpoint paths only require the task row to exist; e.g. `record_node_started` and checkpoint writes at [sqlite.rs](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-store/src/sqlite.rs:1237) and [sqlite.rs](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-store/src/sqlite.rs:1297).

Fix: either reject all non-terminal-event journal/checkpoint writes unless the task is still `Working`, or add real artifact write timestamps into eligibility. Since `task_journal` has no timestamp column outside JSON, the safer smaller fix is status-guarding late writers and testing terminal-task late write rejection.

3. WRONG — A3 is not true in current code; the central overwrite is still only proposed.

Concrete scenario if implementation misses this exact line: a detached/resume/test-helper call can pass `WorkflowRunContext { task_id: None }`; workflow turn rows then remain `NULL task_id` with workflow/node metadata. Task artifact purge will not count those turns as linked to the task, so it can purge task artifacts without waiting on that turn’s completion/finalization.

Spec text requires the overwrite at [rev3 spec](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev3.md:362). Current code at the cited boundary sets `make_rich_sink` and calls `run_from_with_context`, but has no `ctx.task_id = Some(task.clone())`; see [detached.rs](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-coordinator/src/detached.rs:1252).

Fix: implementation must add the central overwrite, not only caller-side fixes. Keep the missing/conflicting `ctx.task_id` tests.

4. SMELL — the 24h floor is wall-clock only; clock jumps can defeat real elapsed age.

Current `now_ms()` uses `SystemTime` and maps pre-epoch errors to `0` at [detached.rs](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-coordinator/src/detached.rs:238). A forward clock jump after completion/finalization, or a bad pre-epoch timestamp later corrected, can make recent rows appear older than TTL+floor. SQL enforces timestamp comparison, not monotonic elapsed time.

If wall-clock retention semantics are acceptable, document that explicitly. If the 24h floor is a real-time safety floor, skip retention on invalid/regressed/implausibly jumped clocks.

5. SMELL — `DedupObserver` currently dedupes all `UsageFinalized` events before inner observers check `fin == TurnFinal`.

Current code marks usage on any `UsageFinalized` at [lib.rs](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-observ/src/lib.rs:75), while turn-log and Prometheus observers ignore non-`TurnFinal`. There are no current `Partial`/`TaskFinal` producers, so this is not a demonstrated deletion bug. But with the signature change, make dedupe explicitly TurnFinal-scoped or add tests proving non-turn-final events cannot suppress the durable barrier.

Verdict: FIX — must fix #1 and #2 before deletion ships; #3 must be verified in implementation before A3 is closed.

