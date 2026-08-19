---
task-type: implement
---

# R2f1b 3d T3a increment 1, slice A1 — public reporting vocabulary and snapshot projections

## Description

Base: `main` = `9aedf175`.

This revision takes the A1/A2 split because the prior combined estimate was not
credible beneath its cap. This document specifies **A1 only**. A1 lands the complete
public reporting vocabulary, raw and filtered snapshot projections, the testable
policy-readiness predicate, module re-exports, projection tests, external public-API
compile evidence, and its handoff. It does not change traversal or the return type of
`sweep_orphans_with_exact_absence`.

A2 is specified in outline below. A2 will add the compatibility scanner seam,
compatibility-backed report population, exact-name traversal, characterization
matrix, ordering evidence, report return-type migration, and mutation audit. After
A2, the original slice A is complete. The later slice B supplies descriptor
enumeration, populates the three root observations, wires authoritative pinned-root
classification, adds platform gating, and provides real-Git authority evidence.

A1 is behavior-preserving. It changes no raw decision, admission rule, refusal,
scan, event, action, custody transition, or CLI behavior. Production does not
construct the new report types until A2.

### Settled boundaries

- T3a decides; T3b acts. T3a reports snapshot decisions and never conveys durable
  action authority.
- In this document, “snapshot” means an owned projection of values observed during
  an ordered scan-and-assessment sequence. It does not mean one coherent,
  simultaneous point-in-time filesystem snapshot.
- A returned report retains no open directory, descriptor, lock, lease, or opaque
  authority token. Once the scan session is consumed by `finish`, even a stored
  `CustodyRootObservationV1::Pinned` value is only historical evidence about that
  completed scan.
- `effective()` is a filtered **snapshot-eligibility** view. It is not action
  authority. T3b must re-read and re-decide the yielded candidate under T3b’s own
  action lock before any effect.
- Borrowed entries provide ergonomic coupling between a row, its exact enumerated
  name, and its assessment. They do not type-enforce inseparability: entries are
  cloneable, names can be copied, and raw decisions are copyable. The safety
  boundary is the absence of retained live authority and the mandatory T3b
  action-time re-decision.
- Add no `effective_decision_at` accessor or other public effective-decision scalar.
- Add no NEW ownership input, variant, or plumbing. `decide_unused_candidate` keeps
  `recovery_owned: bool`, and both production call sites continue to pass `false`.
- Increment 2 installs population admission and construction guards. Increment 3
  supplies retained action authority. Their vocabulary lands now, but their
  production behavior does not.
- The production policy-readiness constant remains `false` through A1, A2, and
  slice B. It may become true only in the same change that lands increment 2’s
  refusing population-admission rule.
- Add no `bridge-core` surface, `libc`, `fdopendir`, descriptor enumeration,
  authoritative enumeration-root pinning, or platform-conditional production
  functionality.
- A1 does not create `sweep/checked_scan.rs`, alter
  `scan_worktree_records`, change `sweep_orphans_with_exact_absence`, or modify
  `sweep_orphans`.
- A2 must preserve today’s eager two-phase behavior, compatibility pin-failure
  semantics, the `sweep_orphans` guard-root canonicalization and early return,
  raw-spelling action scan, malformed-legacy omission, and exact characterization
  matrix.
- `#[cfg_attr(not(unix), allow(dead_code))]` remains permitted where a later private
  scanner seam is necessarily unused on non-Unix, and `#[cfg(unix)]` remains
  permitted for inherently Unix-only tests.

### A1 file organization and public path

A1 uses the existing public path `bridge_worktree::sweep::*`:

- `sweep/report.rs` owns the fifteen public types, accessors, conversions, raw
  projection, filtered snapshot projection, readiness constant, parameterized
  readiness predicate, and unit tests.
- `sweep.rs` declares the private `report` module and re-exports exactly the fifteen
  public types. No existing function body changes in A1.
- `tests/r2f1b_exact_absence_report_api.rs` is an external integration-test crate
  that imports all fifteen names through `bridge_worktree::sweep::*`, compile-asserts
  every name, and contains a never-called signature-check function that type-checks
  every promised public accessor.
- `sweep/checked_scan.rs` is reserved for A2 and must not be created in A1.

Add this module declaration and re-export list in `sweep.rs`:

