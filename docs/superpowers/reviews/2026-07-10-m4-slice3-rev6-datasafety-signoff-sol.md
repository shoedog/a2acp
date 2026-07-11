# M4 Slice 3 rev6 — Data-Safety Sign-off (codex gpt-5.6-sol xhigh, repo-verified)

At design level, the four rev6 fixes are:

- NULL recency: **CLOSED**
- Storage-authored recency: **PARTIAL**
- Purge marker as history: **PARTIAL**
- Memory persistence clock: **CLOSED**

Rev6 is not implemented on `main`; this is design sign-off against the current code.

## Fix closure

1. **NULL recency — CLOSED.** Migration is explicitly DDL-only, leaving migrated task recency NULL ([rev6:300](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev6.md:300), [rev6:323](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev6.md:323)). The eligibility view maps NULL, zero, negative, and sentinel values to `i64::MAX` ([rev6:364](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev6.md:364)), so the task arm cannot pass `< cutoff`. Memory explicitly makes `None` ineligible ([rev6:802](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev6.md:802)).

   This covers every pre-rev6 purgeable row: journal, checkpoint, or linked turn. The current fresh-artifact hazards really do leave `updated_ms` unchanged—legacy checkpoints only insert ([sqlite.rs:646](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-store/src/sqlite.rs:646)), and rich events only allocate a sequence and append ([sqlite.rs:1412](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-store/src/sqlite.rs:1412)). While recency remains NULL, no such task can be purged. The consequence is leakage, not loss. A successful first post-rev6 writer with a valid clock installs a real barrier; an invalid clock installs the intentionally sticky sentinel.

2. **Storage-authored recency — PARTIAL.** The seven-writer inventory is complete. Direct SQL/map searches found no additional production append path:

   - Legacy checkpoint: [sqlite.rs:646](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-store/src/sqlite.rs:646)
   - Node start: [sqlite.rs:1225](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-store/src/sqlite.rs:1225)
   - Sequenced checkpoint: [sqlite.rs:1278](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-store/src/sqlite.rs:1278)
   - Terminal journal: [sqlite.rs:1353](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-store/src/sqlite.rs:1353)
   - Rich journal event: [sqlite.rs:1412](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-store/src/sqlite.rs:1412)
   - Turn finish: [sqlite.rs:756](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-store/src/sqlite.rs:756)
   - Usage finalization: replacement of [sqlite.rs:812](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-store/src/sqlite.rs:812)

   Rev6 correctly moves the clock into each store and reads it after immediate-transaction/guard acquisition ([rev6:467](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev6.md:467)). Caller `ts`, `completed_ms`, and terminal `updated_ms` no longer author `last_artifact_ms`. However, the required ordering reads the clock before the artifact mutation and commit, leaving the stall window described below.

3. **Marker as history — PARTIAL.** The rev5 trace-reference defect is closed in steady state: turns and checkpoints already come from current-row reads ([coordinator.rs:795](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-coordinator/src/coordinator.rs:795)), and rev6 adds current journal existence without consulting the marker ([rev6:1135](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev6.md:1135)). A normal purge followed by a valid late `NodeFinished` therefore restores checkpoint and journal references.

   The broader 200/410/404 promise is not closed: marker granularity, read ordering, and the existing artifact allow-list gate all produce incorrect codes.

4. **Memory clock — CLOSED.** Rev6 specifies `new()`, `Default`, and `with_clock()` exactly ([rev6:748](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev6.md:748)). The current store has one internal initializer ([task_store.rs:597](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-core/src/task_store.rs:597)); I found 139 `MemoryTaskStore::new()` calls, no direct struct literals, and no explicit `MemoryTaskStore::default()` calls. Manual `Default` therefore preserves construction compatibility. Valid existing timestamps retain their meanings; the new clock authors only retention/finalization barriers.

## Ranked findings

1. **WRONG — a transaction/guard-entry timestamp can still age a just-committed artifact into deletion.**

   Rev6 orders operations as acquire transaction, read clock, bump recency, write artifact, commit ([rev6:467](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev6.md:467)). Current sequenced writers have multiple mutations after transaction acquisition before commit—for example [sqlite.rs:1232](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-store/src/sqlite.rs:1232) through [sqlite.rs:1273](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-store/src/sqlite.rs:1273).

   Concrete loss: configure one-day retention; an old terminal task’s writer begins its immediate transaction and reads `artifact_ms=T`. The process is suspended for more than 24 hours before the row and transaction commit. Retention cannot run during the transaction, but immediately after commit the artifact is visible with recency `T`, already older than the effective cutoff. The next sweep deletes the just-committed row. Memory has the same read-to-guard-release window.

   The proposed tests only stall before transaction/guard acquisition ([rev6:1230](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev6.md:1230)); none stalls after the clock read. Rev6 needs a commit-adjacent or explicitly fail-closed long-transaction protocol and a controlled post-clock-read stall test. Merely moving the read inside the lock is insufficient for the claimed stalled-writer guarantee.

