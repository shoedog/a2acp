---
task-type: implement
---

# R2f1b 3d T3a increment 1, slice A2 — compatibility traversal and exact-absence report population

## Description

Implement slice A2 against exact base commit `c637e493544a2e2edd1ca3ae20842a86dcb58f3f`.

Repository inspection at that commit confirms the landed A1 surface: `crates/bridge-worktree/src/sweep/report.rs` is 598 lines, `sweep.rs` re-exports the fifteen stated public report types, `sweep_orphans_with_exact_absence` still returns unit, `scan_worktree_records` still eagerly returns a `Vec`, and the five binary boot callers invoke `sweep_orphans` in statement position. This task changes traversal/report production while preserving the current compatibility/action behavior.

Before editing, verify the exact base commit and re-read every authoritative file under “Spec Refs.” If any factual anchor below is false at that commit, follow the falsification license instead of adapting the implementation to a stale claim.

### Scope and settled boundaries

A2 owns:

- the private checked-scan module and compatibility source;
- the shared record-selection/read policy;
- eager two-phase traversal and ordering evidence;
- public report population and the exact-absence return-type change;
- exact requested-root and canonical-root handling;
- compatibility/action scan separation;
- behavior characterization;
- the concrete report-route mutation audit;
- removal of A1’s four now-obsolete constructor `dead_code` allowances;
- the A2 handoff and pending operator-evidence block.

A2 does not:

- add the increment-2 population-admission rule;
- set `EXACT_ABSENCE_POLICY_READY_V1` to true;
- construct `IneligiblePopulation` or `CannotConstructSubject` production assessments;
- add ownership, locking, transition, publication, settlement, unlink, removal, prune, rename, or backend-cleanup authority;
- populate authoritative root captures in production;
- implement T3b or treat a report as action authority;
- change CLI behavior or the return type of `sweep_orphans`.

T3a decides and reports. T3b will independently re-open, re-read, re-bind, re-apply admission, re-prove exact absence, and retain its own lock and authority through any later action.

### Module placement and literal cross-module seam

Create `crates/bridge-worktree/src/sweep/checked_scan.rs` and declare it as a private child module of `sweep.rs`.

The following block is normative and must land byte-for-byte as the cross-module seam. Every item used by parent `sweep.rs`, and every type exposed through those signatures, is `pub(super)`. The concrete compatibility session remains module-private. Observation fields and `complete_identity` remain private to `checked_scan.rs`.

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

`CompatibilityCheckedScanRootSessionV1` implements the session trait but remains private. Parent `sweep.rs` sees only the trait object and `RootObservationSetV1`; it does not inspect session internals or observation fields.

Do not use `DirectoryIdentityV1::matches` in this classifier. At the base commit, that method treats absent birthtime on either side as a wildcard. This classifier instead requires three complete `(dev, ino, birthtime)` tuples. An absent capture or incomplete tuple yields `Unavailable`; only three complete tuples may yield `Pinned` or `IdentityChanged`. `Unavailable` outranks a mismatch elsewhere in the set.

A2 production `finish` returns `RootObservationSetV1::default()`, so production root classification remains `Unavailable`. Slice B owns real population of the three captures.

### Compatibility source and shared policy

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

Extract one private display-path classifier for legacy, custody, or ignored entries. Both `scan_worktree_records` and the compatibility traversal must call it. Do not introduce a second selection vocabulary. Both paths continue to use the existing production functions:

- `read_sidecar` for legacy reads;
- `is_custody_record_name` for custody selection;
- `read_custody_record_in` for custody reads.

The two traversals may differ only in their required accumulation and iterator-status projections.

A malformed or unreadable legacy sidecar remains silently omitted because `read_sidecar` returns `None`. It produces no report entry, probe call, or decision event. A malformed or otherwise refused custody record remains present as `UnreadableCustody` and does not count as an iterator error.

For deterministic action-scanner pin-failure evidence, `scan_worktree_records` may delegate to a private parent-module helper named `scan_worktree_records_with_pin_opener`. Production supplies `FilesystemCompatibilityPinOpenerV1`. Tests may substitute only the pin-open result. The helper must retain real `read_dir`, real iterator behavior, the shared display classifier, `read_sidecar`, `read_custody_record_in`, accumulation, and the existing return projection.

