---
task-type: implement
---

# Slice B — populate the sweep root observations from a retained enumeration descriptor

## Description

T3a increment 1 landed in three merged slices. `sweep_orphans_with_exact_absence`
now returns a populated `ExactAbsenceSweepReportV1`, but every report it produces
carries `CustodyRootObservationV1::Unavailable`, because production never captures
the three root identities the classifier consumes.

**Slice B closes exactly that hole**, and nothing else.

### A naming collision, resolved

The A2a-1 spec assigned this work to "A2b". The A2b slice as dispatched and merged
(`9ce2074e`) deliberately did not do it: it changed the return type and left root
population to a later slice, recording under *Preserved safety boundaries* that
"Production `CompatibilityCheckedScanRootSessionV1::finish` still returns
`RootObservationSetV1::default()`". The label moved; the obligation did not.

This work is **slice B**. Every reference in
`docs/superpowers/plans/2026-08-19-r2f1b-3d-t3a-inc1-sliceA2a-task.md` §"Enumeration-descriptor
ownership decision" that assigns descriptor-owned enumeration to "A2b" means this
slice. Use one name.

---

## What is actually at your base — verified, with three operator claims refuted

Everything in this section was read on `origin/main` at `9ce2074e`. Three claims
carried in the authoring input are **false** and are marked as such; the spec is
written against what the code does.

### Verified true

| Anchor | Evidence at your base |
|---|---|
| The single production hole | `crates/bridge-worktree/src/sweep/checked_scan.rs`: `impl CheckedScanRootSessionV1 for CompatibilityCheckedScanRootSessionV1 { … fn finish(self: Box<Self>) -> RootObservationSetV1 { RootObservationSetV1::default() } }` |
| Observation shape | `RootObservationSetV1 { retained_enumeration_object, pinned_custody_directory, final_named_root }`, each `Option<RootIdentityCaptureV1>`; `RootIdentityCaptureV1 { dev: Option<u64>, ino: Option<u64>, birthtime: Option<BirthTimeV1> }`. Both `pub(super)`, both `Copy`, `PartialEq`, `Eq`. |
| How the session enumerates today | `CompatibilityCheckedScanSourceV1::open` calls `std::fs::read_dir(enumeration_root)` first and maps its failure to `CheckedScanOpenRefusalV1::CannotEnumerate`; only then calls `self.pin_opener.open_pin(enumeration_root)`. The session stores `names: std::fs::ReadDir` and `custody_root: Option<PinnedDirectoryV1>`. It stores **no path**. |
| `ReadDir` exposes no identity | Confirmed by compilation: `std::fs::ReadDir` has no `as_raw_fd` (`error[E0599]: no method named 'as_raw_fd' found for struct 'ReadDir'`). There is no stable std API to recover the descriptor a `ReadDir` is enumerating. |
| The custody pin is a different descriptor | `FilesystemCompatibilityPinOpenerV1::open_pin` → `PinnedDirectoryV1::open(path, "worktree sweep root")`, which canonicalizes the path, calls `open_directory_no_follow` (its own `open(2)`), and brackets it with two `directory_path_identity` stats. Its descriptor is opened independently and later than the enumeration descriptor. |
| `BirthTimeV1::from_metadata` | `metadata.created().ok().and_then(Self::from_system_time)` — `None` on any target or filesystem without creation-time support, and never an error. |
| The classifier does not use `matches` | `classify_root_observations` compares whole `RootIdentityCaptureV1` values with `==`. It deliberately does not call `DirectoryIdentityV1::matches`, whose `(Some, Some) => eq, _ => true` birthtime arm would let an absent birthtime pass as agreement. |
| A2b's classifier tests exist | `root_observation_classifier_reports_pinned_captures`, `…reports_identity_changes_including_birthtime`, `…refuses_incomplete_captures` in `crates/bridge-worktree/src/sweep.rs`. |

### REFUTED — claim 1: "the classifier requires three complete `(dev, ino, birthtime)` tuples"

It does not. The production source is:

```rust
fn root_capture_has_object_identity(capture: checked_scan::RootIdentityCaptureV1) -> bool {
    capture.dev.is_some() && capture.ino.is_some()
}
```

`classify_root_observations` returns `Unavailable` when any of the three captures is
`None`, or when any capture lacks `dev` **or** `ino`. **Birthtime is never required.**
It participates only through whole-value equality.

### REFUTED — claim 2: "on a birthtime-less filesystem the correct result is `Unavailable`"

It is `Pinned`. A2b's own test proves it:

```rust
fn root_observation_classifier_reports_pinned_captures() {
    let capture = root_capture(Some(1), Some(2), None);   // birthtime absent
    assert_eq!(classify_root_observations(root_observations(capture, capture, capture)),
               CustodyRootObservationV1::Pinned);
}
```

Uniform birthtime absence with equal `dev`/`ino` classifies as `Pinned`. The real
hazard is the opposite one, and A2b named it: *"a present-versus-absent birthtime is
`IdentityChanged`. … future real-observation population must reconsider this policy
explicitly."* **Mixed** birthtime availability across the three captures — not absent
availability — is what silently manufactures a false `IdentityChanged`. That is what
your F8 evidence must discriminate; see *The filesystem-capability boundary* below.

