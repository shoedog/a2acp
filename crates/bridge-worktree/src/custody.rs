//! R2f1b V3 worktree custody records — the read-only classification surface (slice 2a).
//!
//! This module is the *wire contract* for worktree custody, published before any writer
//! exists so that 2b2's writer, 2c's preservation/deletion transitions, and 2d's claim
//! exchange cannot silently diverge from each other. It defines:
//!
//! * [`WorktreeCustodyStateV1`] — the ten states of the focused boundary's §2.2 custody
//!   state machine, with their concrete serde tags pinned by golden bytes.
//! * [`PreservedWorktreeClaimV1`] — §2.2's durable claim, field for field, bounded.
//! * [`PreservationReasonV1`] / [`RecoveryLocatorV1`] — the two closed vocabularies the
//!   claim and the `PreservationUnknown` state draw from.
//! * [`WorktreeCustodyRecordV1`] — the `<target>.custody.v1.json` envelope, with a
//!   canonical-bytes decoder and a typed refusal ([`CustodyRecordDecodeErrorV1`]).
//! * [`LEGAL_CUSTODY_TRANSITIONS_V1`] — the legal-transition table as data.
//! * [`CustodySweepDispositionV1`] — how a sweep classifies a V3 record. **Every arm is
//!   non-destructive**: slice 2a mints no deletion authority whatsoever.
//!
//! Reading is descriptor-relative and bounded: [`read_custody_record_in`] goes through
//! `bridge_core::fs_custody::PinnedDirectoryV1`, so a symlinked record is refused rather
//! than followed, a non-regular file is refused by the kernel, and a multiply-linked
//! record — whose exclusive ownership cannot be proved — is refused too. Every refusal
//! resolves to "unknown", and unknown is never deleted.

use bridge_core::execution_policy::{
    PolicyNodeRefV1, Sha256HexV1, WorktreeCustodyIdV1, WorktreeObjectIdentityV1,
};
use bridge_core::fs_custody::{ChildNameV2, PinnedDirectoryV1, ReservedNameNamespaceV2};
use bridge_core::ids::{AttemptId, AttemptIdentity, ExecutionId};
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::io::Read as _;
use std::path::Path;

/// Custody-record schema version. Independent of the workflow-snapshot version: a V3
/// snapshot publishes a `custody.v1` record.
pub const WORKTREE_CUSTODY_RECORD_SCHEMA_V1: u16 = 1;

/// The V3 record suffix. Deliberately *not* `.meta.json`: `sweep.rs`'s legacy scanner
/// enumerates only `*.meta.json`, so an older binary cannot name — and therefore cannot
/// delete — a V3 checkout (focused boundary §2.2 "Record naming").
pub const CUSTODY_RECORD_SUFFIX: &str = ".custody.v1.json";

/// Total encoded bound for one custody record. A boot sweep reads every record in the
/// worktree root before it does anything else, so an unbounded record is a denial surface.
pub const MAX_CUSTODY_RECORD_JSON_BYTES: usize = 8_192;

/// Bound on every free-form path string in a custody record. Every *other* string in the
/// record is a self-validating newtype (`WorktreeCustodyIdV1`, `ExecutionId`, `AttemptId`,
/// `Sha256HexV1`); the four `WorktreeObjectIdentityV1` canonical paths are the one
/// unbounded axis, and this is where they are bounded.
pub const MAX_CUSTODY_PATH_BYTES: usize = 1_024;

/// Why a checkout is being preserved, or why its disposition is unknown.
///
/// One closed vocabulary serves both `PreservedWorktreeClaimV1::reason` and
/// `WorktreeCustodyStateV1::PreservationUnknown { reason }`, exactly as §2.2 draws them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreservationReasonV1 {
    /// The owning node reached a failure terminal.
    NodeFailure,
    /// The run, node, or session was cancelled.
    Cancellation,
    /// Cleanup completed ambiguously (e.g. a record renamed but the parent sync outcome
    /// is unknown) and the protective reading was taken.
    AmbiguousCleanup,
    /// The checkout was still materializing when the decision had to be made.
    MaterializationInFlight,
    /// A removal's post-conditions disagreed with each other.
    PostConditionDisagreement,
    /// An authorized removal did not complete.
    RemovalFailed,
}

impl PreservationReasonV1 {
    pub const ALL: [Self; 6] = [
        Self::NodeFailure,
        Self::Cancellation,
        Self::AmbiguousCleanup,
        Self::MaterializationInFlight,
        Self::PostConditionDisagreement,
        Self::RemovalFailed,
    ];
}

/// How the preserved checkout is addressed at the moment its claim was published.
///
/// The claim's four object identities say *what* the objects are; the locator says how a
/// recovery consumer reaches the work. The three arms mirror the three answers
/// `host_git.rs`'s registration probes can give (`registration_absent_from_porcelain`
/// `:103`, `registration_absent` `:112`): registered, provably unregistered, unprovable.
///
/// `registration_absent` retains `Absent`, `Present`, and `CannotProve` in addition to
/// transport errors. Both `CannotProve` and `Err` produce
/// [`RecoveryLocatorV1::RegistrationUnproven`]; neither may be collapsed into a definite
/// registered or unregistered claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "locator", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecoveryLocatorV1 {
    /// The linked worktree is still registered with its common dir.
    RegisteredWorktree {},
    /// The directory survives but its git registration was proved absent.
    UnregisteredDirectory {},
    /// Registration could not be proved either way; recovery must re-probe.
    RegistrationUnproven {},
}

impl RecoveryLocatorV1 {
    pub const ALL: [Self; 3] = [
        Self::RegisteredWorktree {},
        Self::UnregisteredDirectory {},
        Self::RegistrationUnproven {},
    ];
}

/// The ten custody states of focused boundary §2.2.
///
/// Every unit-like variant is declared as an empty *struct* payload (`Variant {}`) rather
/// than a bare unit variant. This is the R2f1b F-2 standing rule: `#[serde(tag = ...,
/// deny_unknown_fields)]` on an internally-tagged enum only buffers-and-checks leftover
/// keys for struct-payload variants, so a bare unit variant would silently accept a stray
/// sibling key. The wire form is byte-identical either way.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorktreeCustodyStateV1 {
    /// Protection published before any provider effect.
    ProtectionPrepared {},
    /// Descriptor-bound proof that the candidate target does not exist and is not
    /// registered. Permits removal of the unused marker only — never provider removal.
    UnusedSettled {},
    /// The provider add is in flight.
    Materializing {},
    /// A live, sweep-excluded checkout.
    LiveProtected {},
    /// Preservation has been decided and durably prepared.
    PreservationPrepared {},
    /// Terminal for R2f1b: only R2f2 disposition releases it.
    Preserved {},
    /// Deletion authority has been minted (slice 2c2 owns the transition).
    DeleteAuthorized {},
    /// The checkout was removed; the record is the tombstone.
    Removed {},
    /// A protective live state carrying the successor attempt identity (the record's own
    /// `current_attempt`) and the predecessor claim digest. Same sweep exclusion as
    /// `LiveProtected`.
    RecoveredLive {
        predecessor_claim_digest: Sha256HexV1,
    },
    /// Terminal for R2f1b: disposition is unknown and must not be guessed.
    PreservationUnknown { reason: PreservationReasonV1 },
}

/// Whether a record in a given state may, must, or must not carry a
/// [`PreservedWorktreeClaimV1`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClaimPresenceV1 {
    /// The state asserts no preservation; a claim would assert one that never happened.
    Forbidden,
    /// The pre-effect marker may exist with or without a claim: §5.1 step 4 conditions
    /// the *full* claim on complete target identity, which is not yet observable here.
    Optional,
    /// The state asserts a preservation, so the standalone artifact R2f2 disposes of
    /// must be present.
    Required,
}

/// Whether the object identities in a record — and in its claim, when present — must
/// carry observed `dev`/`ino`, or may still be the plan-derived paths that are all a
/// pre-materialization writer can know.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IdentityCompletenessV1 {
    /// Canonical paths required; `dev`/`ino` may be absent (both, never one).
    MayBeDegraded,
    /// Observed `dev`/`ino` required, so the sweep's descriptor check can run.
    Complete,
}

impl WorktreeCustodyStateV1 {
    /// The settled per-state claim rule. **This is the shape 2b2's writer is built
    /// against**, published here as data so the writer, 2c's transitions, and 2d's
    /// exchange cannot each re-derive it.
    ///
    /// `ProtectionPrepared` is `Optional` because focused boundary §5.1 step 4 makes the
    /// full claim conditional ("If target identity is complete, atomically replace with
    /// the full claim") — the pre-effect marker legally precedes it. The three
    /// preserving states are `Required`: a preservation with no artifact leaves R2f2
    /// nothing to dispose of. Everything else is `Forbidden`.
    #[must_use]
    pub fn claim_presence(&self) -> ClaimPresenceV1 {
        match self {
            Self::ProtectionPrepared {} => ClaimPresenceV1::Optional,
            Self::PreservationPrepared {}
            | Self::Preserved {}
            | Self::PreservationUnknown { .. } => ClaimPresenceV1::Required,
            Self::UnusedSettled {}
            | Self::Materializing {}
            | Self::LiveProtected {}
            | Self::DeleteAuthorized {}
            | Self::Removed {}
            | Self::RecoveredLive { .. } => ClaimPresenceV1::Forbidden,
        }
    }

    /// The settled per-state identity rule, likewise 2b2's foundation.
    ///
    /// Degraded identity is legal exactly where the design says no live identity exists
    /// yet: `ProtectionPrepared` and `UnusedSettled` precede `git worktree add` entirely
    /// (§2.2 — `UnusedSettled` requires proof the target does *not* exist, so it can
    /// never have one); `Materializing` is §5.7 row 4's "during/after partial add,
    /// **before live identity**"; and `PreservationUnknown { materialization_inflight }`
    /// is §5.1's "if materialization is unresolved" record, published precisely because
    /// target identity could not be completed. Every other state has observed the
    /// checkout, so it must carry the evidence the sweep compares by descriptor (P3).
    #[must_use]
    pub fn identity_completeness(&self) -> IdentityCompletenessV1 {
        match self {
            Self::ProtectionPrepared {} | Self::UnusedSettled {} | Self::Materializing {} => {
                IdentityCompletenessV1::MayBeDegraded
            }
            Self::PreservationUnknown {
                reason: PreservationReasonV1::MaterializationInFlight,
            } => IdentityCompletenessV1::MayBeDegraded,
            Self::LiveProtected {}
            | Self::PreservationPrepared {}
            | Self::Preserved {}
            | Self::DeleteAuthorized {}
            | Self::Removed {}
            | Self::RecoveredLive { .. }
            | Self::PreservationUnknown { .. } => IdentityCompletenessV1::Complete,
        }
    }

    #[must_use]
    pub fn kind(&self) -> WorktreeCustodyStateKindV1 {
        match self {
            Self::ProtectionPrepared {} => WorktreeCustodyStateKindV1::ProtectionPrepared,
            Self::UnusedSettled {} => WorktreeCustodyStateKindV1::UnusedSettled,
            Self::Materializing {} => WorktreeCustodyStateKindV1::Materializing,
            Self::LiveProtected {} => WorktreeCustodyStateKindV1::LiveProtected,
            Self::PreservationPrepared {} => WorktreeCustodyStateKindV1::PreservationPrepared,
            Self::Preserved {} => WorktreeCustodyStateKindV1::Preserved,
            Self::DeleteAuthorized {} => WorktreeCustodyStateKindV1::DeleteAuthorized,
            Self::Removed {} => WorktreeCustodyStateKindV1::Removed,
            Self::RecoveredLive { .. } => WorktreeCustodyStateKindV1::RecoveredLive,
            Self::PreservationUnknown { .. } => WorktreeCustodyStateKindV1::PreservationUnknown,
        }
    }
}

