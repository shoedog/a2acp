---
task-type: implement
---
# R2f1b 3c2 Task G targeted repair

## Description

Perform the one contracted targeted repair of the Task G artifact. The
frozen input is exact commit
`4c8e408bc2290db9de9a3c31763cc0a0a2655c76`. Two confirmed blockers plus
one low-severity confirmed diagnostic defect from the implementation
review; nothing else changes. Production remains `LegacyV2` with the V3
route unarmed.

Own `crates/bridge-workflow/src/executor.rs`, focused colocated tests,
and `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`.
Do NOT touch `bin/a2a-bridge/src/smoke.rs` — see repair 3.

Implement exactly these repairs:

1. **Preflight run-cache retention keyed on proven-clean (blocker).** A
   configure failure or provably-unaccepted prompt failure whose
   cleanup returns anything other than exact `Ok(Complete)` — that is,
   `Ok(Unknown)`, `Ok(Retained)`, `Ok(Preserved)`, or a cleanup error —
   exits through the "preflight exhausted" terminal with
   `retain_in_run_cache: false`, so `ensure_preflight` evicts the
   failed cell and a LATER node using the same preflight-enabled agent
   recomputes the preflight and may reconfigure and prompt the same
   logical session despite cleanup not being proven complete — a direct
   violation of the binding retry gate. Repair: retain the preflight
   failure in the run cache whenever cleanup is anything other than
   exact `Ok(Complete)`; evict only failures that are BOTH
   pre-acceptance AND proven-clean. Red: call the preflight entry twice
   with a configure failure and separately a provably-unaccepted prompt
   failure, with cleanup returning each protective disposition
   (`Unknown`, `Retained`, `Preserved`) and a cleanup error; assert
   exactly one configure/prompt/cleanup occurred and ZERO second
   dispatch. On the frozen input the second call redispatches.
2. **Preflight failure-reason preservation (confirmed low-severity,
   disclosed inclusion).** When the preflight response was empty,
   unexpected, or canceled and cleanup succeeds, the reason match's
   `Some(Ok(disposition))` arm reports `cleanup incomplete: Complete` —
   masking the real failure with a self-contradictory message. Repair:
   add a `Some(Ok(Complete))` arm that preserves the underlying
   response-derived reason (empty final / unexpected response / stream
   ended). Red: terminal-empty, unexpected, and canceled preflight
   outputs never produce a reason containing
   `cleanup incomplete: Complete`.
3. **Name the G2 split (blocker — documentation obligation only).** The
   review's caller enumeration found a second production consumer that
   collapses the typed disposition: smoke's generic `cleanup_step` maps
   every `Ok(T)` — including `Ok(Unknown)`/`Retained`/`Preserved` from
   `release_session_observed` — to the artifact value `"completed"`.
   That file is OUTSIDE this task's ownership, and the binding contract
   requires one consumer per task: do not fix it here. Repair: the
   handoff must enumerate this consumer with its exact sites, state
   that the ordinary-smoke aggregate currently stays conservative only
   via the run backstop, and name **G2** as the separate follow-up
   slice (protective mapping of all four typed dispositions into the
   smoke artifact without upgrades, with wire-compatibility review).

## Acceptance Criteria

- Begin with focused red tests; record exact pre-change red commands
  and admissibility; repairs 1 and 2 need tests that fail behaviorally
  on the frozen input.
- The retry gate is now complete across node AND preflight consumers:
  nothing redispatches or replays a session whose cleanup is not
  exactly proven `Complete`; the accepted-prompt never-replay retention
  from the frozen input is preserved unchanged.
- All existing bridge-workflow and bridge-worktree tests keep passing
  unchanged except any that pinned the two defective behaviors,
  migrated with cause.
- Run `cargo test -p bridge-workflow` and `cargo test -p
  bridge-worktree` (all harnesses), plus `git diff --check` and
  `cargo fmt --all -- --check`; no `rustfmt::skip`.
- Refresh the handoff: exact frozen input `4c8e408b`, red evidence, the
  G2 consumer enumeration and naming, honest churn accounting
  (additions plus deletions, post-format), and the statement that G2
  and the aggregate review are still ahead and production V3 remains
  unarmed.
- Stop and report before exceeding **100 changed production lines or
  300 total changed lines** (churn convention, post-format) relative to
  `4c8e408b`.

## Files

- `crates/bridge-workflow/src/executor.rs`
- focused colocated tests
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout)
- repository `AGENTS.md`

## Commit Message

fix(r2f1b): retain unproven preflight cleanup and preserve failure reasons

## Round Contract

This dispatch is the single contracted targeted repair of the Task G
artifact. One hard-read-only Sol/xhigh closure review follows
separately; do not self-repair a rejection. Never restart from a fresh
artifact and never extend the cap.