`scan_worktree_records(root)` retains all existing observables:

- its return type remains `Vec<(String, ScannedWorktreeRecordV1)>`;
- it passes the caller’s raw `root` spelling directly to `read_dir`;
- `read_dir` failure returns an empty vector;
- after successful `read_dir`, it calls the production pin opener on that same raw root;
- pin failure preserves legacy reads and refuses custody reads as not-pinnable;
- display selection and legacy reads use the lossy full path;
- custody reads use the exact `DirEntry::file_name()`;
- iterator-item errors are flattened;
- it performs no canonicalization of its enumeration argument.

### Exact-absence report flow

Change the public signature to this exact declaration:

```rust
pub fn sweep_orphans_with_exact_absence(
    root: &str,
    probe: &dyn ExactAbsenceProbeV1,
) -> ExactAbsenceSweepReportV1
```

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
6. Open the production compatibility source on the canonical root.
7. On source-open failure, return:
   - the exact requested root;
   - the canonical root;
   - `Refused(CannotEnumerate)`;
   - custody root `Unavailable`;
   - no entries.
8. Phase 1 drains `next_name` completely. For each successful name:
   - retain its exact `OsString`;
   - construct the current lossy display path beneath the canonical root;
   - run the shared display classifier;
   - immediately perform the applicable legacy or custody read;
   - collect the resulting intermediate row, if any, before requesting the next name.
9. Increment `skipped_entries` only for `next_name` item errors.
10. Continue until `next_name` returns `None`, then call and complete `finish`.
11. Phase 2 begins only after `finish`. Assess intermediate rows in enumeration order, invoke probes where applicable, construct public entries, and emit one unchanged decision event per retained row.
12. Return `Complete` when no iterator-item error occurred; otherwise return `Incomplete { skipped_entries }`.
13. Classify root observations independently of enumeration completeness.

Populate assessments as follows:

- valid legacy row: `ExactAbsenceRecordAssessmentV1::Legacy(decision)`;
- refused custody read: `ExactAbsenceRecordAssessmentV1::UnreadableCustody(refusal)`;
- decoded custody row:
  `ExactAbsenceRecordAssessmentV1::Custody(CustodyRecordAssessmentV1::new(CustodyStateSnapshotV1::from(&record.state), CustodyExactAbsenceAssessmentV1::Assessed(decision)))`.

A2 does not construct the dormant admission or subject-construction arms. Existing guards and candidate-construction failures continue to project as `Assessed(Refused)`.

Route the existing per-row event through a private helper in `sweep.rs`. Preserve its level, fields, and message exactly:

```rust
tracing::info!(record = path, ?decision, "made exact-absence decision");
```

The helper may have a test-only thread-local counter or sink. It must not add a public reporter API. Install and clear any test sink with panic-safe scoped state so concurrent tests do not share observations.

The traversal remains eager. Do not stream assessment or logging alongside enumeration. Every selected successful name is read before the following `next_name`, and no assessment, probe, or decision event occurs before `None` and completed `finish`.

Because `finish` precedes phase-2 assessment, root evidence and later row decisions are ordered historical evidence, not one coherent point-in-time snapshot and not retained authority.

### Root-spelling behavior

Missing-root behavior is input-shape dependent and must remain explicit:

- An absolute missing path beneath an existing ancestor is accepted by `canonicalize_lenient`, retains the precise appended canonical value, reaches source open, and reports `CannotEnumerate`.
- A missing relative leaf whose empty parent cannot be resolved reports `CannotCanonicalize` with no canonical root.

For a deliberately non-canonical but resolvable input, assert both:

- `requested_root().as_bytes()` exactly equals the supplied string bytes;
- `canonical_root()` equals the precise expected lenient-canonical value.

### Compatibility/action separation

`sweep_orphans` continues to discard the report, canonicalize its independent guard root, warn and return early on guard failure, and pass the raw supplied root to the action scanner.

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
- deterministic guard failure, proving the exact warning root and message and proving the action observer is never called.

