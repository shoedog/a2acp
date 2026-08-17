# R2f1b 3d T3a handoff — exact absence decides, never acts

## Delivered boundary

T3a adds a typed, state-agnostic `ExactAbsenceCandidateV1` decision:

| Observation | T3a decision |
| --- | --- |
| target present | `Refused(TargetPresent)` |
| target absent but registered | `Refused(RegisteredButAbsent)` |
| target and registration both absent | `Authorized` |
| probe failure or recovery ownership | `Refused(CannotProve)` |

The decision code has no record writer, provider removal, lock acquisition, directory removal, or
custody-state transition. `Authorized` is only a typed value. T3b must repeat the proof while it
holds B19's refusing proof-to-transition-to-unlink window, publish `UnusedSettled`, and perform
the descriptor-safe removal.

No edge was added to `LEGAL_CUSTODY_TRANSITIONS_V1`; `cleanup_failed_add` remains absent from every
new custody path.

## B18 seam design

Chosen: `sweep::ExactAbsenceProbeV1` is a synchronous capability trait, following the existing
`LeaseProbe` pattern. `HostGitWorktree` implements it with a synchronous `git worktree list
--porcelain -z` process call and shares the existing exact porcelain parser. It observes target
existence first, then registration only when the target is absent. Spawn/list failures return
`Err`, which the decision maps to `CannotProve`. Target existence is read with no-follow
`symlink_metadata`, never `try_exists`: a dangling final symlink is an extant target and
therefore returns `TargetPresent`, never authorization.

This preserves the sweep's real synchronous contract: it is called at boot and from a `Drop`
backstop. The implementation performs an explicit blocking host command; it does not call
`Handle::block_on`, create a nested Tokio runtime, or wait on an async task. The pre-existing
sweep already performs synchronous Git commands, so this is a new read-only host capability at
the same boundary, not an async API disguised as synchronous. The T3a decision-only sweep is
separate from the pre-existing legacy reclaimer, so it cannot route authorization into removal.

Rejected:

- Calling private async `registration_absent` through `block_on`: risks executor deadlock and
  makes the sync sweep contract false.
- Making the sweep async: impossible for `WorktreeRunEndGuard::drop` and incorrectly changes the
  boot surface to a per-turn async contract.
- Inferring absence from a Git error or target `false` alone: collapses the required fourth answer
  into authorization.

## Population and inventory coupling

The same `decide_unused_candidate` definition serves a source-bearing marker candidate (the legacy
marker arm of the boot sweep) and the backend's in-memory frozen candidate. It accepts no custody
state, so it cannot make a state-specific definition by accident.

The existing V3 custody record deliberately has no canonical source; its original sweep was
forbidden to run Git. T3a leaves its bytes untouched. When a boot sweep encounters that source-less
population it returns `CannotProve` and logs only that refusal. This is a deliberate fail-closed
falsification of the tempting schema extension: adding source to the record would violate this
task's no-record-mutation boundary. T3b receives a source-bearing candidate explicitly when it
owns the acting path.

Each production materialization flight retains its frozen source/target candidate. T2's initial
`begin_transfer` CAS changes an active owner to `TransferPublishing` *before* the durable
`Transferred` publication and before insertion into `preparation_recovery_flights`. T3a therefore
locks the recovery inventory and then the active-flight map in the same recovery→active order T2
uses to move the owner. It refuses when either map owns the exact candidate, including an active
owner whose transfer CAS has succeeded. The locks are released before the host probe. Thus the
durable-publication-to-inventory interval cannot authorize a transfer-owned candidate; a transfer
after the atomic sample remains harmless because T3a is decision-only and T3b must re-prove under
its later action lock. `transfer_owned_active_candidate_refuses_before_recovery_inventory_publish`
pins that otherwise-reachable interval.

## Red-first evidence

The disposable red-first test file was compiled against unmodified base `1d7826dd` before its
implementation was introduced. The required T3a seam did not exist, so the named test batteries
failed at compilation rather than producing a misleading “zero tests selected” success:

```text
$ cargo test -p bridge-worktree --lib unused_candidate_settles_only_after_exact_absence -- --exact
error[E0425]: cannot find function `decide_unused_candidate` in this scope
  --> crates/bridge-worktree/src/sweep.rs:...
error[E0433]: failed to resolve: use of undeclared type `ExactAbsenceCandidateV1`
  --> crates/bridge-worktree/src/sweep.rs:...
error: could not compile `bridge-worktree` (lib test) due to previous errors

$ cargo test -p bridge-worktree --lib recovery_owned_candidate_refuses_even_when_exact_absence_is_observed -- --exact
error[E0599]: no method named `decide_unused_candidate_for_recovery` found for struct `WorktreeBackend`
  --> crates/bridge-worktree/src/backend.rs:...
error: could not compile `bridge-worktree` (lib test) due to previous error

$ cargo test -p bridge-worktree --lib exact_absence_proof_serves_marker_and_candidate_populations -- --exact
error[E0433]: failed to resolve: use of undeclared type `ExactAbsenceObservationV1`
  --> crates/bridge-worktree/src/sweep.rs:...
error: could not compile `bridge-worktree` (lib test) due to previous error

$ cargo test -p bridge-worktree --lib unused_candidate_settles_only_after_exact_absence -- --exact
error[E0599]: no method named `encode_canonical` found for the pre-implementation proof result
  --> crates/bridge-worktree/src/sweep.rs:...
error: could not compile `bridge-worktree` (lib test) due to previous error
```

