---
task-type: code-review
---

# Verify-hardening follow-ups — bounded re-look on the repair delta

## Description

Review `git diff 9ed91769..a3e10e3d` in this checkout. This is the repair for
the four BLOCKER WRONGs your counted review raised on `1d7826dd..9ed91769`
(verbatim: `docs/superpowers/reviews/2026-08-17-verify-hardening-sol-review.md`).
All four were accepted as valid; none were argued down. Tooling, test
infrastructure and documentation only — no production behavior changes.

What the repair claims, and what to check:

1. **Bounded gate could green a test that never ran its race.** Now one
   ABSOLUTE deadline (`Instant::now() + BOUND`, remaining time passed to each
   `wait_timeout`, so spurious wakes cannot extend it), plus a sticky
   `TEST_GATE_TIMED_OUT` record asserted in `Drop for PreparationFlightTestHooks`
   — guarded by `std::thread::panicking()` so a real failure still wins.
   Check: can a timeout still be silent? Can the Drop assertion fire on the
   WRONG test, or leak across tests, given the static is process-global and
   tests run in parallel? That last one is the question I would most want a
   second pair of eyes on.
2. **Ambient `TARGET`/`PACKAGES` redirection.** Both are now `readonly` literals.
   Check nothing else in the script still consumes ambient state in a way that
   changes what gets checked (`RUSTFLAGS` is deliberately still appended to —
   say if you think that is wrong).
3. **`Cargo.lock` custody.** The probe now copies the workspace to a `mktemp -d`
   and runs there; the repository lockfile is never written. Check the copy is
   faithful enough to be a real gate (it excludes `target`, `.git`, `.claude`),
   that `set -euo pipefail` plus the `EXIT INT TERM HUP` trap cannot leave the
   temp dir behind on a normal failure, and whether excluding `.git` breaks
   anything a `cargo check` needs.
4. **Flake document overclaim.** Finding 3 is relabelled UNRESOLVED, the
   injected `EBADF` is named at
   `bin/a2a-bridge/src/compatibility_schedule_state.rs:60`, and the
   correlation-implies-causation sentence is gone. Check no other claim in that
   document still rests on non-discriminating evidence.

**Operator evidence — falsifiable, not premise.** Each fix was verified on the
host before commit:

- Unreleased gate now FAILS its owning test in 2.07 s with "the gated schedule
  was NOT exercised, so this test's result is not evidence" (probe run with the
  bound temporarily lowered, then reverted).
- Hostile `TARGET=x86_64-apple-darwin PACKAGES="-p bridge-store"` still checked
  `-p bridge-core --target x86_64-pc-windows-msvc`.
- Repository `Cargo.lock` byte-identical after a gate run; gate still exits 0
  guarded and 101 with the real `E0433` unguarded.
- fmt clean; workspace clippy `-D warnings` clean; suite 4,140/0/13 across 90 —
  unchanged totals, so the sticky assertion is a no-op on passing tests.

If any of this does not match the code, report the mismatch rather than
accommodating it.

## Acceptance Criteria

1. Rule each of the four findings FIXED or NOT-FIXED at line-numbered source.
2. For any NOT-FIXED, give a constructible input or state and the incorrect
   result.
3. Call out any NEW defect the repair introduces — the process-global sticky
   static under parallel tests is the most likely place for one.
4. Tag every finding WRONG or SMELL; WRONG needs a concrete failure scenario.
   WRONG first.
5. End with `VERDICT: APPROVE` or `VERDICT: REJECT` and a one-line SUMMARY.

## Files

- `tools/check-nonunix.sh` — hard-coded target/packages, isolated workspace.
- `crates/bridge-worktree/src/backend.rs` — `await_test_gate_release`,
  `TEST_GATE_TIMED_OUT`, `Drop for PreparationFlightTestHooks`.
- `docs/superpowers/reviews/2026-08-17-coverage-lane-flake-family-investigation.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-17-verify-hardening-sol-review.md` — the
  counted review these four findings come from.
