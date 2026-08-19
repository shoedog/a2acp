---
task-type: implement
---

# Author the A2a task spec — the first half of the A2 split you proposed

## Description

Your A2 spec v2 passed a second review with eight valid findings. The trend was
converging (11 → 8, severity falling), but four findings were consequences of
the folds themselves rather than findings about behavior: the platform matrix
generated fixture-attestation gaps, the two-commit protocol generated a
missing base-tree problem, the scan engine generated a projection-asymmetry
problem. That is a sizing symptom, so the owner has taken **the A2a/A2b split
you proposed in v2's sizing section**.

Author the **complete A2a task spec**. Not a design note, not a plan, not a
diff — a task spec an implementer can execute, in the same shape as v2.

Emit it between the extraction markers, nothing outside them.

### The split, as you defined it

> - A2a: the production-bound scan engine, compatibility-source refactor,
>   preserved action-scanner projection, and same-root conformance evidence,
>   without changing the public exact-absence return type;
> - A2b: report return/population, eager assessment and tracing, root/capability
>   evidence, UTF-8 characterization, platform matrix, mutation audit, and final
>   handoff, based on the accepted A2a commit.

That boundary is settled. A2a **does not** change the public signature of
`sweep_orphans_with_exact_absence`; it keeps returning `()`. A2a is a
behavior-preserving refactor that installs the engine A2b will build on.

Because A2a makes no public break and adds no new observable behavior, its
tests are characterization and structural evidence, not genuine runtime red.
Say so plainly rather than manufacturing red.

### Environment facts

Your working tree is checked out at `c637e493`, the base commit, and the
repository is authoritative over every claim you make. Read the code; do not
restate this brief as though you had observed it. Where you assert a fact,
you may be asked which file and line you read it from.

You cannot read anything outside the repository — no `~`, no `$HOME`, no
installed templates. The spec you emit must never name a path outside the
repository, because the implementer runs in a container with only the code
tree mounted.

---

## The pinned seam — do not re-derive it

v2's normative cross-module seam is **rustfmt-verified and must not be
reformatted**. Measured under the repo's pinned toolchain (`rust-toolchain.toml`
channel 1.94.0, `rustfmt 1.8.0-stable`, no `rustfmt.toml`):

| Block | `rustfmt --check --edition 2021` |
|---|---:|
| v2 seam block | **exit 0 — gate passes** |
| v1 seam block | exit 1 — gate fails |

Round 2 raised a BLOCKER claiming rustfmt would rewrite v2's block, specifically
the split `read_legacy` return type. **That finding is refuted.** The split is
rustfmt's own output — the single-line form measures exactly 100 characters, at
`max_width` — and rustfmt is a no-op on the block. Do not "re-normalize" it;
doing so reintroduces the round-1 defect.

**What you must decide:** which declarations of that seam land in A2a and which
land in A2b. The root-observation types (`RootIdentityCaptureV1`,
`RootObservationSetV1`, `complete_identity`) support root/capability evidence,
which is A2b's. State the division explicitly.

Whatever subset A2a lands must be **byte-identical to the corresponding lines of
the pinned block** and must itself pass `rustfmt --check` as written. The
operator will verify both mechanically. If landing a subset would require
reformatting, say so and land the whole block instead, justifying any
`dead_code` allowances as A2b-pending.

---

## Round-2 findings — disposition in the split

Fold the A2a ones. For the A2b ones, do not solve them, but record them in a
short `### Deferred to A2b` section so they are not lost.

### A2a OWNS these

**F3 (MAJOR, confirmed by probe) — the projection asymmetry is A2a's core risk.**
v2 gives deterministic pin-failure injection only to the action projection via
`scan_worktree_records_with_pin_opener` (v2 lines 249, 251, 414), while the
exact-report route hardcodes `FilesystemCompatibilityPinOpenerV1`. An
implementer can therefore satisfy the conformance matrix by testing one
projection and asserting the other by construction. Since A2a's entire purpose
is that both projections delegate to one engine, this is the finding that
decides whether A2a is provable. Define a private report-side
post-canonicalization opener seam and state the exact observable equivalence to
compare — including how decoded custody results are compared when the report
route does not expose the complete record.

**F5 (MAJOR) — descriptor ownership.** The design retains `std::fs::ReadDir`,
which exposes no inspectable identity for the directory object being
enumerated, so `retained_enumeration_object` cannot simply be populated later.
Decide now: make enumeration descriptor-owned in A2a, or redefine the field's
meaning and budget the enumerator redesign explicitly as A2b work. Do not leave
it implicit — this is the seam A2b builds on.

**F7 (MINOR, confirmed by probe) — no capture infrastructure.** At the base,
`crates/bridge-worktree/Cargo.toml` `[dev-dependencies]` contains exactly
`bridge-coordinator` and `bridge-controller`. There is no `tracing-subscriber`
and no existing capture utility. If A2a requires any scoped tracing evidence,
either authorize the dev-dependency addition explicitly in the Files section or
define the small shared test utility. Do not mandate capture the crate cannot
perform.

**F2-derived (BLOCKER in v2, and it does NOT fully move to A2b) — fixture
attestation.** v2 mandated APFS and ext4 evidence but defined no fixture-root
supply and no filesystem attestation, so a test in a default temporary directory
could execute on tmpfs or overlayfs and be recorded as the completed ext4 row.
The *birthtime capability* row is A2b's. But A2a's scan engine drives real
directory enumeration, `read_sidecar`, and pin opening, and **this lane has
already shipped a defect that passed on macOS/APFS and on container overlayfs
and failed only on ubuntu/ext4** — inode reuse after unlink and recreate, in
exactly this area. So A2a still needs the *mechanism*: define how each fixture
root is supplied and how its filesystem type and mount identity are verified and
recorded before the test runs. Keep the mechanism, drop the capability row.

### Deferred to A2b — record, do not solve

- **F4** — the runtime-red tests have no reproducible tree to run against base
  production code. Dissolves for A2a, which has no genuine red by construction,
  and returns for A2b when the return type changes. A2b will need a frozen
  test-only patch on an exact base with recorded identity and diff.
- **F6** — the source-incompatible public return-type change at version `0.3.1`,
  enforced only by handoff prose. A2a makes no public break, so this is entirely
  A2b's. Note it as a blocking pre-publication obligation, not an A2a one.
- **F8** — the capability test passes for either `Some` or `None` and captured
  `cargo test` output does not reveal which; needs a `--nocapture` probe or a
  machine-readable artifact.
- **F9** — the mutation inventory presents both final `deepest_existing_path`
  bracket calls as unconditional, but the comparator can return `CannotProve`
  after an unavailable initial resolution and before those calls. The symbol is
  at `crates/bridge-core/src/fs_custody.rs:1511`, used as a resolver at line
  1777. Distinguish possible call edges from guaranteed observations.

---

## Sizing

v2's counted-line metric was a genuine improvement — deterministic, added
nonblank physical lines after the fmt gate, one row per line, no contingency and
no borrowing. **Keep that metric.** Re-derive the worksheet for A2a alone.

For calibration: A1 declared 700 counted lines and landed 698, and passed review
with zero findings on its first round. A2 as a whole declared 1,650. A2a is a
strict subset. If your honest A2a estimate approaches A1's scale, that is the
right neighbourhood; if it approaches the full 1,650, the split has not actually
divided the work and you should say so rather than restate the total.

## Output contract

Emit the complete A2a task spec between the markers, with:

- the same front matter (`task-type: implement`);
- `## Description`, `## Acceptance Criteria`, `## Files`, `## Spec Refs`,
  `## Commit Message`;
- an explicit statement of which seam declarations A2a lands and which A2b does;
- the four A2a findings folded, each resolved to one answer, not a menu;
- a `### Deferred to A2b` section listing the four deferred findings;
- a falsification license, as v2 had: the repository is authoritative, and an
  implementer who finds a stated anchor false must stop and report rather than
  adapt the implementation to a stale claim;
