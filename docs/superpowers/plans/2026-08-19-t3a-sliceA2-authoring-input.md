---
task-type: design
---

# Author the T3a increment 1 slice A2 implementation task spec

## Description

Produce the **complete, dispatch-ready implementation task spec for slice A2** and emit it
between the extraction markers. You own the whole document. The session cwd is at `main` =
`c637e493`, authoritative for every factual claim.

### Where the lane stands

**A1 landed** (PR #60). Your A1 spec drove a clean implementation: `verify: PASS`, `review:
APPROVE` with zero findings from both reviewers, 698 lines against a 700-line cap, host gate
green. `main` now carries `crates/bridge-worktree/src/sweep/report.rs` (598 lines) and
re-exports the fifteen public types from `sweep.rs`:

```rust
mod report;

pub use report::{
    CannotConstructSubjectV1, ClaimAuthorityObjectV1, ClaimAuthorityUnavailableReasonV1,
    ClaimAuthorityUnavailableV1, CustodyExactAbsenceAssessmentV1, CustodyRecordAssessmentV1,
    CustodyRootObservationV1, CustodyStateSnapshotV1, ExactAbsenceEnumerationV1,
    ExactAbsenceRecordAssessmentV1, ExactAbsenceRootRefusalV1, ExactAbsenceScanStatusV1,
    ExactAbsenceSweepEntryV1, ExactAbsenceSweepReportV1, IneligiblePopulationV1,
};
```

**Ground the spec in that landed surface, not in A1's description of it.** If the two differ,
the repository wins and you should say so.

### What A2 is

A2 owns traversal, the compatibility source, report population, characterization, event
ordering, the return-type change, exact-root handling, and the mutation audit. You already
specified it in outline inside the A1 spec; that outline is authoritative scope and is
reproduced below with its headings demoted one level. Turn it into a full task spec.

### Hard-won constraints — carry these into the spec

Each was paid for in this lane. They are not stylistic.

- **Behaviour preservation governs the traversal.** Today's flow is EAGER: `scan_worktree_records`
  returns a `Vec`, so everything is enumerated and read before anything is assessed or logged.
  Preserve the two-phase ordering; do not stream.
- **`PinnedDirectoryV1::open(..).ok()`** means a pin failure leaves legacy rows proceeding while
  custody rows become the "not pinnable" refusal. `open()` must refuse only on `read_dir` failure.
- **`sweep_orphans` canonicalizes a guard root** with `canonicalize_lenient` and returns early on
  failure, while passing the RAW root to `scan_worktree_records`. Preserve both; scope any
  raw-spelling rule to `scan_worktree_records` alone.
- **A malformed legacy sidecar is silently OMITTED today** (`read_sidecar` returns `None`, no push).
  Preserve that; requiring it to appear would itself be a behaviour change.
- **`registration_absent_from_porcelain` decodes UTF-8 before the comparator**, so an invalid-UTF-8
  registration path returns `ConfigInvalid` and never reaches `compare_path_identities`. Scope any
  comparator inventory to valid-UTF-8 fields and include the decode-refusal branch.
- **The policy-readiness gate stays false** until the admission rule lands, so a `Preserved` record
  with a vanished target cannot become effectively authorized in the interval.

### Container reality — violating this stops the dispatch

The implement container mounts **only the code tree** plus a credentials file, and `HOME` is
`/root`. **Never instruct the implementer to read a path outside the repository.** An earlier spec
told it to read `~/.claude/handoff-template.md`; the agent correctly refused and produced nothing,
twice, before the cause was found. State handoff CONTENT requirements and name the required
headings inline; the operator applies any installed template on the host.

The container also has **no compile loop** (its focused test runs hit a crates.io 403), so the
operator runs the host gates. Include an `OPERATOR EVIDENCE — PENDING` heading with unticked
checkboxes and `PENDING OPERATOR` markers for fmt, clippy, and
`cargo test --workspace --locked --no-fail-fast`.

### Sizing — the owner changed the rule

Caps now price **review burden**, not bytes:

- Lines specified **literally, byte-for-byte** in the spec are pre-reviewed and do **not** count.
- Everything the implementer authors freely counts, measured as **logical lines of code**, not
  physical lines, which are gameable by wrapping.
- Measure on a clean, fully committed tree; `git diff --numstat` ignores staged and untracked bytes.

Give a cap you believe with a per-component budget, and a mandatory pre-edit stop that says to
report a revised estimate rather than compress declarations, tests, evidence or the handoff. If A2
does not fit one dispatch, say so and propose the split — that judgement was right last time.

### Test evidence — this lane has shipped four tests that proved less than they claimed

One passed on macOS/APFS and on the container's overlayfs and failed only on ubuntu/ext4, because it
depended on inode reuse after unlink-and-recreate. Another was a fixture whose substring replacement
was a no-op, so it exercised a different scenario entirely while two reviewers cited it as
confirming the fix. Therefore:

- For each test, say what production mutation it would catch, not merely what it asserts.
- Prefer deterministic injection over provoking real filesystem faults; where a test must construct
  real state, name the environments that can prove it.
- A2 CAN have genuine behavioural red, because A1's vocabulary already compiles on `main`. Name
  exactly which tests are red against `c637e493`, and why.

### The A2 outline you previously wrote — authoritative scope, reproduce it faithfully

#### A2 outline — not part of A1 implementation

A2 owns all traversal, compatibility-source, report-population, characterization,
event-ordering, return-type, exact-root, and mutation-audit work described below.
Do not partially implement it in A1.

##### A2 module placement and visibility

`sweep/checked_scan.rs` owns the scanner traits, compatibility source, pin-opener
seam, compatibility session, raw observations, and classifier. Every item used by
parent `sweep.rs`, and every type exposed through one of those signatures, is
`pub(super)`. The concrete compatibility session stays private to
`checked_scan.rs`. Observation fields and `complete_identity` stay private because
classification and its construction tests live in that module.

The following declarations are the A2 cross-module seam:

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
    fn next_name(
        &mut self,
    ) -> Option<Result<OsString, CheckedScanEntryRefusalV1>>;

    fn read_legacy(
        &self,
        enumerated_name: &OsStr,
        record_display: &str,
    ) -> Option<WorktreeSidecar>;

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
    let (
        Some(retained_enumeration_object),
        Some(pinned_custody_directory),
        Some(final_named_root),
    ) = (
        observations
            .retained_enumeration_object
            .and_then(|identity| identity.complete_identity()),
        observations
            .pinned_custody_directory
            .and_then(|identity| identity.complete_identity()),
        observations
            .final_named_root
            .and_then(|identity| identity.complete_identity()),
    )
    else {
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

`CompatibilityCheckedScanRootSessionV1` is module-private and implements the
`pub(super)` session trait. `sweep.rs` sees only the trait object and
`RootObservationSetV1`; it does not inspect session internals or observation fields.

The classifier does not use `DirectoryIdentityV1::matches`. At the base commit,
that method treats missing birthtime on either side as a wildcard, whereas this
classification requires complete `(dev, ino, birthtime)` captures. A missing
capture or incomplete tuple yields `Unavailable`; only three complete tuples can
yield `Pinned` or `IdentityChanged`. `Unavailable` outranks mismatch.

A2’s compatibility source returns `RootObservationSetV1::default()`, so production
classification remains `Unavailable`. Slice B replaces the source and populates
the three captures.

##### A2 compatibility source, shared policy, and deterministic pin-failure evidence

Implement `CompatibilityCheckedScanSourceV1<P>` over the current
`std::fs::read_dir`, parameterized only by `P: CompatibilityPinOpenerV1`.

Do not introduce a second independent selection or read-policy vocabulary. Extract
one private display-path classifier for legacy, custody, or ignored entries and use
it from both `scan_worktree_records` and the compatibility report traversal. Both
paths must continue to use the existing production `read_sidecar`,
`is_custody_record_name`, and `read_custody_record_in` machinery. Their wrappers may
differ only where this outline explicitly requires a distinct accumulation or
status projection.

Its open sequence is exact:

1. Call `std::fs::read_dir(enumeration_root)`.
2. Return `CheckedScanOpenRefusalV1::CannotEnumerate` only if that call fails.
3. After successful `read_dir`, call `open_pin`.
4. Retain the resulting `Option<PinnedDirectoryV1>` in the real compatibility
   session.
5. `read_legacy` continues through path-based `read_sidecar` regardless of the pin.
6. With no pin, `read_custody` returns
   `CustodyReadRefusalV1::Unreadable("sweep root is not pinnable".to_string())`.
7. `finish` returns `RootObservationSetV1::default()` and consumes the session.

The generic fake scanner proves iterator states and ordering, but it is not
admissible evidence for compatibility pin behavior. A separate test constructs the
real compatibility source with a deterministic failing pin opener and an actual
readable directory containing a valid legacy sidecar and a custody-named entry. It
must prove:

- the real source’s `read_dir` succeeds;
- open returns a session rather than `CannotEnumerate`;
- the legacy row is present and read through production `read_sidecar`;
- the custody row is present as the exact not-pinnable refusal;
- enumeration is `Complete`;
- production `finish` classifies as `Unavailable`.

Only pin creation is replaced. The test must not replace the compatibility source,
session, enumeration, selection, reads, or finish behavior.

For deterministic conformance of the action scanner’s same-root pin-failure row,
`scan_worktree_records` may delegate to one private parent-module helper named
`scan_worktree_records_with_pin_opener`. The public function supplies
`FilesystemCompatibilityPinOpenerV1`; tests may supply a failing
`CompatibilityPinOpenerV1`. The helper must retain the public function’s real
`std::fs::read_dir`, entry iteration, shared display-path classification,
`read_sidecar`, `read_custody_record_in`, accumulation, and return projection.
Tests substitute only the `open_pin` result. No scanner source, session, iterator,
selection rule, read result, or finish result is injectable through this helper.

Add a same-root conformance matrix using one real readable directory, the same root
spelling, and the same pin-opener outcome for
`scan_worktree_records_with_pin_opener` and the compatibility source:

| Fixture | Required conformance |
|---|---|
| Valid matching legacy sidecar | both select it and read the same sidecar |
| Malformed or unreadable legacy sidecar | both omit it |
| Valid custody record | both select it and decode the same record |
| Unreadable, malformed, over-bound, symlinked, directory-shaped, or multiply-linked custody entry | both retain a custody row with the same refusal classification |
| Unrelated filename | both omit it |
| Pin failure with valid legacy and custody names | both preserve the legacy row and retain the custody row with the exact not-pinnable refusal |

The pin-failure row must use the private helper with the same deterministic failing
opener behavior supplied to the real compatibility source. Production continues to
supply `FilesystemCompatibilityPinOpenerV1` on both paths.

Iterator-item status remains an intentional projection difference:
`scan_worktree_records` flattens item errors, while the report records
`Incomplete { skipped_entries }`. Test that distinction separately. The same-root
matrix must not replace the separate canonical-exact-root versus raw-action-root
test.

##### A2 scan flow and root-spelling evidence

A2 changes the signature to:

```rust
pub fn sweep_orphans_with_exact_absence(
    root: &str,
    probe: &dyn ExactAbsenceProbeV1,
) -> ExactAbsenceSweepReportV1
```

Its behavior is:

1. Copy the supplied `root` directly into `requested_root`, preserving its UTF-8
   bytes exactly.
2. At the exact-absence entry point, invoke the existing
   `canonicalize_lenient(root)` exactly once for the supplied sweep root.
   This count applies only to that entry-point root conversion. It does not include
   the existing `sweep_orphans` guard-root conversion, per-record
   `worktree_under_root` calls, the `std::fs::canonicalize` calls in sibling guards,
   or the internal ancestor loop inside `canonicalize_lenient`.
3. Do not substitute a direct `std::fs::canonicalize` call for the entry-point
   helper.
4. On lenient-canonicalization failure, return `canonical_root: None`,
   `Refused(CannotCanonicalize)`, root `Unavailable`, and no entries.
5. Open the compatibility source on the canonical root.
6. On source-open failure, return the canonical root,
   `Refused(CannotEnumerate)`, root `Unavailable`, and no entries.
7. Phase 1 drains `next_name`. For every successful yielded name, construct the
   current lossy display path, apply the shared display-path classifier,
   immediately perform the applicable legacy or custody read, and collect an
   intermediate row before requesting the next name.
8. Count only `next_name` item errors in `skipped_entries`. A malformed or otherwise
   unreadable custody record becomes an emitted unreadable row and does not increase
   that count.
9. Continue until `next_name` returns `None`, then call `finish`.
10. Only after `finish` completes may phase 2 assess, invoke the exact-absence probe,
    or emit a decision event.
11. Phase 2 assesses collected rows in enumeration order, constructs public
    entries, and logs each raw `assessment.decision()` through the unchanged event
    shape.
12. Return `Complete` when no iterator-item error occurred; otherwise return
    `Incomplete { skipped_entries }`. Root classification is independent of that
    enumeration result.

Ordering tests must independently prove:

- every selected successful name is read and collected before the following
  `next_name` invocation; and
- no assessment, probe call, or decision event occurs until `next_name` has returned
  `None` and `finish` has completed.

Because `finish` precedes phase-2 assessment, the resulting root observation and
later row decision are ordered historical evidence. A2 must not describe them as
one coherent point-in-time snapshot or as retained authority.

Legacy `read_sidecar` returns `None` on either read or JSON failure at the base.
Preserve that as silent omission: no public entry, probe call, or decision event.

Missing-root behavior is input-shape dependent:

- An absolute missing path beneath an existing ancestor is accepted by
  `canonicalize_lenient`, which canonicalizes that ancestor and appends the missing
  tail. It reaches source open and reports `CannotEnumerate`.
- A missing relative input such as `new-root` can fail while the helper attempts to
  resolve its empty parent. That case reports `CannotCanonicalize` with
  `canonical_root: None`.

Add a direct non-canonical-root test that supplies a deliberately non-canonical
string spelling and asserts:

- `requested_root().as_bytes()` equals the supplied string’s bytes exactly; and
- `canonical_root()` equals the precise expected lenient canonical value.

Add separate missing-root tests that assert:

- an absolute missing path below an existing ancestor retains the expected lenient
  canonical value and reports `CannotEnumerate`; and
- an absent relative leaf whose empty parent cannot be resolved has
  `canonical_root() == None` and reports `CannotCanonicalize`.

##### A2 compatibility/action scan separation

`scan_worktree_records(root)` retains every existing observable and performs no
root canonicalization of its enumeration input:

- it passes the caller’s raw spelling directly to `std::fs::read_dir`;
- `read_dir` failure returns an empty vector;
- after successful `read_dir`, it calls the production pin opener on that same raw
  root for custody reads;
- pin failure does not prevent legacy reads;
- display selection and legacy reads use the current lossy full path;
- custody reads use the exact `DirEntry::file_name()`;
- iterator-item errors are flattened;
- the return type remains `Vec<(String, ScannedWorktreeRecordV1)>`.

The exact-absence entry point enumerates its canonical root. The compatibility/action
wrapper must preserve its separate guard-root canonicalization while keeping the
action scan raw.

A2 may extract the compatibility/action phase into one private parent-module helper
named `sweep_compatibility_action_phase_with`. Production must call that helper with
the existing `canonicalize_lenient` guard operation and public
`scan_worktree_records` action scanner. Tests may substitute only the guard result
and an action-scan observer. The helper must retain the existing warning, early
return, `root_cwd` use, per-row compatibility decisions, and action handling.

The production sequence remains equivalent to:

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

Inside the private helper, the preserved guard and raw action scan remain:

```rust
let Ok(root_cwd) = guard_root(root) else {
    tracing::warn!(root, "skipping worktree sweep with non-canonical root");
    return;
};
for (path, scanned) in action_scan(root) {
    // Existing compatibility/action handling remains unchanged.
}
```

The production `guard_root` is `canonicalize_lenient`, and the production
`action_scan` is `scan_worktree_records`. The guard result supplies `root_cwd` to
the existing compatibility decisions. It must not become the enumeration argument:
the action scanner continues to receive the raw `root`.

Use two separate deterministic tests:

- A stable symlinked-root alias test uses a guard that succeeds. It proves that the
  exact-absence entry point supplies the canonical root to its scanner, that the
  compatibility guard resolves the expected canonical `root_cwd`, and that the
  compatibility/action scanner receives the raw alias spelling rather than
  `root_cwd`.
- A guard-failure test calls `sweep_compatibility_action_phase_with` with a
  deterministic failing guard and an action-scan observer. Through a scoped tracing
  subscriber, it must observe the existing warning with the supplied raw `root` and
  message `skipping worktree sweep with non-canonical root`; it must also prove the
  action-scan observer was never called. This test does not reuse the stable alias
  fixture or claim a successful raw action scan.

No test may simulate guard failure by changing the filesystem between phases. The
private helper is the only permitted guard-failure conformance seam.

`WorktreeRunEndGuard`, custody locking, recovery classification, and deletion paths
continue to consume only the compatibility result.

The return-type audit covers statement-position callers, explicit unit bindings,
unit-returning function pointers, unit-constrained closures, function-body tail
expressions, unified `if` and `match` branches, generic consumers that inferred
unit, and macro expression contexts. The five binary boot callers invoke
`sweep_orphans` in statement position and require no CLI behavior change.

##### A2 characterization matrix

For a readable custody record whose path guards pass and whose valid complete claim
constructs an `ExactAbsenceCandidateV1`:

| Population | Current raw decision |
|---|---|
| `ProtectionPrepared` with claim | probe mapping |
| `ProtectionPrepared` without claim | `Refused` |
| `PreservationPrepared` with required claim | probe mapping |
| `Preserved` with required claim | probe mapping |
| `PreservationUnknown`, any of six reasons, with required claim | probe mapping |
| `UnusedSettled`, `Materializing`, `LiveProtected`, `DeleteAuthorized`, `Removed`, `RecoveredLive` | `Refused` |
| Missing required claim or forbidden claim present | decode refusal, emitted `UnreadableCustody`, raw `Refused` |

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
| Custody worktree outside sweep root | raw `Refused`; probe not called |
| Custody record not the expected sibling | raw `Refused`; probe not called |
| Claim source/common/worktree cannot construct authority | raw `Refused`; probe not called |
| Undecodable, over-bound, symlinked, directory-shaped, or multiply-linked custody entry | emitted unreadable entry; raw `Refused` |
| Valid matching in-root legacy sidecar whose source/common authority constructs | probe mapping |
| Non-matching or outside-root legacy sidecar | raw `Refused`; probe not called |
| Malformed or unreadable legacy sidecar | silently omitted; no probe and no decision event |

The load-bearing fixture is a real persisted `Preserved` custody record with a valid
complete claim, a vanished target, and `BothAbsent`. Its raw result remains
`Authorized`. Its production `effective()` projection yields no entry because
policy readiness is false. Even after a future ready report yields it as a snapshot
candidate, T3b must re-prove the candidate under its own action lock.

`MultiLink` is asserted only on Unix. Permission-dependent unreadability is
supplementary; primary refusal tests use deterministic entry type, symlink,
injected-open, or decode failures.

A2’s deterministic tests additionally cover:

- lenient canonicalization refusal, including an absent relative leaf whose empty
  parent cannot be resolved;
- an absolute missing root beneath an existing ancestor reaching
  `CannotEnumerate`;
- source-open refusal making zero custody-pin calls;
- complete enumeration;
- injected `Ok, Err, Ok, Err` producing
  `Incomplete { skipped_entries: 2 }`;
- every selected row read before the following name;
- no assessment, probe, or decision event before `None` and completed `finish`;
- equal complete three-capture identities classifying `Pinned`;
- unequal complete identities classifying `IdentityChanged`;
- any absent capture, absent `dev`, absent `ino`, or absent birthtime classifying
  `Unavailable`;
- iterator incompleteness remaining independent of root classification;
- real compatibility pin failure preserving legacy rows and emitting custody rows
  as not-pinnable;
- the same-root compatibility/action conformance matrix, including its deterministic
  shared pin-failure opener;
- malformed legacy omission causing zero probe calls and decision events;
- malformed custody inclusion not incrementing `skipped_entries`;
- exact non-UTF-8 custody-name identity surviving from enumeration into
  `enumerated_name()`;
- an invalid-UTF-8 `worktree ` path field in porcelain producing the exact
  `ConfigInvalid` decode refusal before path-identity comparison;
- the same invalid-UTF-8 registration refusal, when returned through the report’s
  probe path, producing an emitted raw `Refused` assessment and unchanged
  `Refused` decision event;
- the stable canonical exact-scan versus raw alias action-scan test;
- the separate deterministic guard-failure test observing the warning and zero
  action-scan calls;
- exact requested-root spelling and canonical-root values for non-canonical,
  absolute-missing, and relative-refusal inputs;
- every scan/root combination in the historical scan-evidence table.

For decision-event observation, route production’s existing per-row tracing call
through a private helper in `sweep.rs` and install a test-only thread-local counter
or sink there. Do not add a public reporter API or alter the tracing event’s fields,
level, or message.

##### A2 mutation audit

Audit only the concrete report-production route through
`HostGitWorktree::observe_exact_absence`. A downstream implementation of the public
`ExactAbsenceProbeV1` can perform arbitrary effects and is outside this proof. The
independent compatibility/action handling that follows the discarded report in
`sweep_orphans` is also outside the report-traversal no-action proof; its existing
effects remain separately guarded.

The normative inventory of concrete observations and effects is:

- The exact-absence entry-point call to `canonicalize_lenient`, including its
  repeated `std::fs::canonicalize` observations while finding the nearest existing
  ancestor.
- Compatibility enumeration through `std::fs::read_dir`, including iterator item
  reads.
- The scanner’s post-`read_dir` `PinnedDirectoryV1::open` observation tree:
  `Path::canonicalize`; the first `directory_path_identity` call using
  `std::fs::symlink_metadata`; read-only no-follow directory open
  (`O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC` on Unix); descriptor `File::metadata`
  through `directory_identity`; the second `directory_path_identity` call using
  `std::fs::symlink_metadata`; and comparison of the before, descriptor, and after
  identities. This branch opens and observes; it does not call a sync or mutation
  method.
- Existing unbounded legacy `std::fs::read` through `read_sidecar`.
- Descriptor-relative custody open and bounded reads through
  `read_custody_record_in`, including file metadata, link-count and length
  observation, bounded byte reading, and canonical decoding.
- Per-record `worktree_under_root` calls to `canonicalize_lenient`.
- The ordinary `std::fs::canonicalize` calls in legacy and custody
  record/sibling-placement guards.
- `ExactAbsenceCandidateV1::from_legacy` and `from_claim` flowing through
  `capture_directory_identity`: absolute-path checks,
  `std::fs::canonicalize`, `verify_payload_directory_identity`,
  `std::fs::symlink_metadata`, its second `std::fs::canonicalize` self-resolution
  check, and metadata-derived directory identity.
- `source_common_dir_identity` invoking
  `git -C <source> rev-parse --path-format=absolute --git-common-dir`, followed by
  the same `capture_directory_identity` observation tree for the returned common
  directory.
- `HostGitWorktree::observe_exact_absence` revalidating source and common-directory
  identity before and after the registration probe, including the same
  canonicalization, symlink-metadata, metadata, and Git common-directory
  observations.
- Both target checks through `Path::symlink_metadata`, before and after the Git
  registration probe.
- The synchronous `git worktree list --porcelain -z` registration observation.
- For every porcelain field beginning with `worktree `,
  `registration_absent_from_porcelain` first applies `std::str::from_utf8` to the
  path bytes. An invalid-UTF-8 field returns
  `BridgeError::ConfigInvalid { reason: "worktree registration path is not valid UTF-8" }`
  before `compare_path_identities`; that branch performs no comparator, resolver, or
  case-sensitivity observation. The report path maps this probe error to raw
  `Refused`. Direct decode-refusal evidence and report-projection evidence are both
  required.
- Every valid-UTF-8 registration field flows to
  `compare_path_identities`. A byte-identical field returns `Same` through the
  comparator’s byte-identical short circuit without filesystem resolution.
- For each non-byte-identical, valid-UTF-8 registration field,
  `registration_absent_from_porcelain → compare_path_identities →
  compare_path_identities_with_resolver`.
- The comparator’s initial and final stability-bracket calls to
  `deepest_existing_path` for both the valid-UTF-8 porcelain path and candidate
  target. Each resolver walk performs `std::fs::metadata` on successive ancestors;
  on `NotFound`, `std::fs::symlink_metadata` distinguishes a genuinely missing
  component from another unresolved object; at the deepest existing ancestor it
  captures object identity, calls `std::fs::canonicalize`, calls
  `std::fs::metadata` on the canonical result, and verifies that the canonical
  object identity matches. The final pair of resolver calls repeats the complete
  snapshots before a computed verdict is retained; drift yields `CannotProve`.
- When missing tails differ only by ASCII case and therefore require the case-mode
  branch, `case_sensitive_at` calls `std::fs::read_dir` on the resolved ancestor
  and examines at most 64 entries. For each attempted sample it calls
  `std::fs::symlink_metadata` on the enumerated entry, probes the alternate-case
  name with `std::fs::symlink_metadata`, and rechecks the original sampled name
  with `std::fs::symlink_metadata` before accepting either an existing-alternate
  or `NotFound` answer. Pure tail comparison and ASCII-case transformation add no
  filesystem effect.
- Allocation, collection, subprocess execution, and tracing.

Record symbol-to-symbol call-path evidence showing no application edge from the
report traversal to provider remove or prune, `remove_worktree`,
`remove_worktree_if_safe`, `remove_dir_all`, `remove_file`, rename, custody
publication or replacement, settlement, transition, backend cleanup, or any T3b
action.

Do not call the path globally effect-free: Git subprocesses and tracing exist, and a
configured tracing sink may write. Byte snapshots are corroborating final-content
evidence only; they cannot exclude mutation followed by restoration.

#### Falsification license

Every symbol, caller count, matrix row, and behavioral statement in this task is an
operator claim measured against `9aedf175`; the checked-out repository is
authoritative. If a symbol is absent, `read_sidecar` does not silently omit, the two
entry points do not enumerate different root spellings, the state decoder admits a
different population, the `sweep_orphans` guard-root canonicalization or early
return differs, `registration_absent_from_porcelain` does not apply the stated
UTF-8 decode refusal before comparison, a non-byte-identical valid-UTF-8
registration field does not reach the stated path-identity observation tree,
`PinnedDirectoryV1::open` differs from the stated observation tree, a listed call
edge differs, or any matrix result is wrong, record the exact source evidence and
stop rather than forcing the implementation to match this task.

Finding the work smaller than described is a good outcome. The A1/A2 split,
T3a-decides/T3b-acts boundary, action-time T3b re-decision, and exclusion of new
ownership plumbing remain settled even if another factual anchor is disproved.


## Acceptance Criteria

The document you emit must:

1. Begin with `---`, then the design front matter line `task-type: implement`, then `---`, a `#`
   title, and the five headings Description, Acceptance Criteria, Files, Spec Refs, Commit Message
   in that order, with only the commit message under the last.
2. Carry the outline's literal declarations forward as literal declarations. Do not emit an
   instruction telling the implementer to write them.
3. Name the genuinely-red tests against `c637e493`, and state honestly what is characterization or
   compiler-totality evidence rather than behavioural red.
4. Preserve every behaviour listed under "Hard-won constraints", each stated as a requirement the
   implementer can check.
5. Contain **no path outside the repository**, and name the handoff headings inline.
6. Carry a falsification license: every anchor is an operator claim the implementer may disprove
   against the repository, and finding the work smaller is a good outcome.
7. Be internally consistent — one cap stated once, counts agreeing everywhere, nothing surviving
   that another instruction removes.

## Spec Refs

Authoritative in this checkout: `crates/bridge-worktree/src/sweep.rs`,
`crates/bridge-worktree/src/sweep/report.rs`, `crates/bridge-worktree/src/host_git.rs`,
`crates/bridge-worktree/src/custody.rs`, `crates/bridge-worktree/src/provider_path.rs`,
`crates/bridge-core/src/fs_custody.rs`.