/// The payload-free discriminant of [`WorktreeCustodyStateV1`], so the transition table
/// and the sweep classification can be expressed as `Copy` data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WorktreeCustodyStateKindV1 {
    ProtectionPrepared,
    UnusedSettled,
    Materializing,
    LiveProtected,
    PreservationPrepared,
    Preserved,
    DeleteAuthorized,
    Removed,
    RecoveredLive,
    PreservationUnknown,
}

impl WorktreeCustodyStateKindV1 {
    pub const ALL: [Self; 10] = [
        Self::ProtectionPrepared,
        Self::UnusedSettled,
        Self::Materializing,
        Self::LiveProtected,
        Self::PreservationPrepared,
        Self::Preserved,
        Self::DeleteAuthorized,
        Self::Removed,
        Self::RecoveredLive,
        Self::PreservationUnknown,
    ];

    /// The exact `"state"` tag this kind serializes as. Kept next to the enum so a
    /// `rename_all` change cannot drift away from operator-facing reporting.
    #[must_use]
    pub fn wire_tag(self) -> String {
        match self {
            Self::ProtectionPrepared => "protection_prepared",
            Self::UnusedSettled => "unused_settled",
            Self::Materializing => "materializing",
            Self::LiveProtected => "live_protected",
            Self::PreservationPrepared => "preservation_prepared",
            Self::Preserved => "preserved",
            Self::DeleteAuthorized => "delete_authorized",
            Self::Removed => "removed",
            Self::RecoveredLive => "recovered_live",
            Self::PreservationUnknown => "preservation_unknown",
        }
        .to_string()
    }

    /// Terminal for R2f1b. `Preserved` and `PreservationUnknown` are released only by
    /// R2f2 disposition (§2.2); `UnusedSettled`, `Removed`, and `RecoveredLive` have no
    /// outgoing edge in the R2f1b machine as §2.2 draws it.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        !LEGAL_CUSTODY_TRANSITIONS_V1
            .iter()
            .any(|(from, _)| *from == self)
    }

    /// Does a record in this state assert a preservation — prepared, terminal, or unknown?
    ///
    /// **The durable disposition source (slice 2c2, P5).** The in-memory cleanup cell that holds
    /// a session's checkout disposition is evicted the moment a flight reports `Ok`, so a later
    /// flight starts from `Reclaim` and would report `Retained` for a checkout whose record on
    /// disk says `Preserved`. Once the cell is gone the RECORD is the authority, and this is the
    /// predicate that reads it. Exhaustive by design: a new state must be classified by decision.
    #[must_use]
    pub fn is_preserving(self) -> bool {
        match self {
            Self::PreservationPrepared | Self::Preserved | Self::PreservationUnknown => true,
            Self::ProtectionPrepared
            | Self::UnusedSettled
            | Self::Materializing
            | Self::LiveProtected
            | Self::DeleteAuthorized
            | Self::Removed
            | Self::RecoveredLive => false,
        }
    }

    /// Is a preservation on disk R2f1b-TERMINAL (as opposed to merely prepared)?
    ///
    /// Split from [`Self::is_preserving`] because a stranded `PreservationPrepared` is resumable
    /// (2c1 repair RA) and must not be reported as a settled preservation.
    #[must_use]
    pub fn is_terminal_preservation(self) -> bool {
        match self {
            Self::Preserved | Self::PreservationUnknown => true,
            Self::PreservationPrepared
            | Self::ProtectionPrepared
            | Self::UnusedSettled
            | Self::Materializing
            | Self::LiveProtected
            | Self::DeleteAuthorized
            | Self::Removed
            | Self::RecoveredLive => false,
        }
    }

    /// How a sweep classifies a record in this state. **Recovery-only**: no arm
    /// authorizes checkout removal in slice 2a.
    #[must_use]
    pub fn sweep_disposition(self) -> CustodySweepDispositionV1 {
        match self {
            Self::ProtectionPrepared
            | Self::Materializing
            | Self::LiveProtected
            | Self::DeleteAuthorized
            | Self::RecoveredLive => CustodySweepDispositionV1::Recover,
            Self::PreservationPrepared | Self::Preserved => CustodySweepDispositionV1::Preserved,
            Self::PreservationUnknown => CustodySweepDispositionV1::Unknown,
            // `UnusedSettled` is a durable, operator-visible stranded-marker category. It
            // permits removal of the *marker* only after a fresh exact-absence proof; no later
            // sweep can re-prove it because the settled record forbids the claim containing the
            // source authority. `Removed` is a tombstone whose checkout is already gone.
            // Neither is a checkout removal, and slice 2a acts on neither.
            Self::UnusedSettled => CustodySweepDispositionV1::StrandedUnusedSettled,
            Self::Removed => CustodySweepDispositionV1::MarkerOnly,
        }
    }
}

/// The legal custody transitions of focused boundary §2.2, **as data**, so 2b2's writer,
/// 2c's preservation/deletion transitions, and 2d's exchange share one edge set.
///
/// Two readings of §2.2 are worth recording, because the ASCII diagram and the prose
/// disagree and the prose wins:
///
/// * `Preserved -> RecoveredLive` is drawn (the vertical under `Preserved` merges into
///   the `RecoveredLive` junction) but is **not** an R2f1b edge: "`Preserved` and
///   `PreservationUnknown` are terminal for R2f1b; only R2f2 disposition releases them."
/// * `RecoveredLive` is drawn as a sink and is left as one here. Its outgoing edges, if
///   any, belong to the slice that owns the exchange (2d) and the disposition (2c).
///
/// `PreservationPrepared -> PreservationUnknown` is not in the diagram but is named by
/// the slice-2 brief's 2c1 scope ("`PreservationPrepared` -> `Preserved` /
/// `PreservationUnknown`").
pub const LEGAL_CUSTODY_TRANSITIONS_V1: &[(
    WorktreeCustodyStateKindV1,
    WorktreeCustodyStateKindV1,
)] = {
    use WorktreeCustodyStateKindV1 as K;
    &[
        (K::ProtectionPrepared, K::UnusedSettled),
        (K::ProtectionPrepared, K::Materializing),
        (K::Materializing, K::LiveProtected),
        (K::Materializing, K::PreservationUnknown),
        (K::LiveProtected, K::PreservationPrepared),
        (K::LiveProtected, K::DeleteAuthorized),
        (K::LiveProtected, K::RecoveredLive),
        (K::PreservationPrepared, K::Preserved),
        (K::PreservationPrepared, K::PreservationUnknown),
        (K::DeleteAuthorized, K::Removed),
    ]
};

#[must_use]
pub fn transition_is_legal(
    from: WorktreeCustodyStateKindV1,
    to: WorktreeCustodyStateKindV1,
) -> bool {
    LEGAL_CUSTODY_TRANSITIONS_V1
        .iter()
        .any(|(left, right)| *left == from && *right == to)
}

/// How a sweep must treat a scanned V3 record. These are the boot-sweep report categories
/// of §5.2 (recovered / preserved / unknown / refused; `legacy-deleted` belongs to the
/// legacy arm, which has no V3 record to classify).
///
/// **Every arm is non-destructive.** Slice 2a's sweep never runs `git`, `remove_dir_all`,
/// reset, clean, or checkout for a V3 record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CustodySweepDispositionV1 {
    /// Live or protective: ownership recovery is a later slice's; deletion never.
    Recover,
    /// Preserved or preserving: awaits R2f2 disposition.
    Preserved,
    /// Unknown disposition — including every corrupt, missing, mismatched, symlinked, or
    /// multiply-linked record. Unknown is never deleted.
    Unknown,
    /// A durable `UnusedSettled` marker remains after its transition but before retirement.
    /// It is intentionally distinct from a `Removed` tombstone so an operator can find this
    /// fail-closed residue without inferring it from a generic marker-only classification.
    StrandedUnusedSettled,
    /// Marker metadata only; never a provider or git removal.
    MarkerOnly,
    /// A custody guard refused the record (e.g. it points outside the sweep root).
    Refused,
}

impl CustodySweepDispositionV1 {
    /// Slice 2a mints no deletion authority. The exhaustive match is deliberate: adding a
    /// destructive arm must be a decision, not an omission.
    #[must_use]
    pub fn authorizes_checkout_removal(self) -> bool {
        match self {
            Self::Recover
            | Self::Preserved
            | Self::Unknown
            | Self::StrandedUnusedSettled
            | Self::MarkerOnly
            | Self::Refused => false,
        }
    }

    #[must_use]
    pub fn report_category(self) -> &'static str {
        match self {
            Self::Recover => "recovered",
            Self::Preserved => "preserved",
            Self::Unknown => "unknown",
            Self::StrandedUnusedSettled => "stranded-unused-settled",
            Self::MarkerOnly => "marker-only",
            Self::Refused => "refused",
        }
    }
}

/// Focused boundary §2.2's preserved-worktree claim, field for field.
///
/// The claim is a *self-contained* durable artifact: R2f2 disposition reads it without the
/// enclosing record, which is why it repeats the identity the envelope also carries. The
/// decoder cross-checks the two.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreservedWorktreeClaimV1 {
    pub schema_version: u16,
    pub custody_id: WorktreeCustodyIdV1,
    pub execution_id: ExecutionId,
    pub origin_attempt_id: AttemptId,
    pub current_attempt: AttemptIdentity,
    pub node: PolicyNodeRefV1,
    pub checkout_fingerprint: Sha256HexV1,
    pub source: WorktreeObjectIdentityV1,
    pub root: WorktreeObjectIdentityV1,
    pub worktree: WorktreeObjectIdentityV1,
    pub common_dir: WorktreeObjectIdentityV1,
    pub reason: PreservationReasonV1,
    pub created_wall_ms: i64,
    pub recovery_locator: RecoveryLocatorV1,
}

/// The `<target>.custody.v1.json` envelope.
///
/// The envelope carries exactly the identity a sweep consults before it decides anything:
/// the custody id (report and lock key), the checkout fingerprint (exact-match plan
/// selection against `FrozenWorktreeCustodyPlanV1::checkout_fingerprint`), the current
/// attempt (run id and lease derivation — §5.2 requires the state to be parsed *before*
/// run ids and leases are examined), and the worktree object identity (the sidecar-sibling
/// and under-root guards). `source`, `root`, and `common_dir` live in the claim only:
/// slice 2a never runs git, so it never needs them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeCustodyRecordV1 {
    pub schema_version: u16,
    pub custody_id: WorktreeCustodyIdV1,
    pub checkout_fingerprint: Sha256HexV1,
    pub current_attempt: AttemptIdentity,
    pub worktree: WorktreeObjectIdentityV1,
    pub state: WorktreeCustodyStateV1,
    pub claim: Option<PreservedWorktreeClaimV1>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CustodyRecordDecodeErrorV1 {
    #[error("custody record exceeded its encoded bound")]
    OverBound,
    #[error("custody record is not well-formed")]
    Malformed,
    #[error("custody record schema version is not supported")]
    SchemaVersion,
    #[error("custody record path is empty or over its byte bound")]
    PathBound,
    #[error("custody state requires a preserved claim")]
    ClaimRequired,
    #[error("custody state must not carry a preserved claim")]
    ClaimForbidden,
    #[error("preserved claim identity disagrees with its record")]
    ClaimIdentityMismatch,
    #[error("custody record bytes are not the canonical encoding")]
    NonCanonical,
    #[error("preserved claim carries a negative wall clock")]
    NegativeWallClock,
    #[error("custody state requires observed directory identity")]
    IdentityIncomplete,
    #[error("object identity carries dev without ino, or ino without dev")]
    IdentityPartial,
    #[error("preserved claim attempt fields contradict each other")]
    ClaimAttemptContradiction,
    #[error("preservation reason disagrees between the state and its claim")]
    ClaimReasonMismatch,
}