```rust
mod report;

pub use report::{
    CannotConstructSubjectV1, ClaimAuthorityObjectV1,
    ClaimAuthorityUnavailableReasonV1, ClaimAuthorityUnavailableV1,
    CustodyExactAbsenceAssessmentV1, CustodyRecordAssessmentV1,
    CustodyRootObservationV1, CustodyStateSnapshotV1,
    ExactAbsenceEnumerationV1, ExactAbsenceRecordAssessmentV1,
    ExactAbsenceRootRefusalV1, ExactAbsenceScanStatusV1,
    ExactAbsenceSweepEntryV1, ExactAbsenceSweepReportV1,
    IneligiblePopulationV1,
};
```

This decomposition adds no `bridge-core` surface and does not change the public
module path.

### Literal public API

The following declarations are normative for `sweep/report.rs`. Implement them
literally, including names, variants, payloads, fields, derives, visibility,
accessors, conversions, signatures, non-exhaustive annotations, and the four
temporary dead-code allowances on A1’s not-yet-wired crate constructors. These are
the fifteen new public types; do not add another public report, capability,
authority, or observation type.

A2 removes the four temporary constructor allowances when it adds their production
callers.

```rust
use std::ffi::{OsStr, OsString};

use crate::custody::{
    CustodyReadRefusalV1, PreservationReasonV1, WorktreeCustodyStateKindV1,
    WorktreeCustodyStateV1,
};

use super::UnusedCandidateDecisionV1;

/// False in A1, A2, and slice B. Increment 2 may change this only in the same
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
    /// A1 lands the vocabulary before A2 supplies the production caller.
    #[allow(dead_code)]
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

    /// Reports only whether the completed scan observed a complete enumeration
    /// through matching pinned root evidence. The returned report retains no
    /// descriptor, lock, or action authority.
    #[must_use]
    pub fn has_authoritative_scan(&self) -> bool {
        matches!(
            self.scan.enumeration(),
            ExactAbsenceEnumerationV1::Complete
        ) && self.scan.custody_root() == CustodyRootObservationV1::Pinned
    }

    /// Yields entries that satisfy the report's snapshot-eligibility filter.
    /// Refused and legacy entries are absent.
    ///
    /// Borrowing keeps the entry, its exact enumerated name, and its assessment
    /// ergonomically coupled for ordinary callers. It does not type-enforce
    /// inseparability: the entry is cloneable, its name can be copied, and its raw
    /// decision is copyable. This API deliberately adds no separate effective-
    /// decision scalar.
    ///
    /// The scan session has ended before this iterator can be consumed. A yielded
    /// entry is input to a future T3b action-time re-decision, not authority to act.
    /// T3b must acquire its own lock, re-read the exact entry, re-establish root and
    /// record identity, apply current admission policy, and repeat the exact-absence
    /// decision before any effect.
    ///
    /// Production A1, A2, and slice B yield no entries because policy readiness is
    /// false.
    pub fn effective(&self) -> impl Iterator<Item = &ExactAbsenceSweepEntryV1> {
        self.entries.iter().filter(move |entry| {
            self.entry_is_effectively_authorized_for_policy(
                entry,
                EXACT_ABSENCE_POLICY_READY_V1,
            )
        })
    }

    /// Private deterministic seam. Production supplies the constant above; tests
    /// supply explicit readiness so both branches are exercised before increment 2.
    fn entry_is_effectively_authorized_for_policy(
        &self,
        entry: &ExactAbsenceSweepEntryV1,
        policy_ready: bool,
    ) -> bool {
        policy_ready
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
    /// A1 lands the vocabulary before A2 supplies the production caller.
    #[allow(dead_code)]
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
    /// A1 lands the vocabulary before A2 supplies the production caller.
    #[allow(dead_code)]
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
    /// Raw, behavior-compatible reporting projection. This is neither snapshot
    /// eligibility nor action authority.
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
    /// A1 lands the vocabulary before A2 supplies the production caller.
    #[allow(dead_code)]
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

#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CustodyExactAbsenceAssessmentV1 {
    /// Increment 2 constructs this after population admission refuses.
    IneligiblePopulation(IneligiblePopulationV1),
    /// Increment 2 constructs this when a guard or authority construction refuses.
    CannotConstructSubject(CannotConstructSubjectV1),
    /// A2 constructs this to retain the current raw decision.
    Assessed(UnusedCandidateDecisionV1),
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IneligiblePopulationV1 {
    /// `ProtectionPrepared` without a claim. Its claim is schema-optional, so this
    /// is not a malformed or missing-required-claim result.
    BareProtectionPrepared,
    /// A canonically decoded state outside increment 2's candidate population.
    /// The enclosing `CustodyRecordAssessmentV1` carries the exact state snapshot.
    StateNotCandidate,
}

#[non_exhaustive]
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
remain ordinary public payloads, as Rust requires. The increment-2 assessment enums
are non-exhaustive so later admission outcomes can be added without requiring a
lossy mapping or an exhaustive downstream match break.

A1 production constructs none of these report values. A2 production constructs only
`Legacy`, `UnreadableCustody`, and
`Custody(CustodyRecordAssessmentV1 { assessment: Assessed(..), .. })`. Increment 2
constructs the dormant admission and subject-construction arms. Do not hide dormant
arms behind `cfg(test)` or remove them.

`CustodyStateSnapshotV1` retains neither an entire custody record nor
`RecoveredLive`’s predecessor digest. Its conversion explicitly names all ten
custody states, and only `PreservationUnknown` retains a
`PreservationReasonV1`.

### Snapshot projection and authority lifetime

`has_authoritative_scan()` implements this historical scan-evidence table:

| Enumeration | Custody-root observation | Result |
|---|---|---|
| `Complete` | `Pinned` | `true` |
| `Complete` | `IdentityChanged` or `Unavailable` | `false` |
| `Incomplete` or `Refused` | any value | `false` |

A `true` result says that the completed scan observed matching pinned-root evidence.
It does not say that the root is still the same object. `finish` consumes and drops
the scan session before the report is returned, and the report stores only values.
Neither `has_authoritative_scan()` nor raw `decision()` can authorize an action.

The private parameterized predicate implements this snapshot filter:

| Condition | Predicate result |
|---|---|
| Policy readiness is false | `false` |
| Enumeration is not `Complete` | `false` |
| Custody root is not `Pinned` | `false` |
| Scan was complete and pinned, but row is `Legacy` | `false` |
| Scan was complete and pinned, non-legacy raw decision is `Refused` | `false` |
| Scan was complete and pinned, policy ready, non-legacy raw decision is `Authorized` | `true` |

Production `effective()` passes `EXACT_ABSENCE_POLICY_READY_V1`. Unit tests in
`report.rs` call `entry_is_effectively_authorized_for_policy` with explicit `false`
and `true` values. This makes the ready path testable without changing production
readiness.

`effective()` returns borrowed entries and adds no separate effective-decision
scalar. That borrowing is an ergonomic projection, not the safety boundary: callers
can clone an entry, copy its name, and copy its raw decision. A positive filter
result means only that an entry satisfied the report’s ordered historical
snapshot-eligibility conditions.

A2 calls `finish` before phase-2 assessment. Consequently,
`Complete + Pinned + Authorized` combines root evidence captured by the completed
scan with a decision assessed later; it is historical sequence evidence, not one
coherent point-in-time snapshot. A2 makes no stronger claim. A design that wanted
coherent snapshot eligibility would have to bracket phase 2 with another final
root-identity check. This slice instead relies on the stronger settled boundary:
T3b independently re-establishes authority and re-decides immediately before acting.

T3b may use a yielded `enumerated_name()` as candidate input, but must perform this
action-time sequence under its own lock:

1. Re-open the selected sweep root and establish its current object identity.
2. Re-read the exact record named by `enumerated_name()` without reconstructing the
   name from `record_path()`.
3. Re-establish record/sibling placement and source, root, worktree,
   common-directory, and source/common-directory binding evidence.
4. Apply the current population-admission rule.
5. Repeat the exact-absence observation against the current target and Git
   registration.
6. Refuse if any root, record, object, policy, or absence observation changed.
7. Keep the T3b lock and its action-time authority alive through the authorized
   effect.

No T3b implementation may remove, prune, settle, transition, or publish solely
because a report entry appeared in `effective()`.

### A1 required tests and evidence

Place the projection tests beside the implementation in `sweep/report.rs`. They
must cover:

- all accessors on the report, scan status, entry, custody assessment, authority
  refusal, and custody snapshot;
- both `From` implementations for every one of the ten custody states;
- `PreservationUnknown` retaining each of the six reasons;
- `RecoveredLive` retaining no predecessor digest;
- every `ExactAbsenceRecordAssessmentV1::decision()` arm;
- `IneligiblePopulation`, `CannotConstructSubject`, and unreadable custody mapping
  to raw `Refused`;
- custody `Assessed(Authorized)` and `Assessed(Refused)` retaining their raw
  decisions;
- every combination in the `has_authoritative_scan()` table;
- production `effective()` returning no entries for an otherwise complete,
  pinned, non-legacy raw-`Authorized` report;
- the private predicate returning `false` for explicit false readiness;
- the private predicate returning `true` for explicit true readiness only for a
  complete, pinned, non-legacy raw-`Authorized` entry;
- the private predicate excluding legacy and raw-`Refused` rows even with explicit
  true readiness;
- the borrowed-entry seam unambiguously: construct a report with distinguishable
  entries, filter `report.entries().iter()` by calling
  `entry_is_effectively_authorized_for_policy(entry, true)`, take the positive row,
  assert `std::ptr::eq` against the corresponding element of `report.entries()`,
  and assert that same borrowed object carries the expected
  `enumerated_name()`.

Create `crates/bridge-worktree/tests/r2f1b_exact_absence_report_api.rs` with this
external-path compile assertion and never-called external signature check:

```rust
use bridge_worktree::custody::{
    PreservationReasonV1, WorktreeCustodyStateKindV1,
};
use bridge_worktree::sweep::{
    CannotConstructSubjectV1, ClaimAuthorityObjectV1,
    ClaimAuthorityUnavailableReasonV1, ClaimAuthorityUnavailableV1,
    CustodyExactAbsenceAssessmentV1, CustodyRecordAssessmentV1,
    CustodyRootObservationV1, CustodyStateSnapshotV1,
    ExactAbsenceEnumerationV1, ExactAbsenceRecordAssessmentV1,
    ExactAbsenceRootRefusalV1, ExactAbsenceScanStatusV1,
    ExactAbsenceSweepEntryV1, ExactAbsenceSweepReportV1,
    IneligiblePopulationV1, UnusedCandidateDecisionV1,
};

