# M4 Slice 3 rev5 — Data-Safety Sign-off (codex gpt-5.6-sol xhigh, repo-verified)

#1 CLOSED · #2 CLOSED · #3 CLOSED · #4 CLOSED — at design level. Rev5 is unimplemented on `main`; this is not implementation sign-off.

### Rev4 findings

- **#1 CLOSED.** Invalid timestamps map to `i64::MAX`; eligibility sanitizes invalid task/turn times; invalid sweeps no-op. The sentinel only flows through `max` and `<` comparisons—there is no `eligible_ms + ttl/floor` arithmetic. Cutoffs use checked multiplication and saturating subtraction. See [rev5:123](</Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev5.md:123>), [rev5:300](</Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev5.md:300>), and [rev5:763](</Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev5.md:763>). SQLite also treats `9223372036854775807` as an integer. Sentinel rows leak intentionally; they do not wrap into eligibility.

- **#2 CLOSED.** Current writers require existence, not `Working`: SQLite sequence allocation uses `WHERE id=?1` at [sqlite.rs:1237](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-store/src/sqlite.rs:1237>), while memory checks only `contains_key` at [task_store.rs:1167](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-core/src/task_store.rs:1167>). Rev5 explicitly preserves this. Therefore cancel-before-start at [coordinator.rs:670](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-coordinator/src/coordinator.rs:670>) and cancel-time `NodeFinished` at [executor.rs:1083](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-workflow/src/executor.rs:1083>) remain writable. Retention cannot trigger the normalization at [detached.rs:1289](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-coordinator/src/detached.rs:1289>).

- **#3 CLOSED.** The complete mutation inventory is:

  - Legacy checkpoint: [sqlite.rs:646](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-store/src/sqlite.rs:646>), [task_store.rs:715](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-core/src/task_store.rs:715>)
  - Start plus journal: [sqlite.rs:1225](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-store/src/sqlite.rs:1225>), [task_store.rs:1160](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-core/src/task_store.rs:1160>)
  - Checkpoint, start removal, plus journal: [sqlite.rs:1278](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-store/src/sqlite.rs:1278>), [task_store.rs:1201](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-core/src/task_store.rs:1201>)
  - Terminal plus journal: [sqlite.rs:1353](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-store/src/sqlite.rs:1353>), [task_store.rs:1257](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-core/src/task_store.rs:1257>)
  - Rich-event journal: [sqlite.rs:1412](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-store/src/sqlite.rs:1412>), [task_store.rs:1316](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-core/src/task_store.rs:1316>), called from [detached.rs:400](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-coordinator/src/detached.rs:400>)
  - Turn finish/relink: [sqlite.rs:756](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-store/src/sqlite.rs:756>), [task_store.rs:777](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-core/src/task_store.rs:777>)
  - Usage finalization: replacement for [sqlite.rs:812](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-store/src/sqlite.rs:812>) and [task_store.rs:818](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-core/src/task_store.rs:818>)

  Direct SQL and memory-map searches found no additional production artifact append path. Rev5 requires all seven to bump recency in the same transaction/guard.

- **#4 CLOSED.** Rev5 requires every memory writer and retention re-check/delete to acquire `journal_fold_guard` first and hold it across all relevant maps at [rev5:706](</Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev5.md:706>). Existing guarded mutation order is already guard-first at [task_store.rs:1167](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-core/src/task_store.rs:1167>); the legacy checkpoint is explicitly moved under it. No current reverse guard acquisition was found.

### Ranked findings

1. **WRONG — migration can immediately purge fresh pre-rev5 artifacts.**

   Rev5 gives every existing task `last_artifact_ms = NULL`, then deliberately interprets `NULL` as fallback to `updated_ms` ([rev5:291](</Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev5.md:291>), [rev5:342](</Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev5.md:342>)). But current late writers do not update `updated_ms`: rich events only bump `last_event_seq` before inserting the journal row at [sqlite.rs:1423](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-store/src/sqlite.rs:1423>), and legacy checkpoints only insert at [sqlite.rs:654](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-store/src/sqlite.rs:654>).

   Concrete loss: a terminal task was updated 30 days ago; traces/turn logging are disabled; a delayed rich event or checkpoint was persisted one hour before upgrading. The default boot sweep sees no linked turn, falls back to the 30-day-old task timestamp, and deletes the one-hour-old artifact.

   Must fix: when `last_artifact_ms IS NULL` and a task has purgeable artifacts, fail closed and make the task ineligible. A later bounded repair/backfill can reclaim it. Add a pre-rev5 late-artifact migration test.

2. **WRONG — new artifact recency is caller-time, not persistence-time.**

   Rev5 explicitly defines `artifact_ms = durable_retention_ms(raw_ms)` at [rev5:448](</Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev5.md:448>). Current production calculates that time before entering storage—for example [detached.rs:296](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-coordinator/src/detached.rs:296>) and [detached.rs:410](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-coordinator/src/detached.rs:410>)—while SQLite only acquires its connection/transaction afterward at [sqlite.rs:1232](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-store/src/sqlite.rs:1232>).

   Concrete loss: a writer obtains its timestamp, then blocks or the process is suspended past the effective age; it eventually commits a fresh journal/checkpoint using the stale timestamp. The next sweep treats the just-committed artifact as old and deletes it. A positive wall-clock rollback produces the same result for a task with no prior bump.

   Must fix: storage must author `last_artifact_ms` after acquiring the SQLite transaction or memory guard, exactly as rev5 already requires for `usage_finalized_ms`. Event/checkpoint `ts` can remain caller-authored. Add stale-caller-time tests for every writer family.

3. **WRONG — the set-once purge marker contradicts permitted post-purge writes.**

   Retention sets `artifacts_purged_at` once ([rev5:659](</Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev5.md:659>)); writers neither reject terminal tasks nor clear/change it. Rev5 then suppresses all trace references whenever the marker exists at [rev5:1035](</Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev5.md:1035>).

   Concrete regression: retention purges an eligible canceled task; immediately afterward, its delayed `NodeFinished` legitimately writes a new checkpoint and journal. Direct artifact requests can return the new data, but task status permanently omits its journal/checkpoint/turn references because the old marker remains.

   Must fix: treat the marker as purge history, not proof that the current artifact set is empty. Build trace references from current rows even when the marker exists, and test purge → late write → discoverability plus correct 410 for still-missing artifacts.

4. **SMELL — the memory store’s storage-authored clock is unspecified.**

   SQLite explicitly gains `now_ms`; `MemoryTaskStore` does not, despite currently accepting all timestamps from callers and having 139 `new/default` construction sites. Rev5 says memory stamps persistence time inside the guard but does not define its clock source. Specify an injected/default store clock and test it; otherwise SQLite/memory parity depends on implementation guesswork.

Select-then-delete is otherwise sound: SQLite’s immediate delete transaction re-queries the shared view, while memory holds the common guard across re-check and deletion. A previously listed candidate cannot be deleted against a committed fresh bump. Sentinel, genuinely large timestamps, zero timestamps, and sticky `i64::MAX` fail closed; the intentional consequence is leakage, not premature deletion.

A3 ownership, persistence-time usage finalization, `TurnFinal` dedupe, no hard task deletion, no ownership backfill, no size eviction, and basic 410/404 behavior remain specified. The post-purge late-write case above is the outstanding 410/reference correctness regression.

No tests were run because rev5 is not implemented; the full workspace gates and all proposed concurrency tests remain unexercised.

**Verdict: FIX — make legacy `NULL` recency ineligible, storage-author every artifact-recency bump, and make purge-marker/reference semantics correct after late writes.**