#### The classifier-policy stop condition

An independent author reviewing this same work reached the opposite disposition and
would have blocked the slice outright: it read the capability boundary as requiring
`Unavailable` whenever birthtime is absent, judged the shipped classifier wrong on that
point, and refused to begin until the operator supplied a base where all three
`(dev, ino, birthtime)` members are required for every capture.

That disposition is **not adopted**, because the falsification license makes the
repository authoritative and the boundary claim it relied on was an operator error,
corrected above. Uniform birthtime absence yielding `Pinned` is the shipped, tested
behavior and slice B preserves it.

But the underlying question is real and A2b deferred it in writing, so it gets a
**conditional stop** rather than silence. Slice B is population-only and **may not
broaden itself into a classifier correction**. `classify_root_observations` is not to be
edited here under any finding.

**Stop and report — do not edit the classifier, and do not proceed past B2 — if the F8
observation shows mixed birthtime availability across the three captures on any required
platform.** That result means production can manufacture an `IdentityChanged` with no
identity change, which is a false positive in a custody-proof path, and deciding between
these four candidate rules is a design question outside this slice:

- any absent capture or absent tuple member yields `Unavailable`;
- `Unavailable` takes precedence over any mismatch;
- three complete equal tuples yield `Pinned`;
- three complete tuples with any inequality yields `IdentityChanged`.

If the F8 observation instead shows homogeneous availability — all three `Some`, or all
three `None` — record that, record the explicit ruling that A2b's strict-equality policy
stands, and continue. **The observation decides; neither the implementer nor this
specification decides in advance.**

Any correction to the classifier is a separate slice with its own review, and is outside
this sizing worksheet.

### REFUTED — claim 3: "build a bridge-core retained-directory enumerator"

Most of it already exists. `crates/bridge-core/src/fs_custody.rs` already contains,
`#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]`:

- `struct DirectoryStreamV1(*mut libc::DIR)` with a `closedir` `Drop`;
- `fn errno_location()` (`__errno_location` on Linux, `__error` on macOS);
- `pub(crate) fn enumerate_directory_names(directory: &File, limit: usize, label: &str)`,
  which does `fcntl(fd, F_DUPFD_CLOEXEC, 0)` → `fdopendir` → `rewinddir` →
  errno-cleared `readdir` loop → `CStr::from_ptr((*entry).d_name.as_ptr())`, skipping
  `.` and `..`;
- a `#[cfg(not(all(unix, any(target_os = "linux", target_os = "macos"))))]` arm
  returning `FsCustodyError::Unsupported`.

`libc` is already a dependency of `bridge-core` (`libc = { workspace = true }`), so
**no manifest or lockfile change is required anywhere.** `libc` is *not* a dependency
of `bridge-worktree` and this design does not need it there.

But `enumerate_directory_names` **cannot be reused as-is**, because four of its
properties would change scan behavior:

1. it is **eager** — it returns `Vec<OsString>`, while `ReadDir` is lazy and the engine
   interleaves `read_legacy` / `read_custody` with iteration;
2. it enforces a **child cap** and returns `EnumerationLimitExceeded`; `read_dir` has
   no cap, so a large sweep root that enumerates today would refuse;
3. it runs `validated_child_name` over every name and fails the whole enumeration on
   refusal; `ReadDir` applies no such filter;
4. a `readdir` error aborts the **entire** enumeration with `Err`, whereas
   `ReadDir::next` yields `Some(Err(_))` per entry, which the engine counts into
   `iterator_error_count` and the report renders as
   `ExactAbsenceEnumerationV1::Incomplete { skipped_entries }`. Adopting it would turn
   a mid-stream entry error into a whole-root `CannotEnumerate` refusal with zero rows.

So slice B adds a **streaming sibling** that reuses `DirectoryStreamV1` and
`errno_location`, and leaves `enumerate_directory_names` and its five call sites
(`fs_custody.rs` ×3, `namespace_transaction.rs` ×2) untouched.

---

## The descriptor-ownership requirement

Carried verbatim from A2a-1 and binding here:

> The field may contain an identity only when it was captured from the exact retained
> directory descriptor whose duplicated descriptor drives name enumeration. Identity
> read from the root path, from the separate custody pin, or from a descriptor that did
> not drive enumeration does not satisfy the field.

Concretely, `retained_enumeration_object` is satisfied **only** by `fstat` on a
descriptor `R` such that the names the scan consumed were read from a *duplicate* of
`R` — a descriptor obtained by `dup`/`F_DUPFD_CLOEXEC`/`File::try_clone`, which shares
`R`'s open file description and therefore provably names the same directory object. A
re-open by path (including `/proc/self/fd/N` or `/dev/fd/N`) is **not** a duplicate: it
re-resolves the name and can land on a different object. Path metadata (`stat`,
`symlink_metadata`, `DirEntry::metadata`) and `PinnedDirectoryV1::identity()` do **not**
satisfy the field, in any combination.

Test `retained_capture_is_not_the_pin_and_not_path_metadata` (below) is the
discriminator: an implementation that fills the field from path metadata or from the
pin cannot pass it.

### The mechanism, named