fn assert_public<T>() {}

#[allow(dead_code)]
fn assert_public_accessor_signatures(
    report: &ExactAbsenceSweepReportV1,
    scan: &ExactAbsenceScanStatusV1,
    entry: &ExactAbsenceSweepEntryV1,
    record_assessment: &ExactAbsenceRecordAssessmentV1,
    custody_assessment: &CustodyRecordAssessmentV1,
    authority_refusal: ClaimAuthorityUnavailableV1,
    snapshot: &CustodyStateSnapshotV1,
) {
    let _: &str = report.requested_root();
    let _: Option<&str> = report.canonical_root();
    let _: &ExactAbsenceScanStatusV1 = report.scan();
    let _: &[ExactAbsenceSweepEntryV1] = report.entries();
    let _: bool = report.has_authoritative_scan();
    let _ = report.effective();

    let _: &ExactAbsenceEnumerationV1 = scan.enumeration();
    let _: CustodyRootObservationV1 = scan.custody_root();

    let _: &str = entry.record_path();
    let _: &std::ffi::OsStr = entry.enumerated_name();
    let _: &ExactAbsenceRecordAssessmentV1 = entry.assessment();

    let _: UnusedCandidateDecisionV1 = record_assessment.decision();
    let _: &CustodyStateSnapshotV1 = custody_assessment.state();
    let _: &CustodyExactAbsenceAssessmentV1 = custody_assessment.assessment();

    let _: ClaimAuthorityObjectV1 = authority_refusal.object();
    let _: ClaimAuthorityUnavailableReasonV1 = authority_refusal.reason();
    let _: ClaimAuthorityUnavailableV1 = ClaimAuthorityUnavailableV1::new(
        authority_refusal.object(),
        authority_refusal.reason(),
    );

    let _: WorktreeCustodyStateKindV1 = snapshot.kind();
    let _: Option<PreservationReasonV1> = snapshot.preservation_reason();
}

