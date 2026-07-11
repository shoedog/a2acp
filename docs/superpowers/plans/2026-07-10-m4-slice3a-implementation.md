# M4 Slice 3a — Implementation Plan (ownership + finalization barrier, NO deletion)

**Source design:** `docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev6.md` (§ Turn-log ownership
write fix, § Explicit usage/no-usage finalization, § Testing → Slice 3a).
**Verdict trail:** rev1 REDESIGN → rev6 FIX; the 3a core has been settled and data-safe since rev3. Remaining
open items are all **3b** (deletion/routes) — see "Deferred to 3b" below.
**Branch:** `feat/m4-slice3a-ownership-finalization`.
**Discipline:** strict TDD — write the named failing test, implement, `cargo test --workspace -j1` green, then next.

## Scope boundary
3a lands ownership + the finalization barrier + the write-path recency bump + DDL-only migration + dedup
scoping + Prometheus-rebuild seeding. **3a performs NO deletion**, ships no `[storage]` config, no
`RetentionService`, no route changes. Those are 3b.

## Tasks (ordered; each = its named rev6 tests)

### T1 — Ownership write-fix
- **T1a (DONE, compiles):** central authoritative overwrite in `spawn_detached_workflow`
  (`detached.rs` — `ctx.task_id = Some(task.clone())` before `run_from_with_context`).
  Tests: `spawn_detached_workflow_overwrites_ctx_task_id`, `detached_runner_overwrites_missing_task_id`,
  `detached_runner_overwrites_conflicting_task_id`. *(edit landed; tests still to add)*
- **T1b:** caller-side ownership at `coordinator.rs:701` (fresh), `batch.rs:860` (child),
  `detached.rs:1622` (detached resume), `batch.rs:513` (batch resume).
  Tests: `detached_fresh_turn_persists_task_id`, `batch_child_turn_persists_task_id`,
  `detached_resume_turn_persists_task_id`, `batch_resume_turn_persists_task_id`.

### T2 — Foundation: schema migration + clock + retention constants
- DDL-only migration adds nullable `turn_log.usage_finalized_ms`, `turn_log.usage_finalization_kind`,
  `tasks.last_artifact_ms`, `tasks.artifacts_purged_at`. No row rewrites.
- Add `RETENTION_NEVER_ELIGIBLE_MS = i64::MAX`, `valid_retention_wall_ms()`, `durable_retention_ms()`.
- Inject a store persistence clock: SQLite `now_ms` field; `MemoryTaskStore::new()/Default` (system wall),
  `MemoryTaskStore::with_clock()` (test).
- Tests: `sqlite_legacy_migration_is_ddl_only`.

### T3 — Explicit no-usage finalization (producer)
- `UsageFinalized` carries `Option<UsageSnapshot>`; every finish path emits `Some(usage)` or explicit `None`.
- Tests: `workflow_success_/failure_/cancel_without_usage_emits_explicit_no_usage`,
  `inbound_disconnect_without_usage_emits_explicit_no_usage`,
  `turn_finish_drop_guard_without_usage_emits_explicit_no_usage`.

### T4 — Finalization barrier persistence (storage-authored) + recency bump
- Replace `update_turn_usage()` with a method that, inside the txn/guard, stamps `usage_finalized_ms`
  from the **store clock** (invalid → `RETENTION_NEVER_ELIGIBLE_MS`), sets `usage_finalization_kind`
  (`usage`/`no_usage`), and for task-linked turns bumps `tasks.last_artifact_ms` in the same mutation.
  Reject contradictory no-usage; a replayed finish never clears an existing barrier.
- Tests: `usage_finalized_some_updates_usage_and_barrier_atomically`,
  `usage_finalized_none_sets_no_usage_barrier`, `usage_finalization_uses_persistence_time_not_old_event_time`,
  `usage_finalization_invalid_clock_uses_never_eligible_timestamp`,
  `no_usage_finalization_rejects_existing_usage_columns`, `turn_finished_upsert_does_not_clear_finalization`,
  `turn_finished_task_linked_bumps_artifact_recency`, `finalize_turn_usage_task_linked_bumps_artifact_recency`,
  `memory_finalization_matches_sqlite`.

### T5 — Dedup TurnFinal-scoping
- `DedupObserver` marks usage dedupe only on `TurnFinal`.
- Test: `dedup_observer_non_turn_final_usage_does_not_suppress_turn_final_barrier`.

### T6 — Prometheus rebuild seeding
- Rebuild seeds dedupe from the new columns without corrupting counters.
- Tests: `prometheus_rebuild_seeds_explicit_no_usage_finalization`,
  `prometheus_rebuild_keeps_pending_finalization_replayable`,
  `prometheus_rebuild_sentinel_finalization_seeds_dedupe_without_retention_eligibility`.

## Deferred to 3b (with the scoping decision made per the rev6 sign-off)
3b = `[storage]` TTL config + `RetentionService` + deletion + drill-down route codes. The rev6 sign-off
(gpt-5.6-sol) left three 3b items; the owner decision (2026-07-10):
1. **Route contract relaxed to task-level 410 (accepted non-goal).** Once a task has had ANY artifact
   purged (`artifacts_purged_at` set), an absent artifact on that task returns **410**; precise
   per-artifact "never-existed vs purged" (404-vs-410) is NOT promised. This closes sign-off #2 and
   simplifies #4 without per-artifact purge history.
2. **Read-to-commit recency window (#1):** stamp recency commit-adjacent, or fail closed on a
   long-running (>TTL) writer transaction. Resolve in 3b with a post-clock-read stall test.
3. **Body-first, race-safe route resolution (#3):** current-row lookup is authoritative; re-read the
   purge marker after an absent body. Resolve in 3b with purge-between-reads tests.
