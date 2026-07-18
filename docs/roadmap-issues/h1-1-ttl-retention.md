# M4 Slice 3b — implement TTL retention (bounded storage)

**Roadmap:** H1-1 (★★★) · **Labels:** `kind:enhancement`, `area:storage`, `area:observability`, `priority:p1`, `status:triage`
**Design of record:** `docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev6.md` · **Resume checklist:** `docs/m4-observability-roadmap.md`
**Depends on:** reliability program delivering the smoke harness / compatibility matrix / phase-specific errors / pinned+floating lanes (the `roadmap.md` resume rule).

## Problem
The SQLite store grows without bound. `task_journal`, `task_node_checkpoints`, `turn_log`, and artifacts
accumulate forever. The bridge is now run as a long-lived operator service (standing deployment on
`127.0.0.1:18080`), so unbounded growth is a when-not-if failure. Slice 3a merged the retention *safety*
foundation (ownership, finalization barriers, recency, DDL-only migration) with **no deletion**; 3b is the
deletion slice and its design is already signed off (rev6, 6 adversarial revisions).

## Scope (implement rev6 as written — it supersedes older sketches)
- [ ] `[storage].artifact_retention_days` only (default 14, `0`=off), `deny_unknown_fields`; **reject** the
      superseded `artifact_retention_max_bytes` / `purge_terminal_tasks_days` knobs.
- [ ] Mandatory 24h wall-clock floor. One bounded boot pass **before** the Prometheus counter rebuild, then
      bounded hourly sweeps (`RetentionService`, batch cap 10,000), all under `BEGIN IMMEDIATE`.
- [ ] Delete only task-linked `task_journal`/`task_node_checkpoints`/exact-linked `turn_log` rows for terminal
      tasks, finalized standalone warm turns, and finalized stale linked turns. **Never delete `tasks` rows.**
      Retain pending/ambiguous legacy `NULL task_id` rows.
- [ ] One shared eligibility definition (`retention_artifact_eligibility` view) drives candidate listing AND
      the delete-time re-check; memory-store parity.
- [ ] Set-once `artifacts_purged_at` marker, atomic with deletion.
- [ ] Route semantics: current row → 200; absent row on a marked task → **task-level 410**; unknown → 404.
      (Per-artifact 404-vs-410 precision is an accepted non-goal.)

## 3a carry-ins to fold
- [ ] Task-level 410 contract.
- [ ] Close the read-to-commit recency window (commit-adjacent clock read or fail-closed) + stalled-writer regression.
- [ ] Re-read the purge marker after an absent body + purge-between-reads race tests.

## Explicitly out of scope for 3b
Size eviction, DB ceilings, VACUUM, transcript redaction, OTLP. (3c is reserved and unscoped.)

## Value
Bounded storage is table-stakes for a long-running service; the design is done and the safety substrate is
merged, so this is high value at low residual risk — execution against a reviewed contract.