fn path_within_bound(path: &str) -> bool {
    !path.is_empty() && path.len() <= MAX_CUSTODY_PATH_BYTES
}

/// Bounds, plus the dev/ino all-or-nothing rule. A half-populated identity is neither
/// observed evidence nor a plan-derived path: the sweep's descriptor comparison would
/// silently read it as absent.
fn validate_object_identity(
    identity: &WorktreeObjectIdentityV1,
    completeness: IdentityCompletenessV1,
) -> Result<(), CustodyRecordDecodeErrorV1> {
    if !path_within_bound(&identity.canonical_path)
        || !path_within_bound(&identity.directory_identity.canonical_path)
    {
        return Err(CustodyRecordDecodeErrorV1::PathBound);
    }
    let dev = identity.directory_identity.dev;
    let ino = identity.directory_identity.ino;
    let btime = identity.directory_identity.btime;
    if dev.is_some() != ino.is_some() || (btime.is_some() && dev.is_none()) {
        return Err(CustodyRecordDecodeErrorV1::IdentityPartial);
    }
    // Platform exclusion (brief risk R-10): `DirectoryIdentityV1` has no dev/ino on
    // non-unix, so requiring them there would make the contract unsatisfiable. The
    // completeness rule is enforced only where the evidence exists.
    #[cfg(unix)]
    if completeness == IdentityCompletenessV1::Complete && dev.is_none() {
        return Err(CustodyRecordDecodeErrorV1::IdentityIncomplete);
    }
    #[cfg(not(unix))]
    let _ = completeness;
    Ok(())
}

impl PreservedWorktreeClaimV1 {
    fn validate(
        &self,
        completeness: IdentityCompletenessV1,
    ) -> Result<(), CustodyRecordDecodeErrorV1> {
        if self.schema_version != WORKTREE_CUSTODY_RECORD_SCHEMA_V1 {
            return Err(CustodyRecordDecodeErrorV1::SchemaVersion);
        }
        for identity in [&self.source, &self.root, &self.worktree, &self.common_dir] {
            validate_object_identity(identity, completeness)?;
        }
        if self.created_wall_ms < 0 {
            return Err(CustodyRecordDecodeErrorV1::NegativeWallClock);
        }
        // The claim carries `execution_id` both directly and inside `current_attempt`;
        // they name one execution. At ordinal 0 no resume has happened, so the
        // delivery-origin attempt is by construction the current one
        // (`AttemptIdentity::initial`) — after a resume it legally differs.
        if self.execution_id != self.current_attempt.execution_id {
            return Err(CustodyRecordDecodeErrorV1::ClaimAttemptContradiction);
        }
        if self.current_attempt.ordinal == 0
            && self.origin_attempt_id != self.current_attempt.attempt_id
        {
            return Err(CustodyRecordDecodeErrorV1::ClaimAttemptContradiction);
        }
        Ok(())
    }
}

impl WorktreeCustodyRecordV1 {
    fn validate(&self) -> Result<(), CustodyRecordDecodeErrorV1> {
        if self.schema_version != WORKTREE_CUSTODY_RECORD_SCHEMA_V1 {
            return Err(CustodyRecordDecodeErrorV1::SchemaVersion);
        }
        let completeness = self.state.identity_completeness();
        validate_object_identity(&self.worktree, completeness)?;
        match (&self.claim, self.state.claim_presence()) {
            (None, ClaimPresenceV1::Required) => {
                return Err(CustodyRecordDecodeErrorV1::ClaimRequired)
            }
            (Some(_), ClaimPresenceV1::Forbidden) => {
                return Err(CustodyRecordDecodeErrorV1::ClaimForbidden)
            }
            (None, _) => return Ok(()),
            (Some(_), _) => {}
        }
        let claim = self.claim.as_ref().expect("claim presence checked above");
        claim.validate(completeness)?;
        if claim.custody_id != self.custody_id
            || claim.checkout_fingerprint != self.checkout_fingerprint
            || claim.current_attempt != self.current_attempt
            // This is exact structural equality between the duplicated envelope and claim bytes,
            // not filesystem verification; differing birthtime presence is contradictory here.
            || claim.worktree != self.worktree
        {
            return Err(CustodyRecordDecodeErrorV1::ClaimIdentityMismatch);
        }
        // The preservation reason is duplicated on purpose: the state's copy drives sweep
        // classification and the identity rule, the claim's copy is what R2f2 reads from
        // the standalone artifact. Duplication is only safe if it cannot disagree.
        if let WorktreeCustodyStateV1::PreservationUnknown { reason } = &self.state {
            if *reason != claim.reason {
                return Err(CustodyRecordDecodeErrorV1::ClaimReasonMismatch);
            }
        }
        Ok(())
    }

    /// The decoder's own rules, run by a WRITER before it publishes.
    ///
    /// Same function the reader enforces, deliberately: a record that this crate would refuse to
    /// read must never be published in the first place, and the way to guarantee that is for the
    /// writer to be checked by the reader's rule rather than by a parallel one.
    pub fn validate_for_publication(&self) -> Result<(), CustodyRecordDecodeErrorV1> {
        self.validate()
    }

    pub fn encode_canonical(&self) -> Result<Vec<u8>, CustodyRecordDecodeErrorV1> {
        self.validate()?;
        let encoded =
            serde_json::to_vec(self).map_err(|_| CustodyRecordDecodeErrorV1::Malformed)?;
        if encoded.len() > MAX_CUSTODY_RECORD_JSON_BYTES {
            return Err(CustodyRecordDecodeErrorV1::OverBound);
        }
        Ok(encoded)
    }

    /// Decode with the workspace's canonical-bytes discipline: the record must round-trip
    /// to the exact input bytes, which makes the persisted form a frozen *byte* contract
    /// rather than merely a field-set contract (`execution_policy`'s `decode_canonical`).
    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, CustodyRecordDecodeErrorV1> {
        if bytes.len() > MAX_CUSTODY_RECORD_JSON_BYTES {
            return Err(CustodyRecordDecodeErrorV1::OverBound);
        }
        let value: Self =
            serde_json::from_slice(bytes).map_err(|_| CustodyRecordDecodeErrorV1::Malformed)?;
        value.validate()?;
        if value.encode_canonical()? != bytes {
            return Err(CustodyRecordDecodeErrorV1::NonCanonical);
        }
        Ok(value)
    }

    /// The sweep disposition for this record. Non-destructive for every state.
    #[must_use]
    pub fn sweep_disposition(&self) -> CustodySweepDispositionV1 {
        self.state.kind().sweep_disposition()
    }
}

/// The V3 record path for a worktree target — the sibling naming of `sidecar_path`, with a
/// suffix the legacy scanner cannot select.
///
/// **2b2 obligation.** The V3 writer must publish this record *instead of*, never
/// alongside, the legacy `sidecar_path` `.meta.json`. Emitting both for one target would
/// make the same checkout selectable by the legacy boot arm — which deletes — while the
/// V3 arm believes it is protecting it, defeating the rollback guarantee that an older
/// binary cannot name a V3 checkout (§2.2 "Record naming"; brief §4 Rollback).
#[must_use]
pub fn custody_record_path(worktree_path: &str) -> String {
    format!("{worktree_path}{CUSTODY_RECORD_SUFFIX}")
}

/// Whether `path` names a V3 custody record with a non-empty target stem.
#[must_use]
pub fn is_custody_record_name(path: &str) -> bool {
    let Some(stem) = path.strip_suffix(CUSTODY_RECORD_SUFFIX) else {
        return false;
    };
    // Scanner input is a directory-entry string; dots have no traversal semantics here.
    let name = stem
        .rsplit('/')
        .next()
        .expect("a string has a final segment");
    !stem.is_empty()
        && !stem.ends_with('/')
        && !matches!(
            ChildNameV2::from_bytes(name.as_bytes()),
            Ok(name)
                if ChildNameV2::parse_reserved(ReservedNameNamespaceV2::RetirementCapture, &name)
                    .is_ok()
        )
}

/// What a caller about to delete a checkout learns by asking whether a V3 custody record exists
/// beside it.
///
/// The question is deliberately *presence*, never *content*. A destructive caller must fail
/// closed, and a decode-based answer inverts that: a corrupt, truncated, or unreadable record
/// would decode-fail and — if the caller keyed on "did I get a record?" — read as absent, which
/// is the one answer that licenses deletion. Mere presence protects, so the protection cannot be
/// removed by damaging the record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CustodyRecordPresenceV1 {
    /// The record name is provably free: a no-follow stat under a pinned parent said `ENOENT`,
    /// or the enclosing directory does not exist at all, so nothing can be at that name.
    ProvablyAbsent,
    /// A directory entry exists at the record name — of any kind, readable or not.
    Present,
    /// The question could not be answered. Callers must treat this exactly as
    /// [`Self::Present`]; [`Self::authorizes_checkout_removal`] already does.
    Inconclusive(String),
}

impl CustodyRecordPresenceV1 {
    /// The only answer that may permit a checkout removal to proceed. The exhaustive match is
    /// deliberate, and mirrors [`CustodySweepDispositionV1::authorizes_checkout_removal`]: a new
    /// arm must be classified by decision, not by falling through a wildcard into "deletable".
    #[must_use]
    pub fn authorizes_checkout_removal(&self) -> bool {
        match self {
            Self::ProvablyAbsent => true,
            Self::Present | Self::Inconclusive(_) => false,
        }
    }
}

/// Is there a V3 custody record beside `worktree_path`?
///
/// Descriptor-relative: the enclosing directory is pinned (`O_NOFOLLOW`, identity-rechecked) and
/// the record name is stat'ed relative to that descriptor, so a same-name replacement of an
/// ancestor cannot redirect the probe and a symlinked record cannot be followed out of the root.
///
/// A missing enclosing directory answers [`CustodyRecordPresenceV1::ProvablyAbsent`] rather than
/// inconclusive, and that is a decision, not an oversight: if the directory holding the checkout
/// is gone then the checkout is gone, there is no work to protect, and answering "inconclusive"
/// there would permanently refuse the legacy cleanup of an already-vanished worktree.
#[must_use]
pub fn probe_custody_record_presence(worktree_path: &str) -> CustodyRecordPresenceV1 {
    let path = Path::new(worktree_path);
    let (Some(parent), Some(stem)) = (path.parent(), path.file_name()) else {
        return CustodyRecordPresenceV1::Inconclusive(format!(
            "worktree path has no enclosing directory: {worktree_path}"
        ));
    };
    let mut record_name = stem.to_os_string();
    record_name.push(CUSTODY_RECORD_SUFFIX);

    let pinned = match PinnedDirectoryV1::open(parent, "worktree custody probe") {
        Ok(pinned) => pinned,
        Err(error) => {
            return match parent.try_exists() {
                Ok(false) => CustodyRecordPresenceV1::ProvablyAbsent,
                _ => CustodyRecordPresenceV1::Inconclusive(format!(
                    "worktree parent is not pinnable: {error}"
                )),
            }
        }
    };
    match pinned.child_entry_exists(&record_name, "worktree custody probe") {
        Ok(true) => CustodyRecordPresenceV1::Present,
        Ok(false) => CustodyRecordPresenceV1::ProvablyAbsent,
        Err(error) => {
            CustodyRecordPresenceV1::Inconclusive(format!("custody record is unstattable: {error}"))
        }
    }
}

