## Prior structural must-fixes: status up front

- **Must-fix #2 (eviction policy out of `TaskStore`): CLOSED.** The `RetentionService` split is real and defensible, with smells noted below.
- **Must-fix #3 (illegible half-deleted state): CLOSED — and over-closed.** `artifacts_purged_at` + 410-vs-404 is exactly right; the backfill/repair surface bolted on around it is more than the fix required.
- **Must-fix #1 (open-world delete predicate): NOT closed well.** The predicate is unchanged; it was relocated behind a Cargo feature. That's a relocation, not a resolution — see Finding 1.

## Ranked findings

### 1. WRONG — `unsafe-terminal-task-purge` ships the most dangerous SQL in the slice permanently untested, per the spec's own gates

The spec defines regression tests that require the feature (`sqlite_terminal_purge_deletes_legacy_null_detached_turn`, `sqlite_terminal_purge_refuses_*`, `sqlite_terminal_purge_cascades_child_tables`, `memory_terminal_purge_removes_all_task_maps`), but Global Constraints and the Testing section keep exactly one gate: `cargo fmt` + clippy + `cargo test --workspace`. No feature-matrix CI job is specified anywhere. Concrete failure: those tests never compile in any specified build; the temp-table victim query — the single riskiest statement in the design, joining `tasks`/`batch`/`sessions`/`turn_log` with GLOB session-id parsing — rots from day one, and the trait's `#[cfg]` method means even *compilation* of the path is unverified by default builds.

Beyond the test gap, the gate doesn't answer the original structural objection. The victim query is still an enumeration of known referents (`task_node_starts`, batch status, pending sessions, unfinalized turns); the next table that references `tasks` (any future merge-state, annotations, whatever) is silently missing from the guard in whatever custom builds exist. And "irreversible deletion requires a recompile" is a worse operational model than it sounds for a project that ships release tarballs: the only users of this path will be running unofficial builds that CI has never exercised.

**Fix: cut terminal hard-delete from Slice 3 entirely.** `tasks` rows are small metadata; the artifact retention in this same slice reclaims the actual bulk (journal, checkpoints, turn rows). Nothing in the data-safety findings requires task-row deletion. When it's genuinely needed, do the Coordinator-owned tombstone/archival flow properly — which the spec already names as the right long-term shape. If the owner insists on keeping it, the minimum is a dedicated CI job building and testing `--features unsafe-terminal-task-purge`; but keeping it is the wrong call.

### 2. SMELL — `artifact_retention_max_bytes` measures a synthetic quantity that corresponds to nothing observable, via a formula that is a maintenance trap

The hand-rolled per-column logical-byte expression (a) ignores indexes, page overhead, WAL, and freelist, and (b) with `VACUUM` an explicit non-goal, the DB file never shrinks regardless of what eviction does — so a user who sets `max_bytes = 100MB` to bound disk will watch the file sit above that indefinitely while the sweep reports `cap_reached = true`. The knob bounds an internal number the user cannot see or verify. Worse, the formula must be hand-updated for every future `turn_log` column — rev2 itself demonstrates the trap by having to patch the expression for its own two new finalization columns, and an omission is silent under-counting no test will catch.

Fix: either measure something real (`PRAGMA page_count * page_size`, with eviction bounding growth rather than promising a ceiling) or defer size eviction out of this slice — TTL alone is adequate for the personal tier, and this is the second-largest chunk of machinery after the hard purge. (Adjacent, for the data-safety reviewer: size eviction has no age floor, so a task terminal for one second is immediately evictable while its `UsageFinalized` may still be queued in the observer channel; structurally the cheap fix is sharing the 24h grace as a minimum-age floor in `RetentionService`.)

### 3. SMELL — the legacy backfill bakes a session-ID naming convention into permanent runtime SQL, and the safety findings don't need it

