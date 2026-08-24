# R2f1b slice 4G handoff — progress epochs and no-progress warnings

Date: 2026-08-24

## What changed

- Replaced the implicit `ActivityReason != Heartbeat` predicate with the public, exhaustive,
  wildcard-free `activity_reason_supports_meaningful_progress_v1` classifier. The high-water
  accumulator still requires a strict advance before it records meaningful progress.
- Classified `UsageHighWater` and `OwnedChildOutput` as activity only, and updated their existing
  producer-facing telemetry tests to prove the counters remain observable without becoming progress.
- Added a synchronous, caller-clocked 30-minute ordinal calculation and attempt-local progress
  epoch. Positive ordinals emit once per epoch; ordinal zero emits nothing; ordinary activity
  updates only the activity clock; meaningful progress resets the epoch.
- Hardened epoch reset against a stale, directly constructed public `AttemptActivity`: even a
  meaningful-progress observation must carry elapsed time strictly later than the recorded progress.
- Added a warning-only poll result whose cancellation, mechanical-impossibility, and terminal-effect
  queries are unconditionally false, including at `u64::MAX` elapsed time.
- Added the eight required integration tests plus a defense-in-depth regression for stale elapsed
  input. Production callers remain bound to the documented monotonic elapsed-time contract.

## Total progress classification

Every current variant is named in one exhaustive `match`; there is no wildcard arm. For every
progress-capable variant, the stated transition is accepted as progress only when its proposed
high-water value is strictly greater than the prior value, so replaying a status cannot suppress a
warning.

- `PhaseTransition` — **progress**: a later phase high-water proves the attempt reached a new fixed
  lifecycle milestone; repeating the current phase is only activity.
- `MessageDelta` — **progress**: a larger cumulative message high-water proves new producer-output
  bytes arrived; an empty or replayed delta cannot advance it.
- `ThoughtDelta` — **progress**: a larger cumulative thought high-water proves new provider reasoning
  bytes arrived, rather than merely another observation of the same content.
- `UsageHighWater` — **activity only**: a larger token or cost aggregate proves accounting changed,
  but not that output, tool, repository, gate, or completion state advanced; a chatty provider must
  not suppress stuck-attempt warnings with usage snapshots alone.
- `ToolTransition` — **progress**: a later owned-tool transition ordinal proves lifecycle movement;
  a repeated tool status fails the strict high-water gate.
- `OwnedChildTransition` — **progress**: a later owned-child transition ordinal proves that child
  entered a new lifecycle state, not that its old state was polled again.
- `OwnedChildOutput` — **activity only**: arbitrary child stdout or stderr proves liveness, not a
  lifecycle, repository, gate, or completion transition; a noisy stuck child must not reset the
  warning epoch.
- `RepositoryOrdinal` — **progress**: a larger repository observation ordinal proves a newly observed
  repository state/change.
- `GateStarted` — **progress**: a later gate ordinal proves a new verification gate began; duplicate
  start observations do not advance.
- `GateExited` — **progress**: a later gate-exit ordinal proves new terminal verification evidence was
  produced.
- `CompletedSetGrowth` — **progress**: a larger completed-set cardinality proves an additional item
  settled.
- `ProducerTerminal` — **progress**: the first advancing terminal high-water closes an authoritative
  producer route; duplicate terminal observations fail the high-water gate.
- `Heartbeat` — **activity only**: it proves liveness but carries no state, output, counter, or
  completion advance.

## Frozen mutation control

- Patch: `docs/superpowers/reviews/2026-08-24-r2f1b-slice4g-mutation-control.patch`
- SHA-256: `80408cb018aaa558838679225490550d986c1ba307e918e6539d16f0cee83978`
- Production mutation: reset the progress epoch for every ordinary activity observation as well as
  meaningful progress. This models the chatty-but-stuck warning-suppression bug while retaining the
  stale-elapsed hardening.
- Applicability: `git apply --check` passed on the candidate before application and again after the
  reverse application restored the candidate.
- Mutant clippy: `cargo clippy --all-targets --all-features --locked -- -D warnings` passed with the
  prescribed populated offline Cargo environment.
- Full-suite comparison command:
  `cargo test --workspace --all-targets --all-features --locked --no-fail-fast`. The
  corrected candidate was green. The mutant slice target was 8 passed and 1 failed; its only
  full-suite red test was
  `bridge-core --test r2f1b_slice4g_progress_epochs::non_progress_activity_updates_only_activity_clock_and_epoch_keeps_climbing`.
  That singleton is therefore the exact candidate-versus-mutant red set.

The control patch and test expectations were hand-authored, then checked and executed; they were not
generated by a mutation tool.

## Verification

- RED-first: before production changes, the required-test target failed `--no-run` because both the
  total classifier and no-progress module imports were absent. This is compile-level evidence that
  the 4G surface did not exist; it is not claimed as an individually executed behavioral red for
  each test. In particular, the refusal behavior pre-existed 4G: 4F's
  `fixed_grace_admission_and_shipped_refusal_are_gated_by_frozen_activation` directly exercises both
  `resolve_execution_policy_v1` and `resolve_execution_policy_with_readiness_v1`. The 4G refusal test
  deliberately reasserts the still-`Disarmed` readiness and resulting manual-only activation without
  creating a second production admission entry point.
