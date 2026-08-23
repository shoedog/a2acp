# R2f1b slice 4A handoff

## Base and scope

- Task-declared base: `origin/main` at `462e676b`.
- Plan of record named by the task: `docs/superpowers/plans/2026-08-23-r2f1b-slice4-decomposition.md`. That path was absent from the prepared clone; the authoritative task and verified load-bearing anchors were used without expanding scope.
- No timer is armed, automatic activation remains refused at production admission, and the executor event loop is unchanged.
- Wall time remains record identity only. Schedule, cleanup, telemetry, and reporting use monotonic attempt offsets.

## Changed files

- `crates/bridge-core/src/execution_policy.rs`
- `crates/bridge-core/src/attempt_activity.rs`
- `crates/bridge-core/src/ports.rs`
- `crates/bridge-workflow/src/executor.rs`
- `crates/bridge-coordinator/src/detached.rs`
- `bin/a2a-bridge/src/main.rs`
- `docs/superpowers/reviews/2026-08-23-r2f1b-slice4a-control.patch`
- `docs/superpowers/reviews/2026-08-23-r2f1b-slice4a-handoff.md`

## Implementation

- Added the 30,000 ms control-action internal timeout and 5,000 ms cancellation internal grace beside the frozen policy constants, outside `liveness_profile_v1`.
- Added synchronous `AttemptScheduleV1` remaining/due math over `Arc<dyn MonotonicClock>`. Construction and observation do not spawn, sleep, select, cancel, or arm a timer.
- The terminal barrier now creates one attempt clock from the already-captured attempt epoch and gives the same `Arc` to telemetry/recorder, schedule math, workflow cleanup, and terminal reporting.
- The detached durable/attempt factory wrapper forwards that exact clock identity. Legacy contexts with no attempt clock retain a system-clock fallback.
- Offline `run-workflow` retains its telemetry factory's clock for prefinal/work, finalization, and end-to-end reporting, matching the cleanup and activity epoch.
- Cleanup interval accounting now stores monotonic millisecond offsets from the attempt clock while preserving union semantics.
- Deliberate scope: `run_agent_preflight_uncached` and `run_node` retain paired `Instant` reads only for turn-local elapsed/TTFT diagnostics. Those durations are never compared across components or against an attempt epoch or policy bound, so threading them would expand this slice beyond its named scheduler, cleanup, and reporting consumers. The pre-existing direct `workflow_history::DirectAttemptBarrier` surface likewise remains excluded: its adjacent self-created recorder and terminal-report epochs are not reached by this slice and should be converged in a follow-up.

## Test-clock decision

The three task-named doubles were not converged. The two module-local `FakeClock` values exercise small core seams, while coordinator `ManualClock` intentionally combines wall and monotonic behavior for a different boundary. Moving them into a cross-crate test utility would touch unrelated preparation/coordinator tests and exceed this sub-slice's ownership purpose. The prepared clone also contains later module-local clock doubles; those remain untouched for the same reason.

## Tests and guards

- `r2f1b_slice4a_policy_tests::internal_action_timers_leave_observable_settlement_margin` asserts both strict internal-to-observable relationships.
- `r2f1b_slice4a_policy_tests::observable_liveness_profile_remains_frozen_for_slice4a` asserts all eight frozen observable values explicitly.
- `attempt_activity::tests::schedule_math_is_exact_at_the_bound_under_the_attempt_clock` checks exact remaining time and due-ness immediately before and at the boundary under a fake clock.
- `attempt_activity::tests::constructing_schedule_math_is_inert` proves schedule construction performs no clock action.
- `attempt_activity::tests::one_attempt_clock_feeds_recorder_telemetry_and_schedule_math` proves identity and observations are shared.
- `sink_tests::served_terminal_barrier_projects_its_production_rich_sink_evidence` now proves the terminal barrier, telemetry factory, and production composite expose the same `Arc` identity.
- `cli_tests::run_workflow_reporting_and_rich_sink_share_attempt_clock` proves offline terminal reporting and the production rich sink expose the same `Arc` identity.
- Existing `automatic_r2f1b_refused_at_production_admission` remains the production-admission guard; its paired manual-admission test prevents a vacuous refuse-all implementation.
- A pre-change object check verified that neither new timer symbol nor `AttemptScheduleV1` existed, so the relationship and boundary tests are compile-red against the pre-change sources rather than assumed green.

## Verification and attribution

- `cargo fmt --all -- --check`, `git diff --check`, and warnings-denied workspace/all-target Clippy pass. The Clippy remediation gates the test-only legacy `measured_workflow_terminal` wrapper with `#[cfg(test)]`; production uses `measured_workflow_terminal_from_elapsed`.
- The complete unmutated bridge-core crate passes: 697 library tests plus every trybuild, integration, and doc target are green. The complete bridge-coordinator and bridge-workflow package run passes 493 tests across all targets.
- The initially reported hidden bridge-core failure was reproduced as an unchanged `retained_resource_flight::tests::recovery_publication_runs_after_releasing_registry_mutex` timeout. It passed alone. A second full run failed a different unchanged process-signal test, which also passed alone. An archived unmodified bridge-recorded base passed 692/692 in the same container/cache, and the final candidate full-crate run passed completely. The changing failure identity plus green exact reruns establishes non-repeatable parallel-load behavior; none of the slice tests failed.
- The full workspace run was attempted with `--no-fail-fast`. It reproduced unchanged CLI/mock-network and outbound failures, then stalled for more than five minutes in an unchanged bridge-api timing test and was interrupted at that owned Cargo session only. The task already records a base CLI failure population that inflates under parallel load; no workspace failure named a changed path or slice test.
- `cargo run -p a2a-bridge --locked -- validate --repo-hygiene` passes at both the implementation and handoff points with 40 tracked artifacts and 8 validated example configs.

## Size

The final diff adds **304 nonblank physical Rust lines**, counted from formatted `git diff -U0` additions. This is below the 450-line cap.

## Frozen mutation control

- Path: `docs/superpowers/reviews/2026-08-23-r2f1b-slice4a-control.patch`
- SHA-256: `21b600c385d60b41b511c0acee30697e4f893b548d52c088300b9a04fb8bfd13`
- Production mutation: changes `R2F1B_CONTROL_ACTION_INTERNAL_TIMEOUT_MS` from 30,000 ms to 31,000 ms, equal to the observable control bound.
- Clean apply: `git apply --check` passed; the mutation was applied and then restored with `git apply -R`.
- Named red test: `r2f1b_slice4a_policy_tests::internal_action_timers_leave_observable_settlement_margin`.
- Actual full-crate red population: exactly one test. The library result was 696 passed / 1 failed / 0 ignored, and every remaining bridge-core test/doc target passed.
- Mutation clippy result: `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` passed with exit 0.
- The mutation changed production only and failed on the strict relationship assertion, not on compilation, dead code, or test infrastructure.
- Restoration: verified the production constant is 30,000 ms and the frozen patch still applies cleanly.

- [ ] `cargo fmt --all -- --check` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-core --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **PENDING OPERATOR**