- no path outside the repository anywhere in the document.

---

## Reference — the full A2 spec v2, verbatim

A2a is a subset of the work below. Reproduce heading levels as they appear.
This reference ends at the end of the document.


# R2f1b 3d T3a increment 1, slice A2 v2 — compatibility traversal and exact-absence report population

## Description

Implement slice A2 against exact base commit `c637e493544a2e2edd1ca3ae20842a86dcb58f3f`.

This v2 was re-anchored by direct read-only inspection of the clean repository tree at that commit. The inspection confirmed:

- `crates/bridge-worktree/src/sweep/report.rs` is 598 lines;
- `sweep.rs` re-exports exactly the fifteen stated public report types;
- `EXACT_ABSENCE_POLICY_READY_V1` remains false;
- the four A1 constructor `dead_code` allowances remain present;
- `sweep_orphans_with_exact_absence` still returns unit;
- `scan_worktree_records` still eagerly returns `Vec<(String, ScannedWorktreeRecordV1)>`;
- the only in-repository call to `sweep_orphans_with_exact_absence` is the statement-position call inside `sweep_orphans`;
- the five binary boot callers invoke `sweep_orphans` in statement position;
- `bridge-worktree` inherits workspace version `0.3.1`, and no workspace member manifest sets `publish = false`.

These are source-tree anchors, not build or test evidence. No build or test was run while authoring this specification.

Before editing, verify the exact base commit and re-read every authoritative file under “Spec Refs.” If any factual anchor below is false at that commit, follow the falsification license instead of adapting the implementation to a stale claim.

### Scope and settled boundaries

A2 owns:

- the private checked-scan module and compatibility source;
- one production-bound checked-scan engine used by both exact-report and compatibility/action projections;
- the shared record-selection/read policy;
- eager two-phase traversal and ordering evidence;
- public report population and the accepted exact-absence return-type change;
- exact requested-root and canonical-root handling;
- compatibility/action scan separation;
- behavior characterization;
- filesystem-capability evidence for the strict birthtime-dependent classifier;
- the concrete report-route mutation audit;
- removal of A1’s four now-obsolete constructor `dead_code` allowances;
- the A2 handoff, pending operator-evidence block, and candidate/evidence-commit protocol.

A2 does not:

- add the increment-2 population-admission rule;
- set `EXACT_ABSENCE_POLICY_READY_V1` to true;
- construct `IneligiblePopulation` or `CannotConstructSubject` production assessments;
- add ownership, locking, transition, publication, settlement, unlink, removal, prune, rename, or backend-cleanup authority;
- populate authoritative root captures in production;
- weaken the strict birthtime-dependent root classifier to accommodate a filesystem lacking creation-time support;
- implement T3b or treat a report as action authority;
- change CLI behavior or the return type of `sweep_orphans`;
- claim that an external `bridge-worktree` consumer exists.

T3a decides and reports. T3b will independently re-open, re-read, re-bind, re-apply admission, re-prove exact absence, and retain its own lock and authority through any later action.

### Accepted semver boundary

At the base, `bridge-worktree` is publishable by default and inherits version `0.3.1`. Changing the public `sweep_orphans_with_exact_absence` return type from `()` to `ExactAbsenceSweepReportV1` is source-incompatible for callers that constrain it to unit, including unit-returning function pointers, closures, generic consumers, or unified expression branches.

A2 deliberately accepts that public API break because the function’s report is the intended T3a product. Do not add a unit-returning compatibility wrapper or a second report-returning entry point. No external consumer or actual downstream breakage is asserted. Record the accepted break in the handoff. The release owner must not publish the changed crate as patch-compatible with `0.3.1`; release-version selection is outside this implementation slice but is blocking before publication.

### Module placement and literal cross-module seam

Create `crates/bridge-worktree/src/sweep/checked_scan.rs` and declare it as a private child module of `sweep.rs`.

The following rustfmt-normalized block is normative and must land byte-for-byte as the cross-module seam. Every item used by parent `sweep.rs`, and every type exposed through those signatures, is `pub(super)`. The concrete compatibility session remains module-private. Observation fields and `complete_identity` remain private to `checked_scan.rs`.

```rust
use std::ffi::{OsStr, OsString};
use std::path::Path;

use bridge_core::fs_custody::{BirthTimeV1, PinnedDirectoryV1};

use crate::custody::{CustodyReadRefusalV1, WorktreeCustodyRecordV1};
use crate::provider_path::WorktreeSidecar;

use super::report::CustodyRootObservationV1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CheckedScanOpenRefusalV1 {
    CannotEnumerate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CheckedScanEntryRefusalV1 {
    CannotReadEntry,
}

pub(super) trait CheckedScanSourceV1 {
    fn open(
        &self,
        enumeration_root: &Path,
    ) -> Result<Box<dyn CheckedScanRootSessionV1>, CheckedScanOpenRefusalV1>;
}

pub(super) trait CheckedScanRootSessionV1 {
    fn next_name(&mut self) -> Option<Result<OsString, CheckedScanEntryRefusalV1>>;

    fn read_legacy(&self, enumerated_name: &OsStr, record_display: &str)
        -> Option<WorktreeSidecar>;

    fn read_custody(
        &self,
        enumerated_name: &OsStr,
    ) -> Result<WorktreeCustodyRecordV1, CustodyReadRefusalV1>;

    fn finish(self: Box<Self>) -> RootObservationSetV1;
}

/// Test injection is below the real compatibility source. Tests replace only the
/// pin-open result while retaining production `read_dir`, name selection, legacy
/// reading, custody handling, session state, and finish behavior.
pub(super) trait CompatibilityPinOpenerV1 {
    fn open_pin(&self, enumeration_root: &Path) -> Option<PinnedDirectoryV1>;
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct FilesystemCompatibilityPinOpenerV1;

impl CompatibilityPinOpenerV1 for FilesystemCompatibilityPinOpenerV1 {
    fn open_pin(&self, enumeration_root: &Path) -> Option<PinnedDirectoryV1> {
        PinnedDirectoryV1::open(enumeration_root, "worktree sweep root").ok()
    }
}

pub(super) struct CompatibilityCheckedScanSourceV1<P> {
    pin_opener: P,
}

impl<P: CompatibilityPinOpenerV1> CompatibilityCheckedScanSourceV1<P> {
    pub(super) const fn new(pin_opener: P) -> Self {
        Self { pin_opener }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct RootIdentityCaptureV1 {
    dev: Option<u64>,
    ino: Option<u64>,
    birthtime: Option<BirthTimeV1>,
}

impl RootIdentityCaptureV1 {
    fn complete_identity(&self) -> Option<(u64, u64, BirthTimeV1)> {
        Some((self.dev?, self.ino?, self.birthtime?))
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct RootObservationSetV1 {
    retained_enumeration_object: Option<RootIdentityCaptureV1>,
    pinned_custody_directory: Option<RootIdentityCaptureV1>,
    final_named_root: Option<RootIdentityCaptureV1>,
}

pub(super) fn classify_root_observations(
    observations: &RootObservationSetV1,
) -> CustodyRootObservationV1 {
    let (Some(retained_enumeration_object), Some(pinned_custody_directory), Some(final_named_root)) = (
        observations
            .retained_enumeration_object
            .and_then(|identity| identity.complete_identity()),
        observations
            .pinned_custody_directory
            .and_then(|identity| identity.complete_identity()),
        observations
            .final_named_root
            .and_then(|identity| identity.complete_identity()),
    ) else {
        return CustodyRootObservationV1::Unavailable;
    };

    if retained_enumeration_object == pinned_custody_directory
        && pinned_custody_directory == final_named_root
    {
        CustodyRootObservationV1::Pinned
    } else {
        CustodyRootObservationV1::IdentityChanged
    }
}
```

