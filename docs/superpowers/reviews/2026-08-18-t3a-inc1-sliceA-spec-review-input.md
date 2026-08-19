---
task-type: spec-review
---

# Spec review — T3a increment 1 slice A

## Description

Review the implementation task spec reproduced verbatim below, before dispatch.
Approve it or send it back. The session cwd is checked out at `main` = `9aedf175`,
the base it targets, and the repository is authoritative.

### Provenance — you authored this revision

Round 1 on the transcribed spec returned 5 blockers / 12 findings — the first
convergence this work has shown. The operator then hand-folded those findings and
round 2 returned **7** blockers, four of which were contradictions introduced by that
hand-folding: a forbidden symbol still referenced three times, a crate-private claim
left standing beside a now-public type, a superseded seam-test bullet, and two
different line caps.

So the operator changed the pipeline. **You wrote this revision**, folding both
rounds' findings and owning the whole document. The operator's role was dispatch,
mechanical extraction between markers, and verification against the code — no
authoring.

Operator-verified on the extracted document before dispatch: the four hand-folding
contradictions are gone (zero references to the forbidden accessor, no surviving
crate-private-observation claim, no superseded ordering bullet, one cap stated once);
it contains **twenty literal type declarations** across five Rust blocks, which is the
artefact round-2 blocker 1 said no prior spec had produced; and the characterization
matrix survives with its load-bearing row intact.

Two round-2 findings were resolved with mechanisms rather than warnings, and are worth
judging on their merits: `effective()` is now a **filtered view yielding only borrowed
authorized entries**, so no decision value leaves the report and the separable-`Copy`
problem is closed structurally; and pin-failure evidence is required through the real
compatibility source.

Sizing is now stated honestly at a 1,000-line cap against a 980-line component
estimate, with a pre-specified A1/A2 follow-up split if the pre-edit estimate
overflows. Judge whether that estimate is credible — but note the spec explicitly
forbids compressing declarations, tests, evidence or handoff to fit, which is the
behaviour previous caps punished.

### What must not be undone

The spec deliberately declares that its only base-compatible red is an API-shape
assertion and that no behavioral red exists for slice A; that it defines arms
production never constructs; and that `effective_decision_at` always returns
`Some(Refused)` in slice A. Those are the contract, not gaps. "Add a behavioral red
test", "remove the unused arms", or "make the effective decision meaningful now"
would each undo a re-scope already paid for twice. If you think the shape is wrong,
say so as a design objection rather than as a spec defect.

## The spec under review

```markdown
# R2f1b 3d T3a increment 1, slice A — public shape, projections, and a compatibility-backed report

## Description

Base: `main` = `9aedf175`.

This is **slice A of two**. It lands the complete public reporting vocabulary, raw
and effective projections, a compatibility-backed report, an injectable scanner
seam, the private three-capture classifier shape, and the characterization matrix.
**Slice B** later supplies descriptor enumeration, populates the three root
observations, wires authoritative pinned-root classification, adds platform gating,
and provides real-Git authority evidence.

**Slice A is behavior-preserving.** It changes no raw decision, admission rule, or
refusal. Its compatibility source returns an empty root-observation set, so
production root authority truthfully remains `Unavailable`.

**T3a decides; T3b acts.** No path introduced or changed here writes, renames,
unlinks, removes, prunes, publishes, settles, or transitions custody state. Existing
action paths remain separate.

### Settled boundaries

- Add no NEW ownership input, variant, or plumbing. `decide_unused_candidate` keeps
  `recovery_owned: bool`, and both production call sites continue to pass `false`.
- Increment 2 installs population admission and construction guards. Increment 3
  supplies retained authority. Their public vocabulary lands now, but their
  production behavior does not.
- The effective-projection readiness gate remains `false` through slice B and may
  become true only in the same change that lands increment 2’s refusing admission
  rule.
- Add no `bridge-core` surface, `libc`, `fdopendir`, descriptor enumeration, or
  platform-conditional production functionality.
- Slice A adds no authoritative enumeration-root pinning beyond the existing
  `PinnedDirectoryV1` used for custody-record reads.
- `#[cfg_attr(not(unix), allow(dead_code))]` remains permitted where a private seam
  is necessarily unused on non-Unix, and `#[cfg(unix)]` remains permitted for
  inherently Unix-only tests.
- Preserve the eager two-phase traversal: enumerate and read all records first;
  assess, probe, and log only after enumeration has finished.
- Preserve compatibility pin failure: successful `read_dir` plus failed
  `PinnedDirectoryV1::open` still permits legacy rows and emits each custody row as
  the existing `"sweep root is not pinnable"` unreadable refusal.

### File organization

Keep the public path `bridge_worktree::sweep::*`, but split the new implementation
to keep the existing large `sweep.rs` reviewable:

- `sweep/report.rs` owns the fifteen public types, accessors, conversions, raw
  projection, effective projection, and readiness gate.
- `sweep/checked_scan.rs` owns crate-private scanner traits, compatibility source,
  pin-opener seam, raw root observations, and classifier.
- `sweep.rs` re-exports the public vocabulary, owns traversal integration and the
  reporting helper, and retains tests whose fixtures exercise existing private
  sweep logic.

This decomposition does not expand `bridge-core` or change the public module path.