#[test]
fn exact_absence_report_vocabulary_is_public() {
    assert_public::<CannotConstructSubjectV1>();
    assert_public::<ClaimAuthorityObjectV1>();
    assert_public::<ClaimAuthorityUnavailableReasonV1>();
    assert_public::<ClaimAuthorityUnavailableV1>();
    assert_public::<CustodyExactAbsenceAssessmentV1>();
    assert_public::<CustodyRecordAssessmentV1>();
    assert_public::<CustodyRootObservationV1>();
    assert_public::<CustodyStateSnapshotV1>();
    assert_public::<ExactAbsenceEnumerationV1>();
    assert_public::<ExactAbsenceRecordAssessmentV1>();
    assert_public::<ExactAbsenceRootRefusalV1>();
    assert_public::<ExactAbsenceScanStatusV1>();
    assert_public::<ExactAbsenceSweepEntryV1>();
    assert_public::<ExactAbsenceSweepReportV1>();
    assert_public::<IneligiblePopulationV1>();
}
```

Because this integration test is compiled as an external crate, it cannot succeed
through private `report` visibility. The generic assertions prove all fifteen names
are publicly re-exported. The never-called function independently requires every
promised accessor and the one public constructor to remain externally callable with
the declared signatures; it would fail to compile if any of them were reduced to
`pub(crate)`. Unit tests inside `report.rs` remain responsible for private
construction seams and projection behavior.

A1 has no behavioral red. Its pre-change failure is compiler/API-shape evidence:
the fifteen public types, module, methods, and re-exports do not exist at the base.
Do not describe that as decision-behavior evidence.

A1’s production mutation audit is bounded and mechanical: the `sweep.rs` diff may
add only `mod report` and the re-exports. Existing production function bodies,
including `scan_worktree_records`, `sweep_orphans_with_exact_absence`,
`sweep_orphans`, `decide_unused_candidate`, and both `false` ownership call sites,
must remain byte-for-byte unchanged apart from formatting forced by the new module
declaration location.

### A2 outline — not part of A1 implementation

A2 owns all traversal, compatibility-source, report-population, characterization,
event-ordering, return-type, exact-root, and mutation-audit work described below.
Do not partially implement it in A1.

#### A2 module placement and visibility

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

#### A2 compatibility source, shared policy, and deterministic pin-failure evidence

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

#### A2 scan flow and root-spelling evidence

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

#### A2 compatibility/action scan separation

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

#### A2 characterization matrix

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

#### A2 mutation audit

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

### Falsification license

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

### A1 sizing and mandatory stop

A1 line cap: at most **700 added-plus-deleted lines**, including production, tests,
module glue, and handoff, measured by the operator from
`9aedf175..<final SHA>` with `git diff --numstat` on a clean committed tree.

| A1 component | Estimated lines |
|---|---:|
| Public vocabulary, documentation, accessors, conversions | 330 |
| Raw and filtered projections plus readiness seam | 50 |
| Module declaration and fifteen-type re-export | 20 |
| Projection, conversion, and external API-shape tests | 145 |
| Installed-template handoff | 90 |
| Contingency | 15 |
| Total estimate | 650 |

This is the recorded pre-edit estimate. If repository facts make A1 exceed the
declared cap, stop before implementation and report the changed component estimate.
Do not compress declarations, tests, evidence, or the handoff to fit. A2 receives a
separate estimate and task spec before dispatch.

### A1 handoff and verification

Create
`docs/superpowers/reviews/2026-08-18-r2f1b-3d-t3a-inc1-sliceA1-handoff.md`.

**Do not look for a handoff template outside the repository.** Your container mounts
only the code tree; `$HOME` is `/root` and there is no `~/.claude`. An earlier
revision of this spec told you to read an installed template at a host path, which
is unsatisfiable here — a previous dispatch correctly refused and made no changes
because of it. Write the handoff from the content requirements below; the operator
applies the installed template on the host, which is an operator obligation and not
yours.

Use these headings, in this order: `## Summary`, `## What changed`,
`## Evidence`, `## OPERATOR EVIDENCE — PENDING`, `## Limits and disclosures`,
`## Sizing`.

