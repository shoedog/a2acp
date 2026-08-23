# T3b slice 4 handoff — unused settlement

## Scope

- Base `origin/main`: `c343e563`.
- Changed implementation/test files: `bin/a2a-bridge/src/compatibility_resolution.rs`, `bin/a2a-bridge/src/compatibility_schedule_state.rs`, `bin/a2a-bridge/src/storage_report.rs`, `crates/bridge-worktree/src/custody.rs`, `crates/bridge-worktree/src/custody_writer.rs`, and `crates/bridge-worktree/src/settle.rs`.
- Review artifacts: this handoff and `2026-08-22-r2f1b-t3b-slice4-mutation-control.patch`.
- Rust added-line count: 753 nonblank lines, below the 790-line cap (measured with added nonblank lines in the cumulative diff against `c343e563`).
- Cargo manifests and `Cargo.lock` are unchanged.

## Settlement boundary

`WorktreeCustodianV1::replace_unused_settled` opens the held `SettlementWindowV1`, re-proves the report-selected candidate under that same window, publishes only the frozen `ProtectionPrepared -> UnusedSettled` transition, and clears the claim. It uses the shared `publish_custody_record_in(pin, name, record, mode)` publication helper. An ambiguous replace is reported as `unused-settlement-transition-uncertain`, stops before retirement, and is never collapsed into a no-effect refusal. The checkout is never changed.

The durable marker is retired only with `retire_captured_regular_child_v2` after its descriptor-bound content snapshot. A post-transition interruption leaves an operator-visible `stranded-unused-settled` record; a captured retirement marker is separately reported by storage as `R2f1b retired custody marker residue (captured; recovery-owned)`. The new terminal state has no outgoing transition and no source field or claim.

The test-only post-transition interruption is thread-local: its arm and settlement run on the arming test's own thread, so a parallel test can neither consume its interruption nor trigger its already-armed assertion.

Driving integration remains intentionally deferred to slice 5, the owner of making `sweep_orphans` drive settlement (including `sweep_orphans_async` and the five call sites). This slice supplies the proof-bound production boundary only; it does not wire a boot caller or `sweep_orphans`.

The verification companion gives the descriptor-stress archive fixture 120 seconds of scheduling headroom and releases its real test descriptor before injecting an unlock-report failure, preventing unrelated parallel test scheduling or an expected panic from producing a false tree-drift or retained-lock failure.

## Out-of-scope changes

- `bin/a2a-bridge/src/compatibility_schedule_state.rs`: the `#[cfg(test)]` release-failure injector now releases the real descriptor before reporting its synthetic failure, so its expected debug panic cannot retain a process-local lock. The arm and synthetic report are both test-only.
- `bin/a2a-bridge/src/compatibility_resolution.rs`: the `#[cfg(unix)] #[test]` wide archive stress fixture uses a 120-second local deadline so concurrent CPU-bound suite work does not exhaust fixture scheduling headroom. The changed deadline is local to that test fixture.

## Bounded-effect audit

The colocated audit starts above the settlement outcome/helper block and source-pins the full settlement and publication boundaries with named missing-anchor diagnostics. It rejects direct process starts, direct filesystem deletion or rename, and Git mutation argv construction. The production probe is `HostGitWorktree`; its exact reachable operation inventory is `git rev-parse` and `list_porcelain_argv` (`git worktree list --porcelain -z`), while the known mutating `remove_argv`, `prune_argv`, and `add_argv` builders are forbidden.

Focused validation passed:

- `cargo fmt --all -- --check`
- `CARGO_HOME=/cargo CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 cargo clippy -p bridge-worktree --all-targets --locked -- -D warnings`
- `CARGO_HOME=/cargo CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --lib --locked --no-fail-fast` (356 library tests passed)
- `CARGO_HOME=/cargo CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 cargo test -p a2a-bridge storage_report::tests::a_captured_custody_marker_is_reported_as_distinct_evidence -- --exact`
- `CARGO_HOME=/cargo CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 cargo test -p a2a-bridge --bin a2a-bridge --locked --no-fail-fast` (1,095 tests passed)
- `CARGO_HOME=/cargo CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 cargo clippy -p a2a-bridge --bin a2a-bridge --locked -- -D warnings`
- The required workspace and package-level operator gates remain pending below.