### Literal public API

The following declarations are normative. Implement them literally, including
names, variants, payloads, fields, derives, visibility, accessors, conversions, and
method signatures. These are the fifteen new public types; do not add another public
report, capability, or observation type.

```rust
use std::ffi::{OsStr, OsString};

use crate::custody::{
    CustodyReadRefusalV1, PreservationReasonV1, WorktreeCustodyStateKindV1,
    WorktreeCustodyStateV1,
};

use super::UnusedCandidateDecisionV1;

/// False in slice A and slice B. Increment 2 may change this only in the same
/// change that installs its refusing population-admission rule.
const EXACT_ABSENCE_POLICY_READY_V1: bool = false;

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use]
pub struct ExactAbsenceSweepReportV1 {
    requested_root: String,
    canonical_root: Option<String>,
    scan: ExactAbsenceScanStatusV1,
    entries: Vec<ExactAbsenceSweepEntryV1>,
}

impl ExactAbsenceSweepReportV1 {
    pub(crate) fn new(
        requested_root: String,
        canonical_root: Option<String>,
        scan: ExactAbsenceScanStatusV1,
        entries: Vec<ExactAbsenceSweepEntryV1>,
    ) -> Self {
        Self {
            requested_root,
            canonical_root,
            scan,
            entries,
        }
    }

    #[must_use]
    pub fn requested_root(&self) -> &str {
        &self.requested_root
    }

    #[must_use]
    pub fn canonical_root(&self) -> Option<&str> {
        self.canonical_root.as_deref()
    }

    #[must_use]
    pub fn scan(&self) -> &ExactAbsenceScanStatusV1 {
        &self.scan
    }

    #[must_use]
    pub fn entries(&self) -> &[ExactAbsenceSweepEntryV1] {
        &self.entries
    }

    /// Reports scan authority only. It does not imply policy readiness or action
    /// eligibility.
    #[must_use]
    pub fn has_authoritative_scan(&self) -> bool {
        matches!(
            self.scan.enumeration(),
            ExactAbsenceEnumerationV1::Complete
        ) && self.scan.custody_root() == CustodyRootObservationV1::Pinned
    }

    /// Yields only effectively authorized entries. Refused entries are absent.
    ///
    /// This deliberately returns borrowed entries rather than `(entry, decision)`
    /// tuples or copyable effective-decision values. The exact enumerated name and
    /// its authorization therefore remain one object. Future action code must
    /// consume these yielded entries and use `enumerated_name()` directly.
    ///
    /// Slice A yields no entries because root authority is `Unavailable` and the
    /// policy-readiness gate is false.
    pub fn effective(&self) -> impl Iterator<Item = &ExactAbsenceSweepEntryV1> {
        self.entries
            .iter()
            .filter(move |entry| self.entry_is_effectively_authorized(entry))
    }

    fn entry_is_effectively_authorized(
        &self,
        entry: &ExactAbsenceSweepEntryV1,
    ) -> bool {
        EXACT_ABSENCE_POLICY_READY_V1
            && self.has_authoritative_scan()
            && !matches!(
                entry.assessment(),
                ExactAbsenceRecordAssessmentV1::Legacy(_)
            )
            && entry.assessment().decision()
                == UnusedCandidateDecisionV1::Authorized
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExactAbsenceScanStatusV1 {
    enumeration: ExactAbsenceEnumerationV1,
    custody_root: CustodyRootObservationV1,
}

impl ExactAbsenceScanStatusV1 {
    pub(crate) fn new(
        enumeration: ExactAbsenceEnumerationV1,
        custody_root: CustodyRootObservationV1,
    ) -> Self {
        Self {
            enumeration,
            custody_root,
        }
    }

    #[must_use]
    pub fn enumeration(&self) -> &ExactAbsenceEnumerationV1 {
        &self.enumeration
    }

    #[must_use]
    pub fn custody_root(&self) -> CustodyRootObservationV1 {
        self.custody_root
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExactAbsenceEnumerationV1 {
    Complete,
    Incomplete { skipped_entries: usize },
    Refused(ExactAbsenceRootRefusalV1),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExactAbsenceRootRefusalV1 {
    CannotCanonicalize,
    CannotEnumerate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CustodyRootObservationV1 {
    Pinned,
    IdentityChanged,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExactAbsenceSweepEntryV1 {
    record_path: String,
    enumerated_name: OsString,
    assessment: ExactAbsenceRecordAssessmentV1,
}

impl ExactAbsenceSweepEntryV1 {
    pub(crate) fn new(
        record_path: String,
        enumerated_name: OsString,
        assessment: ExactAbsenceRecordAssessmentV1,
    ) -> Self {
        Self {
            record_path,
            enumerated_name,
            assessment,
        }
    }

    #[must_use]
    pub fn record_path(&self) -> &str {
        &self.record_path
    }

    #[must_use]
    pub fn enumerated_name(&self) -> &OsStr {
        &self.enumerated_name
    }

    #[must_use]
    pub fn assessment(&self) -> &ExactAbsenceRecordAssessmentV1 {
        &self.assessment
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExactAbsenceRecordAssessmentV1 {
    Legacy(UnusedCandidateDecisionV1),
    UnreadableCustody(CustodyReadRefusalV1),
    Custody(CustodyRecordAssessmentV1),
}

impl ExactAbsenceRecordAssessmentV1 {
    /// Raw, behavior-compatible reporting projection. This is not action
    /// eligibility; action-facing code must iterate the report's `effective()`
    /// projection.
    #[must_use]
    pub fn decision(&self) -> UnusedCandidateDecisionV1 {
        match self {
            Self::Legacy(decision) => *decision,
            Self::UnreadableCustody(_) => UnusedCandidateDecisionV1::Refused,
            Self::Custody(custody) => match custody.assessment() {
                CustodyExactAbsenceAssessmentV1::IneligiblePopulation(_) => {
                    UnusedCandidateDecisionV1::Refused
                }
                CustodyExactAbsenceAssessmentV1::CannotConstructSubject(_) => {
                    UnusedCandidateDecisionV1::Refused
                }
                CustodyExactAbsenceAssessmentV1::Assessed(decision) => *decision,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustodyRecordAssessmentV1 {
    state: CustodyStateSnapshotV1,
    assessment: CustodyExactAbsenceAssessmentV1,
}

impl CustodyRecordAssessmentV1 {
    pub(crate) fn new(
        state: CustodyStateSnapshotV1,
        assessment: CustodyExactAbsenceAssessmentV1,
    ) -> Self {
        Self { state, assessment }
    }

    #[must_use]
    pub fn state(&self) -> &CustodyStateSnapshotV1 {
        &self.state
    }

    #[must_use]
    pub fn assessment(&self) -> &CustodyExactAbsenceAssessmentV1 {
        &self.assessment
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CustodyExactAbsenceAssessmentV1 {
    /// Increment 2 constructs this after population admission refuses.
    IneligiblePopulation(IneligiblePopulationV1),
    /// Increment 2 constructs this when a guard or authority construction refuses.
    CannotConstructSubject(CannotConstructSubjectV1),
    /// Slice A constructs this to retain the current raw decision.
    Assessed(UnusedCandidateDecisionV1),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IneligiblePopulationV1 {
    /// `ProtectionPrepared` without a claim. Its claim is schema-optional, so this
    /// is not a malformed or missing-required-claim result.
    BareProtectionPrepared,
    /// A canonically decoded state outside increment 2's candidate population.
    /// The enclosing `CustodyRecordAssessmentV1` carries the exact state snapshot.
    StateNotCandidate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CannotConstructSubjectV1 {
    RecordedWorktreePathNotAbsolute,
    OutsideSweepRoot,
    RecordFileNotExpectedSibling,
    ClaimAuthorityUnavailable(ClaimAuthorityUnavailableV1),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClaimAuthorityUnavailableV1 {
    object: ClaimAuthorityObjectV1,
    reason: ClaimAuthorityUnavailableReasonV1,
}

impl ClaimAuthorityUnavailableV1 {
    #[must_use]
    pub const fn new(
        object: ClaimAuthorityObjectV1,
        reason: ClaimAuthorityUnavailableReasonV1,
    ) -> Self {
        Self { object, reason }
    }

    #[must_use]
    pub const fn object(&self) -> ClaimAuthorityObjectV1 {
        self.object
    }

    #[must_use]
    pub const fn reason(&self) -> ClaimAuthorityUnavailableReasonV1 {
        self.reason
    }
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimAuthorityObjectV1 {
    Source,
    Root,
    Worktree,
    CommonDirectory,
    SourceCommonDirectoryBinding,
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimAuthorityUnavailableReasonV1 {
    PathMismatch,
    NotAbsolute,
    IdentityIncomplete,
    ObservationUnavailable,
    IdentityChanged,
    OwnershipUnproven,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CustodyStateSnapshotV1 {
    kind: WorktreeCustodyStateKindV1,
    preservation_reason: Option<PreservationReasonV1>,
}

impl CustodyStateSnapshotV1 {
    #[must_use]
    pub const fn kind(&self) -> WorktreeCustodyStateKindV1 {
        self.kind
    }

    #[must_use]
    pub const fn preservation_reason(&self) -> Option<PreservationReasonV1> {
        self.preservation_reason
    }
}

impl From<&WorktreeCustodyStateV1> for CustodyStateSnapshotV1 {
    fn from(state: &WorktreeCustodyStateV1) -> Self {
        match state {
            WorktreeCustodyStateV1::ProtectionPrepared {} => Self {
                kind: WorktreeCustodyStateKindV1::ProtectionPrepared,
                preservation_reason: None,
            },
            WorktreeCustodyStateV1::UnusedSettled {} => Self {
                kind: WorktreeCustodyStateKindV1::UnusedSettled,
                preservation_reason: None,
            },
            WorktreeCustodyStateV1::Materializing {} => Self {
                kind: WorktreeCustodyStateKindV1::Materializing,
                preservation_reason: None,
            },
            WorktreeCustodyStateV1::LiveProtected {} => Self {
                kind: WorktreeCustodyStateKindV1::LiveProtected,
                preservation_reason: None,
            },
            WorktreeCustodyStateV1::PreservationPrepared {} => Self {
                kind: WorktreeCustodyStateKindV1::PreservationPrepared,
                preservation_reason: None,
            },
            WorktreeCustodyStateV1::Preserved {} => Self {
                kind: WorktreeCustodyStateKindV1::Preserved,
                preservation_reason: None,
            },
            WorktreeCustodyStateV1::DeleteAuthorized {} => Self {
                kind: WorktreeCustodyStateKindV1::DeleteAuthorized,
                preservation_reason: None,
            },
            WorktreeCustodyStateV1::Removed {} => Self {
                kind: WorktreeCustodyStateKindV1::Removed,
                preservation_reason: None,
            },
            WorktreeCustodyStateV1::RecoveredLive { .. } => Self {
                kind: WorktreeCustodyStateKindV1::RecoveredLive,
                preservation_reason: None,
            },
            WorktreeCustodyStateV1::PreservationUnknown { reason } => Self {
                kind: WorktreeCustodyStateKindV1::PreservationUnknown,
                preservation_reason: Some(*reason),
            },
        }
    }
}

impl From<WorktreeCustodyStateV1> for CustodyStateSnapshotV1 {
    fn from(state: WorktreeCustodyStateV1) -> Self {
        Self::from(&state)
    }
}
```

