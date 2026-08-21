---
task-type: implement
---
# R2f1b 3d T3a increment 1 — slice B: populate root observations

## Description

Slice B populates the production root observations returned by the checked-scan engine. Historical A2a-1 references assigning descriptor-owned enumeration to “A2b” mean this slice; the merged return-type slice did not take that obligation. The obligation is named **slice B** throughout this specification.

The authoring base is clean `main` at `9ce2074ef2a4e7b7bb81b9561b79ba672f9db9db`.

### Mandatory repository precondition

The repository falsifies one supplied anchor at this base:

- `classify_root_observations` in `crates/bridge-worktree/src/sweep.rs:626-651` treats a capture as complete when `dev` and `ino` are present; `root_capture_has_object_identity` does not require `birthtime`.
- `root_observation_classifier_reports_pinned_captures` in the same file explicitly expects `Pinned` for three equal captures whose `birthtime` is `None`.

Therefore the current classifier can return `Pinned` on a birthtime-less filesystem. That contradicts this task’s required capability boundary: an absent birthtime must make the result `Unavailable`.

This slice is population-only and may not silently broaden itself into a classifier correction. Do not begin implementation on the authored base. Stop and report this discrepancy unless the operator first supplies a repository base on which all three `(dev, ino, birthtime)` members are required for every capture and the classifier tests establish:

- any absent capture or absent tuple member yields `Unavailable`;
- `Unavailable` takes precedence over any mismatch;
- three complete equal tuples yield `Pinned`;
- three complete tuples with any inequality yield `IdentityChanged`.

The prerequisite correction is outside slice B and outside the sizing worksheet below.

### Production requirement after the precondition is satisfied

Replace the production-only default returned by `CompatibilityCheckedScanRootSessionV1::finish` with observations captured from the real scan session.

Add a bridge-core retained-directory enumerator with this contract:

- Opening the enumerator retains one directory descriptor independently of `CompatibilityPinOpenerV1`.
- On a platform where descriptor-owned enumeration is supported without changing `std::fs::read_dir` behavior, the name iterator is created from a duplicate of that exact retained descriptor. It must not reopen the supplied path to create the enumeration driver.
- `retained_enumeration_object` is derived only from metadata read from that retained descriptor. Metadata read from the root path, the independent custody pin, a fresh descriptor, or the duplicate after it has been replaced by another driver does not satisfy this field.
- Opening must preserve the path-resolution behavior needed by the action projection, including accepted raw-root aliases. Do not substitute a no-follow open on the caller’s spelling if that would reject an alias accepted by the current scan.
- Names remain raw `OsString` values. Dot entries are omitted, invalid UTF-8 remains representable, iteration order remains unspecified, and an entry-read failure remains one skipped-entry observation rather than an open refusal.
- If descriptor-owned enumeration is unavailable on a target, retain the current path-based enumeration behavior and expose no retained-enumeration identity. That target must produce `Unavailable`; lack of this capability is a supported outcome, not a test failure or an excuse to change scanning behavior.
- Reuse the existing Linux/macOS descriptor-duplication and directory-stream machinery in `bridge_core::fs_custody` where it preserves this contract. Add no dependency.

`CompatibilityCheckedScanSourceV1::open` must preserve the current sequencing and refusal behavior:

1. Open the enumeration source.
2. If enumeration cannot open, return `CheckedScanOpenRefusalV1::CannotEnumerate` without calling the custody pin opener.
3. Otherwise call the independent custody pin opener exactly once.
4. A custody pin failure does not fail enumeration: legacy rows remain readable and custody rows remain `UnreadableCustody("sweep root is not pinnable")`.

The production session must retain the enumeration root spelling, the bridge-core enumerator, and the independent `PinnedDirectoryV1`. At `finish` it must populate:

- `retained_enumeration_object` from the exact retained descriptor that was duplicated to drive enumeration;
- `pinned_custody_directory` from the actual independent custody pin, if that pin exists;
- `final_named_root` from a fresh directory descriptor opened through the session’s enumeration-root name after enumeration has ended.

Each populated `RootIdentityCaptureV1` copies the observed descriptor metadata exactly: `dev`, `ino`, and `BirthTimeV1::from_metadata`. An unavailable descriptor, metadata read, platform member, or birthtime leaves the corresponding capture or member absent; it does not turn a completed scan into an enumeration refusal.

Do not compare these observations with `DirectoryIdentityV1::matches`. Its absent-birthtime wildcard is intentionally weaker than the complete-tuple proof required here.

### Behavioral preservation