/// Best-effort read of the custody state durably recorded beside `worktree_path`.
///
/// **Deliberately NOT the deletion gate's question.** The gate asks *presence*, never content, so
/// that damaging a record cannot remove its protection. This asks content, and it is used only
/// where a wrong answer is protective or neutral: re-deriving a lost `Preserve` disposition
/// (slice 2c2, P5) and labelling a refusal report truthfully. `None` therefore means "no answer" —
/// absent, unreadable, or undecodable — and every caller must treat it as "no additional
/// knowledge", never as "not preserved".
#[must_use]
pub fn probe_custody_record_state(worktree_path: &str) -> Option<WorktreeCustodyStateKindV1> {
    let path = Path::new(worktree_path);
    let (Some(parent), Some(stem)) = (path.parent(), path.file_name()) else {
        return None;
    };
    let mut record_name = stem.to_os_string();
    record_name.push(CUSTODY_RECORD_SUFFIX);
    let pinned = PinnedDirectoryV1::open(parent, "worktree custody state probe").ok()?;
    match pinned.child_entry_exists(&record_name, "worktree custody state probe") {
        Ok(true) => {}
        Ok(false) | Err(_) => return None,
    }
    read_custody_record_in(&pinned, &record_name)
        .ok()
        .map(|record| record.state.kind())
}

/// Why a custody record on disk could not be read into a decoded record. Every variant
/// classifies as [`CustodySweepDispositionV1::Unknown`] — never as deletable.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CustodyReadRefusalV1 {
    /// The entry could not be opened under descriptor custody: it is a symlink
    /// (`O_NOFOLLOW`), is not a regular file, is unreadable, or vanished.
    #[error("custody record is not readable under descriptor custody: {0}")]
    Unreadable(String),
    /// The record has more than one hard link, so exclusive custody of its bytes cannot
    /// be proved.
    #[error("custody record has more than one link")]
    MultiLink,
    /// The record is larger than [`MAX_CUSTODY_RECORD_JSON_BYTES`].
    #[error("custody record exceeded its encoded bound")]
    OverBound,
    #[error("custody record is undecodable: {0}")]
    Decode(#[from] CustodyRecordDecodeErrorV1),
}