Public structs have private fields and read-only accessors. Public enum payloads
remain ordinary public payloads, as Rust requires. Production in slice A constructs
only `Legacy`, `UnreadableCustody`, and
`Custody(CustodyRecordAssessmentV1 { assessment: Assessed(..), .. })`. Do not
production-construct the increment-2 arms, hide them behind `cfg(test)`, or remove
them.

`CustodyStateSnapshotV1` retains neither an entire custody record nor
`RecoveredLive`’s predecessor digest. Its conversion explicitly names all ten
custody states, and only `PreservationUnknown` retains a
`PreservationReasonV1`.

### Effective-projection safety

`has_authoritative_scan()` means exactly:

| Enumeration | Custody-root observation | Result |
|---|---|---|
| `Complete` | `Pinned` | `true` |
| `Complete` | `IdentityChanged` or `Unavailable` | `false` |
| `Incomplete` or `Refused` | any value | `false` |

It describes scan evidence, not action authority. Public raw `decision()` remains
available for reporting and compatibility assertions.

`effective()` is an authorization-capable filtered view, not a tuple-valued decision
map:

| Condition | `effective()` result for the row |
|---|---|
| Enumeration is not `Complete` | row omitted |
| Custody root is not `Pinned` | row omitted |
| Policy readiness is false | row omitted |
| Scan authoritative and ready, but row is `Legacy` | row omitted |
| Scan authoritative and ready, non-legacy raw decision is `Refused` | row omitted |
| Scan authoritative and ready, non-legacy raw decision is `Authorized` | borrowed entry yielded |