The `session_id = task.id OR prefix-match "{task}-resume-{digits}"` GLOB join appears **twice** (backfill CTE and the hard-delete victim query) and is duplicated a third time in `MemoryTaskStore`. This is a foreign key by string convention: if the resume-session ID format ever changes, matching silently degrades, and `ORDER BY length(t.id) DESC LIMIT 1` is an admission that the join is ambiguous today. The thing is, the data-safety findings are already closed without it: the write-path fix stops new `NULL task_id` rows, and the "ambiguous workflow/node rows are never deleted" guard makes the legacy set safe. That set is bounded and frozen — for a personal-tier tool it's a trivial number of rows sitting harmlessly forever. The entire backfill surface (SQL, memory impl, reconciliation interplay, 4+ tests) exists to reclaim and re-aggregate a small static tail.

Fix: drop the runtime backfill. Keep the write-path fix and the ambiguity protection. If usage-rollup completeness for legacy rows ever matters, a one-shot offline migration tool is the right home — not permanent sweep SQL. Cutting Finding 1 removes the second copy of the GLOB match for free.

### 4. SMELL — the slice is three features wearing one config table; the correctness prerequisites should land as their own slice

After rev2, "[storage] retention" bundles: (a) turn-log **correctness repairs** — the ownership write-fix at five coordinator call sites plus the `usage_finalized_ms` barrier through `bridge-observ` and both stores; (b) a TTL/size **janitor**; (c) a compile-gated **deleter**. Layer (a) is not retention — it's Slice-1 schema correctness that metrics/usage aggregation benefits from independently, and it touches a different set of crates (observ, coordinator, workflow) than the janitor does. It's also a hard prerequisite: every delete predicate trusts the barrier, so the barrier should ship and age before anything deletes against it. Split into 3a (ownership + finalization barrier + reconciler, no deletion) and 3b (RetentionService + `[storage]` + routes). This matches how the project has de-risked every prior invasive change, and it shrinks each review's blast radius to one concern.

### 5. SMELL — the eligibility predicate is written out ~6 times and acceptance criterion 14 is already false as stated

"Terminal status + all linked turns finalized" appears in the candidate listing, three times as guard CTEs inside `delete_task_artifact_set`, again in the hard-delete victim query, and twice more re-derived in `MemoryTaskStore`. Re-checking at delete time is correct (TOCTOU), but seven hand-copied instances of the same predicate is drift waiting for a column rename. Meanwhile criterion 14 claims "eviction policy lives in `RetentionService`, not `TaskStore`" — but eligibility, ordering (`ORDER BY` in every candidate query), and byte accounting all live in store SQL, necessarily. State the boundary honestly: `TaskStore` owns *eligibility mechanics and atomic guards*, `RetentionService` owns *sequencing, caps, and grace*. Then centralize the SQL predicate as one shared fragment (or a view) so the guard and the listing can't diverge.

### 6. WRONG (minor) — `ArtifactDeleteStats` accumulation corrupts its own byte fields

The pass pseudo-code sets `bytes_before` once, then `stats += self.store.delete_task_artifact_set(...)` where each returned `ArtifactDeleteStats` also carries `bytes_before`/`bytes_after`. Concrete failure: two deletions each returning `bytes_before = N` sum into the pass total, producing garbage before/after numbers in logs. Split the type: per-delete row counts (summable) vs. pass-level byte snapshot (set once). Trivial, but it's in the spec's normative pseudo-code twice.

## 12-month regrets (Q5)

1. **The feature gate.** In a year it's either dead weight that every schema change must be manually audited against (in a build nobody runs), or someone's custom build hard-deleted rows a newer table still referenced. Cut it now; the eventual tombstone design supersedes it anyway.
2. **The session-ID GLOB convention in SQL.** It converts a transient ID-format choice into load-bearing schema. Drop the backfill; the ambiguous tail is cheaper to keep than the coupling.

What rev2 got right and should keep unchanged: the finalization barrier, the write-path `task_id` fix, never-treat-NULL-as-orphan, `artifacts_purged_at` with 410/404 distinction, bounded sweeps, and the `RetentionService` extraction. The core is sound; the revision needs subtraction, not more machinery.

## Verdict

**REVISE** — (1) cut `unsafe-terminal-task-purge` from the slice (or, weaker, add a dedicated feature-enabled CI job); (2) drop the runtime legacy backfill, keep write-fix + ambiguity protection; (3) split correctness prerequisites (ownership + barrier) into their own slice ahead of the janitor; (4) fix or defer size eviction's byte accounting.