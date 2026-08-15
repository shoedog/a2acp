---
task-type: code-review
---
# R2f1b 3c2 Task G2 closure review

## Description

Perform the one counted closure review of the complete Task G2 line:
exact diff `2a912d18..50f3336e` in this checkout, where `2a912d18` is
the accepted Task G head and `50f3336e` is the current head. This is the
closure declared by the G2 round contract; it is capped at one pass with
no repair loop inside it. G2 is the named one-consumer split from Task G
and the last implementation work before the 3c2 aggregate review.

The line has three commits:

1. `737239ae` — the typed mapping: smoke's release step records the
   exact `BackendCleanupDispositionV1` — `"completed"` ONLY for exact
   `Complete`; `Unknown`/`Retained`/`Preserved` serialize as their own
   non-upgrading values; error/timeout and the cancel/retire steps
   unchanged; the artifact aggregate folds protectively without relying
   on the run backstop; three behaviorally fail-first protective reds.
   Its advisory review adjudicated the mapping CORRECT and traced
   persistence (workflow history schema accepts `unknown`),
   `workflow-stats`, and compatibility readers as compatible — and
   REJECTed on exactly one blocker: `fallback-plan`'s
   `validate_cleanup` gated cancel/release/retire through one shared
   closure accepting only the old four-value vocabulary, so a genuine
   protective artifact became `invalid or incomplete smoke cleanup
   record` (a command error) BEFORE eligibility classification, where
   the old collapsed `"completed"` had produced structured
   `eligible:false` JSON.
2. `bc313dc6` — the contracted targeted repair under an
   operator-authorized narrow ownership expansion (release-field
   validation in `fallback_plan.rs` only; the operator ruled
   fallback-plan fail-closes rather than collapses, so the
   one-consumer-per-task clause required no further split): the release
   field gets its own accepted set (old four plus `unknown`,
   `retained`, `preserved`); cancel/retire keep the old vocabulary; the
   pre-spawn whole-wire equality authorization is unchanged; protective
   releases add `source_diagnostics_incomplete` and rerun emission
   still requires an empty reason list. Per-value fail-first CLI
   regressions assert command success, `eligible:false`, the diagnostic
   reason, no rerun, and no execution. Production churn 4 lines, total
   99. Its advisory review adjudicated the code repair CORRECT and
   rejected solely on the red in-container whole-bin test gate.
3. `50f3336e` — the disclosed operator docs completion (22 handoff
   lines, zero code): the exact post-commit host run of the whole-bin
   target on `bc313dc6` is green — **1,090 passed / 0 failed** — and
   the container failure carried the ledgered flock-EBADF hermetic
   signature (instance 10 of the class; 10/10 host-green on
   exact-command controls; the G2 diff touches no process, lock, or
   liveness code).

Adjudicate:

- the attempt-1 blocker (fallback-plan reader break) as FIXED, PARTIAL,
  or OPEN against `50f3336e`, including that no OTHER validation site
  or reader rejects or upgrades the new vocabulary;
- the operator's no-further-split ruling (fail-closing reader ≠
  collapsing consumer) and the narrow ownership expansion — falsify
  either if you can;
- the repair-advisory gate blocker as FIXED given the dated
  exact-command host evidence and the class history;
- the G2 binding criteria across the full line: `"completed"` is
  producible only by exact `Complete`; each protective disposition
  round-trips through the artifact without upgrade; the aggregate never
  reads complete from a protective release; cancel/retire/error/timeout
  behavior byte-identical; pre-spawn authorization unchanged;
- scope: across the line only `bin/a2a-bridge/src/smoke.rs`,
  `bin/a2a-bridge/src/fallback_plan.rs`,
  `bin/a2a-bridge/tests/fallback_plan_cli.rs`, and the implementer
  handoff changed; `Cargo.lock` unchanged; no `rustfmt::skip`; no
  crates/ change; production V3 remains unarmed and the Task A-G
  surfaces are untouched.

Supplied exact-head evidence is corroboration only; you are licensed to
falsify or reject every supplied result:

- head `50f3336e`, clean worktree, branch
  `implement/impl-57172-il1509xh`;
- attempt-1's in-container verify was fully green; the repair's
  in-container verify failed ONLY at the whole-bin target with the
  flock-EBADF signature; the operator host control on exact `bc313dc6`
  ran the whole-bin target **1,090 passed / 0 failed**;
- operator host gates on exact `50f3336e` all exit 0: `git diff
  --check`, formatter, locked all-target/all-feature workspace check
  and Clippy with `-D warnings`, full locked all-feature workspace test
  **4,101 passed / 0 failed / 13 ignored across 90 harnesses**, locked
  release build, `cargo deny check`, and repository hygiene.

## Acceptance Criteria

- Put every WRONG finding before every SMELL finding; each WRONG must
  name a constructible input/state, the incorrect result, realistic
  reachability, and a bounded fix.
- Explicitly adjudicate the two prior blockers and the no-further-split
  ruling, and confirm no regression in the previously sustained Task
  A-G families reachable from these files.
- Give 0-100 confidence and name evidence that would raise, lower, or
  collapse the conclusion.
- End with the review prompt's exact `VERDICT:` and `SUMMARY:` terminal
  lines.

## Files

- `bin/a2a-bridge/src/smoke.rs`
- `bin/a2a-bridge/src/fallback_plan.rs`
- `bin/a2a-bridge/tests/fallback_plan_cli.rs`
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout)
- repository `AGENTS.md`
