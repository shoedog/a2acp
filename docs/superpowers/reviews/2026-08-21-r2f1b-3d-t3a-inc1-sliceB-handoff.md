# Slice B handoff — retained enumeration-root observations

## Candidate checkpoint

- Base tree: `9ce2074ef2a4e7b7bb81b9561b79ba672f9db9db` (`origin/main` slice A2b).
- Pre-edit `git status --short` was empty; the implementation began from a clean tree.
- This is an implementation candidate only. Cargo gates and the F8 probe are operator-owned and were not run in this container.

## Pre-edit checkpoint and disposition

| Anchor | Repository evidence | Disposition |
| --- | --- | --- |
| Production hole | `crates/bridge-worktree/src/sweep/checked_scan.rs`, `CompatibilityCheckedScanRootSessionV1::finish`, returned `RootObservationSetV1::default()` | Implement population here. |
| Scan order | `CompatibilityCheckedScanSourceV1::open` opened `read_dir` before `CompatibilityPinOpenerV1::open_pin` | Preserve the order; retained enumeration opens first. |
| Descriptor support | `crates/bridge-core/src/fs_custody.rs`, `DirectoryStreamV1` and `errno_location` already existed under the Linux/macOS gate | Reuse both; do not edit `enumerate_directory_names`. |
| Capture/classifier shape | `RootObservationSetV1`, `RootIdentityCaptureV1`, and `classify_root_observations` matched the task description | Populate only; leave the classifier untouched. |
| Pin identity | `PinnedDirectoryV1::identity()` exposes `dev`, `ino`, and `btime` | Copy it only into `pinned_custody_directory`. |
| Capability boundary | `BirthTimeV1::from_metadata` already maps unsupported creation time to `None` | Observe availability through F8; do not infer it. |

Proceed decision: all factual anchors matched the base. The B1 measured subtotal is within its cap, so this candidate includes B1 and B2 rather than stopping after the behavior-neutral primitive.

## Implementation

- `RetainedDirectoryEnumerationV1` retains one directory descriptor on Linux/macOS, enumerates through an `F_DUPFD_CLOEXEC` duplicate owned by `fdopendir`, and obtains identity only with `fstat` through `File::metadata()` on the retained descriptor.
- It uses `O_DIRECTORY | O_CLOEXEC | O_NONBLOCK`, deliberately omits `O_NOFOLLOW`, skips only `.` and `..`, has no child cap or name filter, and surfaces each `readdir` error lazily. Its fallback wraps `std::fs::ReadDir` and exposes no retained identity.
- The checked scan now retains the raw enumeration root, opens retained enumeration before the pin opener, and fills retained, pinned, and final-named captures in `finish`. The final named capture uses following `std::fs::metadata`.
- `exact_absence_sweep_reports_the_stored_runtime_decision` is the sole amended existing assertion: healthy production roots now report `Pinned`, `has_authoritative_scan()` is true, and `effective().count()` remains zero.
- Changed files: `crates/bridge-core/src/fs_custody.rs`, `crates/bridge-worktree/src/sweep/checked_scan.rs`, `crates/bridge-worktree/src/sweep.rs`, this handoff, and the frozen control patch.
- `Cargo.toml`, `Cargo.lock`, `crates/bridge-core/Cargo.toml`, and `crates/bridge-worktree/Cargo.toml` are unchanged.

## Tests and evidence classification

| Test | Evidence category | Basis |
| --- | --- | --- |
| `retained_enumeration_matches_read_dir_selection_and_order` | Compiler-only evidence | New primitive is absent on the base; fixture covers order, dotfile, and non-UTF-8 selection after compilation. |
| `retained_enumeration_identity_is_the_object_the_names_came_from` | Compiler-only evidence | New primitive is absent on the base; replacement fixture distinguishes retained-descriptor metadata from path metadata. |
| `retained_enumeration_has_no_child_cap` | Compiler-only evidence | New primitive is absent on the base; fixture exceeds the existing 4096 cap. |
| `retained_enumeration_follows_a_symlinked_root_like_read_dir` | Compiler-only evidence | New primitive is absent on the base; fixture locks final-symlink following. |
| `retained_enumeration_refuses_a_non_directory_without_blocking` | Compiler-only evidence | New primitive is absent on the base; regular-file and FIFO fixture lock `O_DIRECTORY`/`O_NONBLOCK`. |
| `production_scan_populates_all_three_root_captures` | Genuine runtime red | The frozen control asserts all three base-session captures and fails because base `finish` returns default observations. |
| `retained_capture_is_not_the_pin_and_not_path_metadata` | Genuine runtime red | The frozen control replaces the root after opening; base lacks a retained capture and fails during runtime observation. |
| `pin_failure_leaves_the_root_observation_unavailable` | Genuine runtime red | The frozen control's pin-success baseline requires `Pinned`; base default observations classify as `Unavailable`. |
| `root_capture_birthtime_capability_is_homogeneous_across_the_three_captures` | Genuine runtime red | The frozen F8 control unwraps the retained capture; base default observations make that a runtime panic. |
| `exact_absence_sweep_reports_the_stored_runtime_decision` | Characterization | End-to-end report amendment locks the intentional `Unavailable` to `Pinned` change and policy double gate. |

