# M4 Slice 3 rev4 — Final Data-Safety Sign-off (codex gpt-5.6-sol xhigh, repo-verified)

#1 CLOSED · #2 OPEN · A3 CLOSED.

Prior-finding verification:

- **#1 CLOSED.** Rev4 removes producer-authored time from `TurnLogFinalized`, requires storage-time stamping inside the mutation, and atomically writes the usage/barrier ([rev4:439](</Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev4.md:439>), [rev4:510](</Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev4.md:510>)). An old event timestamp is no longer carried to storage. Every successfully persisted finalization gets a non-NULL barrier; a dropped command remains NULL intentionally and is undeletable. The separate invalid-clock failure below still defeats the age guarantee.
- **#2 OPEN.** Rev4 guards only three methods ([rev4:348](</Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev4.md:348>)). It misses another journal writer and breaks current cancellation ordering.
- **A3 CLOSED.** Rev4 explicitly inserts the authoritative overwrite immediately before execution and requires both `None` and conflicting-ID tests ([rev4:424](</Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev4.md:424>), [rev4:928](</Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev4.md:928>)). Current code confirms that assignment is presently absent at the cited boundary ([detached.rs:1252](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-coordinator/src/detached.rs:1252>)).
- **Dedup CLOSED.** Only `TurnFinal` marks usage dedupe, and the required Partial/TaskFinal-before-TurnFinal regression test is stated ([rev4:477](</Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev4.md:477>)).
- **Clock acceptance is explicit, but its `now_ms == 0` safety claim is false across clock recovery.**

## Ranked findings

1. **WRONG — a persisted zero timestamp defeats the 24-hour barrier after the clock recovers.**

   Concrete turn scenario: an old completed turn receives its finalization while the wall-clock read fails and returns `0`; storage persists `usage_finalized_ms = 0`. The sweep executed at `now_ms == 0` deletes nothing, but after the clock recovers, eligibility becomes `max(old_completed_ms, 0)`, which is old and immediately below the cutoff. The just-persisted finalization is deleted without waiting 24 hours.

   The same defect exists for tasks: `cancel_task` persists `updated_ms` from the wall clock ([coordinator.rs:846](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-coordinator/src/coordinator.rs:846>)), and `SystemClock` maps clock failure to zero ([clock.rs:22](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-coordinator/src/clock.rs:22>)). After recovery, an old task canceled during the failure can become immediately eligible.

   Rev4’s test covers only a sweep whose current `now_ms` is zero ([rev4:964](</Users/wesleyjinks/code/a2a-bridge/docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev4.md:964>)); it does not test a zero persisted as recency followed by clock recovery. Clock-read failure must fail closed—leave finalization pending or persist a conservative non-eligible value—and needs a recovery test.

2. **WRONG — requiring `Working` breaks the existing live-cancel path and loses required progress writes.**

   Fresh submission creates the task, registers the cancel token, and only then spawns the runner ([coordinator.rs:689](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-coordinator/src/coordinator.rs:689>)). A cancel can therefore fire before the runner writes its first `NodeStarted`: `cancel_task` flips the row to `Canceled`, after which rev4’s proposed guard rejects `record_node_started`.

   During an in-flight cancel, the executor also deliberately emits `NodeFinished` before its terminal event ([executor.rs:1083](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-workflow/src/executor.rs:1083>)). The proposed guard rejects that checkpoint. Any sink error is then normalized to `Failed` with `"checkpoint write failed"` ([detached.rs:1289](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-coordinator/src/detached.rs:1289>)), overwriting the intended Canceled result and dropping the progress artifact.

   Rev4 tests rejection after terminal status but contains no cancel-before-start or cancel-during-node ordering test. The cancel protocol and guard must be coordinated so the active authoritative runner can complete its cancellation journal/terminal sequence without reopening unrestricted late writes.

3. **WRONG — `record_event_sequenced` is an unguarded fourth journal writer.**

   Both stores append `task_journal` through `record_event_sequenced` using task existence only ([sqlite.rs:1412](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-store/src/sqlite.rs:1412>), [task_store.rs:1316](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-core/src/task_store.rs:1316>)); the production rich-event sink calls it directly ([detached.rs:400](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-coordinator/src/detached.rs:400>)).

   A delayed rich-event flush can therefore append to an already-terminal task. That append does not update `tasks.updated_ms`, while task eligibility does not inspect journal timestamps. An already-aged task remains eligible and the fresh journal row can be purged on the next sweep. Rev4’s late-write tests omit this method. It must participate in the same safe writer protocol.

4. **WRONG — the memory legacy-checkpoint guard is not specified atomically enough.**

   Rev4 says to replace the current existence check with a status check “while holding the existing mutation guard,” but the current legacy method has no `journal_fold_guard`: it drops `inner` before acquiring `checkpoints` ([task_store.rs:715](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-core/src/task_store.rs:715>)). A literal surgical replacement permits:

   1. Writer observes `Working`.
   2. Terminal transition or retention acquires `journal_fold_guard`.
   3. Writer resumes and inserts a checkpoint after terminalization or purge.
   4. The next sweep deletes that fresh checkpoint using the old task eligibility time.

   `put_node_checkpoint` must hold `journal_fold_guard` across both the status check and checkpoint insertion, with a controlled interleaving test.

Resume itself does not currently create the alleged terminal→Working window: non-batch resume reads only `working_tasks()` ([detached.rs:1695](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-coordinator/src/detached.rs:1695>)), and batch resume filters children to `Working` ([batch.rs:643](</Users/wesleyjinks/code/a2a-bridge/crates/bridge-coordinator/src/batch.rs:643>)). Thus the boot sweep before resume cannot select those tasks. If terminal reopening is added later, it will require an atomic terminal→Working CAS before any retention selection; rev4 does not presently define such a path.

**Verdict: FIX — fail closed on invalid persisted timestamps; repair cancellation/write ordering; guard `record_event_sequenced`; and make the memory legacy checkpoint status-check/write atomic.**

