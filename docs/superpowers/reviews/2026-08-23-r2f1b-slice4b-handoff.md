# R2f1b slice 4B handoff — constructible, fully refused

## Candidate scope

- `SchedulerActivationReadinessV1` has exactly `Disarmed` and `Armed`; the production
  `scheduler_activation_readiness_v1` body returns only `Disarmed`. That body is the complete 4J flip.
- `deadline_activation_v2_for` is the single decision and construction site: only armed production maps
  to `AutomaticR2f1b`; the other three readiness/policy pairs map to `ManualOnlyR2f1a`.
- `FrozenWorkflowControlsV1.deadline_activation` now carries `DeadlineActivationV2` selected by the
  decision. Frozen-controls validation admits the automatic value; the existing production fixed-grace
  refusal and all other validation are unchanged.
- The configured-workload fingerprint adds a canonical activation dimension only for automatic runs.
  Manual activation therefore retains the exact historical fingerprint while automatic and manual runs
  cannot share a calibration population.
- Workflow admission computes activation after every immutable `entry_snapshot` read. Automatic activation
  with any selected legacy watchdog refuses before control resolution, node identity or checkout freezing,
  registry mutation, backend resolution, session creation, or provider work. Manual activation retains the
  existing watchdog behavior.
- No timer, event-loop, selection, sleep, spawn, cancellation, worktree-concurrency limit, manifest, or
  `Cargo.lock` change is present.

## Required regression evidence

- `scheduler_readiness_and_policy_activation_select_deadline_activation` covers all four decision pairs and
  proves frozen-controls validation carries an automatic value when explicitly exercised with armed
  readiness.
- `manual_deadline_activation_controls_retain_literal_wire_bytes` decodes a literal persisted controls byte
  string containing `"deadline_activation":"manual_only_r2f1a"` and requires re-encoding to equal those
  exact literal bytes. It does not round-trip a freshly constructed fixture.
- `workload_fingerprint_partitions_deadline_activation_without_moving_manual_baseline` requires automatic
  and manual fingerprints to differ and pins the manual value to
  `shape-9892a9f12f1daf2edcc832b7f85437b937abd389e6691cad09c2f0bb0467b1c4`.
- `automatic_v3_refuses_legacy_watchdog_before_effects` observes exactly one registry snapshot and zero
  registry mutations, backend/session/provider resolutions, or checkout freezes. Its intentionally invalid
  max-effort controls also make the watchdog error prove refusal precedes control resolution.
- `disarmed_production_admission_never_obtains_automatic_activation` drives the public
  `WorkflowAdmissionV1::freeze` path and requires manual activation.
- `manual_activation_admits_legacy_watchdog_configuration` is the negative watchdog case and requires normal
  admission with manual activation.

## Frozen single-mutation control

- Path: `docs/superpowers/reviews/2026-08-23-r2f1b-slice4b-mutation-control.patch`.
- SHA-256: `2bae9460b7eccb13f861d616ead56f756d112dbe9329b7097b7abba3b934e5a7`.
- Logical mutation: change the sole automatic decision arm from `(Armed, Production)` to
  `(Disarmed, Production)`.
- `git apply --check` succeeds on the restored candidate; the control was applied and reversed cleanly.
- The full-suite command was
  `CARGO_HOME=/cargo CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast -- --test-threads=1`,
  with localhost excluded from the injected HTTP(S) proxy so loopback fixtures remained local. It reached
  every non-doc test target and measured an actual red population of **7 tests across 4 targets**:
  `scheduler_readiness_and_policy_activation_select_deadline_activation`,
  `automatic_v3_refuses_legacy_watchdog_before_effects`,
  `workload_fingerprint_partitions_deadline_activation_without_moving_manual_baseline`,
  `disarmed_production_admission_never_obtains_automatic_activation`,
  `manual_activation_admits_legacy_watchdog_configuration`,
  `manual_v3_identity_is_the_unbound_historical_fingerprint`, and
  `manual_workload_fingerprint_matches_the_pinned_golden`.
- After those assertion failures, Cargo separately reported 16 doc-test target launch failures because this
  image has no `rustdoc`; those environmental launch errors are not included in the 7-test red population.
- On the same mutated source tree, the exact required
  `cargo clippy --all-targets --all-features --locked -- -D warnings` command exited **0**. It used the same
  locked offline cache and a fresh isolated target directory to avoid stale shared-target metadata.

## Candidate verification

- `cargo fmt --all -- --check`: green.
- `cargo clippy --all-targets --all-features --locked -- -D warnings`: green.
- `cargo build --locked`: green.
- The configured non-doc workspace suite is green on the restored candidate.
- `git diff --check`: green.

## Deliberate exclusions

- `contract_fingerprint` is not added to the configured-workload fingerprint because no production caller
  constructs `FrozenR2f1bContractV1` yet; that residual remains with the first contract-construction slice.
- Fixed grace, progress epochs, scheduler arbitration, the executor multiplexer, issue #22 terminalization,
  and production arming remain in 4C–4J. `crates/bridge-workflow/src/executor.rs` is untouched.
- Through 4C–4J, every production use of the hidden public
  `resolve_execution_policy_with_readiness_v1` seam must remain routed through `WorkflowAdmissionV1`'s
  legacy-watchdog gate; the seam is public only because bridge-core and bridge-workflow are separate crates.
- V2 snapshots and direct sessions gain no new path or behavior; manual activation retains identical wire
  bytes and the configured-workload fingerprint baseline.

## Counted size

After `cargo fmt`, the candidate adds **343 nonblank physical Rust lines** against the **350-line cap**.
Documentation and the frozen mutation patch are excluded, as required.
