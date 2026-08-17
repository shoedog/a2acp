---
task-type: code-review
---

# Verify-hardening — third look, on the second repair delta

## Description

Review `git diff a3e10e3d..589bbf77` in this checkout. This closes the four
BLOCKER WRONGs from your bounded re-look (verbatim:
`docs/superpowers/reviews/2026-08-17-verify-hardening-sol-relook.md`). All four
were accepted; none argued down. Tooling, test infrastructure and documentation
only.

1. **Timeout attribution.** The sticky record moved from a process-global static
   onto the `PreparationFlightTestHooks` instance, so a timeout can only fail
   its own test.
2. **Ambient `RUSTFLAGS`.** Now set exactly (`RUSTFLAGS="-D warnings"`) rather
   than appended to, with `CARGO_ENCODED_RUSTFLAGS` unset.
3. **Signal traps.** `cleanup` is EXIT-only; `INT`/`TERM`/`HUP` exit 130/143/129.
4. **Doc attribution.** Recurrence on a plain step now establishes only that
   instrumentation is not a necessary condition.

**Operator evidence — falsifiable, not premise.** Verified on the host, each
with a probe re-run until admissible:

- Owner/sibling pair: the gate-timing-out test FAILS with the gate message while
  a sibling that never touches a gate stays green (1 passed, 1 failed). One
  probe alone would not have shown the absence of cross-test leakage.
- Hostile `RUSTFLAGS="--cap-lints allow"` now exits **101** on the real `E0433`.
- `TERM` delivered mid-run exits **143** with no success banner. (Two earlier
  attempts at this probe were inadmissible — one `wait`ed on a non-child, one
  signalled after the script had already exited — and are reported as such.)
- Green case still exits 0; repository `Cargo.lock` byte-identical.
- fmt clean; workspace clippy `-D warnings` clean; suite 4,140/0/13 across 90.

## Acceptance Criteria

1. Rule each of the four FIXED or NOT-FIXED at line-numbered source.
2. **Ambient-state audit, explicitly.** Two consecutive rounds have found
   "ambient state defeats this gate" defects (`TARGET`/`PACKAGES`, then
   `RUSTFLAGS`/`CARGO_ENCODED_RUSTFLAGS`). Enumerate EVERY remaining environment
   variable, inherited setting, or config file that could change what this
   script checks or whether it reports success — `CARGO_*`, `RUST*`, cargo
   config discovery in the copied tree, `PATH`/toolchain selection,
   `rust-toolchain.toml`. If the class is not closed, say so plainly; that
   answer decides whether this script lands or is withdrawn.
3. Any NEW defect the repair introduces.
4. Tag findings WRONG or SMELL; WRONG needs a concrete failure scenario.
5. End with `VERDICT: APPROVE` or `VERDICT: REJECT` and a one-line SUMMARY.

## Files

- `tools/check-nonunix.sh`
- `crates/bridge-worktree/src/backend.rs` — `await_test_gate_release`,
  `timed_out_gate`, `Drop for PreparationFlightTestHooks`.
- `docs/superpowers/reviews/2026-08-17-coverage-lane-flake-family-investigation.md`

## Spec Refs

- `docs/superpowers/reviews/2026-08-17-verify-hardening-sol-relook.md`
- `docs/superpowers/reviews/2026-08-17-verify-hardening-sol-review.md`
