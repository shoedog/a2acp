You are doing a WHOLE-BRANCH review of the entire Slice 4 (Compact) change versus `main` for a2a-bridge.
READ-ONLY: `git diff main...HEAD`, `git log main..HEAD --oneline`, read files, grep; do NOT edit/build/test.
This is the high-value CROSS-TASK pass — the per-increment reviews saw each commit in isolation; your job is to
catch interactions, races, and lifecycle bugs that only appear when all 7 tasks compose. Severity-tag
BLOCKER / MAJOR / MINOR with concrete fixes (file:line).

## What Slice 4 added
`compact(ctx)` = summarize gen N → reset to N+1 → seed the summary as the next turn's FIRST part; require-Idle,
no force. Composition over the shipped `reset_session` under a NEW `Compacting` claim. The branch:
`SessionState::Compacting`; `WarmHandle.pending_seed`/`WarmTurn.seed`; `SessionManager::compact_session` +
`expire_after_summarize` + `compact_summarize_timeout`; seed take-and-clear in `checkout_turn` + drop in
`reset_session`; `summarize_collect`; the dual-site seed prepend; `SessionCompact` wire + `session compact`
CLI + config knob.

## Read first
- `git log main..HEAD --oneline` then `git diff main...HEAD`.
- Spec: `docs/superpowers/specs/2026-06-18-slice-4-compact.md` (the "## v2" FIX-1..14 are BINDING).
- KNOWN + DEFERRED (do NOT re-litigate as new blockers): the two PRE-EXISTING warm-session concurrency races
  in `docs/superpowers/2026-06-18-FOLLOWUP-warm-turn-cancellation-tokens.md` (cancel→next-turn op collision;
  force-clear vs producer-start). Slice 4 is require-Idle / no-force and is intended to be CLEAR of them —
  verify that it does NOT depend on `force`/cancel-under-concurrency, but treat the two races themselves as
  already-tracked, not new findings.

## CROSS-TASK review focus (ground each in code, cite file:line)
1. **The `Compacting` claim across the whole lifecycle.** Is `Compacting` consistently handled everywhere
   `is_claimed` matters — `reap_idle` (must NOT reap a Compacting handle mid-summarize), `release`/`cancel`
   (must DEFER, not mutate/remove), a concurrent `checkout_turn` (HandleBusy)? Any state-machine path where a
   `Compacting` handle is reaped, double-released, or stranded? Does the new state break any exhaustive
   `match SessionState` (compile) or any status/observability path?
2. **The seed lifecycle end-to-end.** compact stashes `pending_seed` → `checkout_turn` take-and-clears at the
   TWO resume returns → dispatch prepends at BOTH sites → `reset_session`/clear drops it. Trace every path: can
   the seed be (a) delivered twice, (b) lost when it shouldn't be, (c) survive a clear, (d) leak across
   contexts, (e) delivered on a NON-resume (mint) checkout? Does a compact-then-reconcile-checkout still
   deliver it (the post-reconcile-clean return)?
3. **The EXPIRE keystone under composition (FIX-1).** Every post-claim summarize failure EXPIREs. Confirm: the
   old session is released exactly once (no double-release vs the reset tail), the lease is dropped, the handle
   removed; a deferred release/cancel arriving during summarize is honored; no `?` strands `Compacting`. Does
   `expire_after_summarize` + the reset-tail EXPIRE branch ever double-release `old_id` or leak `new_id`?
4. **summarize ↔ the warm turn machinery.** The summarize drives `backend.prompt` directly (bypassing the
   translator/policy/store). Does that interact badly with anything (the WarmTurnGuard, usage recording, the
   producer)? Could a summarize turn's `backend.prompt` be confused with a real client turn? Is the
   zero-footprint (no TaskId) summarize safe?
5. **The wire/CLI/config integration.** Does `session_compact` map outcomes/errors correctly
   (`MessageTooLarge`→-32603, `NotFound`→`SessionNotFound`→HTTP 400)? Does the handler's `compact_session(&ctx,
   summarize_collect)` wiring type-check and behave? Any regression to `session_clear`/the other session
   methods from the shared dispatch/handler code?
6. **No regressions to shipped behavior.** `reset_session`/`checkout_turn`/reconcile/reap/release/cancel
   semantics from Slices 0–3 must be intact (the seed take-and-clear + the `pending_seed=None` drop were
   inserted into those hot paths). Any behavior change to a non-compact path?

OUTPUT: findings by severity (file:line, concrete fix); a cross-task-correctness verdict; a no-regression
verdict. End with exactly: `BRANCH VERDICT: merge-clean | fix-then-merge | rework`.
