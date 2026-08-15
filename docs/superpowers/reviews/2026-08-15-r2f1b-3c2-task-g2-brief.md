---
task-type: implement
---
# R2f1b 3c2 Task G2: protective typed cleanup dispositions in the smoke artifact

## Description

Begin Task G2 — the named one-consumer split from Task G — on the exact
accepted Task G head `2a912d18067c0ae59a598d2ba1c3c611117e6c7b`. Task G's
review confirmed (100/100) that smoke's generic `cleanup_step` maps every
`Ok(T)` — including `Ok(Unknown)`, `Ok(Retained)`, and `Ok(Preserved)`
from `release_session_observed` — to the serialized artifact value
`"completed"`, giving operators false release evidence whenever cleanup
settles protectively. The ordinary-smoke aggregate currently stays
conservative only via the run backstop. Fix exactly this one consumer.
Production remains `LegacyV2` with the V3 route unarmed; no provider,
live, or billable behavior changes; smoke itself is NOT executed against
live providers in this task.

Own `bin/a2a-bridge/src/smoke.rs`, its colocated/focused tests, and
`docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`.
Do not modify bridge-workflow, bridge-worktree, bridge-api, bridge-core,
or any other `bin/` module.

Implement:

- **typed release step:** the step that consumes
  `release_session_observed` records the exact typed disposition in the
  artifact — `"completed"` ONLY for exact
  `BackendCleanupDispositionV1::Complete`; each protective disposition
  gets its own non-upgrading serialized value (e.g. `"unknown"`,
  `"retained"`, `"preserved"`); errors and timeouts keep their existing
  values. Steps that do not carry a typed disposition (cancel, retire)
  keep their current contract unchanged;
- **protective aggregate:** the artifact's aggregate/cleanup disposition
  folds protectively — a non-`Complete` release can never contribute to
  an aggregate that reads as complete; the existing run backstop
  behavior is preserved, not relied upon;
- **wire-compatibility review:** the handoff documents the serialized
  contract change (the new accepted values for the release step, and
  that "completed" narrowed to exact `Complete`), enumerates every
  reader of the artifact's release field in this repository, and states
  the compatibility posture for each. If any in-repository reader would
  BREAK on the new values, stop and report instead of adapting that
  reader here.

## Acceptance Criteria

- Begin with focused red tests; record exact pre-change red commands and
  admissibility. Inject each protective disposition (`Unknown`,
  `Retained`, `Preserved`) into the release step and assert the
  serialized release value and the aggregate never say completed — all
  three must fail on the frozen input (which serializes `"completed"`).
  Add the positive case: exact `Complete` still serializes
  `"completed"`.
- No behavior change for error/timeout paths or for the cancel/retire
  steps; existing smoke tests keep passing unchanged except any that
  pinned the collapsed mapping, migrated with cause.
- Run the focused smoke tests plus `cargo test -p a2a-bridge --bin
  a2a-bridge` if the harness covers smoke, `git diff --check`, and
  `cargo fmt --all -- --check`; no `rustfmt::skip`.
- Refresh the handoff: exact frozen input `2a912d18`, red evidence, the
  wire-compatibility enumeration, honest churn accounting (additions
  plus deletions, post-format), and the statement that the 3c2
  aggregate review is still ahead and production V3 remains unarmed.
- Stop and report before exceeding **120 changed production lines or
  300 total changed lines** (churn convention, post-format) relative to
  `2a912d18`.

## Files

- `bin/a2a-bridge/src/smoke.rs`
- focused smoke tests
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout; the G2 boundary is stated in its Task G sections)
- repository `AGENTS.md`

## Commit Message

fix(r2f1b): serialize exact typed cleanup dispositions in smoke evidence

## Round Contract

This dispatch performs one implementation attempt and one independent
Sol/xhigh review. Do not self-repair a review rejection. The operator
will first classify it: only a closed, enumerable rejection may receive
one targeted repair on this same artifact followed by one closure
review. An open-class or repeating family parks G2. Never restart from a
fresh artifact and never silently extend the cap.