This resolves the separable-`Copy` problem mechanically: no effective
`UnusedCandidateDecisionV1` value leaves the report. The only positive effective
projection is the borrowed entry containing the exact enumerated name. Future action
code must iterate this view and act on the yielded entry itself.

The raw `decision()` remains intentionally unsuitable for action. Combining it with
`has_authoritative_scan()` still does not establish readiness; the method’s name and
doc comment must make that boundary explicit.

The readiness constant remains false in slice A and slice B. Otherwise, after slice
B first produces `Pinned`, a `Preserved` record with valid claim, vanished target,
and `BothAbsent` could become effectively authorized before increment 2 installs its
admission rule. Test an otherwise authoritative report containing a non-legacy raw
`Authorized` row and assert that `effective().next()` is `None`.

### Literal crate-private scanner and classifier declarations

The following private declarations are also normative:

```rust
use std::ffi::{OsStr, OsString};
use std::path::Path;

use bridge_core::fs_custody::{BirthTimeV1, PinnedDirectoryV1};

use crate::custody::{CustodyReadRefusalV1, WorktreeCustodyRecordV1};
use crate::provider_path::WorktreeSidecar;

use super::report::CustodyRootObservationV1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CheckedScanOpenRefusalV1 {
    CannotEnumerate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CheckedScanEntryRefusalV1 {
    CannotReadEntry,
}

trait CheckedScanSourceV1 {
    fn open(
        &self,
        enumeration_root: &Path,
    ) -> Result<Box<dyn CheckedScanRootSessionV1>, CheckedScanOpenRefusalV1>;
}

trait CheckedScanRootSessionV1 {
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
trait CompatibilityPinOpenerV1 {
    fn open_pin(&self, enumeration_root: &Path) -> Option<PinnedDirectoryV1>;
}

#[derive(Clone, Copy, Debug, Default)]
struct FilesystemCompatibilityPinOpenerV1;

impl CompatibilityPinOpenerV1 for FilesystemCompatibilityPinOpenerV1 {
    fn open_pin(&self, enumeration_root: &Path) -> Option<PinnedDirectoryV1> {
        PinnedDirectoryV1::open(enumeration_root, "worktree sweep root").ok()
    }
}

#[derive(Clone, Copy, Debug)]
struct RootIdentityCaptureV1 {
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
struct RootObservationSetV1 {
    retained_enumeration_object: Option<RootIdentityCaptureV1>,
    pinned_custody_directory: Option<RootIdentityCaptureV1>,
    final_named_root: Option<RootIdentityCaptureV1>,
}

fn classify_root_observations(
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

All scanner-seam and raw-observation types are crate-private.
`CustodyRootObservationV1` is the public classified result.

The classifier deliberately does not use `DirectoryIdentityV1::matches`. At the
base commit, that method treats missing birthtime on either side as a wildcard,
whereas this authority classification requires complete `(dev, ino, birthtime)`
captures. A missing capture or any incomplete tuple yields `Unavailable`; only
three complete tuples can yield `Pinned` or `IdentityChanged`. `Unavailable`
therefore outranks mismatch.

The classifier remains in slice A because its exact private handoff shape and public
classification semantics must be frozen before slice B replaces the source.
Production still passes an empty set and therefore returns `Unavailable`. This is
the project’s fail-closed object-identity model, not a claim that filesystem object
IDs can never be reused.

### Compatibility source and deterministic pin-failure evidence

Implement `CompatibilityCheckedScanSourceV1<P>` over the current
`std::fs::read_dir`, parameterized only by `P: CompatibilityPinOpenerV1`.

Its open sequence is exact:

1. Call `std::fs::read_dir(enumeration_root)`.
2. Return `CheckedScanOpenRefusalV1::CannotEnumerate` only if that call fails.
3. After successful `read_dir`, call `open_pin`.
4. Retain the resulting `Option<PinnedDirectoryV1>` in the real compatibility
   session.
5. `read_legacy` continues through path-based `read_sidecar` regardless of the pin.
6. With no pin, `read_custody` returns
   `CustodyReadRefusalV1::Unreadable("sweep root is not pinnable".to_string())`.
7. `finish` returns `RootObservationSetV1::default()`.

The generic fake scanner proves iterator states and ordering, but it is not
admissible evidence for compatibility pin behavior. Add a separate test that
constructs the real `CompatibilityCheckedScanSourceV1` with a deterministic failing
pin opener. Use an actual readable directory containing a valid legacy sidecar and
a custody-named entry. The test must prove:

- the real source’s actual `read_dir` succeeds;
- open returns a session rather than `CannotEnumerate`;
- the legacy row is present and read through the production legacy implementation;
- the custody row is present as the exact not-pinnable refusal;
- enumeration is `Complete`;
- the compatibility source’s production `finish` still classifies as
  `Unavailable`.

This test replaces only pin creation. It must not replace the compatibility source,
its session, enumeration, row selection, reads, or finish behavior.

### Scan flow

Change:

```rust
pub fn sweep_orphans_with_exact_absence(
    root: &str,
    probe: &dyn ExactAbsenceProbeV1,
) -> ExactAbsenceSweepReportV1
```

Its behavior is:

1. Retain the supplied `root` byte-for-byte as `requested_root`.
2. Call the existing `canonicalize_lenient` exactly once. Do not substitute
   `std::fs::canonicalize`.
3. On lenient-canonicalization failure, return `canonical_root: None`,
   `Refused(CannotCanonicalize)`, root `Unavailable`, and no entries.
4. Open the compatibility source on the canonical root.
5. On source-open failure, return the canonical root,
   `Refused(CannotEnumerate)`, root `Unavailable`, and no entries.
6. Phase 1 drains `next_name`. For every successful yielded name, construct the
   current lossy display path, apply the existing display-based selection
   predicates, immediately perform the applicable legacy or custody read, and
   collect an intermediate row before requesting the next name.
7. Count only `next_name` item errors in `skipped_entries`. A malformed or otherwise
   unreadable custody record becomes an emitted unreadable row and does not increase
   that count.
8. Continue until `next_name` returns `None`, then call `finish`.
9. Only after `finish` completes may phase 2 assess, invoke the exact-absence probe,
   or emit a decision event.
10. Phase 2 assesses the collected rows in enumeration order, constructs the public
    entries, and logs each raw `assessment.decision()` through the unchanged event
    shape.
11. Return `Complete` when no iterator-item error occurred; otherwise return
    `Incomplete { skipped_entries }`. Root classification is independent of that
    enumeration result.

The ordering tests must assert two different properties:

- every selected successful name is read and collected before the next
  `next_name` invocation; and
- no assessment, probe call, or decision event occurs until `next_name` has returned
  `None` and `finish` has completed.

A single assertion that a row is “processed” before the next name is insufficient
because it could incorrectly permit probing or logging during enumeration.

Legacy `read_sidecar` returns `None` on either read or JSON failure at the base
commit. Preserve that as silent omission: no public entry, probe call, or decision
event.

A missing root is not a lenient-canonicalization failure: the helper canonicalizes
the nearest existing ancestor and appends the missing tail. It must therefore reach
source open and report `CannotEnumerate`.

### Compatibility/action scan separation

`scan_worktree_records(root)` keeps every existing observable:

- it enumerates the caller’s raw spelling;
- `read_dir` failure returns an empty vector;
- pin failure does not prevent legacy reads;
- display selection and legacy reads use the current lossy full path;
- custody reads use the exact `DirEntry::file_name()`;
- iterator-item errors are flattened;
- the return type remains `Vec<(String, ScannedWorktreeRecordV1)>`.

The exact-absence report enumerates the canonical root. The compatibility/action
scan continues to enumerate its caller’s raw spelling. A symlinked-root alias test
must assert both entry points retain today’s distinct path behavior.

`sweep_orphans` explicitly discards the report:

```rust
let _ = sweep_orphans_with_exact_absence(
    root,
    &crate::host_git::HostGitWorktree::new(),
);
```

It then performs its existing independent compatibility/action scan unchanged.
`WorktreeRunEndGuard`, custody locking, recovery classification, and deletion paths
continue to consume only the compatibility result.

The return-type change affects more than statement-position callers: audit explicit
unit bindings, unit-returning function pointers, unit-constrained closures,
function-body tail expressions, unified `if`/`match` branches, generic consumers
that inferred unit, and macro expression contexts. The five binary boot callers call
`sweep_orphans` in statement position and require no CLI behavior change.

### Characterization matrix

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
`Authorized`. Its effective projection in slice A yields no entry because the root
is `Unavailable` and policy readiness is false. Do not weaken or reinterpret this
row.

`MultiLink` is asserted only on Unix. Permission-dependent unreadability is
supplementary; primary refusal tests use deterministic entry type, symlink,
injected-open, or decode failures.

### Required evidence

Slice A has exactly one base-compatible runtime red, and it is an API-shape
assertion:

```rust
let report = sweep_orphans_with_exact_absence(root, probe);
assert!(std::mem::size_of_val(&report) > 0);
```

On the base, the returned unit value has size zero. Label this as API-shape evidence,
not decision-behavior evidence. Slice A has no behavioral red and must not
manufacture one.

Raw behavior is supported by runtime characterization. Exhaustive production and
test-side matches provide compiler totality. Record these as separate claims.

Required deterministic seam and projection tests:

- lenient canonicalization failure;
- missing root reaches `CannotEnumerate`;
- source-open refusal makes zero custody-pin calls;
- complete enumeration;
- injected `Ok, Err, Ok, Err` produces
  `Incomplete { skipped_entries: 2 }`;
- every yielded selected row is read before the following `next_name`;
- no assessment, probe, or decision event occurs before `None` and completed
  `finish`;
- equal complete three-capture identities classify `Pinned`;
- unequal complete identities classify `IdentityChanged`;
- any absent capture, absent `dev`, absent `ino`, or absent birthtime classifies
  `Unavailable`;
- iterator incompleteness remains independent of root classification;
- the real compatibility source with only its pin opener forced to fail preserves
  legacy rows and emits custody rows as not-pinnable;
- malformed legacy omission causes zero probe calls and zero matching decision
  events;
- malformed custody inclusion does not increment `skipped_entries`;
- exact non-UTF-8 custody-name identity survives from enumeration into
  `enumerated_name()`;
- a symlinked-root alias preserves canonical exact-scan paths and raw
  compatibility-scan paths;
- every scan/root combination in the `has_authoritative_scan()` table;
- policy readiness false makes an otherwise authoritative raw-`Authorized` custody
  row absent from `effective()`;
- legacy rows never appear in `effective()`, even under an otherwise authoritative,
  ready test-only projection.

For decision-event observation, route production’s existing per-row tracing call
through a crate-private helper and install a test-only thread-local counter or sink
at that helper. Do not add a public reporter API or alter the tracing event’s fields,
level, or message.

### Mutation audit

Audit only the concrete production route through
`HostGitWorktree::observe_exact_absence`. A downstream implementation of the public
`ExactAbsenceProbeV1` can perform arbitrary effects and is outside this proof.

Allowed observations and effects on the concrete route are:

- lenient canonicalization;
- `read_dir` traversal;
- existing unbounded legacy `std::fs::read`;
- bounded custody reads and canonical decoding;
- descriptor and metadata observation;
- allocation and collection;
- `git rev-parse`;
- `git worktree list --porcelain -z`;
- tracing.

Record the call-path evidence showing no application edge from the report traversal
to provider remove or prune, worktree removal, `remove_dir_all`, unlink, rename,
custody publication or replacement, settlement, transition, backend cleanup, or
T3b action.

Do not call the path globally effect-free: Git subprocesses and tracing exist, and a
configured tracing sink may write. Byte snapshots are corroborating final-content
evidence only; they cannot exclude mutation followed by restoration.

### Falsification license

Every anchor, symbol, caller count, matrix row, and behavioral statement in this
task is an operator claim measured against `9aedf175`; the checked-out repository is
authoritative. If a symbol is absent, `read_sidecar` does not silently omit, the two
entry points do not enumerate different root spellings, the state decoder admits a
different population, or any matrix result is wrong, record the exact source
evidence and stop rather than forcing the implementation to match this task.
Finding the work smaller than described is a good outcome.

The T3a-decides/T3b-acts boundary and exclusion of new ownership plumbing remain
settled even if another factual anchor is disproved.

### Sizing and mandatory pre-edit stop

Single limit: at most **1,000 added-plus-deleted lines**, including production,
tests, module glue, and handoff, measured by the operator from
`9aedf175..<final SHA>` with `git diff --numstat` on a clean committed tree.

Indicative component budget:

| Component | Lines |
|---|---:|
| Public vocabulary, accessors, conversions, re-export | 285 |
| Raw/effective projections and readiness gate | 55 |
| Scanner seam, compatibility source, pin opener, classifier | 145 |
| Traversal, wiring, and private reporting helper | 80 |
| Characterization tests | 190 |
| Seam, ordering, projection, and classifier tests | 120 |
| Installed-template handoff | 105 |
| Total estimate | 980 |

Before editing, produce a component estimate using the literal declarations above.
If it exceeds the single limit, stop and propose a follow-up split of
A1 vocabulary/projections from A2 compatibility traversal/characterization. Do not
compress declarations, binding, tests, evidence, or the handoff to fit. Proceeding
above the limit requires explicit operator waiver.

### Handoff and verification

Create
`docs/superpowers/reviews/2026-08-18-r2f1b-3d-t3a-inc1-sliceA-handoff.md` from the
installed `~/.claude/handoff-template.md`. Resolve and read that file; do not
recreate it from memory. Preserve every required template heading.

The handoff must state:

- slice A has no behavioral red;
- the sole red is the non-unit API-shape assertion;
- root authority is `Unavailable` and `effective()` yields no entries;
- raw characterization and compiler totality are different evidence claims;
- which tests the implementer executed and which it did not;
- every non-Unix lint allowance and Unix-only test;
- the report return-type compatibility audit;
- the production mutation-audit call path;
- final numstat and clean-tree evidence belong in an external receipt keyed to the
  final SHA because a committed handoff cannot attest its own final commit.

Include this block verbatim and leave it pending:

```markdown
## OPERATOR EVIDENCE — PENDING
- [ ] `cargo fmt --all -- --check` — PENDING OPERATOR
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings` — PENDING OPERATOR
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — PENDING OPERATOR
```