The handoff must state:

- this revision deliberately split the prior slice A into A1 and A2;
- A1 has no behavioral red and changes no production traversal or decision;
- A1’s pre-change failure is compiler/API-shape evidence;
- the external integration test compiles all fifteen names through
  `bridge_worktree::sweep::*`;
- the external never-called signature-check function type-checks every promised
  public accessor and the public authority-refusal constructor;
- production policy readiness is false;
- the explicit-ready private predicate is test-only mechanism evidence;
- `effective()` is snapshot eligibility, not retained action authority;
- borrowing is ergonomic coupling, not type-enforced inseparability;
- a returned report owns no live scan authority and T3b must re-decide under its own
  lock;
- A1 production constructs none of the report values;
- A2 owns production construction, report return wiring, exact names,
  characterization, and the concrete mutation audit;
- which tests the implementer executed and which it did not;
- the four temporary unconditional constructor dead-code allowances;
- every non-Unix lint allowance and Unix-only test, recording “none” if there are
  none;
- the `sweep.rs` production-body no-change audit;
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
Unix-only code and tests. Do not claim a green Windows all-target baseline without
running one.

## Acceptance Criteria

1. A1 creates `sweep/report.rs` and exposes exactly the fifteen named public types
   through `bridge_worktree::sweep::*`; the external integration test imports and
   compile-asserts every name through that public path and includes the never-called
   external function that type-checks every promised public accessor.
