---
task-type: implement
---

# R2f1b slice 4J — production arming

## Authority and exact base

The owner authorized a narrowly bounded 4J implementation/review lane after PR #99 merge reconciliation:

- exact base: merged `origin/main` `065935e0b3dccfa4338c6fdbedbfda03658cca70`;
- size: at most **80 added nonblank formatted Rust lines**, with deletions not credited;
- convergence: **one hard-read-only review round**;
- effects: no live smoke, provider session, compatibility execution, registry/image mutation, deployment,
  release, or running-operator change; and
- publication: push, PR creation, and merge remain outside this lane.

The plan of record says the body of `scheduler_activation_readiness_v1()` is the complete 4J production
flip. Do not add a configuration key, environment switch, build feature, second readiness decision, or any
other production mechanism.

## Required behavior

1. `scheduler_activation_readiness_v1()` returns `Armed`.
2. Shipped production policy resolves to `AutomaticR2f1b`; explicit `Disarmed` and `ManualTest` inputs remain
   `ManualOnlyR2f1a` negative controls.
3. Production fixed grace becomes active only for valid bounds; zero and greater-than-deadline grace remain
   refused.
4. Automatic activation with a legacy agent watchdog still refuses during admission before checkout planning
   or any provider/process effect.
5. Existing supplied-contract validation, frozen authority, scheduler arbitration, terminalization, cleanup,
   and V1/V2 wire compatibility remain unchanged.

## RED, mutation, and verification

- Before the production edit, change one shipped-readiness regression to require `Armed` and
  `AutomaticR2f1b`; capture its behavioral failure on the exact base.
- After the one-line production flip, make the complete shipped-readiness test population truthful while
  retaining explicit `Disarmed` negatives.
- Run focused production-selection, admission/watchdog, fixed-grace, scheduler, and 4I terminalization tests.
- Freeze a post-implementation mutation that returns production readiness to `Disarmed`; it must survive
  warnings-denied Clippy, and a complete workspace test run must report the actual reddened population. Restore
  the candidate and rerun the full suite.
- Run `git diff --check`, format check, locked workspace check, warnings-denied locked all-target/all-feature
  Clippy, locked all-target/all-feature build, `cargo test --workspace --all-targets --no-fail-fast`, workspace
  doctests, release-bin build, and candidate-built repository hygiene. Report exact totals and exclusions.

## Review and stop conditions

The sole review round is a fresh `gpt-6-astra` hard-read-only review of the frozen exact-base candidate. Findings
must be tagged `WRONG` or `SMELL`, with concrete failure scenarios for every `WRONG`. Stop before review on cap
breach, dirty/unbound custody, an attributed gate failure, or scope expansion. At the one-review cap, classify
the result without silently dispatching another review.

## Expected Rust paths

- `crates/bridge-core/src/execution_policy.rs` — the one-line production readiness flip.
- Existing readiness/admission integration tests under `crates/bridge-core/tests/` and
  `crates/bridge-workflow/tests/` — expectation updates and negative controls only.

## Acceptance Criteria

- A genuine exact-base RED and restored candidate GREEN are retained.
- The full candidate gate set is green with exact totals.
- The production mutation produces an enumerated full-suite RED population and restoration is verified.
- Added nonblank formatted Rust lines are at most 80.
- The one review round returns its verdict on the exact frozen candidate.
- No prohibited external or operator effect occurs.