Do not simulate guard failure by racing or exchanging filesystem objects. The private helper is the only permitted guard-failure seam.

`WorktreeRunEndGuard`, custody locking, recovery classification, and deletion paths continue to consume only the compatibility/action result.

### Compatibility/action conformance matrix

Using one real readable directory and identical root spelling and pin-opener outcome, prove:

| Fixture | Required conformance |
|---|---|
| Valid matching legacy sidecar | Both paths select it and read the same sidecar. |
| Malformed or unreadable legacy sidecar | Both paths omit it. |
| Valid custody record | Both paths select it and decode the same record. |
| Unreadable, malformed, over-bound, symlinked, directory-shaped, or multiply-linked custody entry | Both retain a custody row with the same refusal classification. |
| Unrelated filename | Both omit it. |
| Pin failure with valid legacy and custody names | Both preserve the legacy row and retain the custody row with the exact not-pinnable refusal. |

The pin-failure row must use `scan_worktree_records_with_pin_opener` and the real compatibility source with equivalent deterministic failing openers. Only pin creation is replaced.

Iterator status remains an intentional difference: the action scanner flattens item errors, while the report emits `Incomplete { skipped_entries }`. Test that separately. This matrix does not replace the canonical exact-root versus raw action-root test.

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

### Required tests and evidence classification

Use these test names or equally specific names preserving the stated evidence. Every final test must document the production mutation it catches.

| Required test | Evidence against untouched `c637e493` | Production mutation caught | Proving environments |
|---|---|---|---|
| `returns_report_with_exact_requested_and_lenient_canonical_roots` | Genuine runtime red | Returning unit, losing raw requested spelling, using direct canonicalization, or reporting the wrong canonical value | Portable deterministic filesystem fixture |
| `reports_absolute_missing_and_relative_uncanonicalizable_roots` | Genuine runtime red | Collapsing `CannotEnumerate` into `CannotCanonicalize`, losing the absolute missing canonical value, or inventing a relative canonical root | Portable deterministic filesystem fixture |
| `persisted_preserved_both_absent_is_raw_authorized_but_not_effective` | Genuine runtime red | Losing the raw `Authorized` compatibility result, enabling readiness early, dropping the persisted row, or calling the probe incorrectly | macOS/APFS and Linux overlayfs or ext4 with real Git |
| `invalid_utf8_registration_probe_error_is_reported_as_refused` | Genuine runtime red | Dropping the row/event or translating the probe error into authorization | Unix byte-path support; macOS/APFS and Linux overlayfs or ext4 |
| `registration_path_invalid_utf8_refuses_before_path_comparison` | Green characterization on the base | Moving UTF-8 decoding after the comparator or changing the exact `ConfigInvalid` reason | Unix byte construction; no filesystem race |
| `existing_decision_and_decode_matrix_is_preserved` | Green characterization on the base | Changing any state×claim rule, probe mapping, guard short circuit, or decoder refusal | macOS/APFS and Linux overlayfs or ext4 with real Git |
| `existing_scan_omits_bad_legacy_and_retains_bad_custody` | Green characterization on the base | Emitting malformed legacy rows, dropping custody-named refusals, or counting decode refusal as iterator failure | Portable deterministic entry fixtures; `MultiLink` subcase Unix-only |
| `root_observations_require_three_complete_equal_identities` | New-seam mechanism evidence; does not compile on the base | Reusing wildcard birthtime matching, allowing incomplete captures, or letting mismatch outrank unavailable | Synthetic identities; portable |
| `compatibility_open_refusal_never_calls_pin_opener` | New-seam mechanism evidence | Calling the pin opener before `read_dir` succeeds or treating pin failure as source-open failure | Deterministic missing directory; portable |
| `compatibility_pin_failure_preserves_legacy_and_refuses_custody` | New-seam compatibility evidence | Suppressing all rows on pin failure, refusing legacy reads, or losing the exact custody refusal | Real readable directory with injected pin failure; portable subject to existing custody support |
| `checked_scan_reads_before_next_and_finishes_before_assessment` | New-seam ordering evidence | Streaming, prefetching the next entry before the current read, probing before drain, or assessing/logging before `finish` | Fully injected iterator/order log; portable |
| `checked_scan_counts_only_iterator_errors` | New-seam status evidence | Counting custody refusal as skipped, stopping at the first item error, reordering successful rows, or coupling root classification to enumeration completeness | Injected `Ok, Err, Ok, Err`; portable |
| `compatibility_and_action_scans_conform_on_one_root` | Refactor conformance evidence | Drifting selection, legacy omission, custody refusal classification, exact names, or pin-failure policy between paths | Real directory; Unix for symlink, hard-link, and non-UTF-8 subcases |
| `exact_non_utf8_custody_name_survives_enumeration` | New report-retention evidence | Reconstructing the name from lossy display text or storing only `String` | Unix; macOS/APFS and Linux overlayfs or ext4 |
| `exact_scan_canonical_and_action_scan_raw_alias` | Root-routing conformance evidence | Sending the raw alias to exact enumeration or sending canonical `root_cwd` to the action scanner | Unix symlink support; macOS/APFS and Linux |
| `compatibility_action_guard_failure_warns_and_skips_action_scan` | Helper conformance evidence | Continuing after guard failure, changing the warning, or scanning a raw root after refusal | Injected guard plus scoped tracing subscriber; portable |
| `exact_absence_public_return_type_and_call_contexts_are_total` | Compiler-totality evidence, not behavioral red | Retaining the unit return, omitting the public report type, or leaving an in-repository unit-constrained call context | Compiler evidence |
| Existing `raw_decision_and_historical_eligibility_projections` | Existing A1 mechanism evidence | Weakening the readiness gate, historical scan table, legacy exclusion, raw projection, or borrowed-entry behavior | Portable |