The implementation container has no compile loop. The operator runs the three host
gates above. Do not fabricate totals or silently omit an unrunnable gate.

Report test totals as the count of test binaries plus doc-test suites. Do not sum
every `test result:` line: a `bridge-core` test re-executes a filtered test binary,
and its nested harness output would inflate that sum.

`bridge-core` compiles for Windows in CI while existing worktree areas contain
Unix-only code and tests. Apply non-Unix dead-code allowances where required, but do
not claim a green Windows all-target baseline without running one.

## Acceptance Criteria

1. The fifteen public types match the normative Rust declarations exactly, including
   all variants, payloads, private struct fields, derives, accessors, constructors,
   conversions, and non-exhaustive annotations.
2. The public path remains `bridge_worktree::sweep::*`; internal module
   decomposition adds no `bridge-core` surface.
3. `CustodyRootObservationV1` is public. Scanner-seam types,
   `RootObservationSetV1`, and root identity captures are crate-private.
4. `ExactAbsenceSweepEntryV1` retains both `record_path: String` and
   `enumerated_name: OsString`; its exact-name accessor returns `&OsStr`, and future
   guards do not reconstruct a name from lossy display text.
5. `CustodyStateSnapshotV1` exhaustively converts all ten custody states, retains a
   reason only for `PreservationUnknown`, and retains no predecessor digest or whole
   record.