`CompatibilityCheckedScanRootSessionV1` implements the session trait but remains private. Parent `sweep.rs` sees only the trait object and `RootObservationSetV1`; it does not inspect session internals or observation fields.

Do not use `DirectoryIdentityV1::matches` in this classifier. At the base commit, that method treats absent birthtime on either side as a wildcard. This classifier instead requires three complete `(dev, ino, birthtime)` tuples. An absent capture or incomplete tuple yields `Unavailable`; only three complete tuples may yield `Pinned` or `IdentityChanged`. `Unavailable` outranks a mismatch elsewhere in the set.

A2 production `finish` returns `RootObservationSetV1::default()`, so production root classification remains `Unavailable`. Slice B owns real population of the three captures.

### Filesystem-capability boundary

`BirthTimeV1::from_metadata` calls `Metadata::created()` and maps an error to `None`. Creation-time support therefore varies by operating system, filesystem, mount, and runtime.

The supported boundary is explicit:

- exact-report enumeration and raw decision reporting remain supported when birthtime is unavailable;
- a filesystem without creation-time support cannot produce a complete `RootIdentityCaptureV1` through this seam;
- any missing birthtime yields `CustodyRootObservationV1::Unavailable`;
- `Unavailable` is the required fail-closed result, not a test skip and not permission to fall back to `(dev, ino)`;
- because A2 production `finish` returns the default observation set, every A2 production report remains `Unavailable` on every filesystem;
- when slice B populates the captures, this exact classifier can produce authoritative `Pinned` evidence only on filesystems that expose birthtime for all three observations;
- slice B must not claim universal authoritative-root availability through this seam. Supporting authority on a birthtime-unavailable filesystem requires a separately reviewed identity design, not an A2 accommodation.

The operator evidence must record the observed `Metadata::created()`/`BirthTimeV1::from_metadata` capability for the real APFS and ext4 fixtures. The result may be available or unavailable; the classifier expectation follows the measured capability.

### Compatibility source, shared policy, and mandatory scan engine

Implement `CompatibilityCheckedScanSourceV1<P>` using the current filesystem operations and parameterize only pin opening.

Its open sequence is exact:

1. Call `std::fs::read_dir(enumeration_root)`.
2. Return `CheckedScanOpenRefusalV1::CannotEnumerate` only if that call fails.
3. After successful `read_dir`, call `open_pin`.
4. Retain the returned `Option<PinnedDirectoryV1>` in the session.
5. Permit legacy reads through path-based `read_sidecar` regardless of pin outcome.
6. With no pin, return `CustodyReadRefusalV1::Unreadable("sweep root is not pinnable".to_string())` for every custody read.
7. Consume the session in `finish` and return `RootObservationSetV1::default()`.

A pin failure is not an open failure. A readable directory with a failed pin must still enumerate completely, preserve valid legacy rows, and retain custody-named rows as the exact not-pinnable refusal.

Extract one private display-path classifier for legacy, custody, or ignored entries. Do not introduce a second selection vocabulary. All production scanning must continue to use:

- `read_sidecar` for legacy reads;
- `is_custody_record_name` for custody selection;
- `read_custody_record_in` for custody reads.

A malformed or unreadable legacy sidecar remains silently omitted because `read_sidecar` returns `None`. It produces no intermediate row, report entry, probe call, or decision event. A malformed or otherwise refused custody record remains present as `UnreadableCustody` and does not count as an iterator error.

Create one private parent-module engine named `scan_checked_rows_with_source`. It is the only production code permitted to drive `CheckedScanRootSessionV1`. It must:

1. call `source.open(enumeration_root)`;
2. repeatedly call `next_name`;
3. count an `Err` item and continue;
4. for each successful name, retain the exact `OsString`;
5. construct the lossy full display path below the supplied enumeration root;
6. invoke the one shared display classifier;
7. immediately perform the selected legacy or custody read before requesting the next name;
8. retain any resulting intermediate row in enumeration order;
9. call `finish` exactly once after `next_name` returns `None`;
10. return the rows, iterator-error count, and observation set to its projection.

The intermediate row and engine-result types remain private to `sweep.rs`. No production caller may duplicate the `next_name` → classify → immediate read → `finish` protocol.

Both production paths must use this engine:

- `sweep_orphans_with_exact_absence` constructs the real compatibility source, passes the canonical exact-scan root to `scan_checked_rows_with_source`, then performs report assessment as a second phase;
- `scan_worktree_records` delegates through `scan_worktree_records_with_pin_opener`, which constructs the same compatibility source, passes the raw caller root to `scan_checked_rows_with_source`, discards root observations and iterator-error details, and projects the existing vector.

For deterministic action-scanner pin-failure evidence, `scan_worktree_records_with_pin_opener` accepts only a pin opener. Production supplies `FilesystemCompatibilityPinOpenerV1`. Tests may substitute only the pin-open result. The helper retains real `read_dir`, iterator behavior, the shared display classifier, `read_sidecar`, `read_custody_record_in`, the mandatory engine, accumulation, and the existing return projection.

`scan_worktree_records(root)` retains all existing observables:

- its return type remains `Vec<(String, ScannedWorktreeRecordV1)>`;
- it passes the caller’s raw `root` spelling directly to the engine and underlying `read_dir`;
- source-open failure returns an empty vector;
- after successful `read_dir`, the source calls the production pin opener on that same raw root;
- pin failure preserves legacy reads and refuses custody reads as not-pinnable;
- display selection and legacy reads use the lossy full path;
- custody reads use the exact enumerated `OsString`;
- iterator-item errors are flattened by the projection;
- it performs no canonicalization of its enumeration argument.

Tests that invoke helpers directly do not establish production delegation. Source inspection and production-route tests must prove that both public production paths reach `scan_checked_rows_with_source` and that there is no second production session-driving loop.

### Exact-absence report flow

Change the public signature to this exact declaration:

```rust
pub fn sweep_orphans_with_exact_absence(
    root: &str,
    probe: &dyn ExactAbsenceProbeV1,
) -> ExactAbsenceSweepReportV1
```

This is the accepted source-incompatible API change described above. Do not add a unit wrapper.

The route must behave as follows:

1. Copy the supplied `root` directly into `requested_root`, preserving its UTF-8 bytes exactly.
2. Invoke the existing `canonicalize_lenient(root)` exactly once for the supplied exact-scan root.
3. Do not replace that conversion with `std::fs::canonicalize`.
4. The count above applies only to this entry-point conversion; it excludes the separate `sweep_orphans` guard, per-record `worktree_under_root` calls, sibling guards, and `canonicalize_lenient`’s internal ancestor loop.
5. On lenient-canonicalization failure, return:
   - the exact requested root;
   - `canonical_root: None`;
   - `Refused(CannotCanonicalize)`;
   - custody root `Unavailable`;
   - no entries.
6. Construct the production `CompatibilityCheckedScanSourceV1` and invoke `scan_checked_rows_with_source` on the canonical root.
7. On source-open failure, return:
   - the exact requested root;
   - the canonical root;
   - `Refused(CannotEnumerate)`;
   - custody root `Unavailable`;
   - no entries.
8. The engine performs phase 1: it drains enumeration, reads each selected successful name before requesting the next, counts only iterator-item errors, and completes `finish`.
9. Phase 2 begins only after the engine returns. Assess intermediate rows in enumeration order, invoke probes where applicable, construct public entries, and emit one unchanged decision event per retained row.
10. Return `Complete` when no iterator-item error occurred; otherwise return `Incomplete { skipped_entries }`.
11. Classify root observations independently of enumeration completeness.

Populate assessments as follows:

- valid legacy row: `ExactAbsenceRecordAssessmentV1::Legacy(decision)`;
- refused custody read: `ExactAbsenceRecordAssessmentV1::UnreadableCustody(refusal)`;
- decoded custody row:
  `ExactAbsenceRecordAssessmentV1::Custody(CustodyRecordAssessmentV1::new(CustodyStateSnapshotV1::from(&record.state), CustodyExactAbsenceAssessmentV1::Assessed(decision)))`.

A2 does not construct the dormant admission or subject-construction arms. Existing guards and candidate-construction failures continue to project as `Assessed(Refused)`.

Route the existing per-row event through one private helper in `sweep.rs`. Preserve its level, fields, and message exactly:

```rust
tracing::info!(record = path, ?decision, "made exact-absence decision");
```

Do not use a helper-local counter as event evidence. Use a panic-safe scoped tracing capture for both the decision-event and guard-warning tests. The decision-event assertions must prove:

- level `INFO`;
- exact message `made exact-absence decision`;
- the `record` field has the expected display path;
- the debug-formatted `decision` field has the expected value;
- one event exists for every retained row and none for omitted legacy rows;
- event order equals report-entry order;
- no event is emitted before enumeration returns `None` and `finish` completes.

The scoped capture must not install shared global state across concurrent tests and must not add a public reporter API.

The traversal remains eager. Do not stream assessment or logging alongside enumeration. Every selected successful name is read before the following `next_name`, and no assessment, probe, or decision event occurs before `None` and completed `finish`.

Because `finish` precedes phase-2 assessment, root evidence and later row decisions are ordered historical evidence, not one coherent point-in-time snapshot and not retained authority.

### Supplied-root canonicalization evidence

Stable filesystem output cannot prove that the supplied-root conversion called `canonicalize_lenient` exactly once. Use two separate forms of evidence:

- runtime fixtures prove the exact requested and canonical root results;
- a symbol-scoped source audit proves that `sweep_orphans_with_exact_absence` contains exactly one direct supplied-root call to `canonicalize_lenient(root)` and has no helper or alternate branch that repeats that entry-point conversion.

The handoff must record the source-audit result and list every other `canonicalize_lenient` call reachable in `sweep.rs`, classifying each as the independent compatibility/action guard, a per-record root guard, or another excluded use. An `rg` count without symbol-level inspection is insufficient.

### Root-spelling behavior

Missing-root behavior is input-shape dependent and must remain explicit:

- an absolute missing path beneath an existing ancestor is accepted by `canonicalize_lenient`, retains the precise appended canonical value, reaches source open, and reports `CannotEnumerate`;
- a missing relative leaf whose empty parent cannot be resolved reports `CannotCanonicalize` with no canonical root.

For a deliberately non-canonical but resolvable input, assert both:

- `requested_root().as_bytes()` exactly equals the supplied string bytes;
- `canonical_root()` equals the precise expected lenient-canonical value.

### Compatibility/action separation

`sweep_orphans` continues to discard the returned report explicitly, canonicalize its independent guard root, warn and return early on guard failure, and pass the raw supplied root to the action scanner.

Production remains equivalent to:

```rust
let _ = sweep_orphans_with_exact_absence(
    root,
    &crate::host_git::HostGitWorktree::new(),
);
sweep_compatibility_action_phase_with(
    root,
    my_host,
    probe,
    canonicalize_lenient,
    scan_worktree_records,
);
```

A2 may extract the compatibility/action phase into the private helper named `sweep_compatibility_action_phase_with`. Production supplies the existing guard and scanner. Tests may replace only the guard result and action-scan observer.

The preserved helper behavior is structurally equivalent to:

```rust
let Ok(root_cwd) = guard_root(root) else {
    tracing::warn!(root, "skipping worktree sweep with non-canonical root");
    return;
};
for (path, scanned) in action_scan(root) {
    // Existing compatibility/action handling remains unchanged.
}
```

The guard result supplies `root_cwd` to existing compatibility decisions. It must not become the action scanner’s enumeration argument.

Use separate deterministic tests for:

- a stable symlinked-root alias with a successful guard, proving that the exact scan enumerates the canonical root while the action observer receives the raw alias and the guard produces the expected canonical `root_cwd`; and
- deterministic guard failure, proving the exact `WARN` level, `root` field, message `skipping worktree sweep with non-canonical root`, and that the action observer is never called.

Do not simulate guard failure by racing or exchanging filesystem objects. The private helper is the only permitted guard-failure seam.

`WorktreeRunEndGuard`, custody locking, recovery classification, and deletion paths continue to consume only the compatibility/action result.

### Compatibility/action conformance matrix

Using one real readable directory, identical root spelling, and equivalent pin-opener outcomes, prove:

| Fixture | Required conformance |
|---|---|
| Valid matching legacy sidecar | Both projections select it and read the same sidecar through the shared engine. |
| Malformed or unreadable legacy sidecar | Both projections omit it. |
| Valid custody record | Both projections select it and decode the same record. |
| Unreadable, malformed, over-bound, symlinked, directory-shaped, or multiply-linked custody entry | Both retain a custody row with the same refusal classification. |
| Unrelated filename | Both omit it. |
| Pin failure with valid legacy and custody names | Both preserve the legacy row and retain the custody row with the exact not-pinnable refusal. |

The pin-failure row must use `scan_worktree_records_with_pin_opener` and the real compatibility source with equivalent deterministic failing openers. Only pin creation is replaced.

Iterator status remains an intentional projection difference: the action scanner flattens item errors, while the report emits `Incomplete { skipped_entries }`. Test that separately. This matrix does not replace the canonical exact-root versus raw action-root test.

### Characterization matrix

For a readable custody record whose placement guards pass and whose valid complete claim constructs an `ExactAbsenceCandidateV1`:

| Population | Current raw decision |
|---|---|
| `ProtectionPrepared` with claim | Probe mapping |
| `ProtectionPrepared` without claim | `Refused` |
| `PreservationPrepared` with required claim | Probe mapping |
| `Preserved` with required claim | Probe mapping |
| `PreservationUnknown`, each of its six reasons, with required claim | Probe mapping |
| `UnusedSettled`, `Materializing`, `LiveProtected`, `DeleteAuthorized`, `Removed`, `RecoveredLive` | `Refused` |
| Missing required claim or forbidden claim present | Decode refusal, emitted `UnreadableCustody`, raw `Refused` |

Probe mapping:

| Probe result | Raw decision |
|---|---|
| `BothAbsent` | `Authorized` |
| `TargetPresent` | `Refused` |
| `RegisteredButAbsent` | `Refused` |
| `Err` | `Refused` |

Guards and legacy:

| Fixture | Result |
|---|---|
| Custody worktree outside sweep root | Raw `Refused`; probe not called |
| Custody record not the expected sibling | Raw `Refused`; probe not called |
| Claim source/common/worktree cannot construct authority | Raw `Refused`; probe not called |
| Undecodable, over-bound, symlinked, directory-shaped, or multiply-linked custody entry | Emitted unreadable entry; raw `Refused` |
| Valid matching in-root legacy sidecar whose source/common authority constructs | Probe mapping |
| Non-matching or outside-root legacy sidecar | Raw `Refused`; probe not called |
| Malformed or unreadable legacy sidecar | Silently omitted; no probe and no decision event |

The load-bearing fixture is a real persisted `Preserved` custody record with a valid complete claim, a vanished target, and `BothAbsent`. Its raw result remains `Authorized`. Production `effective()` yields no entry because policy readiness remains false; A2 production root observations also remain unavailable. The existing A1 synthetic complete-and-pinned readiness test must remain intact because it isolates the false policy gate from the unavailable A2 root source. Even after a future ready report yields a snapshot candidate, T3b must re-prove it under its own action lock.

`MultiLink` is asserted only on Unix. Permission-dependent unreadability is supplementary. Primary refusal evidence uses deterministic entry kind, symlink, pin-open, or decode failures.