For the four genuine runtime-red tests, use a local test-only adapter implemented for both `()` and `ExactAbsenceSweepReportV1`. On the untouched base, the function runs and the adapter maps its unit result to no report, causing a runtime failure at the explicit “report required” boundary. After the signature changes, the same tests continue into typed field and behavior assertions. This separates genuine runtime red from compiler-only return-shape evidence.

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
- malformed custody inclusion without increasing `skipped_entries`;
- exact non-UTF-8 custody-name retention;
- raw decision event count and order;
- malformed legacy omission causing zero probe calls and zero events;
- every row of the historical scan-evidence table retained by the A1 report test.

The return-type audit must inspect statement-position calls, explicit unit bindings, unit-returning function pointers, unit-constrained closures, function-body tail expressions, unified `if` and `match` branches, generic consumers that inferred unit, and macro expression contexts. Record the result in the handoff. At the base, the only in-repository exact-function call is the statement-position call inside `sweep_orphans`; the five binary boot callers call `sweep_orphans` and require no change.

Do not rely on inode reuse after unlink-and-recreate. Where an exchange test is necessary, pre-create a distinct replacement and rename it so simultaneous objects cannot share identity. Report which real environments ran filesystem-dependent tests; one filesystem result is not universal evidence.

### Registration-path UTF-8 boundary

`registration_absent_from_porcelain` applies `std::str::from_utf8` to every `worktree ` field before calling `compare_path_identities`.

An invalid-UTF-8 path must return:

```rust
BridgeError::ConfigInvalid {
    reason: "worktree registration path is not valid UTF-8".to_string(),
}
```

That branch performs no comparator, resolver, or case-sensitivity observation. Scope the comparator inventory below only to valid-UTF-8 fields.

Direct decode-refusal evidence and report-projection evidence are both required. When the same error is returned by the report’s probe path, retain the custody or legacy row, produce raw `Refused`, and emit the unchanged `Refused` decision event.

### Mutation audit

Audit only the concrete production report route through `HostGitWorktree::observe_exact_absence`. A downstream implementation of the public `ExactAbsenceProbeV1` may perform arbitrary effects and is outside this proof. The compatibility/action handling that follows the discarded report in `sweep_orphans` is also outside the report-traversal no-action proof.