6. `ClaimAuthorityUnavailableV1` has private fields and the declared constructor and
   accessors. Its object and reason enums are non-exhaustive.
7. Raw `decision()` exhaustively matches every assessment arm without a wildcard.
   Its results match the stated projection.
8. `has_authoritative_scan()` implements every row of its truth table and is
   documented as scan evidence, not action authority.
9. `effective()` returns only borrowed effectively authorized entries and exposes no
   separable effective-decision scalar. Readiness remains false, legacy rows are
   excluded, and the specified projection tests pass.
10. The crate-private scanner traits, refusal enums, compatibility pin-opener seam,
    root-observation declarations, and classifier match their literal declarations.
11. The classifier compares three complete `(dev, ino, birthtime)` tuples directly;
    missing or incomplete evidence yields `Unavailable`.
12. Compatibility open refuses only when `read_dir` fails. The real compatibility
    source, with only its pin opener deterministically failed, proves that legacy
    rows remain present, custody rows use the existing not-pinnable refusal, and
    enumeration completes.
13. Enumeration/read collection and assessment/logging remain two explicit phases.
    The event-order test proves both the per-name read ordering and the absence of
    assessment, probe, or decision events before `finish`.
14. `skipped_entries` counts only iterator-item failures. Unreadable custody is
    emitted without incrementing it; malformed legacy remains silently omitted.