2. The fifteen public types match the normative Rust declarations, including every
   variant, payload, private field, derive, visibility, accessor, constructor,
   conversion, non-exhaustive annotation, and temporary constructor allowance.
3. `CustodyExactAbsenceAssessmentV1`, `IneligiblePopulationV1`, and
   `CannotConstructSubjectV1` are non-exhaustive while their increment-2 production
   arms remain dormant.
4. Public structs have private fields. `ClaimAuthorityUnavailableV1` has only its
   declared constructor and read-only accessors. External signature evidence fails
   to compile if a promised public accessor or that constructor is reduced to
   crate-only visibility.
5. `ExactAbsenceSweepEntryV1` retains both `record_path: String` and
   `enumerated_name: OsString`; its exact-name accessor returns `&OsStr`.
6. `CustodyStateSnapshotV1` exhaustively converts all ten custody states, retains a
   reason only for `PreservationUnknown`, and retains neither a whole record nor a
   predecessor digest.
7. Raw `decision()` explicitly matches every assessment arm without a wildcard and
   implements the stated raw projection.
8. `has_authoritative_scan()` implements every row of its table and is documented
   as historical scan evidence that retains no live authority.
9. The private parameterized predicate accepts explicit readiness. Its tests
   exercise both readiness branches, legacy exclusion, raw refusal, scan status,
   and the positive non-legacy raw-`Authorized` case.
10. Public `effective()` passes the production readiness constant, returns borrowed
    entries, adds no separate effective-decision scalar, and yields no entries while
    production readiness is false. Documentation states that borrowing is ergonomic
    coupling rather than type-enforced inseparability.
11. The explicit-ready borrowed-entry test filters `entries().iter()` through the
    private predicate, pointer-compares the yielded reference with its source slice
    element, and checks the exact name on that same borrowed object.
12. Documentation states the concrete T3b action-time re-decision sequence and
    forbids treating a report or filtered entry as authority to act.
13. A1 does not create `checked_scan.rs`, change
    `sweep_orphans_with_exact_absence`, alter `scan_worktree_records`, or modify any
    existing sweep decision or action body.
14. `decide_unused_candidate` retains `recovery_owned`, both production call sites
    retain `false`, and no ownership, custody, publication, settlement, deletion,
    or CLI behavior changes.
15. A1 adds no `bridge-core` surface, `libc`, `fdopendir`, descriptor enumeration,
    root pinning, Git invocation, filesystem observation, or platform-conditional
    production functionality.
16. A1 claims no behavioral red. Compiler/API-shape evidence, the external public
    re-export and accessor-signature compile assertions, and projection mechanism
    tests are reported separately.
17. The A2 outline fixes every scanner cross-module declaration at
    `pub(super)`, keeps the concrete session and observation fields private to
    `checked_scan.rs`, assigns traversal integration to parent `sweep.rs`, shares
    selection/read policy with `scan_worktree_records`, and requires the same-root
    conformance matrix. Its deterministic pin-failure row uses the named private
    helper with production reads and substitutes only the pin-open result.
18. The A2 outline scopes the exact-scan entry-point
    `canonicalize_lenient` invocation to the supplied exact-scan root while
    preserving per-record helper calls and ordinary `std::fs::canonicalize` guards.
    It also explicitly preserves `sweep_orphans`’s separate guard-root
    `canonicalize_lenient` call, warning, and early return, while requiring
    `scan_worktree_records` and its enumeration argument to remain the raw supplied
    root. Stable alias-path evidence and deterministic guard-failure evidence are
    separate tests using the named private seam.
