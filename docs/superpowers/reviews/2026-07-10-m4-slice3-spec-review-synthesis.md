# M4 Slice 3 (Retention) — Spec-Review Panel Synthesis

**Panel:** codex gpt‑5.5 xhigh (correctness / data-safety, repo-verified) + sonnet (architecture / soundness, spec-only). fable lens skipped (mint failure); gpt‑5.6‑sol attempted for the architecture lens but is unusable here — it crashes `codex-acp` (`AgentCrashed`, transport/kill-switch) on any prompt, including a one-word input, with `effort` skipped, while gpt‑5.5 and gpt‑5.4 run fine on the same harness. Architecture lens therefore stays on sonnet (owner decision).
**Reviewed:** `docs/superpowers/specs/2026-07-10-m4-slice3-retention-design.md`
**Combined verdict: REDESIGN.** codex's REDESIGN (grounded in verified code) supersedes sonnet's REVISE (structural judgment). The spec is **not plannable as written.**

## The convergence (why this is REDESIGN, not REVISE)
Both lenses independently attacked the delete-safety model and both concluded it is broken — from two different directions that share one root cause: **the spec's model of `turn_log` row ownership is wrong, and its delete guard is an open-world blocklist that cannot stay correct.**

- **sonnet (structural):** the terminal-task guard deletes "anything not currently listed as unsafe." The spec *already admits* a gap (merge state / ADR‑0027 lives outside `TaskStore`, invisible to the predicate). A blocklist in a SQL literal with no compiler enforcement will silently kill data the next time a resumable state is added.
- **codex (factual):** the spec assumes `turn_log.task_id IS NULL` ⇔ warm-inline & disposable. **False.** Detached/batch tasks *also* never set `task_id` (`executor.rs:25/243`, `coordinator.rs:701`, `batch.rs:860`, `sqlite.rs:790`). That single wrong assumption breaks both the orphan rule and the terminal-purge cleanup.

## Confirmed must-fixes (data-loss first)

1. **[codex WRONG] Orphan-TTL deletes live crash-resumable turn rows.** With traces on + default `artifact_retention_days=14`, a `Working` *detached* task's completed node-turn older than the cutoff is deleted as an "orphan" (`task_id NULL`) even though the task is resumable — and *immediately* if `artifact_retention_max_bytes>0`. → The retention model must join `turn_log` to real task liveness, not treat NULL `task_id` as disposable.

2. **[codex WRONG] Terminal-purge misses detached turn rows → breaks the 404 contract.** `DELETE FROM turn_log WHERE task_id IN victims` never matches detached rows (NULL `task_id`); after the task record is deleted, `/turns/:turn_id` still returns **200** on leaked rows (`server.rs:1034`). → Cleanup must key on the real turn↔task linkage.

3. **[codex WRONG] `completed_ms` is not a safe "done" marker.** `TurnFinished` upserts `completed_ms` *before* `UsageFinalized` (a later queued command); `update_turn_usage` fails if the row was already evicted (`observ/lib.rs:271/288`, `sqlite.rs:812/842`). With `artifact_retention_max_bytes=1`, eviction between the two irreversibly deletes cost/token data. → Need a **durable turn-log finalization barrier** before a row is eligible for deletion. *(This directly falsifies the spec's own Data-Safety Guards #6/#7, which rely on `completed_ms`.)*

4. **[sonnet WRONG] Open-world delete predicate rots.** Replace the blocklist with a **two-phase tombstone** (`purge_after_ms` written on the victim; Coordinator clears it on re-acquire; sweep hard-deletes only after confirming no live reference) — or gate `purge_terminal_tasks_days` behind a **compile-time feature flag**, not a TOML default.

## Structural changes to fold into the redesign (sonnet SMELLs)
- **Extract policy from the port:** move `evict_artifacts_to_max_bytes` (candidate materialization, sort, loop, `cap_reached`) into a `RetentionService`; leave `list_terminal_artifact_candidates()` + `delete_task_artifact_set()` as thin store primitives.
- **Make half-deleted state legible:** add `artifacts_purged_at: Option<i64>` to `TaskRecord` so Slice‑2 routes can distinguish "never had artifacts" from "purged" instead of a bare 404.
- **Bound the boot sweep:** `LIMIT N` per sweep so a first-ever boot with a large backlog can't stall serve startup in one unbounded transaction.
- **Respect the Coordinator seam** (longer-term): route retention through a Coordinator archival trigger rather than a direct background store delete.

## Also confirmed
- Cited schema/status/cascade anchors are **real** (codex verified `task_store.rs:14/44`, `sqlite.rs:140/150/158/165`), but incomplete for safety.
- Counter rebuild from surviving `turn_log` is defensible — *but only after* the linkage + finalization races are fixed.

## Recommended next step
Feed both raw reviews back to the gpt‑5.5 architect for a **revised spec** (same input + these findings), then re-run this panel on the revision — the exact draft→review→revise loop used for Slice 2. Do **not** proceed to a plan until the turn↔task linkage, the finalization barrier, and the delete-guard model are redesigned.