/// Read one custody record from an already-pinned directory: no-follow, regular-file-only,
/// single-link-only, byte-bounded, canonical-decoded.
pub fn read_custody_record_in(
    dir: &PinnedDirectoryV1,
    name: &OsStr,
) -> Result<WorktreeCustodyRecordV1, CustodyReadRefusalV1> {
    let file = dir
        .open_regular_file(name, "worktree custody record")
        .map_err(|error| CustodyReadRefusalV1::Unreadable(error.to_string()))?;
    let metadata = file
        .metadata()
        .map_err(|error| CustodyReadRefusalV1::Unreadable(error.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if metadata.nlink() != 1 {
            return Err(CustodyReadRefusalV1::MultiLink);
        }
    }
    if metadata.len() > MAX_CUSTODY_RECORD_JSON_BYTES as u64 {
        return Err(CustodyReadRefusalV1::OverBound);
    }
    let mut bytes = Vec::new();
    file.take(MAX_CUSTODY_RECORD_JSON_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| CustodyReadRefusalV1::Unreadable(error.to_string()))?;
    if bytes.len() > MAX_CUSTODY_RECORD_JSON_BYTES {
        return Err(CustodyReadRefusalV1::OverBound);
    }
    Ok(WorktreeCustodyRecordV1::decode_canonical(&bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::execution_policy::{PolicyNodeRefV1, Sha256HexV1, WorktreeCustodyIdV1};
    use bridge_core::fs_custody::{BirthTimeV1, DirectoryIdentityV1};
    use bridge_core::ids::{AttemptId, AttemptIdentity, ExecutionId};

    fn cid(digit: char) -> WorktreeCustodyIdV1 {
        WorktreeCustodyIdV1::parse(format!("custody-{}", digit.to_string().repeat(64))).unwrap()
    }

    fn sha(digit: char) -> Sha256HexV1 {
        Sha256HexV1::parse(digit.to_string().repeat(64)).unwrap()
    }

    fn execution_id(digit: char) -> ExecutionId {
        ExecutionId::parse(format!("exec-{}", digit.to_string().repeat(32))).unwrap()
    }

    fn attempt_id(digit: char) -> AttemptId {
        AttemptId::parse(format!("attempt-{}", digit.to_string().repeat(32))).unwrap()
    }

    fn attempt_identity() -> AttemptIdentity {
        AttemptIdentity {
            execution_id: execution_id('1'),
            attempt_id: attempt_id('2'),
            ordinal: 0,
            parent_attempt_id: None,
        }
    }

    fn attempt_identity_golden() -> String {
        format!(
            "{{\"execution_id\":\"exec-{}\",\"attempt_id\":\"attempt-{}\",\"ordinal\":0}}",
            "1".repeat(32),
            "2".repeat(32)
        )
    }

    fn object(path: &str, dev: u64, ino: u64) -> WorktreeObjectIdentityV1 {
        WorktreeObjectIdentityV1 {
            canonical_path: path.to_string(),
            directory_identity: DirectoryIdentityV1 {
                canonical_path: path.to_string(),
                dev: Some(dev),
                ino: Some(ino),
                btime: None,
            },
        }
    }

    fn object_golden(path: &str, dev: u64, ino: u64) -> String {
        format!(
            "{{\"canonical_path\":\"{path}\",\"directory_identity\":\
             {{\"canonical_path\":\"{path}\",\"dev\":{dev},\"ino\":{ino}}}}}"
        )
    }

    fn claim() -> PreservedWorktreeClaimV1 {
        PreservedWorktreeClaimV1 {
            schema_version: WORKTREE_CUSTODY_RECORD_SCHEMA_V1,
            custody_id: cid('3'),
            execution_id: execution_id('1'),
            // Ordinal 0: the delivery-origin attempt IS the current attempt.
            origin_attempt_id: attempt_id('2'),
            current_attempt: attempt_identity(),
            node: PolicyNodeRefV1 {
                sorted_ordinal: 7,
                id_sha256: sha('5'),
            },
            checkout_fingerprint: sha('6'),
            source: object("/src", 1, 10),
            root: object("/root", 1, 11),
            worktree: object("/root/wt", 1, 12),
            common_dir: object("/src/.git", 1, 13),
            reason: PreservationReasonV1::NodeFailure,
            created_wall_ms: 1_700_000_000_000,
            recovery_locator: RecoveryLocatorV1::RegisteredWorktree {},
        }
    }

    fn claim_golden() -> String {
        format!(
            "{{\"schema_version\":1,\"custody_id\":\"custody-{}\",\"execution_id\":\"exec-{}\",\
             \"origin_attempt_id\":\"attempt-{}\",\"current_attempt\":{},\
             \"node\":{{\"sorted_ordinal\":7,\"id_sha256\":\"{}\"}},\
             \"checkout_fingerprint\":\"{}\",\"source\":{},\"root\":{},\"worktree\":{},\
             \"common_dir\":{},\"reason\":\"node_failure\",\
             \"created_wall_ms\":1700000000000,\
             \"recovery_locator\":{{\"locator\":\"registered_worktree\"}}}}",
            "3".repeat(64),
            "1".repeat(32),
            "2".repeat(32),
            attempt_identity_golden(),
            "5".repeat(64),
            "6".repeat(64),
            object_golden("/src", 1, 10),
            object_golden("/root", 1, 11),
            object_golden("/root/wt", 1, 12),
            object_golden("/src/.git", 1, 13),
        )
    }

    fn live_record() -> WorktreeCustodyRecordV1 {
        WorktreeCustodyRecordV1 {
            schema_version: WORKTREE_CUSTODY_RECORD_SCHEMA_V1,
            custody_id: cid('3'),
            checkout_fingerprint: sha('6'),
            current_attempt: attempt_identity(),
            worktree: object("/root/wt", 1, 12),
            state: WorktreeCustodyStateV1::LiveProtected {},
            claim: None,
        }
    }

    fn live_record_golden() -> String {
        format!(
            "{{\"schema_version\":1,\"custody_id\":\"custody-{}\",\"checkout_fingerprint\":\"{}\",\
             \"current_attempt\":{},\"worktree\":{},\"state\":{{\"state\":\"live_protected\"}},\
             \"claim\":null}}",
            "3".repeat(64),
            "6".repeat(64),
            attempt_identity_golden(),
            object_golden("/root/wt", 1, 12),
        )
    }

    fn preserved_record() -> WorktreeCustodyRecordV1 {
        WorktreeCustodyRecordV1 {
            state: WorktreeCustodyStateV1::Preserved {},
            claim: Some(claim()),
            ..live_record()
        }
    }

    /// A plan-derived (degraded) object identity: canonical path present,
    /// `dev`/`ino` not yet observable.
    fn degraded_object(path: &str) -> WorktreeObjectIdentityV1 {
        WorktreeObjectIdentityV1 {
            canonical_path: path.to_string(),
            directory_identity: DirectoryIdentityV1 {
                canonical_path: path.to_string(),
                dev: None,
                ino: None,
                btime: None,
            },
        }
    }

    /// A claim whose `reason` agrees with `state` and whose four object
    /// identities are degraded or complete as asked.
    fn claim_for(state: &WorktreeCustodyStateV1, degraded: bool) -> PreservedWorktreeClaimV1 {
        let ident = |path: &str, dev: u64, ino: u64| {
            if degraded {
                degraded_object(path)
            } else {
                object(path, dev, ino)
            }
        };
        PreservedWorktreeClaimV1 {
            source: ident("/src", 1, 10),
            root: ident("/root", 1, 11),
            worktree: ident("/root/wt", 1, 12),
            common_dir: ident("/src/.git", 1, 13),
            reason: match state {
                WorktreeCustodyStateV1::PreservationUnknown { reason } => *reason,
                _ => PreservationReasonV1::NodeFailure,
            },
            ..claim()
        }
    }

    /// A record in `state`, with `claim`, whose envelope identity is degraded
    /// or complete as asked.
    fn record_for(
        state: &WorktreeCustodyStateV1,
        claim: Option<PreservedWorktreeClaimV1>,
        degraded: bool,
    ) -> WorktreeCustodyRecordV1 {
        WorktreeCustodyRecordV1 {
            worktree: if degraded {
                degraded_object("/root/wt")
            } else {
                object("/root/wt", 1, 12)
            },
            state: state.clone(),
            claim,
            ..live_record()
        }
    }

    fn preserved_record_golden() -> String {
        format!(
            "{{\"schema_version\":1,\"custody_id\":\"custody-{}\",\"checkout_fingerprint\":\"{}\",\
             \"current_attempt\":{},\"worktree\":{},\"state\":{{\"state\":\"preserved\"}},\
             \"claim\":{}}}",
            "3".repeat(64),
            "6".repeat(64),
            attempt_identity_golden(),
            object_golden("/root/wt", 1, 12),
            claim_golden(),
        )
    }

    /// Discriminates: any drift in the pinned wire tag of any of the ten
    /// `WorktreeCustodyStateV1` variants (focused boundary §2.2's state
    /// machine), including the tag key `"state"`, the `snake_case` renaming,
    /// and the payload shape of the two payload-carrying variants. 2a's
    /// reader, 2b2's writer, and 2d's exchange all match exhaustively on this
    /// enum; a silent tag change between them is a silent deletion path.
    #[test]
    fn custody_state_all_ten_variants_golden_round_trip() {
        let cases: [(WorktreeCustodyStateV1, String); 10] = [
            (
                WorktreeCustodyStateV1::ProtectionPrepared {},
                r#"{"state":"protection_prepared"}"#.to_string(),
            ),
            (
                WorktreeCustodyStateV1::UnusedSettled {},
                r#"{"state":"unused_settled"}"#.to_string(),
            ),
            (
                WorktreeCustodyStateV1::Materializing {},
                r#"{"state":"materializing"}"#.to_string(),
            ),
            (
                WorktreeCustodyStateV1::LiveProtected {},
                r#"{"state":"live_protected"}"#.to_string(),
            ),
            (
                WorktreeCustodyStateV1::PreservationPrepared {},
                r#"{"state":"preservation_prepared"}"#.to_string(),
            ),
            (
                WorktreeCustodyStateV1::Preserved {},
                r#"{"state":"preserved"}"#.to_string(),
            ),
            (
                WorktreeCustodyStateV1::DeleteAuthorized {},
                r#"{"state":"delete_authorized"}"#.to_string(),
            ),
            (
                WorktreeCustodyStateV1::Removed {},
                r#"{"state":"removed"}"#.to_string(),
            ),
            (
                WorktreeCustodyStateV1::RecoveredLive {
                    predecessor_claim_digest: sha('a'),
                },
                format!(
                    "{{\"state\":\"recovered_live\",\"predecessor_claim_digest\":\"{}\"}}",
                    "a".repeat(64)
                ),
            ),
            (
                WorktreeCustodyStateV1::PreservationUnknown {
                    reason: PreservationReasonV1::AmbiguousCleanup,
                },
                r#"{"state":"preservation_unknown","reason":"ambiguous_cleanup"}"#.to_string(),
            ),
        ];
        assert_eq!(cases.len(), WorktreeCustodyStateKindV1::ALL.len());
        for (variant, golden) in cases {
            assert_eq!(
                serde_json::to_string(&variant).unwrap(),
                golden,
                "encode drift for {variant:?}"
            );
            assert_eq!(
                serde_json::from_str::<WorktreeCustodyStateV1>(&golden).unwrap(),
                variant
            );
        }
    }

    /// Discriminates: the F-2 standing rule being dropped for this enum --
    /// `#[serde(tag = "state", deny_unknown_fields)]` on an internally-tagged
    /// enum only buffers-and-checks leftover keys for *struct*-payload
    /// variants, so every unit-like variant here is declared `Variant {}`.
    /// Reverting any of the eight to a bare unit variant would silently
    /// accept a stray sibling key. Mutation-checked (reverted before commit):
    /// reverting `Preserved {}` to a bare `Preserved` turned exactly this test
    /// red while `custody_state_all_ten_variants_golden_round_trip` stayed
    /// green -- which is the F-2 rule's whole point (the wire bytes are
    /// identical, only the strictness differs), and is why the golden test
    /// alone cannot guard it.
    #[test]
    fn custody_state_rejects_unknown_field_on_every_unit_like_variant() {
        for tag in [
            "protection_prepared",
            "unused_settled",
            "materializing",
            "live_protected",
            "preservation_prepared",
            "preserved",
            "delete_authorized",
            "removed",
        ] {
            let json = format!("{{\"state\":\"{tag}\",\"extra\":1}}");
            assert!(
                serde_json::from_str::<WorktreeCustodyStateV1>(&json).is_err(),
                "unit-like variant {tag} must reject a stray sibling key"
            );
        }
    }

    /// Discriminates: `deny_unknown_fields` being dropped from the two
    /// payload-carrying variants.
    #[test]
    fn custody_state_rejects_unknown_field_on_struct_variants() {
        let recovered = format!(
            "{{\"state\":\"recovered_live\",\"predecessor_claim_digest\":\"{}\",\"extra\":1}}",
            "a".repeat(64)
        );
        assert!(serde_json::from_str::<WorktreeCustodyStateV1>(&recovered).is_err());
        assert!(serde_json::from_str::<WorktreeCustodyStateV1>(
            r#"{"state":"preservation_unknown","reason":"ambiguous_cleanup","extra":1}"#
        )
        .is_err());
    }

    /// Discriminates: an unrecognized `"state"` tag being silently accepted
    /// (e.g. via a `#[serde(other)]` catch-all), which would let an
    /// unknown-provenance record classify as a deletable one.
    #[test]
    fn custody_state_rejects_unknown_variant_tag() {
        assert!(serde_json::from_str::<WorktreeCustodyStateV1>(r#"{"state":"bogus"}"#).is_err());
        assert!(
            serde_json::from_str::<WorktreeCustodyStateV1>(r#"{"state":"Preserved"}"#).is_err()
        );
    }

    /// Discriminates: drift in any of the six `PreservationReasonV1` bare
    /// string variant names -- the reason is the vocabulary shared by the
    /// claim's `reason` field and `PreservationUnknown { reason }`.
    #[test]
    fn preservation_reason_all_variants_golden_round_trip() {
        let cases = [
            (PreservationReasonV1::NodeFailure, "\"node_failure\""),
            (PreservationReasonV1::Cancellation, "\"cancellation\""),
            (
                PreservationReasonV1::AmbiguousCleanup,
                "\"ambiguous_cleanup\"",
            ),
            (
                PreservationReasonV1::MaterializationInFlight,
                "\"materialization_in_flight\"",
            ),
            (
                PreservationReasonV1::PostConditionDisagreement,
                "\"post_condition_disagreement\"",
            ),
            (PreservationReasonV1::RemovalFailed, "\"removal_failed\""),
        ];
        assert_eq!(cases.len(), PreservationReasonV1::ALL.len());
        for (variant, golden) in cases {
            assert_eq!(serde_json::to_string(&variant).unwrap(), golden);
            assert_eq!(
                serde_json::from_str::<PreservationReasonV1>(golden).unwrap(),
                variant
            );
        }
        assert!(serde_json::from_str::<PreservationReasonV1>("\"bogus\"").is_err());
    }

    /// Discriminates: drift in the three `RecoveryLocatorV1` variants or in
    /// the `"locator"` tag key, and the F-2 unit-variant rule being dropped
    /// for them.
    #[test]
    fn recovery_locator_all_variants_golden_round_trip_and_reject_unknown_keys() {
        let cases = [
            (
                RecoveryLocatorV1::RegisteredWorktree {},
                r#"{"locator":"registered_worktree"}"#,
            ),
            (
                RecoveryLocatorV1::UnregisteredDirectory {},
                r#"{"locator":"unregistered_directory"}"#,
            ),
            (
                RecoveryLocatorV1::RegistrationUnproven {},
                r#"{"locator":"registration_unproven"}"#,
            ),
        ];
        assert_eq!(cases.len(), RecoveryLocatorV1::ALL.len());
        for (variant, golden) in cases {
            assert_eq!(serde_json::to_string(&variant).unwrap(), golden);
            assert_eq!(
                serde_json::from_str::<RecoveryLocatorV1>(golden).unwrap(),
                variant
            );
            let with_extra = golden.replacen('}', ",\"extra\":1}", 1);
            assert!(serde_json::from_str::<RecoveryLocatorV1>(&with_extra).is_err());
        }
        assert!(serde_json::from_str::<RecoveryLocatorV1>(r#"{"locator":"bogus"}"#).is_err());
    }

    /// Discriminates: drift in `PreservedWorktreeClaimV1`'s field
    /// names/order/nesting -- this is the durable artifact R2f2 disposition
    /// reads, so its exact bytes are the persisted meaning.
    #[test]
    fn preserved_claim_golden_round_trip() {
        assert_eq!(serde_json::to_string(&claim()).unwrap(), claim_golden());
        assert_eq!(
            serde_json::from_str::<PreservedWorktreeClaimV1>(&claim_golden()).unwrap(),
            claim()
        );
    }

    /// Append a key to the *outermost* JSON object, not to whichever nested
    /// object happens to close first.
    fn with_extra_top_level_key(json: &str) -> String {
        let trimmed = json.strip_suffix('}').expect("golden is a JSON object");
        format!("{trimmed},\"extra\":1}}")
    }

    /// Golden coverage for the additive directory birthtime refinement. The legacy object
    /// remains byte-for-byte unchanged when birthtime is absent, while a present timestamp has
    /// one exact seconds/nanoseconds representation and malformed nanoseconds fail strict read.
    #[test]
    fn directory_birthtime_golden_round_trip_and_strict_malformed_refusal() {
        let legacy = object("/root/wt", 1, 12);
        assert_eq!(
            serde_json::to_string(&legacy).unwrap(),
            object_golden("/root/wt", 1, 12),
            "an absent btime must preserve the exact pre-upgrade bytes"
        );

        let mut with_btime = legacy;
        with_btime.directory_identity.btime =
            Some(BirthTimeV1::new(1_700_000_000, 123_456_789).unwrap());
        let golden = "{\"canonical_path\":\"/root/wt\",\"directory_identity\":{\"canonical_path\":\"/root/wt\",\"dev\":1,\"ino\":12,\"btime\":{\"secs\":1700000000,\"nanos\":123456789}}}";
        assert_eq!(serde_json::to_string(&with_btime).unwrap(), golden);
        assert_eq!(
            serde_json::from_str::<WorktreeObjectIdentityV1>(golden).unwrap(),
            with_btime
        );

        let mut record = live_record();
        record.worktree = with_btime;
        let record_golden = live_record_golden().replacen(
            "\"ino\":12}",
            "\"ino\":12,\"btime\":{\"secs\":1700000000,\"nanos\":123456789}}",
            1,
        );
        assert_eq!(
            String::from_utf8(record.encode_canonical().unwrap()).unwrap(),
            record_golden
        );
        assert_eq!(
            WorktreeCustodyRecordV1::decode_canonical(record_golden.as_bytes()).unwrap(),
            record
        );

        let malformed = golden.replace("\"nanos\":123456789", "\"nanos\":1000000000");
        assert!(
            serde_json::from_str::<WorktreeObjectIdentityV1>(&malformed).is_err(),
            "a non-canonical nanosecond component must be rejected by the strict reader"
        );
        let malformed_record = record_golden.replace("\"nanos\":123456789", "\"nanos\":1000000000");
        assert_eq!(
            WorktreeCustodyRecordV1::decode_canonical(malformed_record.as_bytes()),
            Err(CustodyRecordDecodeErrorV1::Malformed)
        );
    }

    /// Discriminates removal of the claim's strict unknown-field policy.
    #[test]
    fn preserved_claim_rejects_unknown_field() {
        let with_extra = with_extra_top_level_key(&claim_golden());
        assert!(serde_json::from_str::<PreservedWorktreeClaimV1>(&with_extra).is_err());
    }

    /// NOTE (nested tolerance, closed here by the canonical re-encode rather
    /// than by an attribute): `AttemptIdentity` (`bridge-core/src/ids.rs:155`)
    /// carries no `#[serde(deny_unknown_fields)]`, so a stray key *inside*
    /// `current_attempt` survives plain deserialization of the claim. Slice 2a
    /// does not widen its blast radius by tightening a workspace-wide identity
    /// type; instead this test pins that the record decoder refuses such bytes
    /// anyway, because the dropped key cannot be reproduced by the canonical
    /// re-encode. Discriminates: the re-encode check being dropped, which
    /// would re-open the hole for every nested type that tolerates it.
    #[test]
    fn nested_unknown_field_is_refused_by_the_canonical_re_encode() {
        let nested = claim_golden().replacen('}', ",\"extra\":1}", 1);
        assert!(
            serde_json::from_str::<PreservedWorktreeClaimV1>(&nested).is_ok(),
            "precondition: the nested identity type itself tolerates the key"
        );
        let record = preserved_record_golden().replacen('}', ",\"extra\":1}", 1);
        assert_eq!(
            WorktreeCustodyRecordV1::decode_canonical(record.as_bytes()),
            Err(CustodyRecordDecodeErrorV1::NonCanonical)
        );
    }

    /// Discriminates: drift in the on-disk `WorktreeCustodyRecordV1` envelope
    /// in either of its two shapes (claim-absent live state, claim-present
    /// preserved state).
    #[test]
    fn custody_record_golden_round_trip_live_and_preserved() {
        assert_eq!(
            String::from_utf8(live_record().encode_canonical().unwrap()).unwrap(),
            live_record_golden()
        );
        assert_eq!(
            WorktreeCustodyRecordV1::decode_canonical(live_record_golden().as_bytes()).unwrap(),
            live_record()
        );
        assert_eq!(
            String::from_utf8(preserved_record().encode_canonical().unwrap()).unwrap(),
            preserved_record_golden()
        );
        assert_eq!(
            WorktreeCustodyRecordV1::decode_canonical(preserved_record_golden().as_bytes())
                .unwrap(),
            preserved_record()
        );
    }

    /// Discriminates: the decoder accepting a record whose schema version is
    /// absent or is not `WORKTREE_CUSTODY_RECORD_SCHEMA_V1` -- either would
    /// let a future record shape be read under today's rules.
    #[test]
    fn custody_record_decode_rejects_missing_and_wrong_schema_version() {
        let wrong = live_record_golden().replace("\"schema_version\":1", "\"schema_version\":2");
        assert_eq!(
            WorktreeCustodyRecordV1::decode_canonical(wrong.as_bytes()),
            Err(CustodyRecordDecodeErrorV1::SchemaVersion)
        );
        let missing = live_record_golden().replacen("\"schema_version\":1,", "", 1);
        assert_eq!(
            WorktreeCustodyRecordV1::decode_canonical(missing.as_bytes()),
            Err(CustodyRecordDecodeErrorV1::Malformed)
        );
        let claim_wrong = preserved_record_golden().replacen(
            "\"claim\":{\"schema_version\":1",
            "\"claim\":{\"schema_version\":9",
            1,
        );
        assert_eq!(
            WorktreeCustodyRecordV1::decode_canonical(claim_wrong.as_bytes()),
            Err(CustodyRecordDecodeErrorV1::SchemaVersion)
        );
    }

    /// Discriminates: the decoder accepting an unknown state tag or an
    /// unknown sibling key anywhere in the envelope.
    #[test]
    fn custody_record_decode_rejects_unknown_variant_and_unknown_field() {
        let unknown_variant =
            live_record_golden().replace("\"state\":\"live_protected\"", "\"state\":\"bogus\"");
        assert_eq!(
            WorktreeCustodyRecordV1::decode_canonical(unknown_variant.as_bytes()),
            Err(CustodyRecordDecodeErrorV1::Malformed)
        );
        let unknown_field = with_extra_top_level_key(&live_record_golden());
        assert_eq!(
            WorktreeCustodyRecordV1::decode_canonical(unknown_field.as_bytes()),
            Err(CustodyRecordDecodeErrorV1::Malformed)
        );
    }

    /// Discriminates: the decoder not bounding the record's free-form strings.
    /// Every other string in the record is a validated newtype (custody id,
    /// execution/attempt id, sha256); the four object identities carry raw
    /// canonical paths, which is the one unbounded axis.
    #[test]
    fn custody_record_decode_rejects_out_of_bounds_path_string() {
        let long = "/".to_string() + &"p".repeat(MAX_CUSTODY_PATH_BYTES);
        let mut record = live_record();
        record.worktree = object(&long, 1, 12);
        let bytes = serde_json::to_vec(&record).unwrap();
        assert_eq!(
            WorktreeCustodyRecordV1::decode_canonical(&bytes),
            Err(CustodyRecordDecodeErrorV1::PathBound)
        );

        let mut empty_path = live_record();
        empty_path.worktree = object("", 1, 12);
        assert_eq!(
            WorktreeCustodyRecordV1::decode_canonical(&serde_json::to_vec(&empty_path).unwrap()),
            Err(CustodyRecordDecodeErrorV1::PathBound)
        );

        let mut claim_path = preserved_record();
        claim_path.claim.as_mut().unwrap().common_dir = object(&long, 1, 13);
        assert_eq!(
            WorktreeCustodyRecordV1::decode_canonical(&serde_json::to_vec(&claim_path).unwrap()),
            Err(CustodyRecordDecodeErrorV1::PathBound)
        );
    }

    /// Discriminates: the decoder not bounding total record size before
    /// deserializing -- an unbounded custody record is a boot-sweep denial
    /// surface.
    #[test]
    fn custody_record_decode_rejects_over_bound_bytes() {
        let bytes = vec![b' '; MAX_CUSTODY_RECORD_JSON_BYTES + 1];
        assert_eq!(
            WorktreeCustodyRecordV1::decode_canonical(&bytes),
            Err(CustodyRecordDecodeErrorV1::OverBound)
        );
    }

    /// Discriminates: the canonical re-encode check being dropped, which is
    /// what makes the persisted form a frozen *byte* contract rather than
    /// merely a field-set contract (the `execution_policy` idiom).
    /// Mutation-checked (reverted before commit): replacing the re-encode
    /// comparison with a bare `encode_canonical()?` turned this test and
    /// `nested_unknown_field_is_refused_by_the_canonical_re_encode` red, and
    /// nothing else -- so those two are the only guards on that property.
    #[test]
    fn custody_record_decode_rejects_non_canonical_bytes() {
        let padded = format!(" {}", live_record_golden());
        assert_eq!(
            WorktreeCustodyRecordV1::decode_canonical(padded.as_bytes()),
            Err(CustodyRecordDecodeErrorV1::NonCanonical)
        );
        // Field reordering decodes to the same value but is not the canonical form.
        let reordered = live_record_golden().replacen(
            "\"schema_version\":1,\"custody_id\"",
            "\"custody_id\"",
            1,
        );
        let reordered =
            reordered.replacen("\"claim\":null}", "\"claim\":null,\"schema_version\":1}", 1);
        assert_eq!(
            WorktreeCustodyRecordV1::decode_canonical(reordered.as_bytes()),
            Err(CustodyRecordDecodeErrorV1::NonCanonical)
        );
    }

    /// Every state, with the two `PreservationUnknown` reason classes split
    /// out, and the claim/identity rule each one is settled to carry.
    fn state_rule_matrix() -> Vec<(
        WorktreeCustodyStateV1,
        ClaimPresenceV1,
        IdentityCompletenessV1,
    )> {
        use ClaimPresenceV1 as P;
        use IdentityCompletenessV1 as I;
        vec![
            (
                WorktreeCustodyStateV1::ProtectionPrepared {},
                P::Optional,
                I::MayBeDegraded,
            ),
            (
                WorktreeCustodyStateV1::UnusedSettled {},
                P::Forbidden,
                I::MayBeDegraded,
            ),
            (
                WorktreeCustodyStateV1::Materializing {},
                P::Forbidden,
                I::MayBeDegraded,
            ),
            (
                WorktreeCustodyStateV1::LiveProtected {},
                P::Forbidden,
                I::Complete,
            ),
            (
                WorktreeCustodyStateV1::PreservationPrepared {},
                P::Required,
                I::Complete,
            ),
            (
                WorktreeCustodyStateV1::Preserved {},
                P::Required,
                I::Complete,
            ),
            (
                WorktreeCustodyStateV1::DeleteAuthorized {},
                P::Forbidden,
                I::Complete,
            ),
            (
                WorktreeCustodyStateV1::Removed {},
                P::Forbidden,
                I::Complete,
            ),
            (
                WorktreeCustodyStateV1::RecoveredLive {
                    predecessor_claim_digest: sha('a'),
                },
                P::Forbidden,
                I::Complete,
            ),
            (
                WorktreeCustodyStateV1::PreservationUnknown {
                    reason: PreservationReasonV1::MaterializationInFlight,
                },
                P::Required,
                I::MayBeDegraded,
            ),
            (
                WorktreeCustodyStateV1::PreservationUnknown {
                    reason: PreservationReasonV1::NodeFailure,
                },
                P::Required,
                I::Complete,
            ),
        ]
    }

    /// Discriminates: the settled per-state rule drifting away from the table
    /// 2b2's writer is built against. Pinned as data, next to the states, so a
    /// later slice cannot quietly re-derive it.
    #[test]
    fn per_state_claim_and_identity_rules_are_pinned() {
        for (state, presence, completeness) in state_rule_matrix() {
            assert_eq!(state.claim_presence(), presence, "presence for {state:?}");
            assert_eq!(
                state.identity_completeness(),
                completeness,
                "completeness for {state:?}"
            );
        }
        // Every kind is covered; the two PreservationUnknown rows collapse to one kind.
        let kinds: std::collections::BTreeSet<_> = state_rule_matrix()
            .into_iter()
            .map(|(state, _, _)| state.kind())
            .collect();
        assert_eq!(kinds.len(), WorktreeCustodyStateKindV1::ALL.len());
    }

    /// Discriminates: the claim-presence rule going unenforced. A preserving
    /// state without its durable claim would leave R2f2 with nothing to
    /// dispose; a state that asserts no preservation carrying one would claim a
    /// preservation that never happened. `ProtectionPrepared` is the one
    /// `Optional` state -- §5.1 step 4 conditions the *full* claim on complete
    /// target identity, so the pre-effect marker may exist either way.
    #[test]
    fn custody_record_decode_enforces_claim_presence_per_state() {
        for (state, presence, completeness) in state_rule_matrix() {
            let degraded = completeness == IdentityCompletenessV1::MayBeDegraded;

            let without = record_for(&state, None, degraded);
            let with = record_for(&state, Some(claim_for(&state, degraded)), degraded);

            let without_result =
                WorktreeCustodyRecordV1::decode_canonical(&serde_json::to_vec(&without).unwrap());
            let with_result =
                WorktreeCustodyRecordV1::decode_canonical(&serde_json::to_vec(&with).unwrap());

            match presence {
                ClaimPresenceV1::Forbidden => {
                    assert!(without_result.is_ok(), "{state:?} without claim");
                    assert_eq!(
                        with_result,
                        Err(CustodyRecordDecodeErrorV1::ClaimForbidden),
                        "{state:?} must not carry a claim"
                    );
                }
                ClaimPresenceV1::Required => {
                    assert_eq!(
                        without_result,
                        Err(CustodyRecordDecodeErrorV1::ClaimRequired),
                        "{state:?} must carry a claim"
                    );
                    assert!(with_result.is_ok(), "{state:?} with claim: {with_result:?}");
                }
                ClaimPresenceV1::Optional => {
                    assert!(without_result.is_ok(), "{state:?} without claim");
                    assert!(with_result.is_ok(), "{state:?} with claim: {with_result:?}");
                }
            }
        }
    }

    /// Discriminates: the identity-completeness rule going unenforced in either
    /// direction. A `Complete` state accepting plan-derived paths with no
    /// observed `dev`/`ino` would let the sweep's descriptor check (P3) be
    /// silently skipped; a `MayBeDegraded` state *rejecting* them would make
    /// the pre-`git add` marker and the materialization-in-flight record
    /// unwritable at exactly the moment §5.7 rows 3-4 require them.
    #[test]
    #[cfg(unix)]
    fn custody_record_decode_enforces_identity_completeness_per_state() {
        for (state, presence, completeness) in state_rule_matrix() {
            let claim_needed = presence == ClaimPresenceV1::Required;
            let degraded_claim = claim_needed.then(|| claim_for(&state, true));
            let complete_claim = claim_needed.then(|| claim_for(&state, false));

            let degraded = record_for(&state, degraded_claim, true);
            let complete = record_for(&state, complete_claim, false);

            let degraded_result =
                WorktreeCustodyRecordV1::decode_canonical(&serde_json::to_vec(&degraded).unwrap());
            let complete_result =
                WorktreeCustodyRecordV1::decode_canonical(&serde_json::to_vec(&complete).unwrap());

            assert!(
                complete_result.is_ok(),
                "{state:?} with complete identity: {complete_result:?}"
            );
            match completeness {
                IdentityCompletenessV1::MayBeDegraded => assert!(
                    degraded_result.is_ok(),
                    "{state:?} must accept a degraded identity: {degraded_result:?}"
                ),
                IdentityCompletenessV1::Complete => assert_eq!(
                    degraded_result,
                    Err(CustodyRecordDecodeErrorV1::IdentityIncomplete),
                    "{state:?} must require observed dev/ino"
                ),
            }
        }
    }

    /// Discriminates: a half-populated object identity being accepted. `dev`
    /// without `ino` (or the reverse) is neither an observed identity nor a
    /// plan-derived one -- it is evidence that cannot be compared, and the
    /// sweep's descriptor check would silently treat it as absent.
    #[test]
    fn custody_record_decode_rejects_partial_object_identity() {
        for (dev, ino) in [(Some(1_u64), None), (None, Some(2_u64))] {
            let mut record = live_record();
            record.worktree = WorktreeObjectIdentityV1 {
                canonical_path: "/root/wt".to_string(),
                directory_identity: DirectoryIdentityV1 {
                    canonical_path: "/root/wt".to_string(),
                    dev,
                    ino,
                    btime: None,
                },
            };
            assert_eq!(
                WorktreeCustodyRecordV1::decode_canonical(&serde_json::to_vec(&record).unwrap()),
                Err(CustodyRecordDecodeErrorV1::IdentityPartial),
                "dev={dev:?} ino={ino:?}"
            );
        }
    }

    /// P4(a). Discriminates: the claim's own attempt fields being allowed to
    /// contradict each other. `execution_id` is carried both directly and
    /// inside `current_attempt`, and at ordinal 0 the delivery-origin attempt
    /// is by construction the current one (`AttemptIdentity::initial`), so both
    /// disagreements are forgeries rather than legal shapes.
    #[test]
    fn custody_record_decode_rejects_self_contradictory_claim_attempt_fields() {
        let mut wrong_execution = preserved_record();
        wrong_execution.claim.as_mut().unwrap().execution_id = execution_id('9');
        assert_eq!(
            WorktreeCustodyRecordV1::decode_canonical(
                &serde_json::to_vec(&wrong_execution).unwrap()
            ),
            Err(CustodyRecordDecodeErrorV1::ClaimAttemptContradiction)
        );

        let mut wrong_origin = preserved_record();
        wrong_origin.claim.as_mut().unwrap().origin_attempt_id = attempt_id('9');
        assert_eq!(
            WorktreeCustodyRecordV1::decode_canonical(&serde_json::to_vec(&wrong_origin).unwrap()),
            Err(CustodyRecordDecodeErrorV1::ClaimAttemptContradiction)
        );

        // Positive control: after a resume the origin attempt legally differs
        // from the current one, so the ordinal-0 rule must not fire there.
        let mut resumed = preserved_record();
        let successor = AttemptIdentity {
            execution_id: execution_id('1'),
            attempt_id: attempt_id('7'),
            ordinal: 1,
            parent_attempt_id: Some(attempt_id('2')),
        };
        resumed.current_attempt = successor.clone();
        {
            let claim = resumed.claim.as_mut().unwrap();
            claim.current_attempt = successor;
            claim.origin_attempt_id = attempt_id('2');
        }
        assert!(
            WorktreeCustodyRecordV1::decode_canonical(&serde_json::to_vec(&resumed).unwrap())
                .is_ok()
        );
    }

    /// P4(b). Discriminates: the duplicated preservation reason being allowed
    /// to disagree between the state and the claim. Both copies are load
    /// bearing -- the state's drives sweep classification and the identity
    /// rule above, the claim's is what R2f2 reads from the standalone artifact
    /// -- so they are cross-checked rather than de-duplicated.
    #[test]
    fn custody_record_decode_rejects_disagreeing_preservation_reason() {
        let state = WorktreeCustodyStateV1::PreservationUnknown {
            reason: PreservationReasonV1::Cancellation,
        };
        let mut claim = claim_for(&state, false);
        claim.reason = PreservationReasonV1::NodeFailure;
        let record = record_for(&state, Some(claim), false);
        assert_eq!(
            WorktreeCustodyRecordV1::decode_canonical(&serde_json::to_vec(&record).unwrap()),
            Err(CustodyRecordDecodeErrorV1::ClaimReasonMismatch)
        );

        let agreeing = record_for(&state, Some(claim_for(&state, false)), false);
        assert!(
            WorktreeCustodyRecordV1::decode_canonical(&serde_json::to_vec(&agreeing).unwrap())
                .is_ok()
        );
    }

    /// P5. Discriminates: the read layer's byte bound. The `metadata().len()`
    /// pre-check and the `take(MAX + 1)` backstop are separate guards -- the
    /// first refuses a large file without reading it, the second catches a file
    /// that grew between the stat and the read.
    #[test]
    #[cfg(unix)]
    fn read_custody_record_refuses_an_over_bound_file() {
        let dir = std::env::temp_dir().join(format!(
            "a2a-bridge-custody-overbound-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let name = std::ffi::OsString::from("big.custody.v1.json");
        std::fs::write(
            dir.join(&name),
            vec![b'x'; MAX_CUSTODY_RECORD_JSON_BYTES + 1],
        )
        .unwrap();

        let pinned = PinnedDirectoryV1::open(&dir, "test").unwrap();
        assert_eq!(
            read_custody_record_in(&pinned, &name),
            Err(CustodyReadRefusalV1::OverBound)
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Discriminates: the envelope/claim identity cross-check being dropped.
    /// The claim is self-contained by design, so a record could otherwise
    /// name one checkout in its envelope and a different one in its claim.
    #[test]
    fn custody_record_decode_rejects_claim_identity_mismatch() {
        let mut mismatched_id = preserved_record();
        mismatched_id.claim.as_mut().unwrap().custody_id = cid('9');
        assert_eq!(
            WorktreeCustodyRecordV1::decode_canonical(&serde_json::to_vec(&mismatched_id).unwrap()),
            Err(CustodyRecordDecodeErrorV1::ClaimIdentityMismatch)
        );

        let mut mismatched_fingerprint = preserved_record();
        mismatched_fingerprint
            .claim
            .as_mut()
            .unwrap()
            .checkout_fingerprint = sha('9');
        assert_eq!(
            WorktreeCustodyRecordV1::decode_canonical(
                &serde_json::to_vec(&mismatched_fingerprint).unwrap()
            ),
            Err(CustodyRecordDecodeErrorV1::ClaimIdentityMismatch)
        );

        let mut mismatched_worktree = preserved_record();
        mismatched_worktree.claim.as_mut().unwrap().worktree = object("/root/other", 1, 12);
        assert_eq!(
            WorktreeCustodyRecordV1::decode_canonical(
                &serde_json::to_vec(&mismatched_worktree).unwrap()
            ),
            Err(CustodyRecordDecodeErrorV1::ClaimIdentityMismatch)
        );

        let mut mismatched_attempt = preserved_record();
        mismatched_attempt.claim.as_mut().unwrap().current_attempt = AttemptIdentity {
            ordinal: 1,
            ..attempt_identity()
        };
        assert_eq!(
            WorktreeCustodyRecordV1::decode_canonical(
                &serde_json::to_vec(&mismatched_attempt).unwrap()
            ),
            Err(CustodyRecordDecodeErrorV1::ClaimIdentityMismatch)
        );
    }

    /// Discriminates: the decoder accepting a negative wall clock, which
    /// would let a forged claim sort before every genuine one.
    #[test]
    fn custody_record_decode_rejects_negative_wall_clock() {
        let mut record = preserved_record();
        record.claim.as_mut().unwrap().created_wall_ms = -1;
        assert_eq!(
            WorktreeCustodyRecordV1::decode_canonical(&serde_json::to_vec(&record).unwrap()),
            Err(CustodyRecordDecodeErrorV1::NegativeWallClock)
        );
    }

    /// Discriminates: drift in the legal-transition table against focused
    /// boundary §2.2's state machine. The table is data precisely so 2b2/2c/2d
    /// cannot each invent their own edge set.
    #[test]
    fn legal_transition_table_pins_the_state_machine() {
        use WorktreeCustodyStateKindV1 as K;
        let expected: &[(K, K)] = &[
            (K::ProtectionPrepared, K::UnusedSettled),
            (K::ProtectionPrepared, K::Materializing),
            (K::Materializing, K::LiveProtected),
            (K::Materializing, K::PreservationUnknown),
            (K::LiveProtected, K::PreservationPrepared),
            (K::LiveProtected, K::DeleteAuthorized),
            (K::LiveProtected, K::RecoveredLive),
            (K::PreservationPrepared, K::Preserved),
            (K::PreservationPrepared, K::PreservationUnknown),
            (K::DeleteAuthorized, K::Removed),
        ];
        assert_eq!(LEGAL_CUSTODY_TRANSITIONS_V1, expected);
        for (from, to) in expected {
            assert!(transition_is_legal(*from, *to), "{from:?} -> {to:?}");
        }
        // Every pair not in the table is illegal -- including self-loops and
        // the reverse of every legal edge.
        for from in K::ALL {
            for to in K::ALL {
                assert_eq!(
                    transition_is_legal(from, to),
                    expected.contains(&(from, to)),
                    "unexpected legality for {from:?} -> {to:?}"
                );
            }
        }
    }

    /// Discriminates: a terminal state gaining an outgoing edge. §2.2:
    /// `Preserved` and `PreservationUnknown` are terminal for R2f1b -- only
    /// R2f2 disposition releases them. Mutation-checked (reverted before
    /// commit): adding `(Preserved, RecoveredLive)` -- the edge §2.2's ASCII
    /// diagram draws and its prose forbids -- turned this test and
    /// `legal_transition_table_pins_the_state_machine` red together, so
    /// terminality is genuinely derived from the table rather than asserted
    /// independently of it.
    #[test]
    fn terminal_states_have_no_outgoing_transitions() {
        use WorktreeCustodyStateKindV1 as K;
        for terminal in [
            K::UnusedSettled,
            K::Preserved,
            K::PreservationUnknown,
            K::Removed,
            K::RecoveredLive,
        ] {
            assert!(terminal.is_terminal(), "{terminal:?} must be terminal");
            for to in K::ALL {
                assert!(
                    !transition_is_legal(terminal, to),
                    "terminal {terminal:?} must not reach {to:?}"
                );
            }
        }
        for live in [
            K::ProtectionPrepared,
            K::Materializing,
            K::LiveProtected,
            K::PreservationPrepared,
            K::DeleteAuthorized,
        ] {
            assert!(!live.is_terminal(), "{live:?} must not be terminal");
        }
    }

    /// Discriminates: any state kind mapping to a sweep disposition that
    /// authorizes provider/git removal. 2a is recovery-only: the whole
    /// disposition vocabulary is non-destructive, and this test fails if a
    /// destructive arm is ever added without revisiting it.
    #[test]
    fn every_state_maps_to_a_non_destructive_sweep_disposition() {
        use WorktreeCustodyStateKindV1 as K;
        let expected = [
            (K::ProtectionPrepared, CustodySweepDispositionV1::Recover),
            (
                K::UnusedSettled,
                CustodySweepDispositionV1::StrandedUnusedSettled,
            ),
            (K::Materializing, CustodySweepDispositionV1::Recover),
            (K::LiveProtected, CustodySweepDispositionV1::Recover),
            (
                K::PreservationPrepared,
                CustodySweepDispositionV1::Preserved,
            ),
            (K::Preserved, CustodySweepDispositionV1::Preserved),
            (K::DeleteAuthorized, CustodySweepDispositionV1::Recover),
            (K::Removed, CustodySweepDispositionV1::MarkerOnly),
            (K::RecoveredLive, CustodySweepDispositionV1::Recover),
            (K::PreservationUnknown, CustodySweepDispositionV1::Unknown),
        ];
        assert_eq!(expected.len(), K::ALL.len());
        for (kind, disposition) in expected {
            assert_eq!(kind.sweep_disposition(), disposition, "for {kind:?}");
            assert!(
                !disposition.authorizes_checkout_removal(),
                "{disposition:?} must not authorize checkout removal in 2a"
            );
        }
        assert!(!CustodySweepDispositionV1::Refused.authorizes_checkout_removal());
    }

    /// Discriminates: `kind()` collapsing two variants onto one kind, or
    /// `ALL` drifting out of sync with the enum.
    #[test]
    fn state_kind_covers_every_variant_exactly_once() {
        use WorktreeCustodyStateKindV1 as K;
        let states = [
            (
                WorktreeCustodyStateV1::ProtectionPrepared {},
                K::ProtectionPrepared,
            ),
            (WorktreeCustodyStateV1::UnusedSettled {}, K::UnusedSettled),
            (WorktreeCustodyStateV1::Materializing {}, K::Materializing),
            (WorktreeCustodyStateV1::LiveProtected {}, K::LiveProtected),
            (
                WorktreeCustodyStateV1::PreservationPrepared {},
                K::PreservationPrepared,
            ),
            (WorktreeCustodyStateV1::Preserved {}, K::Preserved),
            (
                WorktreeCustodyStateV1::DeleteAuthorized {},
                K::DeleteAuthorized,
            ),
            (WorktreeCustodyStateV1::Removed {}, K::Removed),
            (
                WorktreeCustodyStateV1::RecoveredLive {
                    predecessor_claim_digest: sha('a'),
                },
                K::RecoveredLive,
            ),
            (
                WorktreeCustodyStateV1::PreservationUnknown {
                    reason: PreservationReasonV1::NodeFailure,
                },
                K::PreservationUnknown,
            ),
        ];
        assert_eq!(states.len(), K::ALL.len());
        let mut seen = std::collections::BTreeSet::new();
        for (state, kind) in states {
            assert_eq!(state.kind(), kind);
            assert_eq!(kind.wire_tag(), {
                let encoded = serde_json::to_value(&state).unwrap();
                encoded["state"].as_str().unwrap().to_string()
            });
            assert!(seen.insert(kind), "{kind:?} appeared twice");
        }
    }

    /// Discriminates: the V3 record suffix colliding with -- or being
    /// selectable by -- the legacy `*.meta.json` scanner. An older binary
    /// enumerates only `.meta.json`, so it must be unable to name a V3
    /// record (focused boundary §2.2 "Record naming"; brief §4 Rollback).
    #[test]
    fn custody_record_path_is_invisible_to_the_legacy_sidecar_scanner() {
        let path = custody_record_path("/root/ownr-run7-abc");
        assert_eq!(path, "/root/ownr-run7-abc.custody.v1.json");
        assert_eq!(CUSTODY_RECORD_SUFFIX, ".custody.v1.json");
        assert!(!path.ends_with(".meta.json"));
        assert!(is_custody_record_name(&path));
        let target = ChildNameV2::from_bytes(b"ownr-run7-abc.custody.v1.json").unwrap();
        let residue =
            ChildNameV2::reserved(ReservedNameNamespaceV2::RetirementCapture, &target).unwrap();
        assert!(
            !is_custody_record_name(&format!("/root/{}", residue.as_os_str().to_string_lossy())),
            "a retirement residue must never be read as a custody record"
        );
        let staging = ChildNameV2::reserved(ReservedNameNamespaceV2::Staging, &target).unwrap();
        assert!(
            is_custody_record_name(&format!("/root/{}", staging.as_os_str().to_string_lossy())),
            "only retirement residue is excluded from custody record classification"
        );
        assert!(
            !is_custody_record_name("/root/.custody.v1.json"),
            "a single-dot basename has an empty final segment, rejected by the trailing-slash stem guard"
        );
        assert!(
            is_custody_record_name("/root/..custody.v1.json"),
            "a dot-dot stem is a custody record name, not path traversal"
        );
        assert!(!is_custody_record_name("/root/ownr-run7-abc.meta.json"));
        assert!(!is_custody_record_name("/root/custody.v1.json"));
    }

    // -------------------------------------------------------------------------------------
    // probe_custody_record_presence — the deletion gate's read of durable truth (2b1).
    // -------------------------------------------------------------------------------------

    fn probe_root(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "a2a-bridge-custody-probe-{name}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    /// The gate's ALLOW arm. Discriminates a probe that cannot tell a free name from a taken one
    /// — which, in whichever direction it errs, either deletes protected work or permanently
    /// wedges legacy cleanup.
    #[test]
    fn a_checkout_with_no_record_beside_it_probes_provably_absent() {
        let root = probe_root("absent");
        let checkout = root.join("ownr-run7-abc");
        std::fs::create_dir(&checkout).unwrap();

        let presence = probe_custody_record_presence(&checkout.to_string_lossy());

        assert_eq!(presence, CustodyRecordPresenceV1::ProvablyAbsent);
        assert!(presence.authorizes_checkout_removal());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Presence must not depend on the record being decodable. Discriminates a probe implemented
    /// as "read and decode, treat failure as absent" — the exact inversion that would let an
    /// actor unprotect a checkout by truncating its record.
    #[test]
    fn an_undecodable_record_still_probes_present() {
        let root = probe_root("corrupt");
        let checkout = root.join("ownr-run7-abc");
        std::fs::create_dir(&checkout).unwrap();
        std::fs::write(
            custody_record_path(&checkout.to_string_lossy()),
            b"{ not json",
        )
        .unwrap();

        let presence = probe_custody_record_presence(&checkout.to_string_lossy());

        assert_eq!(presence, CustodyRecordPresenceV1::Present);
        assert!(!presence.authorizes_checkout_removal());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A record placed as a symlink (pointing anywhere, including nowhere) still takes the name.
    /// Discriminates a probe that follows symlinks and then reports a dangling one as absent.
    #[test]
    fn a_symlinked_or_dangling_record_name_probes_present() {
        let root = probe_root("symlink");
        let checkout = root.join("ownr-run7-abc");
        std::fs::create_dir(&checkout).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            root.join("does-not-exist"),
            custody_record_path(&checkout.to_string_lossy()),
        )
        .unwrap();

        #[cfg(unix)]
        assert_eq!(
            probe_custody_record_presence(&checkout.to_string_lossy()),
            CustodyRecordPresenceV1::Present
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// `try_exists` treats a dangling final parent symlink as absent only after the pinned open
    /// has refused to follow it. The missing checkout therefore authorizes no record protection.
    #[cfg(unix)]
    #[test]
    fn a_dangling_final_parent_symlink_probes_provably_absent() {
        let root = probe_root("dangling-parent");
        let dangling_parent = root.join("dangling-parent");
        std::os::unix::fs::symlink(root.join("missing-parent"), &dangling_parent).unwrap();

        let checkout = dangling_parent.join("ownr-run7-abc");
        let presence = probe_custody_record_presence(&checkout.to_string_lossy());

        assert_eq!(
            presence,
            CustodyRecordPresenceV1::ProvablyAbsent,
            "a dangling final parent cannot contain a custody record"
        );
        assert!(presence.authorizes_checkout_removal());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A vanished enclosing directory answers ABSENT, not inconclusive. Discriminates a
    /// blanket "any probe failure is inconclusive" rule, which would make the gate refuse the
    /// cleanup of an already-deleted worktree forever — protecting nothing and leaking a map
    /// entry on every such session.
    #[test]
    fn a_vanished_enclosing_directory_probes_provably_absent() {
        let root = probe_root("vanished");
        let checkout = root.join("gone").join("ownr-run7-abc");

        let presence = probe_custody_record_presence(&checkout.to_string_lossy());

        assert_eq!(presence, CustodyRecordPresenceV1::ProvablyAbsent);
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// An enclosing "directory" that exists but cannot be pinned is INCONCLUSIVE, and
    /// inconclusive refuses. Discriminates collapsing every pin failure into absent: here the
    /// parent is a regular file, so it exists (nothing is provably gone) yet no descriptor-bound
    /// answer is available.
    #[test]
    fn an_unpinnable_but_existing_enclosing_path_probes_inconclusive_and_refuses() {
        let root = probe_root("unpinnable");
        let parent = root.join("not-a-directory");
        std::fs::write(&parent, b"file").unwrap();
        let checkout = parent.join("ownr-run7-abc");

        let presence = probe_custody_record_presence(&checkout.to_string_lossy());

        assert!(
            matches!(presence, CustodyRecordPresenceV1::Inconclusive(_)),
            "unexpected presence: {presence:?}"
        );
        assert!(!presence.authorizes_checkout_removal());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A well-formed record probes present, and — the point of the whole gate — 2a's disposition
    /// still refuses removal for it. Pins the two layers agreeing rather than each assuming the
    /// other checks.
    #[test]
    fn a_decodable_record_probes_present_and_its_disposition_still_refuses_removal() {
        let root = probe_root("decodable");
        let checkout = root.join("ownr-run7-abc");
        std::fs::create_dir(&checkout).unwrap();
        let record = live_record();
        std::fs::write(
            custody_record_path(&checkout.to_string_lossy()),
            record.encode_canonical().unwrap(),
        )
        .unwrap();

        assert_eq!(
            probe_custody_record_presence(&checkout.to_string_lossy()),
            CustodyRecordPresenceV1::Present
        );
        assert!(!record.sweep_disposition().authorizes_checkout_removal());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
