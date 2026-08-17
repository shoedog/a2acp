---
task-type: code-review
---

# Verify-hardening follow-ups — counted review

## Description

Review `git diff 1d7826dd..9ed91769` in this checkout — two commits, tooling and
test-infrastructure only. **No production behavior changes.**

These were written in response to three recurring failures in this repo's own
development loop, each of which had cost real time before it was addressed.

**Commit 1 — `tools/check-nonunix.sh` + `tools/ring-stub`.** `bridge-core` gates
`liveness` and `namespace_transaction` behind `#[cfg(unix)]` while `fs_custody`
is unconditional, so a new `fs_custody` helper reaching into them type-checks on
unix and fails CI's Windows lane with `E0433` (or a non-unix `dead_code` warning
under `-D warnings`). CI compiles `bridge-core` for Windows via `bridge-store`,
but every LOCAL gate is unix-only, so the class was invisible until CI. It cost a
landing round in 3a, 3b1, 3c1, 3c2 and 3d-T2.

Running `cargo check --target x86_64-pc-windows-msvc` was blocked by `ring`,
whose C build script cannot cross-compile from macOS. `tools/ring-stub` is a
signature-only stub patched in through a `cargo --config` override, so the
workspace manifest is never edited. It declares its own empty `[workspace]` and
is not a member — `cargo metadata` still reports 17 packages with no `ring`.
The override rewrites `Cargo.lock`, so the script restores it from a backup via
an `EXIT` trap.

**Commit 2 — bounded blocking test gates.** The three gates
(`initial_open_publish`, `control_root_pin`, `custody_sync`) parked a
blocking-pool thread on an unbounded condvar until the test released them. If
the test panicked FIRST, the unwind dropped the runtime and
`BlockingPool::shutdown` joined the parked thread forever — a clean red became
an unbounded hang at 0% CPU. That turned one assertion failure into a 3-hour CI
verify. All three now share `await_test_gate_release`, bounded at 30 s, which
names the unreleased gate and proceeds so the real failure surfaces.

Also included: a findings document for the coverage-lane flake family (no code).

**Operator evidence — falsifiable, not premise.** Verified on this tree:

- The non-unix gate exits **101** with the 3d-T2 `cfg(unix)` guard removed
  (reporting the real `E0433`) and **0** with it present; the working tree is
  left clean either way; it runs in ~3 s.
- The bounded gate was proven by reintroducing the original incident (a
  non-recursive `remove_dir` on a populated control root): previously an
  indefinite hang, now **FAILS in 30.03 s** with exit 101, surfacing both the
  real `ENOTEMPTY` panic and the unreleased-gate diagnostic.
- fmt clean; workspace clippy `-D warnings` clean; full suite
  **4,140 passed / 0 failed / 13 ignored across 90 targets** — identical totals
  before and after the bounded gates, confirming they are a no-op on passing
  tests.

If any of this does not match the code, report the mismatch rather than
accommodating it.

## Acceptance Criteria

1. Can `tools/ring-stub` ever be linked into a real build, or influence a
   non-probe `cargo` invocation? Check the `[workspace]` isolation, the
   `--config` override's blast radius, and whether a developer or CI could
   pick it up accidentally. This is the finding that would matter most.
2. Is the `Cargo.lock` restore robust — on failure, on interrupt, on a script
   that exits early? Could it leave a developer's tree modified, or restore a
   stale lock over a legitimate concurrent edit?
3. Is the 30 s gate bound safe? Could it change the behavior of a *passing*
   test — for instance one that legitimately holds a gate longer under load or
   instrumentation — and so convert a green test into a flake?
4. Does proceeding after the bound (rather than panicking) risk a confusing
   downstream failure, or a test passing when it should not?
5. Is the stub's API surface an honest signature-only shim — no behavior that
   could make a cross-check pass when the real `ring` would fail?
6. Anything else in scope. Tag every finding **WRONG** or **SMELL**. WRONG means
   the code provably does the wrong thing — name the input or state and the
   incorrect result. A finding without a concrete failure scenario is a SMELL.
7. End with `VERDICT: APPROVE` or `VERDICT: REJECT` and a one-line SUMMARY.

Note this is tooling and test infrastructure: weigh findings by whether they
could corrupt a real build, mask a real failure, or mislead a developer.

## Files

- `tools/check-nonunix.sh`, `tools/ring-stub/` — the gate and its stub.
- `crates/bridge-worktree/src/backend.rs` — `await_test_gate_release` and the
  three gate call sites.
- `CONTRIBUTING.md` — the documented gate list.

## Spec Refs

- `docs/superpowers/reviews/2026-08-17-coverage-lane-flake-family-investigation.md`
  — the flake findings document included in commit 2.