15. `canonicalize_lenient` remains the canonicalization helper, and a merely missing
    root reports `CannotEnumerate`.
16. `scan_worktree_records` retains every stated raw-spelling compatibility
    behavior. `sweep_orphans` explicitly discards the report and leaves its
    independent action scan unchanged.
17. The complete characterization matrix exists, including the load-bearing
    `Preserved` plus valid claim plus vanished target plus `BothAbsent` raw
    `Authorized` result and malformed-legacy silent omission.
18. Slice A production constructs only `Legacy`, `UnreadableCustody`, and custody
    `Assessed` results. Increment-2 arms remain public and test-constructible but
    dormant in production.
19. `decide_unused_candidate` retains `recovery_owned` and both `false` call sites.
    No ownership, custody state, transition, publication, settlement, deletion, or
    CLI behavior changes.
20. No `libc`, `fdopendir`, descriptor enumeration, authoritative enumeration-root
    pinning, or platform-conditional production functionality is added.
21. The sole base-compatible red is identified as API-shape evidence. No behavioral
    red is claimed; runtime characterization and compiler totality are reported
    separately.
22. The mutation audit is scoped to concrete `HostGitWorktree` production wiring,
    lists the allowed leaves, and makes no global effect-freedom claim.
23. The installed-template handoff exists at the required path, preserves the
    operator-evidence block verbatim, and records execution and platform exclusions
    honestly.
