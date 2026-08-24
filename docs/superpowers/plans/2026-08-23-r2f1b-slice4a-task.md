---
task-type: implement
---

# R2f1b slice 4A — shared attempt clock and D11 internal timers

## Description

The first sub-slice of R2f1b slice 4. It settles **clock identity** and introduces D11's internal action
timers as constants, so that every later sub-slice arms timers against one agreed clock rather than
inventing its own.

**This sub-slice arms no timer and changes no scheduling behaviour.** `AutomaticR2f1b` remains
unconstructible by any production caller; the event loop is untouched.

Base: `origin/main` = `462e676b`.

Plan of record: `docs/superpowers/plans/2026-08-23-r2f1b-slice4-decomposition.md`.

### Falsification licence — load-bearing anchors only

**Stop and report before editing** if any of these fails:

- `bridge_core::attempt_activity::MonotonicClock` (trait) or `SystemMonotonicClock` is absent, or the trait's
  shape differs from what you find.
- `bridge_core::execution_policy::liveness_profile_v1` is absent or is not a `pub const fn`.
- Any executed behaviour claim in this spec does not hold on the base tree.

**Do NOT stop for immaterial measurement differences** — line numbers, exact diff counts, formatting-only
deltas. Only the cap binds those. If your count differs from the operator's, record both and continue. The
frozen scope document's line numbers are known stale (`liveness_profile_v1` moved 107 → 128, the executor's
bare await 4619 → 5233); cite by symbol, never by line.

### Verified anchors — operator-measured on this base

- `MonotonicClock` and `SystemMonotonicClock` exist in `crates/bridge-core/src/attempt_activity.rs`.
- `liveness_profile_v1` is a `pub const fn` in `crates/bridge-core/src/execution_policy.rs`.
- `MonotonicClock` already has **twelve** consuming files across `bridge-core`, `bridge-acp`, `bridge-api`,
  `bridge-controller` and `bridge-worktree`.
- **Three separate test clock doubles already exist**: `FakeClock` in `attempt_activity.rs`, a second
  `FakeClock` in `preparation_flight.rs`, and `ManualClock` in `bridge-coordinator/src/clock.rs`.
- **D11's internal timers do not exist yet.** There is no control-action or cancellation-grace constant in
  `execution_policy.rs`. This sub-slice adds them.

## What this sub-slice does

**1. Add D11's internal action timers as R2f1b constants.**

- A control-action timer of **30 s**.
- A cancellation-grace timer of **5 s**.

They live beside the frozen profile but are **not** part of `liveness_profile_v1` — that profile carries only
*observable* bounds and must not change. Name them so their relationship to the observable bounds is
self-evident to a reader.

**2. Establish one attempt clock identity.**

One `Arc<dyn MonotonicClock>` per attempt is the single source of monotonic time for recorder, telemetry,
scheduler, cleanup and reporting. Thread it where a consumer currently reaches for time independently.

**Do not invent a parallel clock type.** Reuse `MonotonicClock`.

Given the three existing test doubles, decide and state whether they should converge on one shared test
clock. Converging is permitted and probably right; it is **not** required, and it must not expand this
sub-slice past its cap. If you converge them, that is a deliberate choice to defend in the handoff; if you do
not, say why.

**3. Pure schedule math against a fake clock.**

Deterministic helpers that answer, from the frozen observable bounds plus the two new internal timers, how
long remains and what is due — with no timer arming, no cancellation, no executor change.

## Invariants — must not change

- The eight observable bounds in `liveness_profile_v1`: queue wait 1,800,000 ms; control observable 31,000;
  no-progress snapshot 1,800,000; work cutoff 7,200,000; cancel observable 6,000; cleanup tail 60,000;
  reporting tail 10,000; terminal envelope 7,270,000.
- **The internal timers must be strictly shorter than their corresponding observable bounds.** D11 forbids
  lengthening them to match. A test must assert this relationship rather than restating the numbers.