2. **WRONG — one task-level marker cannot distinguish “this artifact was purged” from “this artifact never existed.”**

   Rev6 promises 404 for never-known artifacts ([rev6:24](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev6.md:24)), but its route rule is “absent plus any task purge marker equals 410” ([rev6:1112](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev6.md:1112)).

   Concrete wrong codes:

   - A task has only a legacy checkpoint; that writer creates no journal row ([sqlite.rs:646](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-store/src/sqlite.rs:646)). Retention deletes the checkpoint and sets the task marker. The journal route now returns 410 even though a journal never existed.
   - A task purges checkpoint `node-a`. A request for valid workflow node `node-b`, which was never written, returns 410 solely because the task marker exists.

   `task_journal_exists()` reports current existence; it supplies no historical per-kind evidence. The test plan covers never-created data only on an unmarked task ([rev6:1241](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev6.md:1241)), so it misses both cases. Preserving the stated contract requires journal-specific and checkpoint-node-specific purge history, or an explicit relaxation to task-set-level 410 semantics.

3. **WRONG — separate task-marker and body reads can return a non-linearizable 404 during purge.**

   Rev6 retains the existing “get task first” order ([rev6:1110](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev6.md:1110)). Current journal handling reads the task at [server.rs:1113](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-a2a-inbound/src/server.rs:1113), then reads the journal separately at [server.rs:1141](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-a2a-inbound/src/server.rs:1141). Artifact handling has the same split at [server.rs:1264](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-a2a-inbound/src/server.rs:1264) and [server.rs:1328](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-a2a-inbound/src/server.rs:1328).

   Concrete ordering:

   1. Route reads an unmarked task.
   2. Retention atomically sets the marker and deletes the requested row.
   3. Route reads the now-absent body.
   4. Route uses its stale task copy and returns 404.

   Before the purge the correct result was 200; after it the correct result is 410. There is no point at which 404 was correct. A store-level current-row/history resolution or, minimally, a marker re-read after an absent body is required. Add controlled purge-between-reads tests for both routes.

4. **WRONG — the current artifact allow-list gate runs before row/history resolution and contradicts rev6’s route order.**

   The handler derives allowed nodes from `workflow_spec_json`, or—when no snapshot exists—from current checkpoint rows ([server.rs:1292](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-a2a-inbound/src/server.rs:1292)). It returns 404 at [server.rs:1315](/Users/wesleyjinks/code/a2a-bridge/crates/bridge-a2a-inbound/src/server.rs:1315) before checking the requested row.

   Concrete regressions:

   - A legacy task has no workflow snapshot. Its checkpoint existed and was purged. Current node enumeration is now empty, so the route returns 404 before it can return the required 410.
   - A current checkpoint row exists but its stored workflow snapshot is malformed or does not contain that node. The route returns 404 despite rev6’s requirement that a present row return 200.

   Rev6 only directs implementation to replace the later `Ok(None)` branch ([rev6:1132](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev6.md:1132)); that cannot fix this earlier exit. Current-row lookup must be authoritative and occur before historical/not-known resolution.

## Ordering matrix

- Never existed, unmarked task: 404 — correct.
- Never existed, but some other task artifact was previously purged: 410 — wrong.
- Existed and purged, snapshotted workflow node: steady-state 410 — correct.
- Existed and purged, legacy task without a workflow snapshot: 404 — wrong.
- Purged then late normal `NodeFinished`: checkpoint and journal return 200 and references reappear — correct in steady state.
- Journal purged then partially rewritten: journal collection returns 200 for the current suffix; current-reference construction is correct in steady state.
- Concurrent purge between task and body reads: can return impossible 404 — wrong.
- `/turns/:turn_id` deliberately remains 404 after deletion; rev6 does not provide per-turn purge history ([rev6:1110](/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev6.md:1110)).

No additional caller-time divergence was found: once a valid storage barrier exists, task eligibility uses the maximum of `updated_ms`, `last_artifact_ms`, and linked-turn completion/finalization. Stale caller timestamps cannot lower the storage barrier; future caller timestamps can only leak data. The shared eligibility view, exact task links, status-free writers, immediate delete re-check, sticky `i64::MAX`, A3 ownership boundary, `TurnFinal` dedupe, no hard delete/backfill/size eviction, and the 3a/3b split otherwise remain intact at design level.

No tests were run: rev6 is unimplemented on `main`, so its proposed gates and concurrency regressions remain unexercised.

**Verdict: FIX — close the read-to-commit recency window, add artifact-specific purge history, and make route resolution body-first and race-safe.**