Add to `crates/bridge-core/src/fs_custody.rs`:

```rust
pub struct RetainedObjectIdentityV1 { pub dev: u64, pub ino: u64, pub birthtime: Option<BirthTimeV1> }

pub struct RetainedDirectoryEnumerationV1 { /* private */ }

impl RetainedDirectoryEnumerationV1 {
    pub fn open(path: &Path) -> Result<Self, std::io::Error>;
    pub fn retained_object_identity(&self) -> Option<RetainedObjectIdentityV1>;
    pub fn next_name(&mut self) -> Option<Result<OsString, std::io::Error>>;
}
```

Descriptor-owned arm, gated exactly `#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]`
to match the existing gate in the same file:

- **`open`** — `OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NONBLOCK)`,
  keeping the returned `File` as the retained descriptor `R`. These are the flags
  `opendir(3)` uses.
  **`O_NOFOLLOW` must not be set, and `open_directory_no_follow_raw` must not be used
  here.** `std::fs::read_dir` follows a final symlink, and the action route passes the
  **raw, uncanonicalized** root — `scan_worktree_records_with_pin_opener` calls
  `scan_compatibility_with_pin_opener(Path::new(root), …)`. Adding `O_NOFOLLOW` would
  refuse symlinked roots the sweep accepts today. `O_DIRECTORY` reproduces `opendir`'s
  `ENOTDIR` refusal for a non-directory, and `O_NONBLOCK` keeps a FIFO substituted for
  the root from blocking the calling thread — the same reasoning `ChildOpenOptionsV1`
  already documents in this file.
- **duplicate** — `libc::fcntl(R.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0)`, then
  `libc::fdopendir(duplicate)`, then `libc::rewinddir`. `fdopendir` takes ownership of
  the descriptor it is given and `closedir` closes it, so it must be handed the
  duplicate, never `R`. On `fdopendir` failure, `libc::close` the duplicate and return
  the OS error. Wrap the stream in the existing `DirectoryStreamV1` so `Drop` closes it.
  Because the duplicate shares `R`'s file offset, `R` must be used only for `fstat` —
  never read.
- **`retained_object_identity`** — `R.metadata()` (an `fstat` on the retained
  descriptor), then `MetadataExt::dev()`, `MetadataExt::ino()`, and
  `BirthTimeV1::from_metadata`. `None` only if the `fstat` itself fails.
- **`next_name`** — one `readdir` per call, lazily: clear errno through
  `errno_location()`, call `libc::readdir`, and on NULL distinguish end-of-stream
  (errno `0` → `None`) from an entry error (errno ≠ 0 → `Some(Err(from_raw_os_error))`).
  Skip `.` and `..`. Apply **no** child cap and **no** `validated_child_name` filter.
  Do **not** latch after an error: `std::fs::ReadDir` does not, and preserving today's
  behavior means preserving that too (recorded as an open item below).

Fallback arm, `#[cfg(not(all(unix, any(target_os = "linux", target_os = "macos"))))]`:
wrap `std::fs::ReadDir`, map `next_name` onto `entry.map(|e| e.file_name())`, and return
`None` from `retained_object_identity`. This is what satisfies "leaves the observation
unavailable on any target where descriptor-owned enumeration cannot be provided without
changing scan behavior" — and it keeps `crates/bridge-worktree` free of any `cfg` for
this field.

Empirically verified on this host before writing this spec: opening a directory,
`F_DUPFD_CLOEXEC`-duplicating it, `fdopendir`-ing the duplicate and looping `readdir`
yields byte-identical names in identical order to `std::fs::read_dir` over the same
directory, and the retained descriptor remains `fstat`-able after `closedir`.

### Worktree integration

In `crates/bridge-worktree/src/sweep/checked_scan.rs`:

- replace the session's `names: std::fs::ReadDir` with the new primitive, constructed in
  `CompatibilityCheckedScanSourceV1::open` **before** `open_pin` — the ordering is
  pinned by `compatibility_open_refusal_never_calls_pin_opener`. Map its `open` failure
  to `CheckedScanOpenRefusalV1::CannotEnumerate`, exactly as `read_dir`'s failure maps
  today;
- retain the enumeration root on the session (`enumeration_root: PathBuf`), because
  `finish` needs it and the session stores no path today;
- populate `finish` with all three captures:
  - `retained_enumeration_object` — from `retained_object_identity()`; `None` maps
    straight through to a `None` field;
  - `pinned_custody_directory` — from `self.custody_root.as_ref().map(PinnedDirectoryV1::identity)`,
    copying `dev`, `ino`, `btime`. A failed pin stays `None`, which is what preserves the
    independent custody pin-failure behavior;
  - `final_named_root` — re-resolve the retained enumeration root with
    **`std::fs::metadata`** (follows), not `symlink_metadata`. Enumeration resolved the
    name by following the final symlink, so the end-of-scan re-resolution must follow
    too; `symlink_metadata` on a symlinked root would compare a symlink inode against a
    directory inode and report `IdentityChanged` on every scan. Use the
    `#[cfg(unix)] { use std::os::unix::fs::MetadataExt as _; … } #[cfg(not(unix))] { … }`
    pattern already present in `crates/bridge-worktree/src/sweep.rs`
    (`recorded_identity_matches_sibling`), so no new dependency and no new import style
    is introduced.