The first run is the refusing/authorized zero-mutation exit gate (present target,
registered-but-absent, probe failure, and both absent); the second is the recovery-owned battery;
the third is the shared-population battery; the fourth is the explicit no-record-byte battery.
The implemented tests now pin all of those arms, including the transfer-owned interval before the
recovery inventory entry exists.

## Verification

- `cargo fmt --all -- --check` — passed.
- `git diff --check` — passed.
- Focused `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --lib
  unused_candidate_settles_only_after_exact_absence -- --exact` and required
  `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets -- -D warnings` — environment-red
  before compilation: Cargo could not fetch `a2a-lf` because the configured CONNECT tunnel returned
  HTTP 403. The dependency-capable verification stage runs those gates.
- `tools/check-nonunix.sh` was not applicable: this change does not touch `crates/bridge-core`.

## Size

`git diff --cached --numstat 1d7826dd` reports 679 added and 20 deleted lines across source and
this handoff, below the 750-line cap.

## Legacy-sidecar validation repair

The exact-absence sweep now passes legacy records through the existing `sidecar_file_matches` and
`worktree_under_root` guards before creating a candidate. A failed guard returns
`Refused(CannotProve)`; this path remains decision-only. Valid in-root sidecars still use the
existing exact-absence predicate.

### Repair verification

- `rustfmt --check crates/bridge-worktree/src/sweep.rs` — passed.
- `git diff --check` — passed.
- Post-fold `git diff --numstat c336d9c7..HEAD`: `156/6` in `sweep.rs`, `10/6` in `reaper.rs`, `21/0` here; 199 changed lines.
- `exact_absence_sweep_refuses_an_out_of_root_legacy_sidecar` — not run, no local toolchain.
  Removing `worktree_under_root` should make it red by returning `Authorized`.
- `exact_absence_sweep_refuses_a_sidecar_that_does_not_match_its_file` — not run, no local
  toolchain. Removing `sidecar_file_matches` should make it red by returning `Authorized`.
- `exact_absence_sweep_authorizes_a_valid_in_root_legacy_sidecar` — not run, no local toolchain.
  Unconditionally refusing legacy sidecars should make it red.
- Cargo test, Clippy, and the workspace suite — not run, no local toolchain; crates.io-dependent
  Cargo work is blocked by the configured HTTP 403 egress policy.

## Repair 2 — resolved identity and recovery inventory consultation

The shared exact-absence path comparator now resolves each absolute path through its nearest
existing canonical ancestor before re-appending an absent tail. Both porcelain registration checks
(sync and async) and recovery-candidate ownership use that one definition. Thus Git's canonical
`worktree` spelling and a candidate's original `/var`, symlinked-parent, trailing-slash, or
existing-parent `..` spelling compare as the same registration. Invalid UTF-8 porcelain paths,
relative candidates, or an identity probe error refuse as `CannotProve`; none can become
`BothAbsent`.

Recovery ownership is consulted at one linearization point: T3a holds
`preparation_recovery_flights` and then `preparation_flights`, the same recovery-to-active order
used by T2's transfer move. While both guards are held it compares the exact frozen source/target
identities. T2 either has not won `begin_transfer`, has a transfer-owned active entry, or has
inserted the recovery entry before removing that active entry while holding those same two guards.
The consultation therefore cannot observe a post-transfer half-published owner. The guards are
released before the host probe; a later transfer remains harmless because this T3a result is
decision-only and T3b must prove again inside its action window.

### Repair 2 verification

- `cargo fmt --all -- --check` and `git diff --check` — passed.
- `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --lib exact_absence -- --nocapture` —
  attempted, but no test compiled or ran: Cargo could not fetch uncached `a2a-lf` because the
  configured CONNECT tunnel returned HTTP 403.
- `cargo clippy --workspace --all-targets -- -D warnings` — not run after that identical
  dependency preflight failure; it requires the dependency-capable host control.
- `git diff --numstat 5cbfddf2` — 240 added / 36 deleted (276 total changed lines); the
  `crates/bridge-core/src/reaper.rs` diff is empty.