The normative observation/effect inventory is:

- the exact-absence entry-point `canonicalize_lenient`, including repeated `std::fs::canonicalize` observations while locating the nearest existing ancestor;
- compatibility `std::fs::read_dir` and iterator-item reads;
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
- UTF-8 decoding of every `worktree ` field before path comparison;
- the invalid-UTF-8 refusal branch, which performs no comparator, resolver, or case-mode observation and maps to raw `Refused`;
- the byte-identical valid-UTF-8 comparator short circuit, which returns `Same` without filesystem resolution;
- every non-byte-identical valid-UTF-8 field flowing through
  `registration_absent_from_porcelain → compare_path_identities → compare_path_identities_with_resolver`;
- the initial and final stability-bracket calls to `deepest_existing_path` for both paths;
- every resolver walk’s `std::fs::metadata`, `NotFound`-distinguishing `std::fs::symlink_metadata`, deepest-ancestor identity capture, canonicalization, canonical-object metadata, and identity check;
- repetition of complete resolver snapshots before retaining a verdict, with drift producing `CannotProve`;
- the ASCII-case-only missing-tail branch’s `case_sensitive_at` observation:
  `read_dir` of the resolved ancestor, at most 64 entries, entry `symlink_metadata`, alternate-case `symlink_metadata`, and recheck of the original sampled name;
- allocation, collection, subprocess execution, and tracing.

Pure tail comparison and ASCII-case transformation add no filesystem effect.

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

Review burden is measured in logical lines, not physical wrapping or bytes. Exact Rust blocks explicitly labeled byte-for-byte in this specification are pre-reviewed and exempt. Everything freely authored counts.

Count each semantic Rust item, field, variant, arm, statement, assertion, fixture case, or test case once regardless of wrapping. Count each nonblank semantic handoff or evidence line once. Perform the final measurement on a clean, fully committed tree. `git diff --numstat` may identify changed files but is not the metric and cannot account safely for staged or untracked work.

| Counted component | Logical-line budget |
|---|---:|
| Freely authored `checked_scan.rs` production implementation | 180 |
| `sweep.rs` traversal, report population, shared-policy, event, and action helpers | 240 |
| `report.rs` allowance and stale-comment cleanup | 20 |
| Checked-scan, report-population, ordering, characterization, and conformance tests | 770 |
| Host-Git UTF-8 and external public-return compiler evidence | 120 |
| Handoff, mutation audit, return audit, and evidence accounting | 140 |
| Contingency | 180 |
| **Total counted cap** | **1,650** |

Before editing, re-estimate every component against the base. If the work will exceed the total row, stop and report the revised component estimate and proposed split. Do not compress declarations, tests, mutation evidence, operator evidence, or the handoff to fit. Do not silently extend the boundary after editing begins.

### Handoff requirements

Create `docs/superpowers/reviews/2026-08-19-r2f1b-3d-t3a-inc1-sliceA2-handoff.md`.

Do not consult a template or path outside the repository. The implementation environment exposes the code tree only. Write the handoff from these inline requirements; the operator applies any installed host template separately.

Use these headings in order:

- `## Summary`
- `## What changed`
- `## Evidence`
- `## OPERATOR EVIDENCE — PENDING`
- `## Limits and disclosures`
- `## Sizing`

The handoff must record:

- exact base and candidate commit identities;
- every changed file;
- the eager two-phase ordering and compatibility/action root separation;
- the test-name/evidence classification above;
- which tests or checks the implementer actually ran and which it could not run;
- the exact genuine-red control results supplied by the operator;
- the return-type context audit;
- the same-root conformance result;
- every Unix-only test and non-Unix allowance, recording “none” where applicable;
- the concrete mutation inventory and symbol-to-symbol no-action call paths;
- that production root classification remains `Unavailable`;
- that production policy readiness remains false;
- that reports retain ordered historical evidence rather than authority;
- that arbitrary probe implementations and the later compatibility/action phase are excluded from the no-action proof;
- that byte snapshots cannot exclude mutation followed by restoration;
- the pre-edit estimate and final operator-measured logical-line count;
- that final clean-tree and commit-keyed evidence is operator-owned and must not be fabricated inside a commit that cannot attest itself.