19. The A2 outline requires direct evidence for exact `requested_root` spelling,
    the expected lenient `canonical_root`, absolute-missing-root
    `CannotEnumerate`, and relative empty-parent `CannotCanonicalize`.
20. The A2 mutation audit explicitly names the
    `PinnedDirectoryV1::open` observation tree; ordinary
    `std::fs::canonicalize`; target `Path::symlink_metadata`; the porcelain
    UTF-8 decode-refusal branch and its raw-`Refused` report projection; the
    `registration_absent_from_porcelain → compare_path_identities` branch for
    non-byte-identical valid-UTF-8 registration fields; every initial and final
    `deepest_existing_path` metadata, symlink-metadata, and canonicalization
    observation; the conditional case-sensitivity
    `read_dir`/`symlink_metadata` tree; Git observations; and the absence of action
    edges.
21. The A2 outline states that `finish` precedes phase-2 assessment, so complete,
    pinned, authorized output is ordered historical evidence rather than one
    coherent point-in-time snapshot; T3b’s independent action-time re-decision
    remains the safety boundary.
22. The complete A2 characterization matrix remains intact, including
    `Preserved` plus valid claim plus vanished target plus `BothAbsent` producing
    raw `Authorized`, and malformed legacy being silently omitted.
23. The installed-template A1 handoff exists at the required path, preserves the
    operator-evidence block verbatim, and records tests, allowances, exclusions,
    deferred A2 work, public-name and public-accessor evidence, and authority limits
    honestly.
24. The implementation remains beneath the declared A1 cap. If the estimate no
    longer fits, work stops before editing.
25. Final operator totals count test binaries and doc-test suites without
    double-counting nested harness output.

## Files

- `crates/bridge-worktree/src/sweep.rs` — add the private `report` module declaration
  and the literal fifteen-type public re-export; do not change existing function
  bodies.
- `crates/bridge-worktree/src/sweep/report.rs` — create; literal public vocabulary,
  conversions, projections, testable readiness predicate, and A1 unit tests.
- `crates/bridge-worktree/tests/r2f1b_exact_absence_report_api.rs` — create; external
  compile assertions for all fifteen `bridge_worktree::sweep::*` public names and
  every promised public accessor.
- `crates/bridge-worktree/src/custody.rs` — read for custody states, reasons, kinds,
  and read refusals; do not modify.
- `crates/bridge-worktree/src/provider_path.rs` — read for A2 factual
  falsification only; do not modify.
- `crates/bridge-worktree/src/host_git.rs` — read for A2 factual falsification only;
  do not modify.
- `crates/bridge-core/src/fs_custody.rs` — read for A2
  `PinnedDirectoryV1::open`, path-identity comparator, resolver, and
  case-sensitivity observation-tree falsification only; do not modify.
- `bin/a2a-bridge/src/main.rs` — read for the deferred A2 caller audit only; do not
  modify.
- `crates/bridge-worktree/src/sweep/checked_scan.rs` — reserved for A2; do not create
  or modify in A1.
- `docs/superpowers/reviews/2026-08-18-r2f1b-3d-t3a-inc1-sliceA1-handoff.md`
  — create, using the headings named in "A1 handoff and verification". Do not read
  any file outside the repository; nothing outside the code tree is mounted.

## Spec Refs

Authoritative at the base commit:

- `crates/bridge-worktree/src/sweep.rs`
- `crates/bridge-worktree/src/host_git.rs`
- `crates/bridge-worktree/src/custody.rs`
- `crates/bridge-worktree/src/provider_path.rs`
- `crates/bridge-core/src/fs_custody.rs`
- `bin/a2a-bridge/src/main.rs`

## Commit Message

feat(worktree): add exact-absence report vocabulary

Introduce the fifteen public exact-absence reporting types, exhaustive raw decision
projection, custody-state snapshot conversion, and borrowed snapshot-eligibility
view without changing production traversal or decisions.

Production readiness remains false, while a private parameterized predicate makes
the future ready branch deterministic to test. Reports retain no live scan
authority: a future T3b consumer must re-read and re-decide the exact candidate
under its own action lock.

This is A1 of the split former slice A. A2 will populate the compatibility-backed
report, preserve eager traversal and characterization behavior, and migrate the
exact-absence entry point to return it.