Root observation capture is additive metadata produced by the existing checked-scan session. It must not change which names are selected, which malformed legacy entries are silently omitted, which custody entries become refusal rows, how iterator errors are counted, when assessments run, the stored decisions, or the action/exact projections. The ten characterization scenarios recorded in the slice A2a-2 handoff must remain unchanged and green. Tests must not compare ordering from independent real-directory traversals.

Add focused tests with these exact obligations:

- `retained_directory_enumerator_duplicates_the_retained_descriptor`: use a deterministic after-retained-open seam to replace the named root before the enumeration driver is created; enumeration and retained metadata must still describe the original object.
- `retained_directory_enumerator_preserves_raw_names_without_dot_entries`: exercise raw names, including invalid UTF-8 on Unix, without asserting filesystem order.
- `retained_directory_enumerator_falls_back_without_claiming_descriptor_ownership`: on unsupported targets, preserve path-based enumeration while reporting no descriptor-owned capability.
- `production_checked_scan_populates_retained_descriptor_capture`: prove the production source, not an injected lookalike, returns a retained-descriptor capture. This must fail on the pre-slice production code because `finish` returns the default set.
- `production_root_replacement_reports_identity_changed`: replace the root through the custody-pin seam after the retained enumerator opens; rows must come from the original retained object while the complete unequal captures classify as `IdentityChanged`.
- `production_pin_failure_keeps_rows_and_reports_unavailable`: preserve legacy and unreadable-custody behavior while the missing custody capture makes the root observation `Unavailable`.
- `action_projection_accepts_raw_root_alias`: prove the production action route still selects the same rows from an accepted non-canonical root spelling and preserves that spelling in projected record paths.
- `root_observation_capability_is_visible`: exercise the production report and emit exactly one single-line record prefixed `SLICE_B_ROOT_CAPABILITY_V1=` followed by JSON containing the three captured identities, `descriptor_owned_enumeration`, `all_three_birthtimes_available`, `expected_classifier`, and `observed_classifier`.

The capability test must derive its expectation from observed facts:

- Expect `Pinned` only when descriptor-owned enumeration is present and all three complete identities are equal.
- Expect `Unavailable` when descriptor-owned enumeration or any identity member, including birthtime, is unavailable.
- The test may pass on either filesystem-capability branch only if the emitted record makes the branch and its identity evidence visible.

### Scope fences

- Do not change `EXACT_ABSENCE_POLICY_READY_V1`; it remains `false`, and increment 2 owns the population-admission rule.
- Do not change `sweep_orphans_with_exact_absence`’s signature or any report vocabulary.
- Add no ownership, locking, transition, unlink, removal, or other action authority. T3a decides and reports; T3b must independently re-open, re-read, re-bind, and re-prove exact absence under its own lock before acting.
- Do not repair the Unix-only separator guard in `is_custody_record_name`; preserve the characterized divergence.

### Counted-line worksheet

Count added nonblank physical lines after `cargo fmt`, against the slice’s rebound base. Every row is an independent hard cap: no contingency, no borrowing between rows, and no compression of evidence to recover an overrun.

| Row | Cap |
| --- | ---: |
| Bridge-core retained-directory enumerator and supported/unsupported platform implementations | 140 |
| Worktree production integration and capture conversion | 70 |
| `retained_directory_enumerator_duplicates_the_retained_descriptor` | 28 |
| `retained_directory_enumerator_preserves_raw_names_without_dot_entries` | 28 |
| `retained_directory_enumerator_falls_back_without_claiming_descriptor_ownership` | 28 |
| `production_checked_scan_populates_retained_descriptor_capture` | 28 |
| `production_root_replacement_reports_identity_changed` | 28 |
| `production_pin_failure_keeps_rows_and_reports_unavailable` | 28 |
| `action_projection_accepts_raw_root_alias` | 28 |
| `root_observation_capability_is_visible` | 28 |
| Slice B handoff | 110 |
| **Total** | **544** |

The 544-line slice is materially larger than the historical 140-line mechanism anchor because it includes eight measured-cost tests, integration, capability evidence, and custody. Do not force it into 140 lines. If the operator requires a smaller dispatch, split at the bridge-core/worktree seam before editing: the mechanism portion is capped at 224 lines and the integration/evidence portion at 320 lines. Both remain parts of **slice B**; neither inherits the obsolete historical label.

### Operator-owned gates and commit custody

The implement container cannot fetch the pinned `a2a-lf` dependency. The implementer must not claim cargo results from that environment.

The implementation-candidate commit contains the implementation, focused tests, and `docs/superpowers/reviews/2026-08-20-r2f1b-3d-t3a-inc1-sliceB-handoff.md`. The handoff must identify its rebound base, list actual counted lines per worksheet row, distinguish compiler-barrier, runtime-red, characterization, and capability evidence, and contain exactly these six unticked lines:

- [ ] **PENDING OPERATOR** — `cargo fmt --all -- --check`
- [ ] **PENDING OPERATOR** — `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings`
- [ ] **PENDING OPERATOR** — `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast`
- [ ] **PENDING OPERATOR** — `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked root_observation_capability_is_visible -- --nocapture`; record the exact selected-test count and the complete `SLICE_B_ROOT_CAPABILITY_V1=` line
- [ ] **PENDING OPERATOR** — `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation-candidate point
- [ ] **PENDING OPERATOR** — `cargo run -p a2a-bridge -- validate --repo-hygiene` at the completed-handoff point

The host operator runs those gates against the exact candidate, replaces each line only with observed evidence, and creates a handoff-only evidence commit. A blocked gate remains unticked with its actual refusal or error. Never invent an exit status, warning count, test total, capability branch, or classifier result.

### Falsification license

The checked-out repository is authoritative. Before editing, rebind the exact base and re-read the named symbols, signatures, sequencing, tests, and platform guards. If any stated anchor is false, including the classifier precondition above, stop without adapting the task and report the exact source evidence. Do not rename symbols, expand scope, substitute path-derived identity, or reinterpret a failing gate to keep the task moving.

## Acceptance Criteria

- [ ] The operator has supplied a rebound base satisfying the complete-birthtime classifier precondition; otherwise the implementer stopped and reported without editing.
- [ ] Supported targets enumerate from a duplicate of the exact retained descriptor, and only metadata from that retained descriptor can populate `retained_enumeration_object`.
- [ ] Unsupported targets preserve scan behavior while producing `Unavailable`.
- [ ] The independent custody pin and the fresh final named-root descriptor populate their own fields without substituting for the enumeration descriptor.
- [ ] Complete equal captures produce `Pinned`; complete unequal captures produce `IdentityChanged`; any absent capture or tuple member produces `Unavailable`.
- [ ] Root capture changes no selected row, omission, iterator count, refusal, assessment, stored decision, or projection, and all ten existing characterization scenarios remain green.
- [ ] The eight focused tests above exist, stay within their individual 28-line caps, and each new production behavior has a pre-change compiler barrier or behavioral-red witness.
- [ ] The targeted capability probe emits the required machine-readable identity, capability, expectation, and observed-result record.
- [ ] Every scope fence holds and no manifest or lockfile changes.
- [ ] The implementation-candidate handoff contains actual per-row counts and exactly six unticked `PENDING OPERATOR` gate lines.
- [ ] Only the host operator fills gate evidence and creates the handoff-only evidence commit.

## Files

Expected slice B modifications:

- `crates/bridge-core/src/fs_custody.rs`
- `crates/bridge-worktree/src/sweep/checked_scan.rs`
- `docs/superpowers/reviews/2026-08-20-r2f1b-3d-t3a-inc1-sliceB-handoff.md`

Repository anchors that must remain unchanged within slice B:

- `crates/bridge-worktree/src/sweep.rs`
- `crates/bridge-worktree/src/sweep/report.rs`
- `crates/bridge-worktree/src/custody.rs`
- `crates/bridge-worktree/tests/r2f1b_exact_absence_report_api.rs`
- `Cargo.toml`
- `Cargo.lock`
- `crates/bridge-core/Cargo.toml`
- `crates/bridge-worktree/Cargo.toml`

## Spec Refs

- `docs/superpowers/plans/2026-08-02-r2f1b-focused-boundary.md`, especially §2.2’s descriptor-bound exact-absence requirement.
- `docs/superpowers/reviews/2026-08-20-r2f1b-3d-t3a-inc1-sliceA2a-1-handoff.md`, for the checked-scan engine and deferred descriptor-owned observation obligation.
- `docs/superpowers/reviews/2026-08-20-r2f1b-3d-t3a-inc1-sliceA2a-2-handoff.md`, for the ten preserved characterization scenarios and separator divergence.
- `docs/superpowers/reviews/2026-08-20-r2f1b-3d-t3a-inc1-sliceA2b-handoff.md`, for the merged report return, deferred F8 capability evidence, and production-default observation boundary.
- `crates/bridge-core/src/fs_custody.rs`, for `BirthTimeV1`, `PinnedDirectoryV1`, descriptor identity, and existing directory-stream machinery.
- `crates/bridge-worktree/src/sweep/checked_scan.rs`, for the production session and sole checked-scan driver.
- `crates/bridge-worktree/src/sweep.rs`, for the repository-authoritative classifier and action/exact routes.

## Commit Message

feat(fs-custody): populate exact-absence root observations (T3a inc1, slice B)