## Mutation control

- Control: `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice4-mutation-control.patch`
- SHA-256: `cb7667c947558e2d6fb041c565a9aa419ac0be8392db107e0e3226d817aeac3f`
- The repair does not change a line the control depends on; the control is carried unchanged.
- Logical mutation: pass a wrong child name to the descriptor-safe marker-retirement primitive.
- Applying the control produces the single expected red test: `settle::tests::unused_candidate_settles_only_after_exact_absence`; the control was then reversed and the clean focused suite passed.

- [x] `cargo fmt --all -- --check` — **see operator evidence below**
- [x] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **see operator evidence below**
- [x] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **see operator evidence below**
- [x] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — **see operator evidence below**
- [x] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **see operator evidence below**
- [x] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **see operator evidence below**


---

# Operator evidence

Recorded at candidate `6666c6e2` (repair) over `e9c0d4c5` (slice 4), parent `origin/main` = `c343e563`.
Run from a checkout under the owner-approved trusted cwd root. Exit status and FAILED counts are
authoritative; per-binary `test result:` lines are not summed.

## Gates

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | **exit 0** |
| `clippy --workspace --all-targets --locked -- -D warnings` | **exit 0** |
| `cargo test -p bridge-worktree --locked --no-fail-fast` | **exit 0, 356 passed / 0 failed** |
| `validate --repo-hygiene` (both points) | **exit 0** |
| `cargo test --workspace --locked --no-fail-fast` | **exit 101 — pre-existing, see below** |

## The workspace gate is red at BASE with an identical population

11 failures in `bin/a2a-bridge` (`fallback_plan_cli`, `smoke_cli`). The operator ran the same two binaries at
`c343e563` in the **same environment**: the **identical 11 test names** fail. The only difference between the
two captures is a timing line. These are host system-integration tests the verify configuration documents as
unrunnable hermetically. Reported, not re-baselined and not silently fixed.

## Same-environment base control

`bridge-worktree` at `c343e563`: **348 passed / 0 failed**. Candidate: **356 passed / 0 failed**. Delta **+8**.

## The frozen transition table is byte-identical

The operator hashed the constant's text on both trees rather than trusting the diff:

| Tree | `LEGAL_CUSTODY_TRANSITIONS_V1` sha (first 16) | rows |
|---|---|---|
| `c343e563` | `46cf2e4caa41ff6e` | 10 |
| candidate | `46cf2e4caa41ff6e` | 10 |

No edge added, none removed, none reordered. `the_frozen_transition_table_is_unchanged` asserts
`len() == 10` and passes.

## Frozen mutation control — RUN

- SHA-256 recomputed by the operator: `cb7667c947558e2d6fb041c565a9aa419ac0be8392db107e0e3226d817aeac3f` — **matches the recorded value**.
- Applied to the actual head `6666c6e2`: **applies cleanly**.
- Result: **355 passed / 1 failed**. The single reddened test is
  `settle::tests::unused_candidate_settles_only_after_exact_absence` — the central settlement precondition,
  and no other test moved.
- Tree restored after the run.

## The four guarantees of a destructive slice — each has a passing test

| Guarantee | Test | Result |
|---|---|---|
| Settles only after exact absence | `unused_candidate_settles_only_after_exact_absence` | ok |
| Crash between transition and retirement strands a **recognizable** marker | `interruption_between_unused_transition_and_marker_retirement_strands_a_recognizable_marker` | ok |
| Settlement's probe uses **query-only** git verbs | `settlement_probe_git_verbs_are_query_only` | ok |
| The frozen table is unchanged | `the_frozen_transition_table_is_unchanged` | ok |

Operator-read assertions inside the settlement test, in **both** directions:

- on settle — the marker is gone (*"only the custody marker may be retired"*), the source checkout directory
  still exists (*"settlement must not touch the source checkout directory"*), and the target is absent;
- on refuse — the outcome is `Refused`, the marker's bytes are **byte-identical to before**, and
  *"a refused settlement must not delete a checkout"*.

A refused settlement therefore changes nothing at all, which is the property that matters most in the first
slice that deletes.

## The probe requirement, and why it changed

An earlier revision of this task required the settlement path to be **spawn-free**. The first dispatch
correctly **stopped and made no changes**, reporting that the sole production `ExactAbsenceProbeV1`
(`HostGitWorktree`) spawns `git`.

The requirement was wrong. The operator enumerated every `git` invocation reachable from that probe:
`rev-parse --path-format=absolute --git-common-dir`, and `worktree list --porcelain -z`. **Both are queries;
neither mutates.** There is no `remove`, `prune`, or `add` on that path. *Read-only* is the property that
matters, and demanding spawn-freedom would have forced a settlement-only observation path — the exact
acting-versus-reporting drift this boundary exists to prevent.

The ban became a **checkable invariant** instead: `settlement_probe_git_verbs_are_query_only` fails if a
mutating verb is ever added to that path, so a future change cannot be silently inherited by a destructive
caller.

## Counted lines

**753** added nonblank physical Rust lines against `c343e563`, post-fmt, against the **790** cap.

A prior dispatch stopped because it measured 747 where the task stated 748. The operator re-measured by two
independent methods with zero whitespace-only added lines and confirms the task's figure; the difference was
immaterial and the falsification licence has since been scoped to load-bearing anchors, not advisory counts.

## Repair disposition

| Finding | Disposition |
|---|---|
| Process-global `AtomicBool` crash-injection | **Real, fixed.** `#[cfg(test)]` only, so production was never affected, but the harness runs tests in parallel threads on one process: a concurrent test could consume another's arming, and two arming at once trips the already-armed assertion. That made the crash-ordering test — the guard on the stranded-marker property — nondeterministic. Now thread-local, matching this repository's own precedent in `compatibility_schedule_state.rs`. |
| "Unmet intent: driving integration deferred" | **Not an artifact defect.** The handoff is correct: the lane plan assigns *"`sweep_orphans` stops discarding the report and drives settlement"* to **slice 5**. The ambiguity was the operator's — "introduces the first production caller" meant the first caller of `reprove_under_window`, which `replace_unused_settled` verifiably is. No boot wiring was added, deliberately. |

The repair converged with verify PASS on all four commands and review APPROVE from both reviewers.

## Out-of-scope changes — disclosed, not reverted

Two `bin/` files outside the stated boundary were inspected by the operator and kept:

1. `compatibility_schedule_state.rs` — releases the real descriptor **before** reporting the synthetic
   failure, so a test panic cannot retain the process-local lock. This is a **root-cause fix for the
   `force_next_release_failure_for` flake**, which this lane has carried for months with an explicitly
   *unproven* mechanism. The edit is inside the test-armed branch; the production path is unaffected.
2. `compatibility_resolution.rs` — extends a wide stress fixture's deadline, which competes with the suite's
   CPU-bound tests. Test-only, and the same load-margin class this repository has hardened before.

Reverting a real flake fix to satisfy a scope boundary would be the worse trade; disclosing it is the point.

## Residual carried forward, not solved

The **stranded `UnusedSettled` marker** remains a real, bounded leak. A crash between the transition and the
retirement leaves a durable record that no later sweep can authorise removing: the operator verified that the
record schema has **no `source` field** and that `UnusedSettled` maps to `ClaimPresenceV1::Forbidden`, so
re-proving registration absence is impossible and the tri-state answer is permanently `cannot-prove` →
refuse. This is correct and fail-closed. It is discoverable through the `CustodyRetirementResidue` operator
category. **No slice may relax the rule to clear it** — no `source` field, no claim on `UnusedSettled`, no
transition out of it.
