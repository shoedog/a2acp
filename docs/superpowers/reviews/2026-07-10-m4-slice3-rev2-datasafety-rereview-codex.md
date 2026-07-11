# M4 Slice 3 rev2 — Data-Safety Re-Review (codex gpt-5.5 xhigh, repo-verified)

Prior WRONG closure status: not all closed.

- #1 `turn_log` ↔ task ownership: CLOSED for default TTL/size paths. Rev2 no longer uses `task_id IS NULL` as orphan, and current detached/batch call sites cited are the right ones to fix.
- #2 terminal purge missing legacy NULL rows: PARTIAL. It fixes the miss/leak shape, but introduces a constructible over-delete.
- #3 `completed_ms` finalization race: PARTIAL. The barrier blocks the simple race, but reconciliation can falsely finalize rows whose usage was emitted but lost before persistence.

Ranked findings:

1. WRONG — terminal hard purge can delete another live task’s legacy NULL turn row.

Scenario: task `A` is old terminal and purge-eligible. A different live task has id `A-resume-1`, same workflow, and a legacy `turn_log` row with `task_id NULL`, `workflow/node` set, `session_id = 'A-resume-1'`. Task IDs are only non-empty strings in `ids.rs`, so this state is constructible. If bounded backfill does not reach that row first, hard purge for `A` matches it as `A`’s resume row and deletes it.

Spec text: hard purge deletes NULL rows where `turn_log.session_id = v.id OR substr(...)= v.id || '-resume-'` without proving the longest/exact owner is the victim. Current code confirms resume run ids use `"{task}-resume-{attempt}"`, and `TaskId`/`ContextId` do not reserve that namespace.

Fix: use the same deterministic owner-resolution subquery as backfill, and delete a legacy NULL row only when its resolved owner is in `retention_task_victims`. Add a regression where victim `task-a` must not delete live task `task-a-resume-1`.

2. WRONG — no-usage reconciliation can mislabel lost usage as `reconciled_no_usage`.

Current observer writes `TurnFinished` and `UsageFinalized` as separate queued commands. `try_send` can drop commands when full, and a process can crash after the finished command is persisted but before usage is persisted. Rev2 then reconciles old rows with all usage columns NULL as no-usage and makes them deletion-eligible.

Concrete state: `TurnFinished` persisted, `UsageFinalized` existed in memory but was dropped/crashed before `update_turn_usage()`. After 24h, rev2 sets `usage_finalization_kind = 'reconciled_no_usage'`, then TTL/size can delete the row. That is not “legitimately no usage.”

Fix: do not time-reconcile post-rev2 rows based only on NULL usage columns. Persist an explicit finalization/no-usage marker from the producer path, or retain ambiguous legacy rows indefinitely/manual-only.

3. SMELL — `spawn_detached_workflow` remains a bypass point for task ownership.

Rev2 fixes call sites, but the central detached runner already receives `task: TaskId` and `mut ctx`. It should set `ctx.task_id = Some(task.clone())` internally before `run_from_with_context`. Otherwise the next caller can reintroduce NULL task ownership.

4. SMELL — migration backfill is not actually bounded.

The migration-time `UPDATE turn_log SET usage_finalized_ms = completed_ms ... WHERE usage columns present` is unbounded. The spec’s bounded boot-sweep claim does not cover this schema migration path. On a large DB this can stall first boot under the store lock.

Fix: either make migration metadata-only/DDL-only and move all row updates into bounded runtime repair, or explicitly accept and test the one-time unbounded migration.

5. WRONG — feature gate is underspecified across crates.

The design puts `#[cfg(feature = "unsafe-terminal-task-purge")]` on `TaskStore` in `bridge-core`, implementation in `bridge-store`, and call/config logic in the bin. Cargo features are per crate. Without explicit feature definitions and forwarding, `cargo build -p a2a-bridge --features unsafe-terminal-task-purge` will not coherently enable the trait and implementation.

Fix: define the feature in all relevant crates or only at the bin with dependency feature forwarding, and add both default-build rejection and feature-build compile tests.

Verdict: FIX — must fix findings 1, 2, and 5 before implementation; 3 and 4 should be tightened in the same slice.