Do **not** modify `classify_root_observations`.

### What this changes downstream, and what it does not

Selection, omission, ordering, decisions, `iterator_error_count`, entry contents, and
the report's roots and entries are all unchanged. Two things do change, both
intentionally:

1. `ExactAbsenceScanStatusV1::custody_root()` becomes `Pinned` for a healthy root
   instead of always `Unavailable`.
2. **Therefore `ExactAbsenceSweepReportV1::has_authoritative_scan()` — a public method
   that returns `false` on every production input today — can now return `true`.** Read
   `report.rs`: `has_authoritative_scan` is `enumeration == Complete && custody_root == Pinned`,
   and `entry_is_effectively_authorized_for_policy` is
   `policy_ready && self.has_authoritative_scan() && …`. Production `effective()` is
   double-gated today; after slice B, `EXACT_ABSENCE_POLICY_READY_V1 == false` is the
   **sole** remaining gate. State this in the handoff and prove `effective().count() == 0`
   in a test on a real root that classifies `Pinned`.

---

## The filesystem-capability boundary, and F8

`Metadata::created()` errors on platforms and filesystems without creation-time
support, so `BirthTimeV1::from_metadata` legitimately yields `None`. Declare the
boundary as it actually behaves:

- **Descriptor-owned enumeration** is supported on `linux` and `macos` under `cfg(unix)`.
  Everywhere else the primitive falls back to `std::fs::read_dir`,
  `retained_object_identity()` is `None`, and the classification is `Unavailable`. That
  is a **supported outcome**, not a failure.
- **Uniform birthtime absence is not a failure either**: with equal `dev`/`ino` it
  classifies `Pinned`, per the refutation above.
- **Mixed birthtime availability across the three captures is the failure mode.** It
  yields a spurious `IdentityChanged` with no identity change. All three captures derive
  from the same object via `fstat`/`fstat`/`stat`, so they should agree — that is a
  property to *test*, not to assume.

**F8 (inherited from A2a-1, deferred by A2b, due here).** A capability test that passes
for either `Some` or `None` proves nothing if the observed branch is invisible in
captured output. Discharge F8 with a test that is simultaneously a real assertion and a
visible observation:

`root_capture_birthtime_capability_is_homogeneous_across_the_three_captures`

- builds a real root, drives the production session, and asserts
  `retained.birthtime.is_some() == pinned.birthtime.is_some()` and
  `pinned.birthtime.is_some() == final_named.birthtime.is_some()` — an assertion that
  genuinely fails on a mixed-capability filesystem;
- `eprintln!`s one machine-readable line, prefixed `SLICE-B-F8`, carrying the fixture's
  `dev` and `ino`, the observed `some`/`none` for each of the three captures, and the
  resulting `CustodyRootObservationV1`.

Author the exact probe command in the handoff for the operator to run and record:

```
CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked \
  root_capture_birthtime_capability_is_homogeneous_across_the_three_captures \
  -- --exact --nocapture
```

Record it as a seventh, separately-headed `## OPERATOR PROBE — PENDING` line. Do not
add it to the six-gate block.

**CI coverage boundary, to state honestly in the handoff:** `.github/workflows/ci.yml`
runs the full `cargo test --workspace --locked` only on `ubuntu-latest`. The `macos-14`
job runs `cargo test -p bridge-store` only, and `windows-latest` runs a single
`bridge-store` test — so the non-unix fallback arm is *compiled* on Windows CI and never
executed, and no bridge-worktree test runs on macOS in CI. The operator host is the only
macOS/APFS observation. Do not claim CI proves the macOS or non-unix behavior.

---

## Scope fences

Slice B does **not**:

- set `EXACT_ABSENCE_POLICY_READY_V1` to `true`, or change `effective()` or
  `entry_is_effectively_authorized_for_policy` — that gate and the population-admission
  rule belong to increment 2;
- change `sweep_orphans_with_exact_absence`'s signature or the report vocabulary in
  `crates/bridge-worktree/src/sweep/report.rs`;
- change `classify_root_observations`. A2b's ruling that strict equality including
  birthtime is retained must be **reconsidered explicitly in the handoff** — a written
  ruling resting on the F8 observation — and left in force in code;
- add ownership, locking, transition, unlink, or removal authority. **T3a decides and
  reports; T3b acts.** A later actor must re-open, re-read, re-bind, and re-prove exact
  absence under its own lock regardless of what the report says;
- repair the Unix-only separator guard in `is_custody_record_name`
  (`!stem.is_empty() && !stem.ends_with('/')`), which A2a-2 characterized deliberately
  and left unrepaired;
- modify `enumerate_directory_names`, `DirectoryStreamV1`, `errno_location`, or any of
  their five existing call sites, beyond reusing the latter two;
- add any dependency. `Cargo.toml`, `Cargo.lock`, `crates/bridge-core/Cargo.toml`, and
  `crates/bridge-worktree/Cargo.toml` must be byte-for-byte unchanged.

### Inherited open items — record, do not act

