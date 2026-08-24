# R2f1b slice 4E handoff: constructive impossibility proof adapters

## Corrected construction boundary

- `bridge_core::mechanical_impossibility` publicly exposes only
  `MechanicalImpossibilityProofV1`, its closed kind vocabulary, and the read-only `kind()` accessor.
  The proof has no `Default`, boolean conversion, public field, deserializer, or unchecked
  constructor.
- There is deliberately no public proof mint in 4E. The result/settlement/route observation
  vocabulary and classifier are `pub(crate)`: a sibling owner adapter in `bridge-core` can consume
  them in 4H without reopening this module, while an external caller still cannot fabricate
  pending-producer, settled-spawn, closed-route, or absent-result values and turn them into a proof.
- The dormant internal classifier retains exactly the three constructive cases:
  1. the retained ACP child identity and signal-0 `ESRCH` observation agree on nonzero PID and
     immutable start time while the sole producer result is pending;
  2. the named managed-container identity and successful removal observation agree on the exact
     immutable container ID, have canonical labels and no failure, and spawn is settled;
  3. nonempty producer and final route sets are all irreversibly closed and the terminal result is
     absent.
- The module-level dead-code allowance is intentional and local because production wiring is
  forbidden in 4E. The crate-private seam gives 4H a place to add an owner-backed adapter without
  widening the unchecked observation vocabulary into the public API.

## Tests and unrepresentability

- Fourteen behavioral tests cover the three positive facts, the six separately named non-proofs,
  PID/start-time and errno ambiguity, removal failure/canonical-label ambiguity, a mismatched
  immutable container ID, independently open producer and final routes, and both present and unknown
  terminal-result states.
- The external slice test re-asserts the production refusal gate while leaving the crate-private
  proof inputs unreachable outside `bridge-core`.
- The shared `trybuild` case attempts to import the classifier and all of its input vocabulary. It
  receives private-item errors for every crate-private adapter boundary in addition to proving that
  the public proof cannot be defaulted, converted from a boolean, or built with struct-literal syntax.
- The updated `.stderr` was generated with `TRYBUILD=overwrite` and then passed a clean run without
  overwrite mode. Behavioral fixtures and expectations were hand-written; only the compile-fail
  stderr was tool-generated.

## Red-first evidence

Before the production module and export existed, the focused behavioral target failed to compile
with unresolved import `bridge_core::mechanical_impossibility`; the named tests could not execute on
the pre-change tree. The compile-fail source likewise referenced the absent module. This is the same
target-level red-first form used by slice 4D.

The repaired candidate passes 14 / 0 adapter behavior tests, 1 / 0 external refusal tests, and 1 / 0
shared compile-fail targets (all three trybuild cases inside that target pass).

## Frozen single-mutation control

- Patch: `docs/superpowers/reviews/2026-08-24-r2f1b-slice4e-mutation-control.patch`
- SHA-256: `222dd10a7df360e4a319605a1e0de3fc90773cae46dc7e0a324ed2710734aeed`
- Production mutation: remove only the immutable process-start-tick equality guard, accepting
  PID-reuse ambiguity as child-exit proof.
- The patch applies cleanly, the exact named negative test reddens, the mutant passes
  `cargo clippy --all-targets --all-features --locked -- -D warnings`, reversal restores a clean
  worktree relative to the staged candidate, and the patch applies cleanly again afterward.

The candidate's local exact configured workspace rerun has an empty failure set. A cached
`--no-fail-fast` mutant run completed the same workspace selection with exactly one failed target and
exactly one failed test: `bridge-core --lib`, caused by
`mechanical_impossibility::tests::pid_reuse_or_undetermined_errno_is_not_child_exit_proof` (708 other
library tests passed and 3 were filtered). The actual mutant-minus-candidate red population is
therefore exactly that one causal test.

## Verification and the bridge-run failure

All commands used the checked-in offline-cache convention: `CARGO_HOME=/cargo`,
`CARGO_NET_OFFLINE=true`, `CARGO_INCREMENTAL=0`, explicit
`RUSTDOC=/usr/local/cargo/bin/rustdoc`, and localhost exclusions in both proxy variables.

- `cargo fmt --all -- --check`: green.
- `cargo clippy --all-targets --all-features --locked -- -D warnings`: green for candidate and
  mutant in the isolated target.
- `cargo build --locked`: green.
- Focused behavior, refusal, and clean trybuild targets: green with the totals above.
- The bridge-managed configured test run supplied with this repair is **not green**: it reached
  `test`, exited 101, and failed
  `backend::tests::settlement_refusal_does_not_mask_the_provider_failure` in `bridge-api` with
  `provider request count was not reached: Elapsed(())`.
- That test and `crates/bridge-api/src/backend.rs` are byte-identical to base `1685aa6c`. Its helper
  has a hard two-second request-observation timeout, and five focused candidate reruns passed in
  approximately 0.26--0.32 seconds. A fresh-target local rerun of the exact configured workspace
  command also passed, including doc-tests. This supports, but does not prove, the review's
  load-sensitive MockServer race hypothesis; no same-environment base-suite control was run. The
  bridge-managed configured gate is therefore still reported as failed rather than replaced by the
  passing diagnostic reruns.
- `cargo run -p a2a-bridge --locked -- validate --repo-hygiene`: green, with 40 tracked artifacts
  and 8 validated example configs.
- `git diff --check`: green.

## Deliberate exclusions and stop boundaries

- `crates/bridge-workflow/src/executor.rs` is byte-identical to the base; its bare
  `inflight.next().await` remains.
- `scheduler_arbitration.rs` is byte-identical to the base. No production caller consumes the proof
  or fills `mechanical_impossibility_proved`.
- Production readiness remains `Disarmed`, and production policy selection remains
  `ManualOnlyR2f1a`.
- No timer, `select!`, sleep, spawn, token, cancellation path, scheduling change, or provider effect
  was added or altered.
- `MAX_WORKTREE_CONFIGURES_IN_FLIGHT`, all manifests, and `Cargo.lock` are untouched.
- Fixed grace, progress epochs/warnings, issue #22 closure, owner-backed workflow observation
  derivation, and production arming remain later slices.

## Counted size

The complete change adds **382 nonblank physical Rust lines after `cargo fmt`**, including the module
export, production module, test module, external refusal test, and compile-fail source. This is within
the 400-line stop boundary with 18 lines of headroom. The handoff, generated stderr, and frozen
mutation patch are non-Rust and excluded.