- `cargo fmt --all -- --check` — green.
- `git diff --check` — green.
- `cargo check --workspace --locked` — green.
- `cargo clippy --all-targets --all-features --locked -- -D warnings` — green.
- `cargo build --locked` — green.
- The exact configured command from `examples/a2a-bridge.containerized.toml` — green after the
  isolated Cargo target was made traversable for intentional UID-65534 child re-exec; the focused
  slice target was 9 passed, 0 failed and `bridge-store --lib` was 264 passed, 0 failed.
- The same configured command and environment on detached base `23e331c6` — `bridge-store --lib`
  was 264 passed, 0 failed. The overall base diagnostic was red in ten unrelated targets named below,
  so it is not presented as a green base gate.
- `cargo build --release --bin a2a-bridge --locked` — green.
- `cargo run -p a2a-bridge --locked -- validate --repo-hygiene` — green (40 tracked artifacts,
  8 validated example configs).

### Diagnostic exclusions and attribution control

- The supplied bridge verifier exited 101 at `bridge-store --lib`; its truncated evidence does not
  identify the individual test. That result is reported, not relabeled as green.
- A reproduction using a `mktemp -d` Cargo target at its default mode `0700` failed
  `fs_custody::tests::path_identity_refuses_an_unreadable_ancestor`,
  `sqlite::r2f0a_history_tests::foreign_owned_canonical_wal_and_shm_are_refused_before_sqlite_inspection`,
  and
  `sqlite::r2f0a_history_tests::selected_platform_privilege_failures_are_typed_and_leave_no_store_artifacts`
  with `PermissionDenied`. All three fixtures re-exec their current test binary after dropping to UID
  65534; that UID could not traverse the mode-`0700` target. Changing only the scratch directory to
  mode `0755` made the unchanged configured candidate command green, including all 264 bridge-store
  tests. The mode-`0700` runs are therefore environmental diagnostics, not gate evidence.
- An earlier concurrent mode-`0700` diagnostic and an earlier mutant run failed the untouched
  `compatibility::tests::staged_candidate_nonzero_exit_retains_process_status`. It passed in the
  serialized diagnostic, focused rerun, configured candidate gate, and both refreshed all-target
  candidate/mutant runs, so those earlier results are excluded as intermittent.
- The first base control put the detached worktree under `/tmp`; 23 tests in
  `a2a-bridge --test r3d0_foundation_cli` correctly rejected that checkout as outside the
  owner-approved `/Users/wesleyjinks/code` trusted cwd root. That invalid control is excluded; the
  base worktree was moved under the approved root before the recorded control above.
- From the approved root, the exact full base control still exited 101 in these unrelated targets:
  `a2a-bridge --test compatibility_cli`, `a2a-bridge --test r3d0_foundation_cli`,
  `a2a-bridge --test reader_dependency_pins`, `a2a-bridge --test toolchain_dependency_pins`,
  `bridge-a2a-inbound --test golden_wire`, `bridge-acp --lib`,
  `bridge-acp --test corpus_replay`, `bridge-core --test r2b1_diagnostics`,
  `lsp-mcp --test characterization`, and `lsp-mcp --test integration` (the visible LSP failures
  could not spawn `rust-analyzer`). This base-wide result is excluded as a checkout/host diagnostic.
  Crucially for the review's attribution question, `bridge-store --lib` was green both within that
  identical-command base run and in a focused base rerun (264 passed, 0 failed); it was also green in
  the final candidate gate. The supplied verifier's bridge-store failure therefore did not reproduce
  under a valid test target on either tree and is not a slice regression.
- A cross-worktree diagnostic reused `/cache/target` between base and candidate and produced missing
  classifier/module imports from mixed Cargo artifacts. It is excluded; all recorded comparisons use
  separate target directories.
- A preliminary focused rerun omitted the prescribed offline Cargo environment and failed before
  compilation while the blocked crates.io proxy tried to download `a2a-lf`; it is excluded.
- Two preliminary mutant-clippy diagnostics are excluded: one omitted the prescribed offline
  environment and hit the blocked crates.io proxy; another used an empty Cargo home and could not
  resolve cached `arc-swap`. The subsequent prescribed offline run is the green mutant-clippy result.

No host smoke or fallback-plan failure occurred.

## Frozen invariants and exclusions

- `crates/bridge-workflow/src/executor.rs` is byte-identical to the base; both SHA-256 values are
  `def9c4fc6dc174f7d744ef2554df4f428550a84725ee71129c7ff7127be684d4`.
- No executor or scheduler-arbitration wiring was added; that remains 4H.
- No timer, wait, `select!`, sleep, spawn, token, cancellation route, terminal route, or
  `MechanicalImpossibilityProofV1` construction was added or changed.
- Production readiness remains `Disarmed`; `AutomaticR2f1b` remains unavailable.
- `MAX_WORKTREE_CONFIGURES_IN_FLIGHT`, all manifests, and `Cargo.lock` are unchanged.
- `ActivityReason` variants are unchanged.

## Size

Added nonblank physical Rust lines after formatting: **344 / 350**. This consists of 49 added lines
in existing Rust files, 117 nonblank lines in the new warning module, and 178 nonblank lines in the
new integration-test file. Docs and the frozen control patch are excluded from the cap.