- **The non-latching entry-error loop.** `std::fs::ReadDir::next` does not latch after a
  `readdir` error, and neither may the new primitive; a persistently failing `readdir`
  can therefore spin the engine's `while let Some(name)` loop. This shape is inherited,
  not introduced. Preserve it and record it; repairing it changes behavior and needs its
  own slice.
- **The Unix-only separator guard**, carried forward from A2a-2 and A2b unrepaired.

---

## Behavior that must not change

All ten A2a-2 characterization scenarios in `checked_scan.rs` must still hold, unedited
except where an assertion mechanically names a changed type:

1. `checked_scan_classifier_preserves_full_path_precedence_and_boundaries`
2. `checked_scan_silently_omits_bad_legacy_and_retains_bad_custody`
3. `checked_scan_counts_iterator_errors_and_continues_in_injected_order`
4. `nondefault_root_observations_survive_exact_without_changing_rows_or_decisions`
5. `enumeration_refusal_retains_canonical_root_and_skips_assessment`
6. `action_projection_erases_only_action_metadata`
7. `injected_sources_use_production_action_and_exact_projections`
8. `injected_sources_prove_action_and_exact_projection_equivalence`
9. `report_side_pin_failure_uses_post_canonicalization_opener_seam`
10. `compatibility_pin_failure_preserves_legacy_and_refuses_custody`

Also unchanged: `compatibility_open_refusal_never_calls_pin_opener`,
`checked_scan_reads_each_selected_name_before_next_and_finishes_once`,
`exact_route_pin_failure_preserves_legacy_and_refuses_custody`,
`exact_route_cannot_canonicalize_without_opening_pin`,
`exact_projection_retains_production_computed_decisions`,
`exact_projection_preserves_legacy_and_custody_decision_matrix`,
`unreadable_custody_refuses_without_probe`,
`exact_route_preserves_canonical_scan_root_and_report_return`,
`exact_projection_reports_forced_iterator_errors`.

**Exactly one existing assertion is expected to change**:
`exact_absence_sweep_reports_the_stored_runtime_decision` in
`crates/bridge-worktree/src/sweep.rs` asserts
`report.scan().custody_root() == CustodyRootObservationV1::Unavailable` on a real root.
That flips to `Pinned`. Amend it, extend it to assert `has_authoritative_scan() == true`
and `effective().count() == 0`, and name the amendment in the handoff.
`exact_absence_sweep_reports_cannot_canonicalize` must stay `Unavailable` — the refusal
path constructs no session and calls no `finish`.

If any other existing test changes colour, that is a behavior change: stop and report.

---

## Required tests

**bridge-core** (`crates/bridge-core/src/fs_custody.rs`):

| Test | What it catches |
|---|---|
| `retained_enumeration_matches_read_dir_selection_and_order` | Divergent selection, order, `.`/`..` handling, dotfile omission, or non-UTF-8 name mangling versus `std::fs::read_dir` over the same directory. |
| `retained_enumeration_identity_is_the_object_the_names_came_from` | Filling the identity from anything but the retained descriptor: after `open`, rename the directory away and create a fresh directory at the same path; the retained identity must stay the original object's and the names must still be the original directory's. |
| `retained_enumeration_has_no_child_cap` | Reusing `enumerate_directory_names`: build a root with more than its 4096-child bound and require complete enumeration. |
| `retained_enumeration_follows_a_symlinked_root_like_read_dir` | Introducing `O_NOFOLLOW`: a symlink to a directory must enumerate the target, as `read_dir` does today. |
| `retained_enumeration_refuses_a_non_directory_without_blocking` | Dropping `O_DIRECTORY`/`O_NONBLOCK`: a regular file and a FIFO (`libc::mkfifo`, per the existing precedent in this file) must both refuse promptly rather than block or enumerate. |

**bridge-worktree** (`crates/bridge-worktree/src/sweep/checked_scan.rs`, and the one
amendment in `sweep.rs`):

| Test | What it catches |
|---|---|
| `production_scan_populates_all_three_root_captures` | A `finish` that still returns the default, or that leaves any one field `None` on a healthy Unix root. |
| `retained_capture_is_not_the_pin_and_not_path_metadata` | **The descriptor-ownership discriminator.** After the session opens, replace the root directory (rename away, recreate at the same name); at `finish` the retained capture must still carry the original object's `dev`/`ino` while `final_named_root` carries the new one, classifying `IdentityChanged`. Any implementation sourcing the field from `std::fs::metadata` or from `PinnedDirectoryV1` fails this. |
| `pin_failure_leaves_the_root_observation_unavailable` | Synthesizing a pinned capture when `open_pin` returned `None`; rows and decisions must be identical to the pin-success run. |
| `root_capture_birthtime_capability_is_homogeneous_across_the_three_captures` | Mixed birthtime availability producing a spurious `IdentityChanged`; also discharges F8's visibility duty. |
| `exact_absence_sweep_reports_the_stored_runtime_decision` (amended) | The end-to-end flip to `Pinned`, `has_authoritative_scan() == true`, and `effective()` still empty under `EXACT_ABSENCE_POLICY_READY_V1 == false`. |