The implementation container has no compile loop. Do not install dependencies, use network access, or invent test results. The operator runs the host gates and fills the pending block.

### OPERATOR EVIDENCE — PENDING

- [ ] `cargo fmt --all -- --check` — PENDING OPERATOR
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings` — PENDING OPERATOR
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — PENDING OPERATOR

Copy these three checkbox lines into the handoff under its `## OPERATOR EVIDENCE — PENDING` heading and leave them unticked. Report test totals as test binaries plus doc-test suites; do not double-count nested filtered subprocess output. Do not claim a green Windows all-target baseline unless one actually ran.

### Falsification license

Every symbol, caller count, matrix row, and behavioral statement in this task is an operator claim measured against `c637e493`. The repository is authoritative.

If any named symbol is absent; the A1 report surface differs; `read_sidecar` does not silently omit failures; the two entry points do not enumerate different root spellings; `sweep_orphans` does not preserve its guard warning and early return; the state decoder admits a different state×claim population; the UTF-8 refusal does not precede comparison; a non-byte-identical valid-UTF-8 field does not reach the stated resolver tree; `PinnedDirectoryV1::open` differs from the stated observation tree; a listed call edge differs; or any matrix result is wrong, record the exact source evidence and stop before editing.

Finding the work smaller than described is a good outcome. The A1/A2 split, T3a-decides/T3b-acts boundary, T3b action-time re-decision, and exclusion of new ownership plumbing remain settled even if another factual anchor is disproved.

## Acceptance Criteria

1. Work begins only from exact base `c637e493544a2e2edd1ca3ae20842a86dcb58f3f`, after the pre-edit factual and sizing checks.
2. `sweep/checked_scan.rs` exists and carries the complete literal cross-module seam exactly as specified.
3. The concrete compatibility session and observation internals remain private to `checked_scan.rs`; parent `sweep.rs` sees only the declared seam.
4. The root classifier requires three complete tuples, returns `Unavailable` for every incomplete case, returns `Pinned` only for three equal complete tuples, and returns `IdentityChanged` only for three complete tuples with a mismatch.
5. Production compatibility `finish` returns the default observation set, so A2 production root evidence remains `Unavailable`.
6. The compatibility source refuses open only when `read_dir` fails, calls the pin opener only after successful `read_dir`, preserves legacy reads on pin failure, and emits the exact not-pinnable custody refusal.
7. Both scanners use one shared display-path classifier and the existing production read functions. No second selection or read-policy vocabulary exists.
8. `scan_worktree_records` retains its public signature, eager vector behavior, raw-root enumeration, flattened iterator errors, legacy omission, exact custody names, and pin-failure semantics.
9. `sweep_orphans_with_exact_absence` has the exact report-returning signature and performs exactly one supplied-root `canonicalize_lenient` conversion.
10. Canonicalization and source-open failures produce the exact requested root, canonical-root option, refusal, unavailable root observation, and empty entry set specified above.
11. Phase 1 drains and reads eagerly; `finish` completes before phase 2; assessment, probes, and events preserve enumeration order.
12. Iterator errors alone determine `skipped_entries`; custody read/decode refusals remain entries and do not increase that count.
13. Report entries retain lossy display paths and exact `OsString` names and use the stated legacy, unreadable-custody, and custody-assessed projections.
14. The per-row tracing event retains its existing level, fields, and message, and a private test-only sink proves its count and ordering without adding public API.
15. `sweep_orphans` still returns unit, discards the report explicitly, preserves its independent guard canonicalization, warning, early return, `root_cwd` decisions, and raw action-scan argument.
16. The same-root conformance matrix passes, including deterministic shared pin failure, while iterator-status projection remains intentionally different.
17. Non-canonical, absolute-missing, relative-refusal, stable-alias, and deterministic guard-failure tests prove the exact root-spelling requirements.
18. The complete state×claim×probe and guard/legacy characterization matrices are tested without relying on no-op byte replacement, permission behavior, inode reuse, or filesystem races.
19. The real persisted `Preserved` plus valid claim plus vanished target plus `BothAbsent` fixture remains raw `Authorized`, while production `effective()` remains empty.
20. `EXACT_ABSENCE_POLICY_READY_V1` remains false, and A2 production does not construct increment-2 admission or subject-construction arms.
21. Direct invalid-UTF-8 registration evidence proves the exact `ConfigInvalid` refusal before comparison; report-path evidence proves emitted raw `Refused` and an unchanged event.
22. The exact non-UTF-8 custody name survives into `enumerated_name()` without reconstruction from lossy display text.
23. The test suite distinguishes genuine runtime red, base-green characterization, new-seam mechanism evidence, and compiler-totality evidence exactly as specified.
24. The return-type audit covers every named expression context and confirms that the five binary boot callers need no CLI change.
25. A1’s four temporary constructor `dead_code` allowances and stale “A1 before A2” constructor comments are removed or updated once production uses the constructors; the fifteen public types and their API remain otherwise unchanged.
26. The mutation audit records every named observation/effect and symbol-to-symbol no-action path without claiming arbitrary probes, tracing sinks, or the subsequent compatibility/action phase are effect-free.
27. No ownership, locking, transition, publication, settlement, deletion, prune, rename, backend-cleanup, or T3b authority is introduced.
28. The handoff exists at the required repository path, uses the required headings, contains the pending operator block, and reports all evidence and exclusions honestly.
29. The final clean committed artifact remains within the declared counted cap; exceeding the pre-edit estimate causes a stop and revised split proposal rather than compressed evidence.
30. The operator supplies fmt, clippy, and full locked workspace-test evidence before completion. Any excluded or failing gate remains explicit.