When invalid serialized fixtures require byte replacement, use a checked exact-once replacement helper and assert that the targeted decoded field changed. A no-op substring replacement is inadmissible evidence.

### Required proving environments

The minimum completion matrix is:

| Environment | Required evidence | Completion effect |
|---|---|---|
| macOS on APFS | The `bridge-worktree` package suite, all applicable A2 real-Git and Unix byte-name rows, and the measured birthtime-capability row | Mandatory |
| Ubuntu/Linux on ext4 | The `bridge-worktree` package suite, all applicable A2 real-Git and Unix byte-name rows, and the measured birthtime-capability row | Mandatory |
| One of the two required environments | The full locked workspace test suite, plus fmt and clippy gates | Mandatory |
| Linux overlayfs | Supplementary reproduction and capability evidence | Optional; cannot substitute for ext4 |
| Windows or another non-Unix target | Compile/test disclosure when available | Optional; absence does not block, and no green baseline may be claimed without a run |

Both mandatory filesystem rows must be complete before the operator declares A2 complete. If either environment is unavailable, implementation may be handed off, but completion remains pending. A birthtime-unavailable ext4 observation is an expected supported result when the classifier returns `Unavailable`; it is not grounds to skip the row.

### Required tests and evidence classification

Use these test names or equally specific names preserving the stated evidence. Every final test must document the production mutation it catches.

| Required test | Evidence against untouched `c637e493` | Production mutation caught | Proving environments |
|---|---|---|---|
| `returns_report_with_exact_requested_and_lenient_canonical_roots` | Genuine runtime red | Returning unit, losing raw requested spelling, using direct canonicalization, or reporting the wrong canonical value | APFS and ext4 |
| `reports_absolute_missing_and_relative_uncanonicalizable_roots` | Genuine runtime red | Collapsing `CannotEnumerate` into `CannotCanonicalize`, losing the absolute missing canonical value, or inventing a relative canonical root | APFS and ext4 |
| `persisted_preserved_both_absent_is_raw_authorized_but_not_effective` | Genuine runtime red | Losing the raw `Authorized` compatibility result, enabling readiness early, dropping the persisted row, or calling the probe incorrectly | APFS and ext4 with real Git |
| `invalid_utf8_registration_probe_error_is_reported_as_refused` | Genuine runtime red | Dropping the row/event or translating a reached probe error into authorization | APFS and ext4 with Unix byte paths and real Git |
| `registration_path_invalid_utf8_obeys_field_order_and_early_return` | Green characterization on the base | Predecoding all fields, decoding a later field after `Same`, or comparing an invalid current field | APFS and ext4; Unix byte construction |
| `existing_decision_and_decode_matrix_is_preserved` | Green characterization on the base | Changing any state×claim rule, probe mapping, guard short circuit, or decoder refusal | APFS and ext4 with real Git |
| `existing_scan_omits_bad_legacy_and_retains_bad_custody` | Green characterization on the base | Emitting malformed legacy rows, dropping custody-named refusals, or counting decode refusal as iterator failure | APFS and ext4; `MultiLink` Unix-only |
| `root_observations_require_three_complete_equal_identities` | New-seam mechanism evidence; does not compile on the base | Reusing wildcard birthtime matching, allowing incomplete captures, or letting mismatch outrank unavailable | Synthetic identities on APFS and ext4 |
| `filesystem_birthtime_capability_is_recorded_and_unavailable_fails_closed` | New-seam capability evidence | Assuming `Metadata::created()` always succeeds or treating missing birthtime as authoritative | Real APFS and real ext4 fixtures |
| `compatibility_open_refusal_never_calls_pin_opener` | New-seam mechanism evidence | Calling the pin opener before `read_dir` succeeds or treating pin failure as source-open failure | APFS and ext4 |
| `compatibility_pin_failure_preserves_legacy_and_refuses_custody` | New-seam compatibility evidence | Suppressing all rows on pin failure, refusing legacy reads, or losing the exact custody refusal | APFS and ext4 |
| `both_production_scans_delegate_to_one_checked_scan_engine` | Production-route source and runtime evidence | Leaving a disconnected helper, duplicating the session protocol, or retaining a divergent production scanner | Source audit plus APFS and ext4 |
| `checked_scan_reads_before_next_and_finishes_before_assessment` | New-seam ordering evidence | Streaming, prefetching the next entry before the current read, probing before drain, or assessing/logging before `finish` | Fully injected order log on both required environments |
| `checked_scan_counts_only_iterator_errors` | New-seam status evidence | Counting custody refusal as skipped, stopping at the first item error, reordering successful rows, or coupling root classification to enumeration completeness | Injected `Ok, Err, Ok, Err` on both required environments |
| `decision_event_capture_matches_contract_and_order` | New-seam tracing evidence | Calling only a helper-local counter, changing level/message/fields, omitting events, or emitting before `finish` | Scoped tracing capture on both required environments |
| `compatibility_and_action_scans_conform_on_one_root` | Refactor conformance evidence | Drifting selection, legacy omission, custody refusal classification, exact names, or pin-failure policy between projections | APFS and ext4 |
| `exact_non_utf8_custody_name_survives_enumeration` | New report-retention evidence | Reconstructing the name from lossy display text or storing only `String` | APFS and ext4 |
| `exact_scan_canonical_and_action_scan_raw_alias` | Root-routing conformance evidence | Sending the raw alias to exact enumeration or sending canonical `root_cwd` to the action scanner | APFS and ext4 with symlink support |
| `compatibility_action_guard_failure_warns_and_skips_action_scan` | Helper and tracing conformance evidence | Continuing after guard failure, changing warning level/message/field, or scanning after refusal | Scoped tracing capture on both required environments |
| `exact_absence_public_return_type_and_call_contexts_are_total` | Compiler-totality evidence, not behavioral red | Retaining the unit return, omitting the public report type, or leaving an in-repository unit-constrained context | Both required environments |
| `effective_iterator_item_type_is_exact_entry_reference` | Compiler API evidence | Weakening or changing `effective()`’s borrowed item type | Both required environments |
| Existing `raw_decision_and_historical_eligibility_projections` | Existing A1 mechanism evidence | Weakening readiness, historical scan table, legacy exclusion, raw projection, or borrowed-entry behavior | Both required environments |

For the four genuine runtime-red tests, use a local test-only adapter implemented for both `()` and `ExactAbsenceSweepReportV1`. On the untouched base, the existing function runs and the adapter maps its unit result to no report, causing a runtime failure at the explicit “report required” boundary. After the signature changes, the same tests continue into typed field and behavior assertions. This separates genuine runtime red from compiler-only return-shape evidence.

All other new-seam tests must be reported honestly: they are not runnable against the untouched base because their private seam does not exist yet. Characterization tests identified as base-green must be staged or otherwise controlled independently before relying on them as preservation evidence.

The checked-scan suite must additionally cover:

- source-open refusal with zero pin calls;
- complete enumeration;
- injected `Ok, Err, Ok, Err` producing exactly two skipped entries;
- every selected row read before the next name;
- no assessment, probe, or event before `None` and completed `finish`;
- equal complete captures yielding `Pinned`;
- unequal complete captures yielding `IdentityChanged`;
- each absent capture and each absent `dev`, `ino`, or birthtime yielding `Unavailable`;
- unavailable outranking another mismatch;
- root classification independent of enumeration completeness;
- measured real-fixture birthtime availability on APFS and ext4;
- malformed custody inclusion without increasing `skipped_entries`;
- exact non-UTF-8 custody-name retention;
- actual decision-event level, message, fields, count, and order;
- malformed legacy omission causing zero probe calls and zero events;
- every row of the historical scan-evidence table retained by the A1 report test.

The public API evidence must pin the iterator item type with an actual typed consumption, not `let _ = report.effective()`. It must include the equivalent of:

```rust
let mut effective = report.effective();
let _: Option<&ExactAbsenceSweepEntryV1> = effective.next();
```

