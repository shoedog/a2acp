# R2f1b slice 4C handoff — preservation and exact cleanup ownership

## Scope and result

- Base: slice 4B.
- Added one synchronous, clock-free cleanup-settlement decision over an observed cleanup state,
  elapsed cancellation-relative milliseconds, and a supplied cancellation-relative deadline.
- The deadline helper anchors at cancellation and caps at
  `work_cutoff + CLEANUP_TAIL_MS`; a cancellation 10 seconds after the work cutoff therefore
  receives a 50-second cleanup interval rather than a fresh 60 seconds.
- `Complete` and `Failed` observations project their terminal state only when their duration is no
  later than both the supplied observation time and deadline. Future or late terminal observations
  normalize to the same exact-owner unsettled path: `Pending` before the deadline and `Partial` at
  it. `NotNeeded` remains terminal because no resource was materialized.
- Owned-unknown observations carry their exact `RecoveryOwnerV1`. The ownerless form instead
  requires `UnidentifiableCleanupOwnerProofV1`, whose flight identity is its only authority source
  and whose field has no public constructor. A caller that knows an owner therefore cannot select
  ownerless cleanup or bypass the sole-owner guard. Slice 4H must mint that proof only from its
  authority-backed failure to identify an owner.
- Added a workflow typestate seam. `WorkflowNodeCancellationSettlementV1<PreservationRequiredV1>`
  has no disposition method; only `after_preservation` can produce the
  `PreservationTypedV1` state that exposes `into_disposition`.
  `TypedWorktreePreservationV1` rejects `Pending`, while preserved, removed, not-needed, and
  durably unknown results are typed.
- `into_disposition` returns the durable `NodeCleanupRecordV2` shape directly and calls its existing
  `validate_coherence` product-table check before consuming the cleanup guard. Thus
  `NotNeeded + Preserved` and every other incoherent cleanup/preservation product are refused rather
  than emitted.
- Added a non-cloneable sole-owner guard. Its exact owner can only settle or transfer; dropping it
  while live increments an observable audit violation. The preservation-first disposition consumes
  that guard and places its stored owner directly in `Partial` or owned `Unknown`.
- The existing `BoundedRecoveryReasonV1::new` remains the sole reason-bounding path. Its existing
  `bounded_recovery_reason_truncates_at_max_bytes` regression continues to assert truncation at
  `MAX_RECOVERY_REASON_BYTES`; this slice adds no alternate bound or raw-string storage path.

## Red-first and required regressions

Before production edits, the new bridge-core target compile-failed on the absent
`NodeCleanupObservationV1`, `cleanup_deadline_after_cancellation_ms_v1`, and
`settle_node_cleanup_v2` symbols. The new bridge-workflow target separately compile-failed on the
absent observation and `cancellation_settlement` module. After implementation:

- bridge-core slice-4C target: 5 passed / 0 failed;
- bridge-core sealed ownerless-proof unit: 1 passed / 0 failed;
- bridge-workflow slice-4C target: 4 passed / 0 failed;
- bridge-workflow compile-fail target proves disposition is unavailable before typed preservation;
- the existing production-admission test reasserts that disarmed production obtains only
  `ManualOnlyR2f1a`, while also compiling through the new cleanup seam.

The regressions cover timely complete/failed/not-needed, late complete/failed exact-owner transfer,
the defensive future-duration-versus-elapsed reclassification, ordinary exact-owner partial
transfer, exact owned unknown, ownerless unknown through only the sealed proof, the binding
work-cutoff cap, preservation-before-disposition typestate, detected
sole-owner drop, rejection of `NotNeeded + Preserved`, no violation after settle/transfer, and exact
transfer through the combined seam.

## Encoding stability

`NodeCleanupV2` itself was not edited. The new encoding regression compares
`serde_json::to_vec` against literal byte strings for:

- `{"state":"complete","duration_ms":17}`;
- `Partial { duration_ms: 60000, recovery_owner }` with fixed literal attempt, flight, and reason;
- owned `Unknown { duration_ms: 60001, recovery_owner }` with the same fixed literal owner.

All three are byte-exact. No persisted schema, tag, field name, field order, optionality, or cleanup
record cap changed.

## Frozen single-mutation control

- Path:
  `docs/superpowers/reviews/2026-08-23-r2f1b-slice4c-mutation-control.patch`
- SHA-256: `8c591a02e2393dfd3fd806111ace03fefbaa47aef25cdeb5f30d7e681b501d31`
- Production mutation: changes the deadline comparison from `elapsed >= deadline` to
  `elapsed > deadline`.
- `git apply --check` passed before measurement and again after restoration; the patch applied
  and reversed cleanly.
- The serialized full workspace suite reached every non-doc target. Against the candidate's zero
  non-doc failures, the actual red population was exactly **4 tests across 2 targets**:
  - `unknown_cleanup_retains_identifiable_owner`;
  - `unsettled_at_deadline_transfers_exact_owner_as_partial`;
  - `work_cutoff_plus_cleanup_tail_cap_binds`;
  - `preservation_first_disposition_transfers_the_guard_owner`.
- The same mutated source passed
  `cargo clippy --all-targets --all-features --locked -- -D warnings`.
- Sixteen doc-test targets separately failed to launch because this image has no `rustdoc`.
  They never executed and are environmental, so they are not part of the four-test red population.
- Restoration re-established the inclusive comparison, and the frozen patch remains cleanly
  applicable.

## Candidate verification

- `cargo fmt --all -- --check`: green.
- `cargo clippy --all-targets --all-features --locked -- -D warnings`: green in a fresh isolated
  target directory.
- `cargo build --locked`: green in a fresh isolated target directory.
- The configured test command exited 101 with exactly one failing test:
  `bridge-api backend::tests::settlement_refusal_does_not_mask_the_provider_failure` (65 passed,
  1 failed, 0 ignored, 0 measured).
- `git diff --check`: green.

The operator gathered the following attribution evidence:

- `crates/bridge-api` is untouched by this diff: the path-scoped diffstat against the base is empty.
- The same test, on this exact candidate tree, in the same `a2a-toolchain:latest` container image and
  cache, under deliberate compile contention: **10/10 ran and passed**. The original failure is
  therefore **non-deterministic**.
- Base `1b328196`, same container, same load: **10/10 ran and passed**.
- Host runs: 25 idle + 10 under load on each tree, **0 failures on both**.
- Proposed mechanism (from review): a hard 2-second `tokio::time::timeout` polling a `wiremock` mock
  server, executed immediately after a 3m17s serialized full-workspace compile.

The failure was never reproduced, so "pre-existing" is an inference from non-determinism plus
untouched code, not a demonstrated base-tree failure.

## Deliberate exclusions

- No cancellation path, timer, sleep, spawn, select, token, or scheduling behavior was added.
- `crates/bridge-workflow/src/executor.rs` and its event loop are untouched.
- The seam is not wired into live cleanup; integration remains with slice 4H.
- A pending `into_disposition` error is an immutable observation snapshot. Slice 4H must reconstruct
  the seam with a fresh elapsed reading on each poll; it must not reuse the returned box as mutable
  timer state.
- Scheduler readiness remains `Disarmed`; no production caller can obtain automatic activation.
- Production fixed grace remains refused under disarmed production.
- `MAX_WORKTREE_CONFIGURES_IN_FLIGHT`, every manifest, and `Cargo.lock` are untouched.
- No compatibility run, provider turn, release, deployment, or operator mutation occurred.

## Counted size

The formatted candidate adds **498 nonblank physical Rust lines** against the 500-line cap.
Documentation and the frozen mutation patch are excluded. The stop boundary is satisfied.