24. The pre-edit estimate is recorded beneath the sizing limit. If it is not, work
    stops before implementation and proposes the specified A1/A2 split.
25. Final operator totals count test binaries and doc-test suites without
    double-counting nested harness output.

## Files

- `crates/bridge-worktree/src/sweep.rs` — public re-exports, traversal integration,
  compatibility/action separation, reporting helper, and integration tests.
- `crates/bridge-worktree/src/sweep/report.rs` — create; literal public vocabulary,
  conversions, and projections.
- `crates/bridge-worktree/src/sweep/checked_scan.rs` — create; private scanner,
  compatibility source, pin-opener seam, raw observations, and classifier.
- `crates/bridge-worktree/src/custody.rs` — read for custody states, reasons, kinds,
  records, and read refusals; do not modify.
- `crates/bridge-worktree/src/provider_path.rs` — read for
  `canonicalize_lenient`, `WorktreeSidecar`, and `read_sidecar`; do not modify.
- `crates/bridge-worktree/src/host_git.rs` — read for the concrete exact-absence
  probe and mutation audit; do not modify.
- `bin/a2a-bridge/src/main.rs` — read for boot-caller compatibility; do not modify.
- `docs/superpowers/reviews/2026-08-18-r2f1b-3d-t3a-inc1-sliceA-handoff.md` —
  create from the installed template.

## Spec Refs

Authoritative at the base commit:

- `crates/bridge-worktree/src/sweep.rs`
- `crates/bridge-worktree/src/custody.rs`
- `crates/bridge-worktree/src/provider_path.rs`
- `crates/bridge-worktree/src/host_git.rs`
- `bin/a2a-bridge/src/main.rs`

## Commit Message

feat(worktree): return a typed exact-absence sweep report

The exact-absence sweep returned unit and exposed each decision only through
tracing. It now returns a typed report containing scan status, exact record identity,
typed assessments, and an exhaustive raw projection that preserves every current
decision.

The action-facing projection yields only borrowed effectively authorized entries,
so effective authority cannot be detached as a copyable decision and paired with a
different row. Policy readiness remains false, and root authority remains
Unavailable until the later descriptor slice, so slice A yields no effective
entries.

The compatibility source preserves eager two-phase ordering, malformed-legacy
omission, and pin-failure behavior. Characterization includes the current
Preserved-record fail-open: a valid claim, vanished target, and BothAbsent still
produce raw Authorized. A later increment closes that behavior with genuine
behavioral-red evidence.
```

---

## Acceptance Criteria

A useful review must:

1. **Rule APPROVE or REJECT**, enumerating every blocking objection with the
   instruction at fault and the wrong thing an implementer would do because of it.
   Label non-blocking improvements as such. Manufacturing a blocker to avoid
   approving is itself a failure mode here.
2. **Check the characterization matrix against the code.** Every row states a
   concrete expected value. Verify them. A wrong row is the most damaging possible
   defect, because the matrix is what will make the next increment's change provably
   red — especially the `Preserved` + valid claim + vanished target + `BothAbsent`
   ⇒ raw `Authorized` row, and the silently-omitted malformed legacy sidecar row.
3. **Test behavior preservation hardest.** Does anything specified here change a
   decision, an enumeration, an ordering, a log line, or the compatibility wrapper's
   observable semantics? The two entry points enumerate different roots today;
   confirm the spec preserves that.
4. **Check the slice boundary.** Is anything here slice B's work, and is anything B
   needs — particularly the frozen public shapes and the crate-private seam — absent
   or shaped so B would require a breaking API change?
5. **Verify the frozen public API is sufficient and implementable**: the fourteen
   types, the privacy split between structs and enum payloads, the `#[non_exhaustive]`
   payloads, and the `OsString` identity.
6. **Judge the evidence honestly.** Is the API-shape-only red claim true for slice
   A? Are the seam tests deterministic as specified? Is the mutation audit
   completable with the stated allowed leaves?
7. **Check sizing.** A 1,000-line cap against a 980-line component estimate. Is the
   estimate credible per component, and is the A1/A2 fallback split correctly cut?

Tag findings **BLOCKER** or **NON-BLOCKING**. A finding without a concrete
consequence for the implementer is non-blocking.

## Files

- `crates/bridge-worktree/src/sweep.rs` — the file the slice changes.
- `crates/bridge-worktree/src/custody.rs` — the frozen state machine; not modified.
- `crates/bridge-worktree/src/provider_path.rs` — `WorktreeSidecar` and `read_sidecar`.
- `crates/bridge-worktree/src/host_git.rs` — the probe, for the mutation audit.
- `bin/a2a-bridge/src/main.rs` — the five boot callers.

## Spec Refs

Your design for this increment is
`docs/superpowers/plans/2026-08-18-r2f1b-3d-t3a-increment1-task-v3.md`, on a planning
branch and **not** in this checkout. Its load-bearing content is transcribed above;
its absence is not a missing input.