The return-type audit must inspect statement-position calls, explicit unit bindings, unit-returning function pointers, unit-constrained closures, function-body tail expressions, unified `if` and `match` branches, generic consumers that inferred unit, and macro expression contexts. Record the result in the handoff. At the base, the only in-repository exact-function call is the statement-position call inside `sweep_orphans`; the five binary boot callers call `sweep_orphans` and require no change.

Do not rely on inode reuse after unlink-and-recreate. Where an exchange test is necessary, pre-create a distinct replacement and rename it so simultaneous objects cannot share identity. Report which real environments ran filesystem-dependent tests; APFS, ext4, and overlayfs results are distinct evidence.

### Registration-path UTF-8 boundary

`registration_absent_from_porcelain` processes `worktree ` fields sequentially in porcelain order. Its compatibility behavior is fixed:

1. For each reached `worktree ` field, decode that field with `std::str::from_utf8`.
2. An invalid current field returns:

```rust
BridgeError::ConfigInvalid {
    reason: "worktree registration path is not valid UTF-8".to_string(),
}
```

3. No comparator, resolver, or case-sensitivity observation occurs for that invalid current field.
4. A valid current field is passed to `compare_path_identities`.
5. `Same` returns `Ok(Present)` immediately, so no later field is decoded.
6. `Different` continues to the next field.
7. `CannotProve` records ambiguity and continues to the next field.
8. After all reached fields, the function returns `CannotProve` if any valid field was ambiguous, otherwise `Absent`.

The required order cases are:

- `[exact-match, invalid-utf8]` returns `Ok(Present)` and never decodes the invalid field;
- `[invalid-utf8, exact-match]` returns the exact `ConfigInvalid` before comparing any field;
- `[valid-nonmatching, invalid-utf8]` processes and compares the first valid field before the second field returns the exact `ConfigInvalid`.

Do not introduce a production two-pass predecode. That would change compatibility behavior.

The direct characterization test must assert the externally visible ordered results. Because earlier comparator invocation in the third case is not fully observable through stable filesystem output, the handoff’s source audit must separately confirm the literal decode-then-compare loop, immediate `Same` return, and continued processing for `Different` and `CannotProve`.

Scope the comparator inventory below to valid UTF-8 fields actually reached before termination. Direct decode-refusal evidence and report-projection evidence are both required. When a reached invalid field causes the report’s probe path to return the error, retain the custody or legacy row, produce raw `Refused`, and emit the unchanged `Refused` decision event.

### Mutation audit

Audit only the concrete production report route through `sweep_orphans_with_exact_absence` and `HostGitWorktree::observe_exact_absence`. A downstream implementation of the public `ExactAbsenceProbeV1` may perform arbitrary effects and is outside this proof. The compatibility/action handling that follows the discarded report in `sweep_orphans` is also outside the report-traversal no-action proof.

The normative observation/effect inventory is:

- the exact-absence entry-point’s one supplied-root `canonicalize_lenient`, including repeated internal `std::fs::canonicalize` observations while locating the nearest existing ancestor;
- compatibility `std::fs::read_dir` and iterator-item reads through `CompatibilityCheckedScanSourceV1`;
- the post-`read_dir` `PinnedDirectoryV1::open` observation tree:
  - `Path::canonicalize`;
  - first `directory_path_identity` through `std::fs::symlink_metadata`;
  - read-only no-follow directory open;
  - descriptor `File::metadata` through `directory_identity`;
  - second `directory_path_identity` through `std::fs::symlink_metadata`;
  - comparison of before, descriptor, and after identities;
- no `sync` or mutation method on that pinning branch;
- unbounded legacy `std::fs::read` through `read_sidecar`;
- descriptor-relative custody open and bounded reads through `read_custody_record_in`, including metadata, link count, length, bounded bytes, and canonical decode;
- per-record `worktree_under_root` calls to `canonicalize_lenient`;
- ordinary `std::fs::canonicalize` calls in legacy and custody record/sibling guards;
- `ExactAbsenceCandidateV1::from_legacy` and `from_claim` through `capture_directory_identity`, including absolute-path checks, canonicalization, `verify_payload_directory_identity`, symlink metadata, self-resolution canonicalization, metadata, and derived directory identity;
- `source_common_dir_identity` invoking `git -C <source> rev-parse --path-format=absolute --git-common-dir`, followed by the same directory-identity capture tree;
- `HostGitWorktree::observe_exact_absence` revalidating source/common-directory identity before and after registration observation;
- both target checks through `Path::symlink_metadata`;
- synchronous `git worktree list --porcelain -z`;
- sequential processing of reached `worktree ` fields;
- per reached field, UTF-8 decoding before comparison of that same field;
- immediate `Present` return on a valid `Same`, without decoding later fields;
- the invalid-current-field refusal, which performs no comparator, resolver, or case-mode observation for that field but may follow observations from earlier valid fields;
- the byte-identical valid-UTF-8 comparator short circuit, which returns `Same` without filesystem resolution;
- every reached non-byte-identical valid-UTF-8 field flowing through
  `registration_absent_from_porcelain → compare_path_identities → compare_path_identities_with_resolver`;
- the initial and final stability-bracket calls to `deepest_existing_path` for both paths;
- every resolver walk’s `std::fs::metadata`, `NotFound`-distinguishing `std::fs::symlink_metadata`, deepest-ancestor identity capture, canonicalization, canonical-object metadata, and identity check;
- repetition of complete resolver snapshots before retaining a verdict, with drift producing `CannotProve`;
- the ASCII-case-only missing-tail branch’s `case_sensitive_at` observation:
  `read_dir` of the resolved ancestor, at most 64 entries, entry `symlink_metadata`, alternate-case `symlink_metadata`, and recheck of the original sampled name;
- allocation, collection, subprocess execution, and tracing.

Pure tail comparison and ASCII-case transformation add no filesystem effect.

Record source evidence for these production paths:

- `sweep_orphans_with_exact_absence → scan_checked_rows_with_source → CompatibilityCheckedScanSourceV1`;
- `scan_checked_rows_with_source → CheckedScanRootSessionV1::{next_name, read_legacy, read_custody, finish}`;
- report phase → `decide_unused_legacy_sidecar` or `decide_unused_custody_record`;
- candidate decision → the supplied `ExactAbsenceProbeV1`;
- production probe → `HostGitWorktree::observe_exact_absence`.

Record symbol-to-symbol call-path evidence establishing that the concrete report traversal has no application edge to:

- provider remove or prune;
- `remove_worktree`;
- `remove_worktree_if_safe`;
- `remove_dir_all`;
- `remove_file`;
- rename;
- custody publication or replacement;
- settlement;
- state transition;
- backend cleanup;
- any T3b action.

Do not describe the route as globally effect-free: it executes Git subprocesses and tracing, and a configured tracing sink may write. Byte snapshots are corroborating final-content evidence only; they cannot exclude mutation followed by restoration.

### Sizing and mandatory pre-edit stop

Use the following deterministic counted-line metric. It replaces the ambiguous v1 “logical line” rule.

1. Measure the final evidence tree against base `c637e493544a2e2edd1ca3ae20842a86dcb58f3f`.
2. For every owned changed file, count each added nonblank physical line after the fmt gate.
3. A replacement counts its added side. Deleted lines do not consume the cap.
4. Exclude only the exact normative Rust block above when it is byte-identical. Any modification to that block removes the exemption.
5. Imports, attributes, helper declarations, macro invocations, assertions, comments, parameterized rows, and nested constructs all count by their nonblank added lines. No line belongs to more than one component.
6. Count every added nonblank handoff and evidence line, including operator-populated evidence in the follow-up evidence commit.
7. Assign each counted line to exactly one worksheet row below by file and purpose. If a line plausibly spans purposes, assign it to the first applicable row from top to bottom.
8. Record the per-file and per-row worksheet in the handoff. `git diff --numstat` may corroborate changed files but is not the final count because it includes blank and exempt lines.