- Wall timestamps identify records only. Monotonic offsets are audit data, never restartable wall deadlines.
- No production caller may construct an automatic attempt after this sub-slice. No timer arms. The executor
  event loop is untouched.
- `MAX_WORKTREE_CONFIGURES_IN_FLIGHT` is untouched, and no new resource-admission cap is introduced.

## Required tests

At least one must fail on the pre-change tree — verify that rather than assuming it.

1. The internal control timer is strictly less than the observable control bound, and the internal
   cancellation grace is strictly less than the observable cancel bound. Assert the *relationship*, so the
   test still protects the invariant if a bound is ever re-tuned.
2. Schedule math is deterministic under a fake clock: given a start instant and a bound, remaining-time and
   due-ness answers are exact at the boundary, not merely near it.
3. The observable profile is unchanged — all eight values, asserted explicitly.
4. A guard proving no timer arms and no automatic attempt becomes constructible in this sub-slice.

## Size

Projection **300** counted lines against a cap of **450**. Counted lines are added nonblank physical Rust
lines after `cargo fmt`; a grep for added nonblank lines already excludes blanks — do not subtract them
again. If the projection will exceed the cap, stop before editing and report a revised estimate rather than
trimming required tests.

## Frozen single-mutation control

Freeze it at `docs/superpowers/reviews/2026-08-23-r2f1b-slice4a-control.patch`.

One **production** mutation — not a test, not a fixture — chosen so that removing it defeats the D11
relationship, for example lengthening an internal timer to equal its observable bound.

It must redden at least test 1. **A control that reddens more than one test is acceptable and usually
stronger**, because it means the invariant is enforced in more than one place. Report the **actual** reddened
population from a **full-crate** run; a population derived from running only the named tests under a filter
is not a population.

**The control must also survive `clippy -D warnings`.** A control that fails on `dead_code` before reaching
its red tests proves nothing. Verify both gates and record both.

Record its SHA-256 in the handoff, verify it applies cleanly, and restore the tree afterwards.

## Handoff

Create `docs/superpowers/reviews/2026-08-23-r2f1b-slice4a-handoff.md` with the base, the changed-file list,
the counted total against the 450 cap, the control's path and SHA-256, the named tests it reddens, and your
decision on the three existing test clock doubles.

**Do not record this candidate's own head commit or tree sha.** The review loop amends, so any head sha
written inside the handoff is rewritten by the next amend. That binding is the operator's, made in the
evidence commit after the candidate is final.

End the handoff with exactly these six unticked lines:

- [ ] `cargo fmt --all -- --check` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-core --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **PENDING OPERATOR**

Operator note, not a defect: `cargo test --workspace` is red at base on the operator's host with 11
pre-existing `bin/a2a-bridge` failures, and that population inflates under parallel load. The operator
compares populations against a same-environment base on an idle machine.

## Acceptance criteria

- [ ] The two D11 internal timers exist as constants, are **not** part of `liveness_profile_v1`, and are
      strictly shorter than their corresponding observable bounds.
- [ ] A test asserts that relationship rather than restating the numbers.
- [ ] One `Arc<dyn MonotonicClock>` is the attempt's single monotonic source for the consumers this
      sub-slice touches; no parallel clock type is introduced.
- [ ] The handoff states the decision taken on the three existing test clock doubles, with a reason.
- [ ] All eight observable bounds are unchanged, asserted by a test.
- [ ] No timer arms; the executor event loop is untouched; no production caller can construct an automatic
      attempt.
- [ ] `MAX_WORKTREE_CONFIGURES_IN_FLIGHT` untouched; no new resource-admission cap.
- [ ] Counted lines stay at or under 450.
- [ ] The frozen control exists, is SHA-256-recorded, mutates production only, reddens at least test 1, and
      **passes `clippy -D warnings`**.
- [ ] The handoff records no head commit or tree sha for this candidate.
- [ ] `Cargo.lock` and every manifest are untouched.
