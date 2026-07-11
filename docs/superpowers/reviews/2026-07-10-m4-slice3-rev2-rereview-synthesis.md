# M4 Slice 3 rev2 — Re-Review Panel Synthesis

**Panel:** codex gpt‑5.5 xhigh (data-safety, repo-verified) + fable `claude-fable-5[1m]` (architecture).
**Reviewed:** `docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev2.md`
**Verdict trajectory:** rev1 = REDESIGN + REVISE → **rev2 = FIX (codex) + REVISE (fable)**. The architecture is now sound; rev2 needs a targeted **rev3 that is mostly subtraction plus one real fix.**

## Did rev2 close the rev1 data-loss WRONGs?
- **#1 turn_log↔task ownership — CLOSED** (default TTL/size paths). No longer treats `NULL task_id` as orphan; the fixed call sites are the right ones.
- **#2 terminal-purge missing legacy NULL rows — PARTIAL.** Fixes the leak but introduces a constructible **over-delete** (see A1).
- **#3 `completed_ms` finalization race — PARTIAL.** The barrier blocks the simple race, but reconciliation can **mislabel lost usage as no-usage** (see A2).

## The convergence: cut two things, fix one
Both lenses independently point at the **terminal hard-delete** and the **legacy backfill** as the weak parts. Removing them resolves four findings at once.

### Must-fix
1. **Cut the terminal hard-delete (`purge_terminal_tasks` / `unsafe-terminal-task-purge`) from Slice 3.**
   - fable **WRONG**: the feature ships the riskiest SQL permanently **untested** (no feature-matrix CI job; `#[cfg]` trait method isn't even compiled by default builds) — it rots from day one, and gating merely *relocates* the open-world predicate rather than resolving it.
   - codex **WRONG (A1)**: the legacy `session_id` GLOB matcher can **delete a *different* live task's turn row** — victim `task-a` matches live `task-a-resume-1` (task-id namespace isn't reserved; constructible).
   - codex **WRONG (A5)**: the Cargo feature is underspecified across `bridge-core`/`bridge-store`/bin — `--features unsafe-terminal-task-purge` won't even compile coherently without forwarding.
   - → **One subtraction kills all three.** Task rows are tiny metadata; artifact retention already reclaims the bulk. Nothing in the data-safety findings needs task-row deletion. Defer it to the Coordinator-owned tombstone/archival flow the spec itself names as the right shape.

2. **Drop the runtime legacy backfill; keep the write-path `task_id` fix + "never delete ambiguous NULL rows."**
   - fable **SMELL**: the backfill bakes a session-ID string convention into permanent SQL (the GLOB match appears 3×) to reclaim a small frozen tail.
   - codex **SMELL (A4)**: the migration-time `UPDATE turn_log …` is **unbounded** and can stall first boot under the store lock.
   - → Dropping it also removes A1's over-match vector for free. If legacy usage-rollup ever matters, do a one-shot offline migration tool, not permanent sweep SQL.

3. **Fix the finalization barrier's lost-vs-no-usage gap (the one thing to actually *add*, carefully).**
   - codex **WRONG (A2)**: `TurnFinished` and `UsageFinalized` are separate queued commands; `try_send` can drop, or a crash can land between them. rev2's 24h reconciliation then marks a row `reconciled_no_usage` and makes it deletion-eligible — **deleting real cost data.**
   - fable (adjacent **SMELL**): size eviction has **no age floor**, so a task terminal for one second is evictable while its `UsageFinalized` may still be queued.
   - → Persist an **explicit no-usage marker from the producer path** (don't infer "no usage" from NULL columns + time), and give `RetentionService` a **minimum-age floor** (reuse the 24h grace) so nothing is deletable inside the finalization window.

### Tighten in the same slice
- **[codex SMELL A3]** `spawn_detached_workflow` should set `ctx.task_id = Some(task)` internally so a future caller can't reintroduce NULL ownership.
- **[fable SMELL]** `artifact_retention_max_bytes` measures a synthetic logical-byte number that never matches disk (`VACUUM` is a non-goal) and must be hand-patched per column → measure `PRAGMA page_count*page_size` or defer size-eviction out of the slice (TTL alone suffices for personal tier).
- **[fable SMELL]** the eligibility predicate is hand-copied ~6–7×; centralize as one SQL fragment/view. Acceptance criterion 14 ("eviction policy lives only in `RetentionService`") is literally false — eligibility/ordering/byte-accounting live in store SQL; state the boundary honestly.
- **[fable WRONG, minor]** `ArtifactDeleteStats` sums `bytes_before` across deletions → garbage byte totals; split per-delete counts from the pass-level snapshot.
- **[fable SMELL]** consider splitting into **3a** (ownership fix + finalization barrier + reconciler, *no deletion*) and **3b** (RetentionService + `[storage]` + routes), so the correctness prerequisite ships and ages before anything deletes against it.

## What rev2 got right — keep unchanged
Finalization barrier (concept), the write-path `task_id` fix, never-treat-NULL-as-orphan, `artifacts_purged_at` with the 410-vs-404 distinction, bounded sweeps, and the `RetentionService` extraction. Both reviewers agree the core is sound.

## Recommended next step
A **rev3 pass that mostly subtracts**: cut terminal hard-delete + backfill, add the producer-side no-usage marker + age floor, and apply the tightenings. This is small enough that it may not need another full architect run — a focused edit + a single data-safety re-check would likely suffice. Do not plan/implement until findings A1, A2, A5 are resolved.
