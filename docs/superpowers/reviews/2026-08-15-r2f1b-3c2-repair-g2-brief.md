---
task-type: implement
---
# R2f1b 3c2 Task G2 targeted repair

## Description

Perform the one contracted targeted repair of the Task G2 artifact. The
frozen input is exact commit
`737239ae16efd6be1ca2cd474586c5d8c751e16f`. One confirmed blocker from
the implementation review; nothing else changes. The typed smoke mapping
itself was adjudicated correct — do not touch it.

The operator explicitly authorizes the narrow ownership expansion this
repair needs: `bin/a2a-bridge/src/fallback_plan.rs`, release-field
validation only. Own that, focused CLI tests, and
`docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`.
Do not touch `smoke.rs` or anything else.

Implement exactly one repair:

1. **`fallback-plan` accepts the new protective release vocabulary.**
   `validate_cleanup` gates cancel, release, and retire with one shared
   closure accepting only the old four values, so a genuine failed
   smoke artifact whose release step now records `"unknown"`,
   `"retained"`, or `"preserved"` is rejected as `invalid or incomplete
   smoke cleanup record` (a command error) BEFORE eligibility
   classification — where the old collapsed `"completed"` passed
   validation and produced structured `eligible:false` JSON. Repair:
   give the release field its own accepted set — the old four values
   plus `unknown`, `retained`, and `preserved`; keep cancel and retire
   on the old vocabulary; keep the pre-spawn exact-equality
   authorization comparison unchanged (it already compares the whole
   wire value). Red: for EACH protective release value, feed a
   complete failed smoke-v2 artifact to `fallback-plan --from` and
   assert command success with structured `eligible:false`,
   diagnostics-incomplete reasoning, and no rerun command — on the
   frozen input the command exits nonzero with no plan. Positive
   pins: an old-vocabulary artifact classifies exactly as before, and
   a pre-spawn artifact's authorization behavior is unchanged.

## Acceptance Criteria

- Begin with focused red tests; record exact pre-change red commands
  and admissibility; all three protective-value cases must fail
  behaviorally on the frozen input.
- No behavior change for cancel/retire validation, the backstop check,
  the grace-timeout bounds, or pre-spawn authorization.
- All existing fallback-plan and smoke tests keep passing unchanged.
- Run the focused fallback-plan/smoke tests and
  `cargo test -p a2a-bridge --bin a2a-bridge`, plus `git diff --check`
  and `cargo fmt --all -- --check`; no `rustfmt::skip`.
- Refresh the handoff: exact frozen input `737239ae`, red evidence, the
  completed reader enumeration (fallback-plan now compatible), honest
  churn accounting (additions plus deletions, post-format), and the
  statement that the 3c2 aggregate review is still ahead and production
  V3 remains unarmed.
- Stop and report before exceeding **60 changed production lines or 200
  total changed lines** (churn convention, post-format) relative to
  `737239ae`.

## Files

- `bin/a2a-bridge/src/fallback_plan.rs` (release-field validation only)
- focused CLI tests
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout)
- repository `AGENTS.md`

## Commit Message

fix(r2f1b): teach fallback-plan the protective release vocabulary

## Round Contract

This dispatch is the single contracted targeted repair of the Task G2
artifact. One hard-read-only Sol/xhigh closure review follows
separately; do not self-repair a rejection. Never restart from a fresh
artifact and never extend the cap.