Classify every test explicitly as **genuine runtime red**, **compiler-only evidence**,
or **characterization**, and for anything you classify as genuine runtime red supply a
reproducible control: an exact test-only patch frozen against a recorded base tree, that
tree's identity, the patch's SHA-256, and the command that runs it. Do not claim red you
did not observe; you cannot run `cargo` in this container, so a claimed observation is a
fabrication. `retained_capture_is_not_the_pin_and_not_path_metadata` and
`production_scan_populates_all_three_root_captures` are the natural runtime oracles —
they fail on the untouched base for a behavioral reason, not a compilation one.

---

## Handoff and custody

Create `docs/superpowers/reviews/2026-08-21-r2f1b-3d-t3a-inc1-sliceB-handoff.md` and
`docs/superpowers/reviews/2026-08-21-r2f1b-3d-t3a-inc1-sliceB-genuine-red-control.patch`.

**You make the implementation-candidate commit only.** The implement container's egress
cannot fetch the pinned `a2a-lf` dependency, so `cargo` cannot build here. Gate
execution and the handoff-only evidence commit belong to the host operator. Do not
attempt the gates, do not run `git diff --cached --check`, and do not invent totals.
Reporting a gate as blocked is correct behavior; inventing one is not.

Carry these six lines unticked under `## OPERATOR EVIDENCE — PENDING`:

- [ ] `cargo fmt --all -- --check` — PENDING OPERATOR
- [ ] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — PENDING OPERATOR
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — PENDING OPERATOR
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — PENDING OPERATOR
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` (implementation point) — PENDING OPERATOR
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` (handoff point) — PENDING OPERATOR

Plus, separately headed `## OPERATOR PROBE — PENDING`, the F8 `--nocapture` command
above. Add `CARGO_INCREMENTAL=0 cargo test -p bridge-core --locked --no-fail-fast` as a
seventh gate line only if you judge it necessary and say why; the workspace gate already
covers it.

The handoff must record: base identity and clean-tree status; a pre-edit checkpoint
giving each factual anchor's disposition and source location, the per-row estimates, and
the proceed-or-stop decision; every changed file; the test-name-to-evidence-category
table with the frozen control's tree identity and patch hash; the explicit ruling on
A2b's strict-equality classifier policy; the `has_authoritative_scan()` consequence and
that `EXACT_ABSENCE_POLICY_READY_V1` remains `false`; the declared capability boundary
and the CI coverage boundary; the two inherited open items; that no manifest or lockfile
changed; and the final counted-line worksheet.

Do not consult a template outside the repository; none exists there and these inline
requirements are the owner-approved replacement.

---

## Sizing

Counted lines are added nonblank physical lines after the fmt gate, one row per line, no
contingency, no borrowing.

**Per-test cost, measured — not estimated, and the authoring input's figure is low.**
The ten A2a-2 characterization tests in `checked_scan.rs` total **348** nonblank lines
including their `// Evidence:` comments and `#[test]` attributes: **mean 34.8**, range
16–55. A2b's eight report tests in `sweep.rs` total 197: mean 24.6. The circulated
figure of "roughly 28" is below both this module's mean and the blended mean of 30.5.
Plan filesystem-fixture tests at **32** and pure-unit tests at **22**. Re-measure against
your base before editing.

### The reserved anchor is refuted by measurement — split proposal

A2a-1 reserved **140** counted lines for "that bridge-core enumerator, worktree
integration, and focused tests". The honest estimate for exactly that scope is rows B1-1
through B2-2 below: **75 + 30 + 140 + 55 + 175 = 475**, a **3.4×** miss. Compressing
evidence to fit 140 would mean dropping the descriptor-ownership discriminator, the
no-child-cap test, or the symlink-follow test — the three tests that make this slice
falsifiable. Do not do that.

The split, and its trigger: the B1 block touches **no file under
`crates/bridge-worktree/`** and changes no behavior — production root observations stay
`Unavailable` until B2 lands. **If any B1 row exceeds its cap, or the B1 subtotal
exceeds its cap, stop after B1 and hand off B1 alone as a complete, behavior-neutral
slice**, reporting the revised B2 estimate. Otherwise complete both blocks in a single
implementation-candidate commit.

**B1 — bridge-core primitive (no behavior change)**

| Counted component | Estimate | Cap |
|---|---:|---:|
| B1-1 `RetainedDirectoryEnumerationV1` + `RetainedObjectIdentityV1`, descriptor-owned arm | 75 | 105 |
| B1-2 fallback arm for every other target | 30 | 45 |
| B1-3 focused bridge-core tests (5) | 140 | 185 |
| **B1 subtotal** | **245** | **335** |

**B2 — worktree population**

| Counted component | Estimate | Cap |
|---|---:|---:|
| B2-1 `checked_scan.rs` session swap, root retention, `finish` population | 55 | 80 |
| B2-2 focused bridge-worktree tests (4 new + 1 amended) | 175 | 220 |
| B2-3 F8 capability probe and its recorded artifact | 40 | 65 |
| B2-4 genuine-red control: frozen patch, recorded identity, documentation | 55 | 90 |
| B2-5 slice B handoff | 110 | 145 |
| **B2 subtotal** | **435** | **600** |
| **Total** | **680** | **935** |