The frozen, test-only genuine-red control is [`2026-08-21-r2f1b-3d-t3a-inc1-sliceB-genuine-red-control.patch`](2026-08-21-r2f1b-3d-t3a-inc1-sliceB-genuine-red-control.patch), against tree `9ce2074ef2a4e7b7bb81b9561b79ba672f9db9db`, SHA-256 `41adfca294eb1a0a08b9f00a1774deb386df208dd1a567c1072aeef3b7ccae19`. Git generated its exact hunk and its four test-only oracles reproduce the production, retained-replacement, pin-success, and F8 base failures. The reproducible control command is:

```text
git apply docs/superpowers/reviews/2026-08-21-r2f1b-3d-t3a-inc1-sliceB-genuine-red-control.patch
CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked slice_b_control_ -- --nocapture
```

The control was not applied or run here; no runtime-red outcome is claimed.

## Classifier, capability, and CI boundaries

`classify_root_observations` is unchanged. The ruling remains A2b's strict whole-capture equality, including birthtime: if the operator's F8 observation is homogeneous, keep that rule in force. No F8 observation was made in this container, so this candidate does not fabricate a ruling from an unobserved branch. If F8 reports mixed birthtime availability, stop at B2, report the false `IdentityChanged` risk, and do not edit the classifier in this slice.

Linux and macOS provide descriptor-owned enumeration. Every other target uses `ReadDir`, returns no retained identity, and therefore classifies the root as `Unavailable`; that is supported. Uniform birthtime absence is also supported and classifies matching `dev`/`ino` captures as `Pinned`. Only mixed availability is the stop condition.

CI proves the full workspace only on `ubuntu-latest`. Its `macos-14` job runs only `bridge-store`; `windows-latest` runs one `bridge-store` test. Thus Windows compiles the fallback but does not execute it, and CI does not execute bridge-worktree tests on macOS. The operator host is the macOS/APFS observation point.

Inherited open items remain unchanged: a persistent `readdir` error can remain non-latching, as `ReadDir` does, and the Unix-only separator guard in `is_custody_record_name` remains deliberately unrepaired.

`has_authoritative_scan()` can now be true for a healthy complete production scan. Therefore `EXACT_ABSENCE_POLICY_READY_V1 == false` is the sole remaining production gate for `effective()`; readiness was not changed and the amended test still proves an empty effective iterator.

## OPERATOR EVIDENCE — PENDING

- [ ] `cargo fmt --all -- --check` — PENDING OPERATOR
- [ ] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — PENDING OPERATOR
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — PENDING OPERATOR
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — PENDING OPERATOR
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` (implementation point) — PENDING OPERATOR
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` (handoff point) — PENDING OPERATOR

## OPERATOR PROBE — PENDING

- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked root_capture_birthtime_capability_is_homogeneous_across_the_three_captures -- --exact --nocapture` — record the `SLICE-B-F8` line and apply the classifier stop condition above.

## Final counted-line worksheet

Counts are added nonblank physical lines measured from this candidate before the operator fmt gate; remeasure after fmt before evidence handoff. No row exceeds its cap.

| Component | Estimate | Candidate lines | Cap |
| --- | ---: | ---: | ---: |
| B1-1 descriptor-owned primitive | 75 | 71 | 105 |
| B1-2 fallback arm | 30 | 19 | 45 |
| B1-3 bridge-core tests | 140 | 117 | 185 |
| B1 subtotal | 245 | 207 | 335 |
| B2-1 checked-scan population | 55 | 46 | 80 |
| B2-2 focused worktree tests and amendment | 175 | 117 | 220 |
| B2-3 F8 test and visible probe artifact | 40 | 33 | 65 |
| B2-4 frozen genuine-red control | 55 | 57 | 90 |
| B2-5 handoff | 110 | 71 | 145 |
| B2 subtotal | 435 | 324 | 600 |
| Total | 680 | 531 | 935 |
