---
task-type: implement
---

# T3b slice 5A repair — one host-specific test defect

## Description

The slice 5A candidate passed verify (fmt/clippy/build/test all exit 0) and both reviewers returned
APPROVE with no BLOCKER and no MAJOR. It fails exactly one test on the operator's macOS host. Repair that
one test. **Do not redesign, do not restart, do not touch production code.**

Base: `refs/t3b/slice5a-candidate` = `275ca88e`, whose parent is `origin/main` = `3d654a0e`.

Confirmed correct and not to be disturbed: `EXACT_ABSENCE_POLICY_READY_V1` is still `false`; five
`sweep_orphans_async` call sites with zero sync calls remaining; `spawn_blocking` offload with no
`async_trait` and no new trait; `LEGAL_CUSTODY_TRANSITIONS_V1` byte-identical at ten rows; 547 counted
lines under the 790 cap.

### Falsification license — scoped to load-bearing anchors

Stop and report if a load-bearing anchor is false: a named symbol absent, a signature different, a
described behaviour that does not hold. Do NOT stop for immaterial measurement differences; counts here
are advisory and only the cap binds.

## The defect

`sweep::tests::policy_selected_settlement_retires_v3_and_legacy_unused_markers` panics at its
`.expect("the legacy marker must be selected in the exact-absence report")`.

**It is a host-specific test defect, not a production defect.** The operator diagnosed it to ground:

- The test derives its expected marker paths from `unique_temp_dir(...)`, which is built on
  `std::env::temp_dir()`.
- On macOS `/var` is a symlink to `private/var`, so `env::temp_dir()` returns
  `/var/folders/…` while canonicalisation yields `/private/var/folders/…`. **They differ.**
- The sweep reports `record_path()` values derived from the **canonicalised** root, so the expected and
  reported paths never match on macOS.
- In the verify container `/tmp` is not a symlink, so the two coincide and the test passes. That is why
  verify was green and the host gate was red.

Evidence that the failure is *only* path shape, not selection logic: the test's own
`assert_eq!(report.entries().len(), 2)` **passes** immediately before the failing `find`. Both entries
are present and correctly assessed; only the path comparison fails.

## The fix

Make the test compare canonicalised paths on both sides, so it is correct on any host regardless of
whether the temp directory is reached through a symlink. Canonicalise the root (or the expected marker
paths) before building the expected values, rather than weakening the assertion or comparing only file
names — the test must still prove the *right* marker was selected.

Apply the same treatment to any sibling test in this file that compares a `record_path()` against a path
derived from `unique_temp_dir` and would fail for the same reason. Enumerate what you changed; do not
change tests that are already correct.

**Do not change production code.** The production canonicalisation is correct and is what makes the
comparison mismatch; the test is what is wrong.

## Size

Expect well under **30** added nonblank Rust lines on top of `275ca88e`. Cumulative cap is 790 and the
candidate is at 547.

## Frozen control

If the repair changes any line the control at
`docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice5a-wiring-control.patch` depends on, re-cut it and
record the new SHA-256; otherwise carry it unchanged and say so explicitly.

## Handoff

Update `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice5a-handoff.md` in place with the repaired file
list, the new counted total against 790, and a short note that the failure was a macOS symlinked-temp
path-shape defect in the test rather than a production defect.

**Do not record this candidate's own head commit or tree sha.** The review loop amends, so any head sha
written inside the handoff is rewritten by the next amend. That binding is the operator's.

Keep the six operator gate lines unticked and exactly as they are:

- [ ] `cargo fmt --all -- --check` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **PENDING OPERATOR**

Operator note, not a defect: `cargo test --workspace` is red at base on the operator's host with 9
pre-existing `bin/a2a-bridge` failures (`fallback_plan_cli`, `smoke_cli`). The operator compares
populations against the base rather than attributing them.

## Acceptance criteria

- [ ] `policy_selected_settlement_retires_v3_and_legacy_unused_markers` passes on a host where the temp
      directory is reached through a symlink.
- [ ] The test still proves the correct marker was selected — the assertion is not weakened to a
      filename-only or existence-only check.
- [ ] No production code changed.
- [ ] `EXACT_ABSENCE_POLICY_READY_V1` is still `false`.
- [ ] `LEGAL_CUSTODY_TRANSITIONS_V1` is still ten rows and unchanged.
- [ ] Cumulative counted lines stay at or under 790.
- [ ] `Cargo.lock` and every manifest remain untouched.
