---
task-type: implement
---
# R2f1b 3c2 Task G: protective disposition consumers and reconciliation shields

## Description

Begin Task G — the final 3c2 implementation task — on the exact accepted
Task F2 head `f17e2bd37868c398d5c04d175ffee2a5cc5c1a00`. Wire the
protective cleanup dispositions through to their consumers: the workflow
executor must see and honor the EXACT `BackendCleanupDispositionV1` that
Task E's cleanup cell records, and retry must be possible only when
cleanup is proven complete. Production remains `LegacyV2` with the V3
route unarmed; no HTTP or provider behavior changes.

Own `crates/bridge-workflow/src/executor.rs`, the affected
workflow/worktree test doubles, the production-route assertion (a test
asserting production construction assigns `resource_flight_route_v3 =
None` — test code only, wherever it best fits), and
`docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`.
Do not modify bridge-api or bridge-core production code, the Task A-F
surfaces, or `bin/` production wiring. The lane's roadmap cursor is
maintained by the operator outside this checkout — do not edit roadmap
files here.

Implement, per the binding salvage design:

- **exact disposition return:** `cleanup_cold_session` returns the exact
  cleanup disposition instead of collapsing it; enumerate ALL its
  callers and wrappers in the handoff and show each one either
  propagates the exact value or applies a protective (never-upgrading)
  fold;
- **retry gate:** redispatch/retry of a session's work is gated on the
  exact `Complete` disposition — `Ok(Unknown)` is a successful call
  whose cleanup is NOT proven and must not permit redispatch;
- **reconciliation shields:** post-acceptance persistence failure is
  fatal and nonretryable; the worktree's two-field
  `CleanupReportV1 { result, checkout }` contract is unchanged and its
  inner/checkout outcomes remain separate — only exact
  `Complete + Complete` may become `Complete`;
- **guards:** add the production-route assertion (production V3 remains
  unarmed) and a guard that the two-field ContainerRw cleanup contract
  is byte-compatible (no variant, field, or fold-rule change).

## Acceptance Criteria

- Begin with focused red tests; record exact pre-change red commands and
  admissibility. A compile failure counts only when it is specifically
  the missing Task G API; zero selected tests does not.
- `Ok(Unknown)` from cleanup cannot redispatch (red on the pre-change
  tree where the collapsed disposition permits it).
- Post-acceptance persistence failure is fatal/nonretryable (red where
  it is retried today, if it is; otherwise record the pre-change
  behavior honestly and pin it).
- Worktree inner/checkout outcomes remain separate: an `Unknown`
  checkout with a `Complete` inner (and vice versa) never folds to
  `Complete`.
- The production API route assertion holds: production construction
  assigns `resource_flight_route_v3 = None`.
- All existing bridge-workflow and bridge-worktree tests keep passing
  unchanged except any that pinned the collapsed disposition, migrated
  with cause.
- Run `cargo test -p bridge-workflow` and `cargo test -p
  bridge-worktree` (all harnesses), plus `git diff --check` and
  `cargo fmt --all -- --check`; no `rustfmt::skip`.
- Refresh the handoff: exact frozen input `f17e2bd3`, red evidence, the
  caller/wrapper enumeration, honest churn accounting (additions plus
  deletions, post-format), and the statement that production V3 remains
  unarmed and 3c2's aggregate review is still ahead.
- Stop and report a split before exceeding **350 changed production
  lines or 700 total changed lines** (churn convention) relative to
  `f17e2bd3`. If another consumer besides the executor turns out to
  collapse a protective disposition, split it out — one consumer per
  task — rather than widening this one.

## Files

- `crates/bridge-workflow/src/executor.rs`
- affected workflow/worktree test doubles
- the production-route assertion (test code only)
- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md`
  (this checkout; the binding Task G contract is restated in full in
  this brief's Description and criteria — the salvage plan file is not
  in this checkout's lineage)
- repository `AGENTS.md`

## Commit Message

feat(r2f1b): gate workflow retry on exact proven-complete cleanup

## Round Contract

This dispatch performs one implementation attempt and one independent
Sol/xhigh review. Do not self-repair a review rejection. The operator
will first classify it: only a closed, enumerable rejection may receive
one targeted repair on this same artifact followed by one closure
review. An open-class or repeating family parks Task G. Never restart
from a fresh artifact and never silently extend the cap.