For calibration: A2b measured 430 against a 610 cap and converged.

---

## Acceptance Criteria

Gates are operator-owned; these are the conditions you are responsible for.

1. `RetainedDirectoryEnumerationV1` exists in `crates/bridge-core/src/fs_custody.rs`,
   retains one directory descriptor, and drives `next_name` from a
   `F_DUPFD_CLOEXEC` duplicate of that same descriptor — not a re-open by path.
2. `retained_object_identity` reads `fstat` on the retained descriptor only. No code
   path fills `retained_enumeration_object` from `std::fs::metadata`,
   `std::fs::symlink_metadata`, `DirEntry::metadata`, or `PinnedDirectoryV1`.
3. The primitive matches `std::fs::read_dir` on selection, order, laziness, `.`/`..`
   exclusion, per-entry error surfacing, symlink following, non-directory refusal, and
   the absence of any child cap or name filter. It does not use `O_NOFOLLOW` and does
   not call `open_directory_no_follow_raw`.
4. `enumerate_directory_names`, `DirectoryStreamV1`, `errno_location`, and their five
   existing call sites are unmodified; `DirectoryStreamV1` and `errno_location` are
   reused rather than duplicated.
5. The fallback arm compiles on every non-`(linux|macos)` target, wraps
   `std::fs::ReadDir`, and returns `None` from `retained_object_identity`, so the
   observation is `Unavailable` there.
6. `CompatibilityCheckedScanRootSessionV1::finish` populates all three captures:
   retained descriptor `fstat`; the custody pin's `identity()` when the pin opened, and
   `None` when it did not; and the enumeration root re-resolved with `std::fs::metadata`.
7. The enumeration descriptor is opened before the pin opener is consulted, and an open
   failure still yields `CheckedScanOpenRefusalV1::CannotEnumerate` without calling the
   pin opener.
8. All ten A2a-2 characterization scenarios and the nine other pre-existing
   `checked_scan.rs` tests are unchanged and green; exactly one pre-existing assertion
   changes, in `exact_absence_sweep_reports_the_stored_runtime_decision`, and it is named
   and justified.
9. `classify_root_observations` is unchanged — under any finding, including a finding
   that its policy is wrong — and the handoff carries an explicit written ruling on
   A2b's strict-equality policy grounded in the F8 observation. If that observation shows
   mixed birthtime availability on any required platform, the run stopped and reported
   under the classifier-policy stop condition instead of proceeding or editing.
10. `EXACT_ABSENCE_POLICY_READY_V1` remains `false`, `effective()` and
    `entry_is_effectively_authorized_for_policy` are unchanged, and a test proves
    `effective().count() == 0` on a root that classifies `Pinned`.
11. The `has_authoritative_scan()` consequence — that readiness becomes the sole
    remaining production gate — is stated in the handoff and covered by a test.
12. Every required test above exists, each classified as genuine runtime red,
    compiler-only evidence, or characterization, with no misclassification; the
    genuine-red control exists with a recorded base tree, patch SHA-256, and run command.
13. F8 is discharged: the homogeneity assertion is real, and the observed branch is
    visible through the `SLICE-B-F8` line and the operator probe command.
14. The capability boundary and the CI coverage boundary are declared; the non-latching
    entry-error loop and the Unix-only separator divergence are carried forward as open
    items and not repaired.
15. No dependency, manifest, or lockfile change: `Cargo.toml`, `Cargo.lock`,
    `crates/bridge-core/Cargo.toml`, and `crates/bridge-worktree/Cargo.toml` are
    byte-for-byte unchanged.
16. The handoff exists with the six `PENDING OPERATOR` lines unticked plus the separate
    operator probe line, and exactly one implementation-candidate commit exists — or the
    B1 stop rule fired and the run reported instead.
17. Every counted worksheet row and each subtotal is within cap, or the run stopped and
    reported the revised estimate.

Do not claim any gate result. Do not tick a pending box.

---

## Files

- `crates/bridge-core/src/fs_custody.rs` — add `RetainedDirectoryEnumerationV1`,
  `RetainedObjectIdentityV1`, both `cfg` arms, and the five focused tests. Reuse
  `DirectoryStreamV1` and `errno_location`; leave `enumerate_directory_names` alone.
- `crates/bridge-worktree/src/sweep/checked_scan.rs` — session field swap, enumeration-root
  retention, `finish` population, and the four new focused tests.
- `crates/bridge-worktree/src/sweep.rs` — read-only except the single amendment to
  `exact_absence_sweep_reports_the_stored_runtime_decision`. `classify_root_observations`
  must not change. Its `recorded_identity_matches_sibling` is the `cfg(unix)` metadata
  pattern to mirror.
- `crates/bridge-worktree/src/sweep/report.rs` — read-only; the vocabulary,
  `has_authoritative_scan`, `effective`, and `EXACT_ABSENCE_POLICY_READY_V1` are settled.
- `crates/bridge-core/src/namespace_transaction.rs` — read-only reference for the two
  existing `enumerate_directory_names` call sites.
- `bin/a2a-bridge/src/main.rs` — read-only reference for the five `sweep_orphans` boot
  callers; change only if mechanically required, and justify it.