## Files

- `crates/bridge-worktree/src/sweep.rs`
  - declare the private checked-scan module;
  - extract the shared display classifier;
  - preserve and optionally delegate the action scanner;
  - implement eager report population and the return-type change;
  - add the private decision-event and compatibility/action helpers;
  - add characterization, ordering, routing, and report tests.
- `crates/bridge-worktree/src/sweep/checked_scan.rs`
  - create with the literal seam, private compatibility session, real compatibility source, default finish behavior, classifier, and module-local construction tests.
- `crates/bridge-worktree/src/sweep/report.rs`
  - retain the fifteen-type API and false readiness gate;
  - remove the four production-obsolete constructor allowances;
  - update stale A1-only constructor documentation;
  - retain and do not weaken existing projection tests.
- `crates/bridge-worktree/src/host_git.rs`
  - add direct invalid-UTF-8 registration-path characterization evidence only;
  - do not change production registration or comparator behavior.
- `crates/bridge-worktree/tests/r2f1b_exact_absence_report_api.rs`
  - add compiler-totality evidence for the public report-returning function and its public return type.
- `docs/superpowers/reviews/2026-08-19-r2f1b-3d-t3a-inc1-sliceA2-handoff.md`
  - create from the inline handoff requirements; do not consult anything outside the repository.
- `crates/bridge-worktree/src/custody.rs`
  - read-only production reference for state×claim rules, name selection, decoding, and custody refusals; do not modify unless the falsification license stops the task.
- `crates/bridge-worktree/src/provider_path.rs`
  - read-only production reference for lenient canonicalization and silent legacy omission; do not modify.
- `crates/bridge-core/src/fs_custody.rs`
  - read-only production reference for root pinning and valid-UTF-8 path-identity observation trees; do not modify.
- `bin/a2a-bridge/src/main.rs`
  - read-only caller-audit reference; no CLI changes.

## Spec Refs

Authoritative at base commit `c637e493544a2e2edd1ca3ae20842a86dcb58f3f`:

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

Add the compatibility checked-scan source, preserve eager two-phase traversal, and
populate ordered exact-absence reports with exact names, root status, and unchanged
raw decisions.

Keep the compatibility/action scan raw-root behavior and policy-readiness refusal
intact, with deterministic ordering, pin-failure, characterization, and no-action
evidence for the T3a report route.