There is no contingency row and no borrowing between rows.

| Counted component | Counted-line cap |
|---|---:|
| Freely authored `checked_scan.rs` production implementation outside the normative block | 200 |
| `sweep.rs` scan engine, traversal, report population, shared policy, event, and action helpers | 280 |
| `report.rs` allowance, iterator-type evidence, and stale-comment cleanup | 20 |
| Checked-scan, report-population, ordering, tracing, characterization, capability, and conformance tests | 820 |
| Host-Git UTF-8 and external public-return compiler evidence | 140 |
| Handoff, mutation audit, return audit, platform evidence, and sizing worksheet | 190 |
| **Total counted cap** | **1,650** |

The freely authored production allocation is 500 lines; most of the cap remains tests and evidence. A1’s measured 698-of-700 result is context, not a multiplier for A2 production complexity.

Before editing, estimate every row against the base. Stop if any row or the total will exceed its cap. Report the revised row estimates and propose this split:

- A2a: the production-bound scan engine, compatibility-source refactor, preserved action-scanner projection, and same-root conformance evidence, without changing the public exact-absence return type;
- A2b: report return/population, eager assessment and tracing, root/capability evidence, UTF-8 characterization, platform matrix, mutation audit, and final handoff, based on the accepted A2a commit.

Do not compress declarations, tests, mutation evidence, platform evidence, operator evidence, or the handoff to fit. Do not silently extend the boundary after editing begins.

### Handoff requirements

Create `docs/superpowers/reviews/2026-08-19-r2f1b-3d-t3a-inc1-sliceA2-handoff.md`.

Do not consult a template or path outside the repository. Write the handoff from these inline requirements.

Use these headings in order:

- `## Summary`
- `## What changed`
- `## Evidence`
- `## OPERATOR EVIDENCE — PENDING`
- `## Limits and disclosures`
- `## Sizing`

Use a two-commit custody protocol:

1. The implementation-candidate commit contains all production code, tests, and a handoff with operator-owned fields still pending.
2. After creating that commit, the operator records its exact SHA as the implementation candidate and runs every claimed gate against that exact commit.
3. If production or test code changes, create a new implementation candidate and rerun the affected evidence; do not attach old results to a new candidate.
4. The operator then updates only the handoff with the implementation-candidate SHA, gate results, platform results, mutation and return audits, and final counts.
5. Commit that handoff-only update as the evidence commit.
6. The handoff must describe the evidence commit as “the commit containing this completed handoff” and must not embed or predict its own SHA. Git commit metadata identifies it after creation.
7. Verify that the implementation-candidate-to-evidence-commit diff changes only the handoff.
8. The final tree must be clean at the evidence commit.

The handoff must state which tree each claim attests. Tests, fmt, clippy, source audit, and behavior evidence attest the exact implementation-candidate SHA. The evidence commit attests only the completed in-repository handoff and its linkage to that candidate. Do not claim that a commit contains a literal self-hash.

The handoff must record:

- exact base and implementation-candidate commit identities;
- that the evidence commit is the commit containing the completed handoff and does not self-name;
- confirmation that the candidate-to-evidence diff changes only the handoff;
- every changed file;
- the accepted source-incompatible public return-type change and publication boundary;
- the mandatory production delegation through `scan_checked_rows_with_source`;
- the eager two-phase ordering and compatibility/action root separation;
- the test-name/evidence classification above;
- which tests or checks the implementer actually ran and which it could not run;
- the exact genuine-red control results supplied by the operator;
- the return-type context audit;
- the typed `Iterator<Item = &ExactAbsenceSweepEntryV1>` evidence;
- the supplied-root `canonicalize_lenient` source audit;
- the same-root conformance result;
- actual decision-event and guard-warning level, message, fields, count, and ordering evidence;
- every Unix-only test and non-Unix allowance, recording “none” where applicable;
- separate macOS/APFS, Ubuntu/Linux ext4, and optional overlayfs results;
- the measured birthtime capability on APFS and ext4 and the classifier outcome implied by each;
- the concrete mutation inventory and symbol-to-symbol no-action call paths;
- that production root classification remains `Unavailable`;
- that production policy readiness remains false;
- that reports retain ordered historical evidence rather than authority;
- that arbitrary probe implementations and the later compatibility/action phase are excluded from the no-action proof;
- that byte snapshots cannot exclude mutation followed by restoration;
- the pre-edit estimate and final deterministic counted-line worksheet;
- that final clean-tree and evidence-commit custody is operator-owned and must not be fabricated inside the implementation candidate.

The implementation container has no compile loop. Do not install dependencies, use network access, or invent test results. The operator runs the gates and fills the pending block.

### OPERATOR EVIDENCE — PENDING

Copy these lines into the handoff under its `## OPERATOR EVIDENCE — PENDING` heading and leave them unticked in the implementation candidate:

- [ ] `cargo fmt --all -- --check` — PENDING OPERATOR
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings` — PENDING OPERATOR
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — PENDING OPERATOR
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — macOS/APFS — PENDING OPERATOR
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — Ubuntu/Linux ext4 — PENDING OPERATOR
- [ ] APFS and ext4 birthtime-capability observations recorded — PENDING OPERATOR
- [ ] implementation-candidate SHA recorded and candidate-to-evidence diff limited to the handoff — PENDING OPERATOR

Report test totals as test binaries plus doc-test suites; do not double-count nested filtered subprocess output. Do not claim a green Windows all-target baseline unless one actually ran.

### Falsification license

Every symbol, caller count, matrix row, and behavioral statement in this task is an anchored claim against `c637e493`. The repository remains authoritative.

If any named symbol is absent; the A1 report surface differs; the manifest/version boundary differs; `read_sidecar` does not silently omit failures; the two entry points do not enumerate different root spellings; `sweep_orphans` does not preserve its guard warning and early return; the state decoder admits a different state×claim population; `registration_absent_from_porcelain` does not decode and compare reached fields sequentially with immediate `Same` return; `[exact-match, invalid-utf8]` does not return `Present`; an invalid current field reaches comparison; a reached non-byte-identical valid-UTF-8 field does not reach the stated resolver tree; `BirthTimeV1::from_metadata` does not map unavailable `Metadata::created()` to `None`; `PinnedDirectoryV1::open` differs from the stated observation tree; a listed call edge differs; or any matrix result is wrong, record the exact source evidence and stop before editing.

Finding the work smaller than described is a good outcome. The A1/A2 split, T3a-decides/T3b-acts boundary, T3b action-time re-decision, and exclusion of new ownership plumbing remain settled even if another factual anchor is disproved.

## Acceptance Criteria

1. Work begins only from exact base `c637e493544a2e2edd1ca3ae20842a86dcb58f3f`, after the factual, platform, semver, and sizing checks.
2. `sweep/checked_scan.rs` exists and carries the complete rustfmt-normalized cross-module seam byte-for-byte.
3. The concrete compatibility session and observation internals remain private to `checked_scan.rs`; parent `sweep.rs` sees only the declared seam.
4. The root classifier requires three complete tuples, returns `Unavailable` for every incomplete case, returns `Pinned` only for three equal complete tuples, and returns `IdentityChanged` only for three complete tuples with a mismatch.
5. Birthtime availability is treated as a filesystem capability; missing creation-time support remains fail-closed `Unavailable` and is recorded separately for APFS and ext4.
6. Production compatibility `finish` returns the default observation set, so A2 production root evidence remains `Unavailable`.
7. The compatibility source refuses open only when `read_dir` fails, calls the pin opener only after successful `read_dir`, preserves legacy reads on pin failure, and emits the exact not-pinnable custody refusal.
8. One production-bound `scan_checked_rows_with_source` engine exclusively owns the `next_name` → immediate read → `finish` protocol, and both production projections delegate to it.
9. Both projections use one shared display classifier and the existing production read functions. No second selection, read-policy, or production session-driving loop exists.
10. `scan_worktree_records` retains its public signature, eager vector behavior, raw-root enumeration, flattened iterator errors, legacy omission, exact custody names, and pin-failure semantics.
11. `sweep_orphans_with_exact_absence` has the exact report-returning signature; the source-incompatible change is accepted and no unit wrapper or alternate report entry point is added.
12. The handoff records that no external consumer is established and that the changed crate must not be published as patch-compatible with `0.3.1`.
13. `sweep_orphans_with_exact_absence` performs exactly one supplied-root `canonicalize_lenient` conversion, proven by runtime root fixtures plus a symbol-scoped source audit.
14. Canonicalization and source-open failures produce the exact requested root, canonical-root option, refusal, unavailable root observation, and empty entry set specified above.
15. Phase 1 drains and reads eagerly; `finish` completes before phase 2; assessment, probes, entries, and events preserve enumeration order.
16. Iterator errors alone determine `skipped_entries`; custody read/decode refusals remain entries and do not increase that count.
17. Report entries retain lossy display paths and exact `OsString` names and use the stated legacy, unreadable-custody, and custody-assessed projections.
18. The per-row tracing event retains its existing level, fields, and message, and scoped tracing capture proves its actual contract and ordering without a helper-local counter or public API.
19. `sweep_orphans` still returns unit, discards the report explicitly, preserves its independent guard canonicalization, warning, early return, `root_cwd` decisions, and raw action-scan argument.
20. The same-root conformance matrix passes, including deterministic shared pin failure, while iterator-status projection remains intentionally different.
21. Non-canonical, absolute-missing, relative-refusal, stable-alias, and deterministic guard-failure tests prove the exact root-spelling requirements.
22. The complete state×claim×probe and guard/legacy characterization matrices are tested without relying on no-op byte replacement, permission behavior, inode reuse, or filesystem races.
23. The real persisted `Preserved` plus valid claim plus vanished target plus `BothAbsent` fixture remains raw `Authorized`, while production `effective()` remains empty.
24. `EXACT_ABSENCE_POLICY_READY_V1` remains false, and A2 production does not construct increment-2 admission or subject-construction arms.
25. Registration-path characterization proves per-field decode-before-compare, immediate `Same` return, the three required field orderings, and the exact invalid-current-field `ConfigInvalid` result without adding a two-pass predecode.
26. Report-path invalid-UTF-8 evidence retains the affected row, produces raw `Refused`, and captures the unchanged `Refused` decision event.
27. The exact non-UTF-8 custody name survives into `enumerated_name()` without reconstruction from lossy display text.
28. The test suite distinguishes genuine runtime red, base-green characterization, new-seam mechanism evidence, capability evidence, tracing evidence, and compiler-totality evidence exactly as specified.
29. The external API test consumes `.next()` as `Option<&ExactAbsenceSweepEntryV1>` and does not rely on `let _ = report.effective()`.
30. The return-type audit covers every named expression context and confirms that the five binary boot callers need no CLI change.
31. A1’s four temporary constructor `dead_code` allowances and stale “A1 before A2” constructor comments are removed or updated once production uses the constructors; the fifteen public types and their API remain otherwise unchanged.
32. The mutation audit records every named observation/effect and symbol-to-symbol no-action path without claiming arbitrary probes, tracing sinks, or the subsequent compatibility/action phase are effect-free.
33. No ownership, locking, transition, publication, settlement, deletion, prune, rename, backend-cleanup, or T3b authority is introduced.
34. The mandatory `bridge-worktree` evidence runs on macOS/APFS and Ubuntu/Linux ext4; overlayfs and non-Unix evidence are classified only as specified.
35. The handoff exists at the required repository path, uses the required headings, contains the pending operator block, and follows the implementation-candidate/evidence-commit protocol without claiming a commit’s self-hash.
36. The final evidence tree remains within every allocated counted-line row and the 1,650-line total; a projected excess causes the specified A2a/A2b split proposal rather than compressed evidence.
37. The operator supplies fmt, clippy, a full locked workspace suite, both mandatory filesystem rows, candidate identity, and handoff-only evidence-commit proof before completion. Any excluded or failing gate remains explicit.

## Files

- `crates/bridge-worktree/src/sweep.rs`
  - declare the private checked-scan module;
  - extract the shared display classifier;
  - implement the mandatory `scan_checked_rows_with_source` engine;
  - delegate both production scan projections through that engine;
  - preserve the action scanner’s raw-root projection;
  - implement eager report population and the accepted return-type change;
  - add the private decision-event and compatibility/action helpers;
  - add characterization, ordering, tracing, routing, capability, and report tests.
- `crates/bridge-worktree/src/sweep/checked_scan.rs`
  - create with the literal rustfmt-normalized seam, private compatibility session, real compatibility source, default finish behavior, classifier, and module-local construction tests.
- `crates/bridge-worktree/src/sweep/report.rs`
  - retain the fifteen-type API and false readiness gate;
  - remove the four production-obsolete constructor allowances;
  - update stale A1-only constructor documentation;
  - pin `effective()`’s item type and retain all existing projection tests.
- `crates/bridge-worktree/src/host_git.rs`
  - add direct field-ordered invalid-UTF-8 registration-path characterization evidence only;
  - do not change production registration, early-return, or comparator behavior.
- `crates/bridge-worktree/tests/r2f1b_exact_absence_report_api.rs`
  - add compiler-totality evidence for the public report-returning function and exact borrowed iterator item type.
- `docs/superpowers/reviews/2026-08-19-r2f1b-3d-t3a-inc1-sliceA2-handoff.md`
  - create from the inline handoff requirements;
  - retain operator fields as pending in the implementation candidate;
  - become the only changed file in the follow-up evidence commit.
- `crates/bridge-worktree/src/custody.rs`
  - read-only production reference for state×claim rules, name selection, decoding, and custody refusals; do not modify unless the falsification license stops the task.
- `crates/bridge-worktree/src/provider_path.rs`
  - read-only production reference for lenient canonicalization and silent legacy omission; do not modify.
- `crates/bridge-core/src/fs_custody.rs`
  - read-only production reference for birthtime capability, root pinning, and valid-UTF-8 path-identity observation trees; do not modify.
- `bin/a2a-bridge/src/main.rs`
  - read-only caller-audit reference; no CLI changes.
- `Cargo.toml`
  - read-only semver reference for workspace version and publication status; do not modify in this slice.
- `crates/bridge-worktree/Cargo.toml`
  - read-only semver reference showing inherited package version and default publishability; do not modify in this slice.

## Spec Refs

Authoritative at base commit `c637e493544a2e2edd1ca3ae20842a86dcb58f3f`:

- `Cargo.toml`
- `crates/bridge-worktree/Cargo.toml`
- `crates/bridge-worktree/src/sweep.rs`
- `crates/bridge-worktree/src/sweep/report.rs`
- `crates/bridge-worktree/src/host_git.rs`
- `crates/bridge-worktree/src/custody.rs`
- `crates/bridge-worktree/src/provider_path.rs`
- `crates/bridge-core/src/fs_custody.rs`
- `crates/bridge-worktree/tests/r2f1b_exact_absence_report_api.rs`
- `bin/a2a-bridge/src/main.rs`

## Commit Message

feat(worktree): return exact-absence sweep reports

Add one production-bound compatibility checked-scan engine, preserve eager
two-phase traversal, and populate ordered exact-absence reports with exact names,
root status, and unchanged raw decisions.

Keep compatibility/action scanning on the raw supplied root, preserve per-field
registration parsing and policy-readiness refusal, and add deterministic tracing,
filesystem-capability, conformance, and no-action evidence for the T3a report
route.