- `.github/workflows/ci.yml` — read-only reference for the coverage boundary.
- `docs/superpowers/reviews/2026-08-21-r2f1b-3d-t3a-inc1-sliceB-handoff.md` — create.
- `docs/superpowers/reviews/2026-08-21-r2f1b-3d-t3a-inc1-sliceB-genuine-red-control.patch` — create.
- `Cargo.toml`, `Cargo.lock`, `crates/bridge-core/Cargo.toml`,
  `crates/bridge-worktree/Cargo.toml` — must not change.

---

## Spec Refs

Authoritative at your base commit:

- `crates/bridge-worktree/src/sweep/checked_scan.rs` —
  `CompatibilityCheckedScanSourceV1::open`, `CompatibilityCheckedScanRootSessionV1::finish`,
  `RootObservationSetV1`, `RootIdentityCaptureV1`, `CompatibilityPinOpenerV1`
- `crates/bridge-worktree/src/sweep.rs` — `classify_root_observations`,
  `root_capture_has_object_identity`, `ExactScanOutcomeV1::into_report`,
  `scan_worktree_records_with_pin_opener`, `sweep_orphans_with_exact_absence_with_pin_opener`,
  `recorded_identity_matches_sibling`
- `crates/bridge-worktree/src/sweep/report.rs` — `EXACT_ABSENCE_POLICY_READY_V1`,
  `has_authoritative_scan`, `entry_is_effectively_authorized_for_policy`,
  `ExactAbsenceScanStatusV1`, `CustodyRootObservationV1`
- `crates/bridge-worktree/src/custody.rs` — `is_custody_record_name`
- `crates/bridge-core/src/fs_custody.rs` — `BirthTimeV1::from_metadata`,
  `DirectoryIdentityV1::matches`, `PinnedDirectoryV1::open` / `identity`,
  `open_directory_no_follow_raw`, `directory_identity`, `directory_path_identity`,
  `enumerate_directory_names`, `DirectoryStreamV1`, `errno_location`,
  `open_child_no_follow`, `ChildOpenOptionsV1`
- `crates/bridge-core/Cargo.toml` — `libc` is already a dependency here and not in
  `crates/bridge-worktree/Cargo.toml`
- `docs/superpowers/plans/2026-08-19-r2f1b-3d-t3a-inc1-sliceA2a-task.md` —
  "Enumeration-descriptor ownership decision"; the field's meaning; the 140-line
  reservation; F8-of-A2
- `docs/superpowers/reviews/2026-08-20-r2f1b-3d-t3a-inc1-sliceA2b-handoff.md` — the
  deferral of root population, F8's deferral, and the strict-equality classifier ruling
  that this slice must reconsider explicitly
- `.github/workflows/ci.yml` — the ubuntu/macOS/Windows job scope

---

## Commit Message

feat(worktree): populate the sweep root observations from a retained descriptor

Add `RetainedDirectoryEnumerationV1` to bridge-core: it opens and retains one
directory descriptor with the flags `opendir(3)` uses, drives name enumeration
from an `F_DUPFD_CLOEXEC` duplicate of that same descriptor, and exposes `fstat`
on the retained one. Selection, order, laziness, `.`/`..` exclusion, per-entry
error surfacing, symlink following, and the absence of any child cap match
`std::fs::read_dir` exactly; `enumerate_directory_names` is eager, capped and
whole-enumeration-failing, so it is reused for neither.

Move the compatibility checked-scan session onto it and fill all three root
captures in `finish`: the retained enumeration object from the retained
descriptor, the pinned custody directory from the separate custody pin, and the
final named root by re-resolving the enumeration root at end of scan. Production
root classification is no longer always `Unavailable`, so
`has_authoritative_scan()` can now be true and readiness is the sole remaining
gate.

`EXACT_ABSENCE_POLICY_READY_V1` stays false and `effective()` stays empty. The
report still carries ordered historical evidence, not authority — a later actor
must re-open, re-read, re-bind, and re-prove exact absence under its own lock.

No dependency, manifest, or lockfile change.

---

## Falsification license

Every symbol, signature, flag, line count, and behavioral statement above is a claim
measured against your base. **The repository is authoritative.** Three claims circulated
with this work have already been refuted here — the classifier's birthtime requirement,
the birthtime-less outcome, and the assertion that no retained-descriptor enumerator
exists in bridge-core — so treat the rest as fallible in the same way.

If `finish` does not return `RootObservationSetV1::default()`; if
`RootObservationSetV1` or `RootIdentityCaptureV1` differs in shape; if
`classify_root_observations` requires more or less than `dev` and `ino` on all three
captures; if the session already stores its enumeration root; if
`enumerate_directory_names`, `DirectoryStreamV1`, or `errno_location` differ from the
descriptions above; if `libc` is already a dependency of `bridge-worktree`, or is not
one of `bridge-core`; if `PinnedDirectoryV1::identity()` is not public; if any of the
ten named A2a-2 tests is absent or differently named; or if any inherited finding no
longer applies — record the exact repository evidence and **stop before editing**.

Do not adapt the design to a false anchor. Finding the work smaller than described is a
good outcome; finding it larger is a report, not a compression target.