//! The R2f1b V3 custody writer (slice 2b2) — the first writer of [`WorktreeCustodyRecordV1`].
//!
//! # Creation ordering, inverted
//!
//! Today's V2 bound path is `git worktree add` → `write_sidecar`: the checkout exists for a
//! window in which nothing on disk says it is owned, and `write_sidecar` itself does no
//! `sync_all` and no parent sync, so a crash can lose the record entirely. §2.5 inverts that:
//!
//! ```text
//! acquire publication cell → acquire custody cell → pin the worktree root by descriptor
//!   → stage + fsync the record → NO-REPLACE publish `ProtectionPrepared` → parent sync
//!   → replace `Materializing` → parent sync
//!     → git worktree add (custody-aware; never `cleanup_failed_add`)
//!   → capture source/root/worktree/common-dir identities BY DESCRIPTOR
//!   → replace `LiveProtected` → parent sync
//! ```
//!
//! Every state after the first is a REPLACE, which is why 2b1's replace primitive had to precede
//! this module.
//!
//! # Production reachability
//!
//! This writer is **production-unreachable** by construction (§5.2, and the slice's §2c
//! self-pass): it runs only when a [`BoundWorktreeCustodyV1`] reaches the backend, which requires
//! a `FrozenR2f1bContractV1` to have been admitted, and no production path constructs one. It
//! lands now so that the ordering, the crash matrix, and the add prohibition are settled before
//! the slice that makes V3 reachable.
//!
//! # Staged-source residue policy (2b2 obligation from the PARKED-1 review, opus S-9)
//!
//! This module is the first production-shaped caller of the `fs_custody` publication primitives,
//! so it owns the answer to "what happens to the staged temp when the outcome is ambiguous".
//!
//! * **Naming.** Every publication stages under `<target>.custody.v1.json.staging-<32 hex>` with
//!   a freshly minted nonce. Two consequences, both load-bearing: a later attempt (retry, next
//!   transition, another process) can never collide with its own or anyone's residue, so the
//!   sequence CONVERGES rather than wedging on `EEXIST`; and the name matches NEITHER sweep
//!   pattern — `is_custody_record_name` requires the exact `.custody.v1.json` ending, and the
//!   legacy scanner requires `.meta.json` — so residue is inert to both arms and can never be
//!   read as a record or selected as a checkout.
//! * **The writer unlinks the staging name in NO arm.** Not on success, not on failure, not on
//!   the staging error path. `std::fs::remove_file` addresses a NAME; what this code holds is a
//!   descriptor. Between `create_new` returning and any later unlink, the name can have been
//!   exchanged (recreated after a rename consumed it, or swapped while we hold the fd), so an
//!   unlink is a deletion of an object whose identity was never proved — the one act this whole
//!   design refuses. The durable arm's unlink was the sharpest case: a committed rename frees the
//!   source name, so the call is a no-op EXCEPT when another actor has since created a file
//!   there, which is precisely when it destroys a foreign object.
//! * **What actually happens to the staged file, arm by arm.** After
//!   [`CustodyPublicationV1::Durable`], `ParentSyncAmbiguous`, and `TargetIdentityUnverified` the
//!   rename committed, so the source NAME is already free and there is no residue — the "leave it
//!   in place" rule is vacuously satisfied for those three. Residue genuinely survives in exactly
//!   two situations: a true `Err` refusal (the ordinary no-replace `EEXIST`, where another owner
//!   published first — §5.7 row 2's "quarantine temp"), and `RenameOutcomeUnverified` where the
//!   source name is occupied by something we cannot prove is ours. In the second, leaving it is
//!   not merely conservative but mandatory: unlinking would destroy a foreign file.
//! * **Reclamation owner — stated truthfully.** It is NOT the boot sweep: `scan_worktree_records`
//!   matches `*.meta.json` and `is_custody_record_name` only, and a staging name satisfies
//!   neither *by design*, so no sweep will ever see one. The owner is the operator, via the
//!   storage report, which classifies residue as `Evidence` with a note naming it (see
//!   `storage_report::WorktreeRecordKindV1::CustodyStagingResidue`) and leaves disposition to the
//!   owner. Residue is bounded (one small file per failed publication), inert, and
//!   self-describing: its name records the exact target it was staged for.
//!
//! # Fault-seam counting (PARKED-1 review, opus S-6)
//!
//! `PinnedDirectoryV1`'s publication-rename countdown is ONE counter shared by publish AND
//! replace on a directory. A crash-matrix test arming "call N" over a
//! publish-then-replace-then-replace sequence must count every rename. The sequence this module
//! performs is exactly: call 1 = `ProtectionPrepared` publish, call 2 = `Materializing` replace,
//! call 3 = the terminal replace (`LiveProtected` / `PreservationUnknown`).

use crate::custody::{
    read_custody_record_in, transition_is_legal, ClaimPresenceV1, CustodyReadRefusalV1,
    IdentityCompletenessV1, PreservationReasonV1, PreservedWorktreeClaimV1, RecoveryLocatorV1,
    WorktreeCustodyRecordV1, WorktreeCustodyStateKindV1, WorktreeCustodyStateV1,
    CUSTODY_RECORD_SUFFIX, WORKTREE_CUSTODY_RECORD_SCHEMA_V1,
};
use crate::custody_lock::{
    acquire_custody_lock_blocking_in, acquire_publication_lock_blocking_in, CustodyLockGuardV1,
    CustodyLockRefusalV1, PublicationLockGuardV1,
};
use crate::host_git::HostGitWorktree;
use crate::settle::{reprove_under_window, ProvenSettlementV1, SettlementWindowV1};
use crate::sweep::{ExactAbsenceProbeV1, ExactAbsenceSweepEntryV1, ExactAbsenceSweepReportV1};
use bridge_core::execution_policy::{
    select_custody_plan_v1, BoundWorktreeCustodyV1, FrozenCheckoutEffectV1, WorktreeCustodyIdV1,
    WorktreeObjectIdentityV1,
};
#[cfg(unix)]
use bridge_core::fs_custody::BirthTimeV1;
use bridge_core::fs_custody::{
    open_options_create_new_owner_private, required_file_content_snapshot_v2,
    retire_captured_regular_child_v2, ChildNameV2, CustodyPublicationV1, DirectoryIdentityV1,
    FsCustodyError, MarkerRetirementOutcomeV1, PinnedDirectoryV1, RegularChildRefV1,
    ReservedNameNamespaceV2,
};
use bridge_core::liveness::{acquire_lease_in, LeaseGuard};
use bridge_workflow::run_spec::WorkflowSnapshotV3;
use std::ffi::{OsStr, OsString};
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Why a custody transition could not be completed.
///
/// [`Self::Ambiguous`] is separate from [`Self::Failed`] on purpose: an ambiguous publication may
/// have taken effect, so the record on disk may already be the new state. A caller must not treat
/// it as "nothing happened" — in particular it must not conclude the checkout is unprotected.
#[derive(Debug, thiserror::Error)]
pub enum CustodyWriteRefusalV1 {
    #[error("custody cell could not be entered: {0}")]
    Lock(#[from] CustodyLockRefusalV1),
    #[error("custody record could not be built: {0}")]
    Record(#[from] crate::custody::CustodyRecordDecodeErrorV1),
    #[error("custody publication failed before any effect: {0}")]
    Failed(String),
    /// The publication may or may not have taken effect. Protective: unknown never licenses
    /// deletion, and never licenses believing the checkout is unprotected either.
    #[error("custody publication outcome is ambiguous: {0}")]
    Ambiguous(String),
}

impl From<FsCustodyError> for CustodyWriteRefusalV1 {
    fn from(error: FsCustodyError) -> Self {
        Self::Failed(error.to_string())
    }
}

/// The outcome of attempting the one frozen `ProtectionPrepared -> UnusedSettled`
/// transition and then retiring only its custody marker.
///
/// `StrandedUnusedSettled` is deliberately distinct from a generic refusal: the transition is
/// known durable and the marker remains. It is an operator-visible, fail-closed residual; no
/// later sweep can reconstruct the forbidden claim needed to authorize a second retirement.
#[derive(Debug)]
#[must_use = "a settlement result records whether marker retirement completed or left durable residue"]
pub enum UnusedSettlementOutcomeV1 {
    /// The marker was retired after the durable `UnusedSettled` transition.
    Settled,
    /// The durable `UnusedSettled` marker remains and requires operator visibility.
    StrandedUnusedSettled(String),
    /// The replace may have published `UnusedSettled`; the marker was not retired and no retry
    /// is authorized because the durable transition state cannot be established.
    TransitionUncertain(String),
    /// Retirement may have moved or unlinked the marker but could not establish its terminal
    /// durability. No retry is authorized from this result.
    RetirementUncertain(String),
    /// No transition was proven durable.
    Refused(String),
}

impl UnusedSettlementOutcomeV1 {
    #[must_use]
    pub fn report_category(&self) -> &'static str {
        match self {
            Self::Settled => "unused-settled-retired",
            Self::StrandedUnusedSettled(_) => "stranded-unused-settled",
            Self::TransitionUncertain(_) => "unused-settlement-transition-uncertain",
            Self::RetirementUncertain(_) => "unused-settlement-retirement-uncertain",
            Self::Refused(_) => "unused-settlement-refused",
        }
    }
}

fn stranded_unused_settled(proof: &ProvenSettlementV1, detail: &str) -> UnusedSettlementOutcomeV1 {
    tracing::warn!(
        category = "stranded-unused-settled",
        custody_id = proof.custody_id().as_str(),
        worktree_path = proof.worktree_path(),
        detail,
        "unused settlement left a durable marker for operator disposition"
    );
    UnusedSettlementOutcomeV1::StrandedUnusedSettled(detail.to_string())
}

// Test-only crash injection. Thread-local because the test's arm and settlement run on the
// arming test's own thread, so parallel tests cannot consume each other's interruption.
#[cfg(test)]
thread_local! {
    static INTERRUPT_UNUSED_SETTLEMENT_AFTER_TRANSITION: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(crate) fn arm_unused_settlement_interruption_for_test() {
    INTERRUPT_UNUSED_SETTLEMENT_AFTER_TRANSITION.with(|cell| {
        assert!(
            !cell.replace(true),
            "unused-settlement interruption is already armed"
        );
    });
}

#[cfg(test)]
fn interrupt_unused_settlement_after_transition_for_test() -> bool {
    INTERRUPT_UNUSED_SETTLEMENT_AFTER_TRANSITION.with(|cell| cell.replace(false))
}

#[cfg(not(test))]
fn interrupt_unused_settlement_after_transition_for_test() -> bool {
    false
}

/// What one `preserve_after_cancel` sequence settled on.
///
/// **No arm authorizes deletion, and the exhaustive [`Self::is_protective`] match is how a later
/// arm is forced to be classified rather than defaulting into permissiveness** — the same
/// discipline as `CustodySweepDispositionV1::authorizes_checkout_removal`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "an unhandled preservation outcome hides an ambiguous or refused transition, and an \
              ambiguous transition must never be read as a completed one"]
pub enum PreservationOutcomeV1 {
    /// A durable `Preserved` claim now exists. R2f1b-terminal.
    Preserved,
    /// A durable `PreservationUnknown { reason }` claim now exists. R2f1b-terminal.
    PreservationUnknown(PreservationReasonV1),
    /// The record was already `Preserved` when the barrier ran (§5.7's last row).
    AlreadyPreserved,
    /// The record was already `PreservationUnknown` when the barrier ran.
    AlreadyUnknown,
    /// A publication may or may not have taken effect. Both candidate states are protective;
    /// nothing further was written (§5.7 "claim renamed, parent sync ambiguous").
    Ambiguous(String),
    /// Nothing was published; the prior state stands.
    Refused(String),
}

impl PreservationOutcomeV1 {
    /// Every arm is protective. There is no arm that permits provider removal, reset, clean,
    /// checkout, or prune, and adding one must be a decision.
    #[must_use]
    pub fn is_protective(&self) -> bool {
        match self {
            Self::Preserved
            | Self::PreservationUnknown(_)
            | Self::AlreadyPreserved
            | Self::AlreadyUnknown
            | Self::Ambiguous(_)
            | Self::Refused(_) => true,
        }
    }

    /// Did this sequence leave an R2f1b-terminal preservation on disk?
    #[must_use]
    pub fn is_terminal_preservation(&self) -> bool {
        matches!(
            self,
            Self::Preserved
                | Self::PreservationUnknown(_)
                | Self::AlreadyPreserved
                | Self::AlreadyUnknown
        )
    }
}

impl From<CustodyWriteRefusalV1> for PreservationOutcomeV1 {
    /// The ambiguity-discard hardening (2b1 opus S-7) at this slice's own call sites: the ONE
    /// conversion from a write refusal to an outcome, so an `Ambiguous` write can never be folded
    /// into a `Refused` ("nothing happened") answer by a caller writing its own match.
    fn from(refusal: CustodyWriteRefusalV1) -> Self {
        match refusal {
            CustodyWriteRefusalV1::Ambiguous(detail) => Self::Ambiguous(detail),
            other => Self::Refused(other.to_string()),
        }
    }
}

/// Focused boundary §5.1's deletion authority, as a value that cannot be forged or replayed.
///
/// > "Globally healthy workflow success is the only automatic deletion path. It CASes to
/// > `DeleteAuthorized` and mints an unforgeable `DeletionCapabilityV1`.
/// > `HostGitWorktree::remove_v2` takes that capability — **not a raw path**."
///
/// **Unforgeable** means two separate things, and both are structural rather than conventional:
///
/// * **Not constructible outside this module.** Every field is private and there is no public
///   constructor, no `Default`, and no `From`. The ONLY expression anywhere that builds one is in
///   [`WorktreeCustodianV1::authorize_deletion`], which runs under both custody cells and refuses
///   unless the record on disk transitions `LiveProtected -> DeleteAuthorized`. A caller in
///   `backend.rs` — or in any other crate — cannot write one down.
/// * **Not usable twice.** It is neither `Clone` nor `Copy`, and the only way to reach a provider
///   removal is [`Self::revalidate_for_removal`], which takes `self` BY VALUE. A capability is
///   therefore consumed exactly once, and a failed revalidation consumes it too — retrying needs a
///   fresh mint, which needs the record to be `LiveProtected` again, which a `DeleteAuthorized`
///   record is not. That is what "no re-mint from the stale capability" means mechanically.
///
/// It carries the identity the removal is authorized FOR — the repo, the target, and the four
/// object identities observed at materialization — so `remove_v2` never has to be handed a path
/// by a caller, and a capability minted for one checkout can never name another.
pub struct DeletionCapabilityV1 {
    custody_id: WorktreeCustodyIdV1,
    canonical_source: String,
    worktree_path: String,
    identities: MaterializedIdentitiesV1,
}

impl std::fmt::Debug for DeletionCapabilityV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeletionCapabilityV1")
            .field("custody_id", &self.custody_id.as_str())
            .field("worktree_path", &self.worktree_path)
            .finish()
    }
}

impl DeletionCapabilityV1 {
    #[must_use]
    pub fn custody_id(&self) -> &WorktreeCustodyIdV1 {
        &self.custody_id
    }

    #[must_use]
    pub fn canonical_source(&self) -> &str {
        &self.canonical_source
    }

    #[must_use]
    pub fn worktree_path(&self) -> &str {
        &self.worktree_path
    }

    /// §5.1's "revalidates source/root/target/common-dir identities **immediately before** Git
    /// removal", made unskippable: this is the only way to obtain the [`AuthorizedRemovalV1`] that
    /// [`crate::provider::WorktreeProvider::remove_v2`] requires, so no provider — production or
    /// double — can run a git removal without the revalidation having just happened.
    ///
    /// It is a SECOND check, not a duplicate of the mint's. The mint refuses to authorize a
    /// swapped object graph at CAS time; this refuses to act on one that was swapped in the window
    /// between the CAS and the removal. Both are required: the first keeps a bad graph out of the
    /// durable `DeleteAuthorized` state, the second keeps `git worktree remove` off it.
    ///
    /// The capability is consumed either way (`self` by value), so a refusal cannot be retried
    /// into a success.
    pub fn revalidate_for_removal(self) -> Result<AuthorizedRemovalV1, String> {
        if !WorktreeCustodianV1::identities_reverify(&self.identities) {
            return Err(format!(
                "object identity changed since deletion was authorized for {}",
                self.worktree_path
            ));
        }
        Ok(AuthorizedRemovalV1 { capability: self })
    }
}

/// A [`DeletionCapabilityV1`] whose object identities were revalidated by descriptor with no
/// intervening await. Private field: the only way to build one is
/// [`DeletionCapabilityV1::revalidate_for_removal`].
#[derive(Debug)]
pub struct AuthorizedRemovalV1 {
    capability: DeletionCapabilityV1,
}

impl AuthorizedRemovalV1 {
    #[must_use]
    pub fn canonical_source(&self) -> &str {
        self.capability.canonical_source()
    }

    #[must_use]
    pub fn worktree_path(&self) -> &str {
        self.capability.worktree_path()
    }

    #[must_use]
    pub fn custody_id(&self) -> &WorktreeCustodyIdV1 {
        self.capability.custody_id()
    }
}

/// What one `LiveProtected -> DeleteAuthorized` CAS settled on.
///
/// **Exactly one arm carries deletion authority**, and the exhaustive [`Self::is_authorized`]
/// match is how a later arm is forced to be classified rather than defaulting into permissiveness
/// — the same discipline as `CustodySweepDispositionV1::authorizes_checkout_removal` and
/// `PreservationOutcomeV1::is_protective`.
#[derive(Debug)]
#[must_use = "an unhandled authorization outcome hides a refusal or an ambiguous CAS, and an \
              ambiguous CAS must never be read as a completed one"]
pub enum DeletionAuthorizationV1 {
    /// The record now says `DeleteAuthorized` and this is its single-use capability.
    ///
    /// Boxed for the size difference against the two `String` arms. Boxing does NOT weaken the
    /// single-use property: `Box<DeletionCapabilityV1>` is still neither `Clone` nor `Copy`, and
    /// the capability is still consumed by value out of the box.
    Authorized(Box<DeletionCapabilityV1>),
    /// Nothing was published; the prior state stands and the checkout keeps its protection.
    Refused(String),
    /// The CAS may or may not have taken effect. Both candidate states are non-preserving and
    /// non-removing, so nothing further is written and NO capability is minted — an ambiguous
    /// authorization is exactly the state that must not license a git removal.
    Ambiguous(String),
}

impl DeletionAuthorizationV1 {
    #[must_use]
    pub fn is_authorized(&self) -> bool {
        match self {
            Self::Authorized(_) => true,
            Self::Refused(_) | Self::Ambiguous(_) => false,
        }
    }
}

/// What recording the `DeleteAuthorized -> Removed` tombstone settled on.
#[derive(Debug, PartialEq, Eq)]
#[must_use = "an unrecorded tombstone leaves the checkout recovery-owned and must not be ignored"]
pub enum RemovalRecordV1 {
    /// The tombstone is durable.
    Recorded,
    /// The tombstone's publication outcome is unknown. Protective: the checkout is already gone,
    /// and both candidate record states (`DeleteAuthorized`, `Removed`) are non-preserving, so no
    /// work can be lost — but the record must not be reported as settled.
    Ambiguous(String),
    /// Nothing was written; the record still says whatever it said.
    Refused(String),
}

/// The four object identities §2.2's claim records, captured by descriptor at materialization.
///
/// Risk R-8's accepted disposition: `FrozenWorktreeCustodyPlanV1` carries only
/// `{custody_id, checkout_fingerprint, target_cwd}`, so three of the four identities do not exist
/// in the contract at all and must be observed when the objects do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterializedIdentitiesV1 {
    pub source: WorktreeObjectIdentityV1,
    pub root: WorktreeObjectIdentityV1,
    pub worktree: WorktreeObjectIdentityV1,
    pub common_dir: WorktreeObjectIdentityV1,
}

/// Inputs for one production-inactive successor claim exchange.
///
/// The successor binding is owned because the custodian retains it while publishing the
/// `RecoveredLive` record. Every other input is borrowed from the resume coordinator that slice 5
/// will eventually own.
pub struct ClaimExchangeRequestV1<'a> {
    pub worktree_root: &'a Path,
    pub worktree_path: &'a str,
    pub predecessor_snapshot: &'a WorkflowSnapshotV3,
    pub successor_snapshot: &'a WorkflowSnapshotV3,
    pub predecessor_binding: &'a BoundWorktreeCustodyV1,
    pub successor_binding: BoundWorktreeCustodyV1,
    pub retained: &'a MaterializedIdentitiesV1,
    /// Shared predecessor-recovery and successor-live lease namespace. Slice 5 must bind this
    /// canonical namespace to configuration; this production-inactive API intentionally does not.
    ///
    /// The durable `RecoveredLive.predecessor_claim_digest` is the predecessor *snapshot* digest:
    /// the normative `LiveProtected -> RecoveredLive` edge has no preserved claim to digest.
    pub lease_namespace_dir: &'a Path,
}

/// The result of the production-inactive successor claim exchange.
///
/// An unsuccessful result never gives a caller permission to configure a provider. In particular,
/// `LeaseUnavailable` means the `RecoveredLive` replace is already durable and recovery-owned;
/// callers must leave it alone rather than attempting a second custody write.
#[must_use = "an unhandled exchange outcome can hide a durable protective record or live lease"]
pub enum ClaimExchangeOutcomeV1 {
    /// The successor record and its live lease are both owned by the returned value.
    Exchanged(ClaimExchangeReadyV1),
    /// The record was published, but the successor lease could not be acquired afterwards.
    LeaseUnavailable(String),
    /// The record replace may have taken effect; do not write anything further.
    Ambiguous(String),
    /// Validation or an unambiguous pre-publication check refused the exchange.
    Refused(String),
}

/// A completed exchange whose successor lease remains held for the caller's live attempt.
///
/// Slice 5 will retain this while it resolves/configures the provider. There is deliberately no
/// provider handle here: the mechanism proves the precondition but does not activate V3 serving.
#[must_use = "dropping the exchange result releases the successor live lease"]
pub struct ClaimExchangeReadyV1 {
    predecessor_claim_digest: bridge_core::execution_policy::Sha256HexV1,
    successor_attempt: bridge_core::ids::AttemptIdentity,
    _successor_live_lease: LeaseGuard,
}

impl ClaimExchangeReadyV1 {
    #[must_use]
    pub fn predecessor_claim_digest(&self) -> &bridge_core::execution_policy::Sha256HexV1 {
        &self.predecessor_claim_digest
    }

    #[must_use]
    pub fn successor_attempt(&self) -> &bridge_core::ids::AttemptIdentity {
        &self.successor_attempt
    }

    #[must_use]
    pub fn successor_live_lease_path(&self) -> &Path {
        self._successor_live_lease.path()
    }
}

/// A plan-derived identity: the canonical path with no observed `dev`/`ino`.
///
/// Legal only where §2.2 says no live identity exists yet — `ProtectionPrepared`,
/// `UnusedSettled`, `Materializing`, and `PreservationUnknown{materialization_inflight}`. The
/// decoder enforces that rule ([`IdentityCompletenessV1`]); this function just produces the shape.
#[must_use]
pub fn planned_identity(path: &str) -> WorktreeObjectIdentityV1 {
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

/// Observe a directory's identity BY DESCRIPTOR (`O_NOFOLLOW`), never by re-canonicalizing a
/// string (§2.2). Falls back to the degraded shape when the object cannot be opened, so a claim
/// can still be published for a checkout whose objects have already moved.
#[must_use]
pub fn observed_identity(path: &str) -> WorktreeObjectIdentityV1 {
    let Ok(file) = bridge_core::fs_custody::open_directory_no_follow_raw(Path::new(path)) else {
        return planned_identity(path);
    };
    let Ok(metadata) = file.metadata() else {
        return planned_identity(path);
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        WorktreeObjectIdentityV1 {
            canonical_path: path.to_string(),
            directory_identity: DirectoryIdentityV1 {
                canonical_path: path.to_string(),
                dev: Some(metadata.dev()),
                ino: Some(metadata.ino()),
                btime: BirthTimeV1::from_metadata(&metadata),
            },
        }
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        planned_identity(path)
    }
}

/// A held custody cell over one checkout, with its worktree root pinned by descriptor.
///
/// The guards are fields so their lifetime is exactly this value's: every transition published
/// through a `WorktreeCustodianV1` is serialized against every sweep, deletion, and other writer
/// for the same checkout, and the cells release when the custodian is dropped.
pub struct WorktreeCustodianV1 {
    custody: BoundWorktreeCustodyV1,
    worktree_path: String,
    record_name: OsString,
    root: PinnedDirectoryV1,
    root_path: PathBuf,
    // Declared in acquisition order; Rust drops fields in declaration order, so the publication
    // cell (outer) releases AFTER the custody cell (inner) — the reverse-of-acquisition order a
    // nested lock discipline requires.
    _custody_cell: CustodyLockGuardV1,
    _publication_cell: PublicationLockGuardV1,
}

impl std::fmt::Debug for WorktreeCustodianV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorktreeCustodianV1")
            .field("worktree_path", &self.worktree_path)
            .field("custody_id", &self.custody.plan.custody_id.as_str())
            .finish()
    }
}

impl WorktreeCustodianV1 {
    /// Enter both cells and pin the worktree root. **Blocking** — the writer is the one caller
    /// entitled to the blocking acquirers (`custody_lock.rs`, contention), and every call site
    /// must therefore be off the async executor.
    ///
    /// `worktree_root` is the canonical `[worktrees].root`; `worktree_path` is the frozen target,
    /// whose parent must be that root (already enforced by `validate_bound_worktree`).
    pub fn enter(
        worktree_root: &Path,
        worktree_path: &str,
        custody: BoundWorktreeCustodyV1,
    ) -> Result<Self, CustodyWriteRefusalV1> {
        let publication_cell =
            acquire_publication_lock_blocking_in(worktree_root, worktree_path, &|| {
                tracing::info!(
                    worktree_path,
                    "waiting for the checkout publication cell before a custody transition"
                );
            })?;
        let custody_cell =
            acquire_custody_lock_blocking_in(worktree_root, &custody.plan.custody_id, &|| {
                tracing::info!(
                    custody_id = custody.plan.custody_id.as_str(),
                    "waiting for the custody cell before a custody transition"
                );
            })?;
        let root = PinnedDirectoryV1::open(worktree_root, "worktree custody root")?;
        let record_name = record_file_name(worktree_path)?;
        Ok(Self {
            custody,
            worktree_path: worktree_path.to_string(),
            record_name,
            root,
            root_path: worktree_root.to_path_buf(),
            _custody_cell: custody_cell,
            _publication_cell: publication_cell,
        })
    }

    #[must_use]
    pub fn custody_id(&self) -> &WorktreeCustodyIdV1 {
        &self.custody.plan.custody_id
    }

    /// The binding this custodian was entered with, so a caller can retain exactly what a LATER
    /// custodian must be re-entered with to publish a preservation over the same record.
    #[must_use]
    pub fn binding(&self) -> &BoundWorktreeCustodyV1 {
        &self.custody
    }

    #[must_use]
    pub fn worktree_path(&self) -> &str {
        &self.worktree_path
    }

    #[must_use]
    pub fn worktree_root(&self) -> &Path {
        &self.root_path
    }

    /// The pinned root, exposed so a test can arm `fs_custody`'s fault seams against the very
    /// descriptor the writer publishes through. There is no other way to inject a rename or sync
    /// fault at the exact call the crash matrix names.
    #[must_use]
    pub fn pinned_root(&self) -> &PinnedDirectoryV1 {
        &self.root
    }

    /// Settle one report-selected, exactly absent candidate through the production read-only
    /// Git probe. The probe can spawn only the two query verbs pinned by the colocated test; this
    /// method itself never starts a process and never asks Git to mutate a worktree.
    ///
    /// This is an associated function rather than an instance method because `ProvenSettlementV1`
    /// already owns the held publication and custody cells. Entering another writer custodian
    /// here would block on those same cells and break the one-window proof-to-retirement rule.
    pub fn replace_unused_settled(
        worktree_root: &Path,
        canonical_worktree_path: &str,
        report: &ExactAbsenceSweepReportV1,
        entry: &ExactAbsenceSweepEntryV1,
    ) -> UnusedSettlementOutcomeV1 {
        let probe = HostGitWorktree::new();
        Self::replace_unused_settled_with_probe(
            worktree_root,
            canonical_worktree_path,
            report,
            entry,
            &probe,
        )
    }

    /// The same settlement route with an injected read-only probe for focused tests. Production
    /// uses [`Self::replace_unused_settled`], which supplies [`HostGitWorktree`].
    pub(crate) fn replace_unused_settled_with_probe(
        worktree_root: &Path,
        canonical_worktree_path: &str,
        report: &ExactAbsenceSweepReportV1,
        entry: &ExactAbsenceSweepEntryV1,
        probe: &dyn ExactAbsenceProbeV1,
    ) -> UnusedSettlementOutcomeV1 {
        let window = match SettlementWindowV1::open(worktree_root, canonical_worktree_path) {
            Ok(window) => window,
            Err(refusal) => return UnusedSettlementOutcomeV1::Refused(refusal.to_string()),
        };
        let proof = match reprove_under_window(window, report, entry, probe) {
            Ok(proof) => proof,
            Err(refusal) => return UnusedSettlementOutcomeV1::Refused(refusal.to_string()),
        };
        Self::replace_proven_unused_settled(proof)
    }

    fn replace_proven_unused_settled(proof: ProvenSettlementV1) -> UnusedSettlementOutcomeV1 {
        let mut settled = proof.record().clone();
        if settled.state.kind() != WorktreeCustodyStateKindV1::ProtectionPrepared
            || !transition_is_legal(
                WorktreeCustodyStateKindV1::ProtectionPrepared,
                WorktreeCustodyStateKindV1::UnusedSettled,
            )
        {
            return UnusedSettlementOutcomeV1::Refused(
                "the frozen custody table does not permit unused settlement".to_string(),
            );
        }
        settled.state = WorktreeCustodyStateV1::UnusedSettled {};
        settled.claim = None;
        if let Err(error) = settled.validate_for_publication() {
            return UnusedSettlementOutcomeV1::Refused(error.to_string());
        }
        match publish_custody_record_in(
            proof.pinned_root(),
            proof.record_name(),
            &settled,
            PublicationModeV1::Replace,
        ) {
            Ok(()) => {}
            Err(CustodyWriteRefusalV1::Ambiguous(detail)) => {
                return UnusedSettlementOutcomeV1::TransitionUncertain(detail)
            }
            Err(error) => return UnusedSettlementOutcomeV1::Refused(error.to_string()),
        }
        if interrupt_unused_settlement_after_transition_for_test() {
            return stranded_unused_settled(
                &proof,
                "test interruption after durable transition and before marker retirement",
            );
        }
        let marker = match proof
            .pinned_root()
            .open_regular_file(proof.record_name(), "settled custody marker")
        {
            Ok(marker) => marker,
            Err(error) => return stranded_unused_settled(&proof, &error.to_string()),
        };
        let expected = match required_file_content_snapshot_v2(&marker, "settled custody marker") {
            Ok(snapshot) => snapshot.object,
            Err(error) => return stranded_unused_settled(&proof, &error.to_string()),
        };
        match retire_captured_regular_child_v2(
            proof.pinned_root(),
            proof.record_name(),
            expected,
            "settled custody marker",
        ) {
            MarkerRetirementOutcomeV1::Retired => UnusedSettlementOutcomeV1::Settled,
            MarkerRetirementOutcomeV1::RefusedNoEffect(error)
            | MarkerRetirementOutcomeV1::CapturedRetained(error)
            | MarkerRetirementOutcomeV1::RuntimeUnsupported(error) => {
                stranded_unused_settled(&proof, &error)
            }
            MarkerRetirementOutcomeV1::CompileUnsupported => {
                stranded_unused_settled(&proof, "marker retirement is unsupported on this platform")
            }
            MarkerRetirementOutcomeV1::CaptureUncertain(error)
            | MarkerRetirementOutcomeV1::RetiredUnsynced(error) => {
                UnusedSettlementOutcomeV1::RetirementUncertain(error)
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn replace_proven_unused_settled_for_test(
        proof: ProvenSettlementV1,
    ) -> UnusedSettlementOutcomeV1 {
        Self::replace_proven_unused_settled(proof)
    }

    /// Publish `ProtectionPrepared` with the NO-REPLACE primitive, before any provider effect.
    ///
    /// No-replace is the right primitive exactly once, here: an existing record at this name is
    /// another owner's protection, and clobbering it would silently take custody of a checkout
    /// someone else is protecting. Every later transition replaces, because by then the record at
    /// that name is this custodian's own.
    pub fn publish_protection_prepared(&self) -> Result<(), CustodyWriteRefusalV1> {
        let record = self.record(WorktreeCustodyStateV1::ProtectionPrepared {}, None, None)?;
        self.stage_and_settle(&record, PublicationModeV1::NoReplace)
    }

    /// Replace with `Materializing` — the state `git worktree add` runs under (§2.5).
    pub fn replace_materializing(&self) -> Result<(), CustodyWriteRefusalV1> {
        let record = self.record(WorktreeCustodyStateV1::Materializing {}, None, None)?;
        self.stage_and_settle(&record, PublicationModeV1::Replace)
    }

    /// Replace with `LiveProtected`, carrying the observed target identity the sweep compares by
    /// descriptor.
    pub fn replace_live_protected(
        &self,
        identities: &MaterializedIdentitiesV1,
    ) -> Result<(), CustodyWriteRefusalV1> {
        let record = self.record(
            WorktreeCustodyStateV1::LiveProtected {},
            Some(identities.worktree.clone()),
            None,
        )?;
        self.stage_and_settle(&record, PublicationModeV1::Replace)
    }

    /// Replace with `PreservationUnknown { reason }` and its required claim.
    ///
    /// §5.7 row 4 ("during/after partial add, before live identity → report preservation unknown;
    /// never delete target") and §5.1 ("if materialization is unresolved, publish
    /// `PreservationUnknown{materialization_inflight}`"). Slice 2c1 also reaches this from
    /// [`Self::preserve_after_cancel`] when the claim cannot be minted over the retained
    /// identities.
    pub fn replace_preservation_unknown(
        &self,
        reason: PreservationReasonV1,
        identities: &MaterializedIdentitiesV1,
        recovery_locator: RecoveryLocatorV1,
        created_wall_ms: i64,
    ) -> Result<(), CustodyWriteRefusalV1> {
        self.replace_preserving_state(
            WorktreeCustodyStateV1::PreservationUnknown { reason },
            reason,
            identities,
            recovery_locator,
            created_wall_ms,
        )
    }

    /// Replace with `PreservationPrepared` and its required claim — §5.1 step 3.
    ///
    /// 2a's `claim_presence` makes the claim REQUIRED here, not optional: the design's step 3/4
    /// split is about which *state* is durable, not about whether an artifact exists. That data is
    /// built against, never re-derived (2a's docstring on `claim_presence`).
    pub fn replace_preservation_prepared(
        &self,
        reason: PreservationReasonV1,
        identities: &MaterializedIdentitiesV1,
        recovery_locator: RecoveryLocatorV1,
        created_wall_ms: i64,
    ) -> Result<(), CustodyWriteRefusalV1> {
        self.replace_preserving_state(
            WorktreeCustodyStateV1::PreservationPrepared {},
            reason,
            identities,
            recovery_locator,
            created_wall_ms,
        )
    }

    /// Replace with `Preserved` and its required claim — §5.1 step 4, R2f1b-terminal.
    pub fn replace_preserved(
        &self,
        reason: PreservationReasonV1,
        identities: &MaterializedIdentitiesV1,
        recovery_locator: RecoveryLocatorV1,
        created_wall_ms: i64,
    ) -> Result<(), CustodyWriteRefusalV1> {
        self.replace_preserving_state(
            WorktreeCustodyStateV1::Preserved {},
            reason,
            identities,
            recovery_locator,
            created_wall_ms,
        )
    }

    fn replace_preserving_state(
        &self,
        state: WorktreeCustodyStateV1,
        reason: PreservationReasonV1,
        identities: &MaterializedIdentitiesV1,
        recovery_locator: RecoveryLocatorV1,
        created_wall_ms: i64,
    ) -> Result<(), CustodyWriteRefusalV1> {
        let claim = PreservedWorktreeClaimV1 {
            schema_version: WORKTREE_CUSTODY_RECORD_SCHEMA_V1,
            custody_id: self.custody.plan.custody_id.clone(),
            execution_id: self.custody.attempt.execution_id.clone(),
            origin_attempt_id: self.custody.origin_attempt_id.clone(),
            current_attempt: self.custody.attempt.clone(),
            node: self.custody.node.clone(),
            checkout_fingerprint: self.custody.plan.checkout_fingerprint.clone(),
            source: identities.source.clone(),
            root: identities.root.clone(),
            worktree: identities.worktree.clone(),
            common_dir: identities.common_dir.clone(),
            reason,
            created_wall_ms,
            recovery_locator,
        };
        let record = self.record(state, Some(identities.worktree.clone()), Some(claim))?;
        self.stage_and_settle(&record, PublicationModeV1::Replace)
    }

    /// The custody state currently on disk for this checkout, read under the held cells.
    ///
    /// `Ok(None)` means the record name is free. An unreadable or undecodable record is an `Err`,
    /// never `None`: for a *transition* (unlike the deletion gate, whose discipline is
    /// presence-not-content) the from-state has to be known, and guessing it is how an illegal
    /// edge gets published over someone else's protection.
    pub fn current_state_kind(
        &self,
    ) -> Result<Option<WorktreeCustodyStateKindV1>, CustodyReadRefusalV1> {
        match self
            .root
            .child_entry_exists(&self.record_name, "custody record")
        {
            Ok(false) => return Ok(None),
            Ok(true) => {}
            Err(error) => return Err(CustodyReadRefusalV1::Unreadable(error.to_string())),
        }
        read_custody_record_in(&self.root, &self.record_name)
            .map(|record| Some(record.state.kind()))
    }

    /// Read the full current record under the held publication and custody cells.
    fn current_record(&self) -> Result<Option<WorktreeCustodyRecordV1>, CustodyReadRefusalV1> {
        match self
            .root
            .child_entry_exists(&self.record_name, "custody record")
        {
            Ok(false) => return Ok(None),
            Ok(true) => {}
            Err(error) => return Err(CustodyReadRefusalV1::Unreadable(error.to_string())),
        }
        read_custody_record_in(&self.root, &self.record_name).map(Some)
    }

    /// Re-observe the four object identities captured at materialization and compare them by
    /// DESCRIPTOR (§2.2: "Identity is checked by descriptor, not by re-canonicalizing a string, at
    /// every decision point").
    ///
    /// The comparison is on observed `dev`/`ino` plus birthtime when both sides carry it. A
    /// degraded re-observation (the object is gone, or cannot be opened no-follow) never matches
    /// a complete retained identity — so a
    /// vanished object fails verification rather than silently passing as "same path".
    #[must_use]
    pub fn identities_reverify(retained: &MaterializedIdentitiesV1) -> bool {
        [
            &retained.source,
            &retained.root,
            &retained.worktree,
            &retained.common_dir,
        ]
        .into_iter()
        .all(|expected| {
            let observed = observed_identity(&expected.canonical_path);
            observed.directory_identity.dev.is_some()
                && expected
                    .directory_identity
                    .matches(&observed.directory_identity)
        })
    }

    /// Exchange a validated predecessor claim for a successor-owned `RecoveredLive` record.
    ///
    /// The snapshot validation runs before this method opens a custody cell, writes a record, or
    /// acquires either lease. The returned ready value is the only successful outcome; it
    /// deliberately owns no provider handle, so slice 5 remains responsible for the later effect.
    ///
    /// This method takes blocking flocks for both custody cells and the predecessor/successor
    /// lease transfer; call it off the async executor, as with [`Self::enter`].
    ///
    /// It holds the predecessor recovery lease from validation through publication until the
    /// successor lease is acquired. A crash before the replace leaves `LiveProtected` and a
    /// process-released predecessor lease, so a later request retries cleanly. A crash after the
    /// replace but before the successor lease leaves `RecoveredLive` with neither lease, which
    /// the exact re-entry arm reacquires without publishing another transition.
    pub fn claim_exchange_for_successor(
        request: ClaimExchangeRequestV1<'_>,
    ) -> ClaimExchangeOutcomeV1 {
        claim_exchange_for_successor_impl(request)
    }

    /// §5.1's `preserve_after_cancel`, steps 3–7, for one custody-governed checkout.
    ///
    /// The cells are already held (they are this custodian's), so step 1 is satisfied by
    /// construction and step 2 — "close deletion admission" — is what holding the publication cell
    /// with the writer's blocking acquirer *is*: every deletion-side caller takes the same cell
    /// with the refusing acquirer and fails closed while we hold it.
    ///
    /// The sequence, and why each branch is where it is:
    ///
    /// 1. **Read the from-state.** Already `Preserved`/`PreservationUnknown` ⇒ return it unchanged:
    ///    §5.7's last row ("crash after preserved terminal: no automatic provider replay; claim
    ///    awaits R2f2"), and the transition table has no outgoing edge from either.
    /// 2. **Reverify the retained identities BEFORE minting anything** (P7). A mismatch means the
    ///    object graph is not the one we materialized, so no claim may assert it. The retained
    ///    identities — what we *did* materialize, observed at materialization — are what gets
    ///    recorded, and the state becomes `PreservationUnknown{AmbiguousCleanup}` instead of
    ///    `Preserved`. The replacement's identity never enters any record.
    /// 3. **`LiveProtected → PreservationPrepared → (Preserved | PreservationUnknown)`.** Never a
    ///    direct `LiveProtected → PreservationUnknown`: 2a's frozen table has no such edge, and
    ///    this slice adds none. A from-state that is ALREADY `PreservationPrepared` resumes at the
    ///    terminal step — see the resume rule below.
    /// 4. **Any ambiguous publication stops the sequence.** After an ambiguous replace the
    ///    from-state is genuinely unknown (either the prior state or the new one), so a further
    ///    replace could publish an illegal edge. Both candidates are protective — §5.7 "claim
    ///    renamed, parent sync ambiguous: prior prepared state or ambiguous claim remains
    ///    protective; report unknown" — so the answer is `Ambiguous`, and nothing else is written.
    ///
    /// # The `PreservationPrepared` resume rule (repair RA)
    ///
    /// A stranded `PreservationPrepared` record — the two-step's whole reason for existing — RESUMES
    /// to its terminal state instead of being refused as "no legal edge". It is a legal from-state:
    /// 2a's frozen table contains both `(PreservationPrepared, Preserved)` and
    /// `(PreservationPrepared, PreservationUnknown)`, and before this repair those were the only two
    /// edges in the table with no producer at all — the dead-wire-contract shape this slice refuses
    /// elsewhere. It is also *reachable*, not hypothetical: a crash or an ambiguous outcome between
    /// the two renames leaves exactly this state, and
    /// `claim_renamed_with_ambiguous_parent_sync_stays_protective` manufactures it deliberately.
    ///
    /// The resume **re-derives `verified` from the live objects and does not trust the stranded
    /// record's claim.** The stranded claim was minted at prepare time; anything could have happened
    /// to the object graph since, including the substitution P7 exists to refuse. Re-reading its
    /// identities would launder a stale assertion into a terminal one.
    ///
    /// The prepared re-publish is SKIPPED on resume, and that is not merely an optimization:
    /// `PreservationPrepared → PreservationPrepared` is not an edge in the table, so republishing
    /// would be a self-loop the frozen contract does not contain.
    pub fn preserve_after_cancel(
        &self,
        reason: PreservationReasonV1,
        retained: &MaterializedIdentitiesV1,
        recovery_locator: RecoveryLocatorV1,
        created_wall_ms: i64,
    ) -> PreservationOutcomeV1 {
        let from = match self.current_state_kind() {
            Ok(Some(kind)) => kind,
            Ok(None) => {
                return PreservationOutcomeV1::Refused(
                    "no custody record exists for this checkout".to_string(),
                )
            }
            Err(error) => return PreservationOutcomeV1::Refused(error.to_string()),
        };
        let resuming = match from {
            WorktreeCustodyStateKindV1::Preserved => {
                return PreservationOutcomeV1::AlreadyPreserved
            }
            WorktreeCustodyStateKindV1::PreservationUnknown => {
                return PreservationOutcomeV1::AlreadyUnknown
            }
            WorktreeCustodyStateKindV1::LiveProtected => false,
            WorktreeCustodyStateKindV1::PreservationPrepared => true,
            other => {
                return PreservationOutcomeV1::Refused(format!(
                    "no legal preservation edge from {}",
                    other.wire_tag()
                ))
            }
        };

        let verified = Self::identities_reverify(retained);
        let terminal_reason = if verified {
            reason
        } else {
            PreservationReasonV1::AmbiguousCleanup
        };
        // LOCATOR DOWNGRADE (repair RB). The locator is materialization-time evidence, and the
        // reverification that just failed is the only thing that could still vouch for it — the
        // common dir is one of the four objects it checks, so a claim asserting
        // `RegisteredWorktree` after a failed reverify would assert registration of an object graph
        // we have just proved we cannot recognise. Applied to the PREPARED publication too, not
        // only the terminal one: `verified` is known before either is written, so publishing a
        // confident locator and then contradicting it one rename later would leave a crash window
        // in which the durable record is more confident than the writer ever was.
        let recovery_locator = if verified {
            recovery_locator
        } else {
            RecoveryLocatorV1::RegistrationUnproven {}
        };

        if !resuming {
            if let Err(error) = self.replace_preservation_prepared(
                reason,
                retained,
                recovery_locator,
                created_wall_ms,
            ) {
                return PreservationOutcomeV1::from(error);
            }
        }
        let settled = if verified {
            self.replace_preserved(reason, retained, recovery_locator, created_wall_ms)
        } else {
            self.replace_preservation_unknown(
                terminal_reason,
                retained,
                recovery_locator,
                created_wall_ms,
            )
        };
        match settled {
            Ok(()) if verified => PreservationOutcomeV1::Preserved,
            Ok(()) => PreservationOutcomeV1::PreservationUnknown(terminal_reason),
            Err(error) => PreservationOutcomeV1::from(error),
        }
    }

    /// §5.1's deletion CAS: `LiveProtected -> DeleteAuthorized`, and the mint of its capability.
    ///
    /// This is the ONLY producer of a [`DeletionCapabilityV1`] anywhere in the workspace, and it
    /// runs under both custody cells (this custodian holds them), so no sweep, no gate, and no
    /// other writer can interleave with it.
    ///
    /// # Every refusal, and why it is a refusal rather than a check the caller could skip
    ///
    /// 1. **The from-state must be exactly `LiveProtected`.** `Preserved`, `PreservationUnknown`
    ///    and `PreservationPrepared` all refuse, which is where §5.1's monotonicity lives on the
    ///    DURABLE side: "once a preserved claim exists, only R2f2's explicit local
    ///    retain/archive/delete disposition can clear it; no later healthy projection or TTL can
    ///    mint deletion authority." `DeleteAuthorized` refuses too — that is "no re-mint from the
    ///    stale capability" (§5.7's crash-after-authorization row): a crash between the CAS and
    ///    the removal leaves a record whose sweep disposition is `Recover`, and recovery owns it.
    ///    Every other state has no `-> DeleteAuthorized` edge in 2a's frozen table, and this slice
    ///    adds none.
    /// 2. **The retained identities must reverify by descriptor.** A swapped object graph must
    ///    never be authorized for deletion — authorizing it would durably assert that the objects
    ///    we materialized are the ones now at those paths, and the whole point of the capability is
    ///    that `remove_v2` acts on the identity, not on the string.
    /// 3. **An ambiguous CAS mints nothing.** After an ambiguous replace the record may be
    ///    `LiveProtected` or `DeleteAuthorized`; both are protective and neither is `Removed`, so
    ///    the safe answer is to publish nothing further and authorize nothing.
    ///
    /// The record's `worktree` identity is the RETAINED one, not a fresh observation: it was just
    /// reverified, and re-observing would re-open the substitution window the reverification
    /// closed.
    pub fn authorize_deletion(
        &self,
        canonical_source: &str,
        retained: &MaterializedIdentitiesV1,
    ) -> DeletionAuthorizationV1 {
        let from = match self.current_state_kind() {
            Ok(Some(kind)) => kind,
            Ok(None) => {
                return DeletionAuthorizationV1::Refused(
                    "no custody record exists for this checkout".to_string(),
                )
            }
            Err(error) => return DeletionAuthorizationV1::Refused(error.to_string()),
        };
        if from != WorktreeCustodyStateKindV1::LiveProtected {
            return DeletionAuthorizationV1::Refused(format!(
                "no legal deletion-authorization edge from {}",
                from.wire_tag()
            ));
        }
        let record = match self.current_record() {
            Ok(Some(record)) => record,
            Ok(None) => {
                return DeletionAuthorizationV1::Refused(
                    "no custody record exists for this checkout".to_string(),
                )
            }
            Err(error) => return DeletionAuthorizationV1::Refused(error.to_string()),
        };
        if record.custody_id != self.custody.plan.custody_id
            || record.checkout_fingerprint != self.custody.plan.checkout_fingerprint
            || record.current_attempt != self.custody.attempt
            || record.worktree != retained.worktree
        {
            return DeletionAuthorizationV1::Refused(
                "custody record does not match this checkout's retained ownership".to_string(),
            );
        }
        if !Self::identities_reverify(retained) {
            return DeletionAuthorizationV1::Refused(
                "retained object identities no longer verify by descriptor".to_string(),
            );
        }
        match self.replace_delete_authorized(retained) {
            Ok(()) => DeletionAuthorizationV1::Authorized(Box::new(DeletionCapabilityV1 {
                custody_id: self.custody.plan.custody_id.clone(),
                canonical_source: canonical_source.to_string(),
                worktree_path: self.worktree_path.clone(),
                identities: retained.clone(),
            })),
            Err(CustodyWriteRefusalV1::Ambiguous(detail)) => {
                DeletionAuthorizationV1::Ambiguous(detail)
            }
            Err(other) => DeletionAuthorizationV1::Refused(other.to_string()),
        }
    }

    fn replace_delete_authorized(
        &self,
        retained: &MaterializedIdentitiesV1,
    ) -> Result<(), CustodyWriteRefusalV1> {
        let record = self.record(
            WorktreeCustodyStateV1::DeleteAuthorized {},
            Some(retained.worktree.clone()),
            None,
        )?;
        self.stage_and_settle(&record, PublicationModeV1::Replace)
    }

    /// §5.1's final step: "then records `Removed`" — the tombstone, published only after the
    /// provider proved the target and the registration are both gone.
    ///
    /// The from-state must be `DeleteAuthorized`. Recording `Removed` over anything else would
    /// assert a removal this custodian never authorized, and `Removed` is where the record stops
    /// protecting a checkout against a `MarkerOnly` sweep.
    ///
    /// The retained worktree identity is written rather than a fresh observation for a blunt
    /// reason: the object is GONE, so a fresh observation would degrade to a path with no
    /// `dev`/`ino` and 2a's completeness rule would reject the record outright. The tombstone's
    /// honest content is the identity of the object that was removed.
    pub fn record_removed(&self, retained: &MaterializedIdentitiesV1) -> RemovalRecordV1 {
        let from = match self.current_state_kind() {
            Ok(Some(kind)) => kind,
            Ok(None) => {
                return RemovalRecordV1::Refused(
                    "no custody record exists for this checkout".to_string(),
                )
            }
            Err(error) => return RemovalRecordV1::Refused(error.to_string()),
        };
        if from != WorktreeCustodyStateKindV1::DeleteAuthorized {
            return RemovalRecordV1::Refused(format!(
                "no legal removal-tombstone edge from {}",
                from.wire_tag()
            ));
        }
        let record = match self.record(
            WorktreeCustodyStateV1::Removed {},
            Some(retained.worktree.clone()),
            None,
        ) {
            Ok(record) => record,
            Err(error) => return RemovalRecordV1::Refused(error.to_string()),
        };
        match self.stage_and_settle(&record, PublicationModeV1::Replace) {
            Ok(()) => RemovalRecordV1::Recorded,
            Err(CustodyWriteRefusalV1::Ambiguous(detail)) => RemovalRecordV1::Ambiguous(detail),
            Err(other) => RemovalRecordV1::Refused(other.to_string()),
        }
    }

    fn record(
        &self,
        state: WorktreeCustodyStateV1,
        worktree: Option<WorktreeObjectIdentityV1>,
        claim: Option<PreservedWorktreeClaimV1>,
    ) -> Result<WorktreeCustodyRecordV1, CustodyWriteRefusalV1> {
        // Degraded-vs-observed is decided by the STATE's own settled rule, not by what happens to
        // be observable: 2a published `identity_completeness` as data precisely so the writer,
        // the reader, and 2c/2d cannot each re-derive it.
        let worktree = worktree.unwrap_or_else(|| match state.identity_completeness() {
            IdentityCompletenessV1::MayBeDegraded => {
                planned_identity(self.custody.plan.target_cwd.as_str())
            }
            IdentityCompletenessV1::Complete => {
                observed_identity(self.custody.plan.target_cwd.as_str())
            }
        });
        debug_assert!(
            (state.claim_presence() == ClaimPresenceV1::Required) == claim.is_some()
                || state.claim_presence() == ClaimPresenceV1::Optional,
            "a state's claim rule is 2a's data, not the writer's choice"
        );
        let record = WorktreeCustodyRecordV1 {
            schema_version: WORKTREE_CUSTODY_RECORD_SCHEMA_V1,
            custody_id: self.custody.plan.custody_id.clone(),
            checkout_fingerprint: self.custody.plan.checkout_fingerprint.clone(),
            current_attempt: self.custody.attempt.clone(),
            worktree,
            state,
            claim,
        };
        record.validate_for_publication()?;
        Ok(record)
    }

    /// Stage the encoded record as a fresh, nonce-named private file, fsync IT, then publish or
    /// replace under the pinned root and parent-sync.
    fn stage_and_settle(
        &self,
        record: &WorktreeCustodyRecordV1,
        mode: PublicationModeV1,
    ) -> Result<(), CustodyWriteRefusalV1> {
        publish_custody_record_in(&self.root, &self.record_name, record, mode)
    }

    #[cfg(test)]
    fn settle_residue(&self, staged_path: &Path, outcome: &CustodyPublicationV1) {
        settle_custody_staging_residue(&self.root, &self.record_name, staged_path, outcome);
    }
}

/// Stage one custody record through the sole publication derivation shared by writers and
/// unused-marker settlement. The pinned root and one-component record name make every write and
/// parent sync descriptor-relative; staging residue is deliberately quarantined, never unlinked.
fn publish_custody_record_in(
    pin: &PinnedDirectoryV1,
    name: &OsStr,
    record: &WorktreeCustodyRecordV1,
    mode: PublicationModeV1,
) -> Result<(), CustodyWriteRefusalV1> {
    let bytes = record.encode_canonical()?;
    let staged_name = staged_record_name(name)?;
    let staged_path = pin.canonical_path().join(&staged_name);
    let mut file = open_options_create_new_owner_private()
        .open(&staged_path)
        .map_err(|error| {
            CustodyWriteRefusalV1::Failed(format!("custody record could not be staged: {error}"))
        })?;
    let staged = (|| -> Result<(), CustodyWriteRefusalV1> {
        file.write_all(&bytes).map_err(|error| {
            CustodyWriteRefusalV1::Failed(format!("custody record could not be written: {error}"))
        })?;
        // Publication synchronizes the parent directory; the caller must synchronize the new
        // file before it is named or a crash can leave a durable empty record.
        file.sync_all().map_err(|error| {
            CustodyWriteRefusalV1::Failed(format!("custody record could not be synced: {error}"))
        })
    })();
    if let Err(error) = staged {
        quarantine_custody_residue(pin, name, &staged_path, &error.to_string());
        return Err(error);
    }

    let source = RegularChildRefV1::new(OsStr::new(&staged_name), &file);
    let published = match mode {
        PublicationModeV1::NoReplace => {
            pin.publish_new_regular_child(source, name, "worktree custody record")
        }
        PublicationModeV1::Replace => {
            pin.replace_regular_child(source, name, "worktree custody record")
        }
    };
    match published {
        Err(error) => {
            quarantine_custody_residue(pin, name, &staged_path, &error.to_string());
            Err(error.into())
        }
        Ok(outcome) => {
            settle_custody_staging_residue(pin, name, &staged_path, &outcome);
            match outcome.ambiguity() {
                None => Ok(()),
                Some(detail) => Err(CustodyWriteRefusalV1::Ambiguous(detail.to_string())),
            }
        }
    }
}

fn settle_custody_staging_residue(
    pin: &PinnedDirectoryV1,
    name: &OsStr,
    staged_path: &Path,
    outcome: &CustodyPublicationV1,
) {
    if outcome.is_durable() {
        return;
    }
    quarantine_custody_residue(
        pin,
        name,
        staged_path,
        outcome.ambiguity().unwrap_or_default(),
    );
}

fn quarantine_custody_residue(
    pin: &PinnedDirectoryV1,
    name: &OsStr,
    staged_path: &Path,
    detail: &str,
) {
    tracing::warn!(
        root = %pin.canonical_path().display(),
        custody_record = %name.to_string_lossy(),
        staged = %staged_path.display(),
        detail,
        "quarantining a staged custody record; it is left as inert recovery evidence"
    );
}

/// Return the exact frozen checkout effect that a custody binding selected from a snapshot.
fn binding_matches_snapshot<'a>(
    binding: &BoundWorktreeCustodyV1,
    snapshot: &'a WorkflowSnapshotV3,
) -> Option<&'a FrozenCheckoutEffectV1> {
    if binding.attempt != snapshot.attempt
        || binding.origin_attempt_id != snapshot.delivery_spec.attempt_id
    {
        return None;
    }
    snapshot
        .delivery_spec
        .node_execution_identities
        .iter()
        .find(|identity| identity.node == binding.node)?
        .provider_attempts
        .iter()
        .find_map(|provider_attempt| {
            matches!(
                select_custody_plan_v1(&snapshot.r2f1b, &provider_attempt.checkout),
                Ok(Some(plan)) if plan == &binding.plan
            )
            .then_some(&provider_attempt.checkout)
        })
}

fn validate_claim_exchange(
    predecessor_snapshot: &WorkflowSnapshotV3,
    successor_snapshot: &WorkflowSnapshotV3,
    predecessor_binding: &BoundWorktreeCustodyV1,
    successor_binding: &BoundWorktreeCustodyV1,
) -> Result<
    (
        bridge_core::execution_policy::Sha256HexV1,
        FrozenCheckoutEffectV1,
    ),
    String,
> {
    predecessor_snapshot
        .validate()
        .map_err(|error| format!("invalid predecessor snapshot: {error}"))?;
    successor_snapshot
        .validate()
        .map_err(|error| format!("invalid successor snapshot: {error}"))?;
    WorkflowSnapshotV3::validate_successor(predecessor_snapshot, successor_snapshot)
        .map_err(|error| format!("invalid successor relationship: {error}"))?;
    let predecessor_checkout = binding_matches_snapshot(predecessor_binding, predecessor_snapshot)
        .ok_or_else(|| {
            "predecessor custody binding does not match the frozen snapshot".to_string()
        })?;
    let successor_checkout = binding_matches_snapshot(successor_binding, successor_snapshot)
        .ok_or_else(|| {
            "successor custody binding does not match the frozen snapshot".to_string()
        })?;
    if predecessor_binding.plan != successor_binding.plan
        || predecessor_binding.node != successor_binding.node
        || predecessor_checkout != successor_checkout
    {
        return Err("successor custody binding changed the frozen checkout identity".to_string());
    }
    Ok((
        predecessor_snapshot.digest().map_err(|error| {
            format!("predecessor snapshot digest could not be computed: {error}")
        })?,
        predecessor_checkout.clone(),
    ))
}

fn validate_frozen_exchange_graph(
    frozen_checkout: &FrozenCheckoutEffectV1,
    worktree_root: &Path,
    worktree_path: &str,
    retained: &MaterializedIdentitiesV1,
) -> Result<(), String> {
    let FrozenCheckoutEffectV1::Worktree {
        canonical_source_cwd,
        canonical_worktree_root,
        target_cwd,
        ..
    } = frozen_checkout
    else {
        return Err("custody binding selected a direct checkout".to_string());
    };
    if worktree_root != Path::new(canonical_worktree_root.as_str()) {
        return Err("claim-exchange root does not match the frozen worktree root".to_string());
    }
    let frozen_target = Path::new(target_cwd.as_str());
    if frozen_target.parent() != Some(Path::new(canonical_worktree_root.as_str())) {
        return Err("frozen worktree target is not a direct child of its frozen root".to_string());
    }
    if frozen_target != Path::new(worktree_path) {
        return Err(
            "claim-exchange worktree path does not match the frozen checkout target".to_string(),
        );
    }
    if retained.source.canonical_path != canonical_source_cwd.as_str()
        || retained.root.canonical_path != canonical_worktree_root.as_str()
        || retained.worktree.canonical_path != target_cwd.as_str()
    {
        return Err("retained identities do not name the frozen checkout object graph".to_string());
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum ClaimExchangeRecordArmV1 {
    PublishRecoveredLive,
    ReenterRecoveredLive,
}

fn classify_claim_exchange_record(
    record: &WorktreeCustodyRecordV1,
    predecessor_binding: &BoundWorktreeCustodyV1,
    successor_binding: &BoundWorktreeCustodyV1,
    retained: &MaterializedIdentitiesV1,
    predecessor_snapshot_digest: &bridge_core::execution_policy::Sha256HexV1,
) -> Result<ClaimExchangeRecordArmV1, String> {
    if record.custody_id != predecessor_binding.plan.custody_id
        || record.checkout_fingerprint != predecessor_binding.plan.checkout_fingerprint
        || record.worktree != retained.worktree
    {
        return Err(
            "custody record does not match the retained frozen checkout identity".to_string(),
        );
    }
    match &record.state {
        WorktreeCustodyStateV1::LiveProtected {}
            if record.current_attempt == predecessor_binding.attempt =>
        {
            Ok(ClaimExchangeRecordArmV1::PublishRecoveredLive)
        }
        WorktreeCustodyStateV1::RecoveredLive {
            predecessor_claim_digest,
        } if predecessor_claim_digest == predecessor_snapshot_digest
            && record.current_attempt == successor_binding.attempt =>
        {
            Ok(ClaimExchangeRecordArmV1::ReenterRecoveredLive)
        }
        WorktreeCustodyStateV1::LiveProtected {} => {
            Err("live predecessor custody record has a different attempt".to_string())
        }
        WorktreeCustodyStateV1::RecoveredLive { .. } => Err(
            "RecoveredLive custody record does not exactly match this successor request"
                .to_string(),
        ),
        state => Err(format!(
            "no legal claim-exchange edge from {}",
            state.kind().wire_tag()
        )),
    }
}

fn preflight_frozen_record(
    worktree_root: &Path,
    worktree_path: &str,
    frozen_target: &str,
) -> Result<WorktreeCustodyRecordV1, String> {
    let root = PinnedDirectoryV1::open(worktree_root, "claim-exchange frozen-record preflight")
        .map_err(|error| error.to_string())?;
    let record_name = record_file_name(worktree_path).map_err(|error| error.to_string())?;
    let record = read_custody_record_in(&root, &record_name).map_err(|error| error.to_string())?;
    if record.worktree.canonical_path != frozen_target {
        return Err(
            "custody record worktree does not match the frozen checkout target".to_string(),
        );
    }
    Ok(record)
}

fn claim_exchange_for_successor_impl(
    request: ClaimExchangeRequestV1<'_>,
) -> ClaimExchangeOutcomeV1 {
    let ClaimExchangeRequestV1 {
        worktree_root,
        worktree_path,
        predecessor_snapshot,
        successor_snapshot,
        predecessor_binding,
        successor_binding,
        retained,
        lease_namespace_dir,
    } = request;
    // This is intentionally the first operation: no custody lock, lease acquisition, or write
    // may occur before both individual snapshots and their successor relationship validate.
    let (predecessor_claim_digest, frozen_checkout) = match validate_claim_exchange(
        predecessor_snapshot,
        successor_snapshot,
        predecessor_binding,
        &successor_binding,
    ) {
        Ok(validated) => validated,
        Err(detail) => return ClaimExchangeOutcomeV1::Refused(detail),
    };
    if let Err(detail) =
        validate_frozen_exchange_graph(&frozen_checkout, worktree_root, worktree_path, retained)
    {
        return ClaimExchangeOutcomeV1::Refused(detail);
    }
    if !WorktreeCustodianV1::identities_reverify(retained) {
        return ClaimExchangeOutcomeV1::Refused(
            "retained object identities no longer verify by descriptor".to_string(),
        );
    }
    let FrozenCheckoutEffectV1::Worktree { target_cwd, .. } = &frozen_checkout else {
        return ClaimExchangeOutcomeV1::Refused(
            "custody binding selected a direct checkout".to_string(),
        );
    };
    if let Err(detail) = preflight_frozen_record(worktree_root, worktree_path, target_cwd.as_str())
    {
        return ClaimExchangeOutcomeV1::Refused(detail);
    }

    let predecessor_recovery_lease =
        match acquire_lease_in(lease_namespace_dir, predecessor_snapshot.attempt.run_id()) {
            Ok(lease) => lease,
            Err(error) => return ClaimExchangeOutcomeV1::Refused(error.to_string()),
        };

    let custodian =
        match WorktreeCustodianV1::enter(worktree_root, worktree_path, successor_binding) {
            Ok(custodian) => custodian,
            Err(error) => return ClaimExchangeOutcomeV1::Refused(error.to_string()),
        };

    let current = match custodian.current_record() {
        Ok(Some(record)) => record,
        Ok(None) => {
            return ClaimExchangeOutcomeV1::Refused(
                "no custody record exists for this checkout".to_string(),
            )
        }
        Err(error) => return ClaimExchangeOutcomeV1::Refused(error.to_string()),
    };
    let record_arm = match classify_claim_exchange_record(
        &current,
        predecessor_binding,
        custodian.binding(),
        retained,
        &predecessor_claim_digest,
    ) {
        Ok(arm) => arm,
        Err(detail) => return ClaimExchangeOutcomeV1::Refused(detail),
    };
    if !WorktreeCustodianV1::identities_reverify(retained) {
        return ClaimExchangeOutcomeV1::Refused(
            "retained object identities no longer verify by descriptor".to_string(),
        );
    }

    match record_arm {
        ClaimExchangeRecordArmV1::PublishRecoveredLive => {
            if !transition_is_legal(
                WorktreeCustodyStateKindV1::LiveProtected,
                WorktreeCustodyStateKindV1::RecoveredLive,
            ) {
                return ClaimExchangeOutcomeV1::Refused(
                    "the frozen custody table does not permit claim exchange".to_string(),
                );
            }
            let recovered = match custodian.record(
                WorktreeCustodyStateV1::RecoveredLive {
                    predecessor_claim_digest: predecessor_claim_digest.clone(),
                },
                Some(retained.worktree.clone()),
                None,
            ) {
                Ok(record) => record,
                Err(error) => return ClaimExchangeOutcomeV1::Refused(error.to_string()),
            };
            match custodian.stage_and_settle(&recovered, PublicationModeV1::Replace) {
                Ok(()) => {}
                Err(CustodyWriteRefusalV1::Ambiguous(detail)) => {
                    return ClaimExchangeOutcomeV1::Ambiguous(detail)
                }
                Err(other) => return ClaimExchangeOutcomeV1::Refused(other.to_string()),
            }
        }
        ClaimExchangeRecordArmV1::ReenterRecoveredLive => {}
    }
    let successor_attempt = custodian.binding().attempt.clone();
    drop(custodian);
    let successor_live_lease =
        match acquire_lease_in(lease_namespace_dir, successor_attempt.run_id()) {
            Ok(lease) => lease,
            Err(error) => return ClaimExchangeOutcomeV1::LeaseUnavailable(error.to_string()),
        };
    drop(predecessor_recovery_lease);
    ClaimExchangeOutcomeV1::Exchanged(ClaimExchangeReadyV1 {
        predecessor_claim_digest,
        successor_attempt,
        _successor_live_lease: successor_live_lease,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PublicationModeV1 {
    NoReplace,
    Replace,
}

fn record_file_name(worktree_path: &str) -> Result<OsString, CustodyWriteRefusalV1> {
    let name = Path::new(worktree_path).file_name().ok_or_else(|| {
        CustodyWriteRefusalV1::Failed(format!("worktree target has no file name: {worktree_path}"))
    })?;
    let name = ChildNameV2::from_bytes(name.as_encoded_bytes()).map_err(|error| {
        CustodyWriteRefusalV1::Failed(format!(
            "worktree target has an invalid child name: {error:?}"
        ))
    })?;
    if ChildNameV2::parse_reserved(ReservedNameNamespaceV2::RetirementCapture, &name).is_ok() {
        return Err(CustodyWriteRefusalV1::Failed(
            "worktree target uses the reserved retirement-capture namespace".into(),
        ));
    }
    let mut record = name.as_os_str().to_os_string();
    record.push(CUSTODY_RECORD_SUFFIX);
    Ok(record)
}

/// `<record name>.staging-<32 hex>`.
///
/// The nonce is what makes a retry converge instead of colliding with its own residue, and the
/// suffix ORDER matters: appending after `.custody.v1.json` means `is_custody_record_name` (which
/// requires that exact ending) rejects it, so no scan can read residue as a record.
fn staged_record_name(record_name: &OsStr) -> Result<OsString, CustodyWriteRefusalV1> {
    let nonce = WorktreeCustodyIdV1::mint()
        .map_err(|error| CustodyWriteRefusalV1::Failed(format!("staging nonce: {error:?}")))?;
    let mut name = record_name.to_os_string();
    // The typed id is `"custody-" + 64 lowercase hex`; take the HEX, not the prefix, so the
    // residue suffix is a pure hex token `is_staged_custody_residue` can recognise.
    let hex = &nonce.as_str()[WorktreeCustodyIdV1::PREFIX.len()..][..STAGING_NONCE_HEX_LEN];
    name.push(format!(".staging-{hex}"));
    Ok(name)
}

/// The staging nonce's exact length, in hex characters.
pub const STAGING_NONCE_HEX_LEN: usize = 32;

/// Is `name` a staged custody-publication residue?
///
/// Recognition is EXACT — `<non-empty stem>.custody.v1.json.staging-` followed by exactly
/// [`STAGING_NONCE_HEX_LEN`] LOWERCASE hex characters and nothing else. Loose recognition is not a
/// harmless nicety: the storage report classifies whatever this accepts as bridge-owned
/// `Evidence`, so an over-permissive predicate would label an operator's own file beside a
/// checkout as bridge residue and offer it for disposition on that basis.
#[must_use]
pub fn is_staged_custody_residue(name: &str) -> bool {
    let base = name.rsplit_once('/').map_or(name, |(_, base)| base);
    let marker = const_format_staging_marker();
    let Some(index) = base.rfind(&marker) else {
        return false;
    };
    // A non-empty target stem is required, exactly as `is_custody_record_name` requires one:
    // `.custody.v1.json.staging-<hex>` naming no target is not this writer's artifact.
    if index == 0 {
        return false;
    }
    let nonce = &base[index + marker.len()..];
    nonce.len() == STAGING_NONCE_HEX_LEN
        && nonce
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn const_format_staging_marker() -> String {
    format!("{CUSTODY_RECORD_SUFFIX}.staging-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custody::{custody_record_path, read_custody_record_in, WorktreeCustodyStateKindV1};
    use bridge_core::execution_policy::{
        FrozenWorktreeCustodyPlanV1, PolicyNodeRefV1, Sha256HexV1,
    };
    use bridge_core::fs_custody::PublicationRenameFaultV1;
    use bridge_core::ids::{AttemptId, AttemptIdentity, ExecutionId};
    use bridge_core::SessionCwd;
    use std::ffi::OsString;

    fn root(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "a2a-bridge-custody-writer-{name}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        std::fs::canonicalize(&path).unwrap()
    }

    /// Pre-create the replacement while the authorized directory is still live, then rename
    /// both objects into place. Two simultaneously live directories cannot share an inode, so
    /// this constructs a deterministic swap even when the filesystem promptly recycles inodes.
    /// The displaced original remains at the returned path until the test root is removed.
    fn replace_directory_with_precreated_sibling(path: &Path) -> PathBuf {
        let parent = path.parent().expect("test directory has a parent");
        let name = path
            .file_name()
            .expect("test directory has a name")
            .to_string_lossy();
        let replacement = parent.join(format!("{name}.swap-replacement"));
        let displaced = parent.join(format!("{name}.swap-original"));
        std::fs::create_dir(&replacement).unwrap();

        let before = observed_identity(&path.to_string_lossy());
        let candidate = observed_identity(&replacement.to_string_lossy());
        assert!(
            !before
                .directory_identity
                .matches(&candidate.directory_identity),
            "precondition: simultaneously live original and replacement must have distinct identities"
        );

        std::fs::rename(path, &displaced).unwrap();
        std::fs::rename(&replacement, path).unwrap();

        let after = observed_identity(&path.to_string_lossy());
        assert!(
            !before.directory_identity.matches(&after.directory_identity),
            "precondition: the same-name replacement must not match the retained identity"
        );
        displaced
    }

    fn binding(target: &Path) -> BoundWorktreeCustodyV1 {
        let attempt_id = AttemptId::parse(format!("attempt-{}", "2".repeat(32))).unwrap();
        BoundWorktreeCustodyV1 {
            attempt: AttemptIdentity {
                execution_id: ExecutionId::parse(format!("exec-{}", "1".repeat(32))).unwrap(),
                attempt_id: attempt_id.clone(),
                ordinal: 0,
                parent_attempt_id: None,
            },
            origin_attempt_id: attempt_id,
            node: PolicyNodeRefV1::from_node_id(0, "node"),
            plan: FrozenWorktreeCustodyPlanV1 {
                custody_id: WorktreeCustodyIdV1::mint().unwrap(),
                checkout_fingerprint: Sha256HexV1::parse("6".repeat(64)).unwrap(),
                target_cwd: SessionCwd::parse(&target.to_string_lossy()).unwrap(),
            },
        }
    }

    fn identities(target: &Path) -> MaterializedIdentitiesV1 {
        let path = target.to_string_lossy().into_owned();
        MaterializedIdentitiesV1 {
            source: observed_identity(&path),
            root: observed_identity(&target.parent().unwrap().to_string_lossy()),
            worktree: observed_identity(&path),
            common_dir: planned_identity("/src/.git"),
        }
    }

    fn record_state(worktree_root: &Path, target: &Path) -> Option<WorktreeCustodyStateKindV1> {
        let pinned = PinnedDirectoryV1::open(worktree_root, "test").ok()?;
        let name = Path::new(&custody_record_path(&target.to_string_lossy()))
            .file_name()?
            .to_os_string();
        read_custody_record_in(&pinned, &name)
            .ok()
            .map(|record| record.state.kind())
    }

    fn residue(worktree_root: &Path) -> Vec<String> {
        std::fs::read_dir(worktree_root)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| is_staged_custody_residue(name))
            .collect()
    }

    /// §5.7 row 3, and the ordering the whole slice rests on: the record is durable, in
    /// `ProtectionPrepared`, before any provider effect could run. Discriminates a writer that
    /// stages the record but never publishes it, and one that publishes a state the sweep would
    /// not protect.
    #[test]
    fn protection_prepared_is_published_and_readable_before_any_provider_effect() {
        let worktree_root = root("prepared");
        let target = worktree_root.join("ownr-run7-abc");

        let custodian =
            WorktreeCustodianV1::enter(&worktree_root, &target.to_string_lossy(), binding(&target))
                .unwrap();
        custodian.publish_protection_prepared().unwrap();

        assert_eq!(
            record_state(&worktree_root, &target),
            Some(WorktreeCustodyStateKindV1::ProtectionPrepared)
        );
        assert!(
            !target.exists(),
            "the checkout must not exist yet: the record precedes `git worktree add`"
        );
        assert!(
            !custodian
                .pinned_root()
                .canonical_path()
                .join("ownr-run7-abc.meta.json")
                .exists(),
            "the V3 path must never emit the legacy sidecar name"
        );
        assert!(
            residue(&worktree_root).is_empty(),
            "a durable publish clears its own staging"
        );
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// §5.7 row 1 + row 2, together: an already-published record at the target name refuses the
    /// no-replace publish (another owner got there first), the FINAL record is untouched, and the
    /// staged temp is quarantined rather than silently deleted.
    ///
    /// Discriminates a writer that uses the REPLACING primitive for the first publication — which
    /// would clobber another owner's protection and silently take custody of their checkout.
    #[test]
    fn a_foreign_record_refuses_the_first_publication_and_the_temp_is_quarantined() {
        let worktree_root = root("foreign-record");
        let target = worktree_root.join("ownr-run7-abc");
        let first =
            WorktreeCustodianV1::enter(&worktree_root, &target.to_string_lossy(), binding(&target))
                .unwrap();
        first.publish_protection_prepared().unwrap();
        let owner = std::fs::read(custody_record_path(&target.to_string_lossy())).unwrap();
        drop(first);

        let second =
            WorktreeCustodianV1::enter(&worktree_root, &target.to_string_lossy(), binding(&target))
                .unwrap();
        let refused = second.publish_protection_prepared();

        assert!(
            matches!(refused, Err(CustodyWriteRefusalV1::Failed(_))),
            "a taken record name is a provable refusal, never an ambiguity: {refused:?}"
        );
        assert_eq!(
            std::fs::read(custody_record_path(&target.to_string_lossy())).unwrap(),
            owner,
            "the first owner's record must be byte-identical afterwards"
        );
        assert_eq!(
            residue(&worktree_root).len(),
            1,
            "the staged temp is quarantined (§5.7 row 2), not unlinked"
        );
        drop(second);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// The staged residue is invisible to BOTH sweep patterns. Discriminates a staging name built
    /// as `<nonce>.custody.v1.json` or `<target>.meta.json.tmp`: the first would be scanned as an
    /// undecodable record on every boot, the second handed to the legacy arm, which deletes.
    #[test]
    fn staged_residue_matches_neither_sweep_pattern_and_never_collides_with_itself() {
        let worktree_root = root("residue-naming");
        let target = worktree_root.join("ownr-run7-abc");
        let name = record_file_name(&target.to_string_lossy()).unwrap();

        let first = staged_record_name(&name).unwrap();
        let second = staged_record_name(&name).unwrap();

        assert_ne!(
            first, second,
            "a fresh nonce is what makes a retry converge"
        );
        for staged in [&first, &second] {
            let staged = staged.to_string_lossy().into_owned();
            assert!(is_staged_custody_residue(&staged));
            assert!(
                !crate::custody::is_custody_record_name(&staged),
                "residue must not be selectable by the V3 scan: {staged}"
            );
            assert!(
                !staged.ends_with(".meta.json"),
                "residue must not be selectable by the legacy scan: {staged}"
            );
            assert_eq!(
                Path::new(&staged).components().count(),
                1,
                "the staging name must stay a single path component"
            );
        }
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// Discriminates treating a retirement-residue spelling as a valid checkout basename: this
    /// reservation makes the scanner's exact residue classification unambiguous.
    #[test]
    fn a_retirement_capture_basename_cannot_mint_a_custody_record() {
        let worktree_root = root("retirement-capture-basename");
        let target = worktree_root.join(".a2a-v2-rtc-ordinary");
        std::fs::create_dir(&target).unwrap();

        let refused = record_file_name(&target.to_string_lossy());
        assert!(
            matches!(&refused, Err(CustodyWriteRefusalV1::Failed(reason)) if reason.contains("retirement-capture")),
            "reserved checkout basename must not mint an ambiguous marker: {refused:?}"
        );
        assert!(
            !crate::custody::is_custody_record_name(&custody_record_path(
                &target.to_string_lossy()
            )),
            "the reserved spelling is reserved for core retirement residue"
        );

        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// The full ordered sequence, and the state the sweep sees at each step. Discriminates a
    /// writer that skips `Materializing` (so a crash during the add is indistinguishable from a
    /// crash before it) and one that publishes an illegal edge.
    #[test]
    fn the_transition_sequence_is_prepared_then_materializing_then_live_protected() {
        let worktree_root = root("sequence");
        let target = worktree_root.join("ownr-run7-abc");
        let custodian =
            WorktreeCustodianV1::enter(&worktree_root, &target.to_string_lossy(), binding(&target))
                .unwrap();

        custodian.publish_protection_prepared().unwrap();
        assert_eq!(
            record_state(&worktree_root, &target),
            Some(WorktreeCustodyStateKindV1::ProtectionPrepared)
        );
        custodian.replace_materializing().unwrap();
        assert_eq!(
            record_state(&worktree_root, &target),
            Some(WorktreeCustodyStateKindV1::Materializing)
        );
        std::fs::create_dir_all(&target).unwrap();
        custodian
            .replace_live_protected(&identities(&target))
            .unwrap();
        assert_eq!(
            record_state(&worktree_root, &target),
            Some(WorktreeCustodyStateKindV1::LiveProtected)
        );

        for (from, to) in [
            (
                WorktreeCustodyStateKindV1::ProtectionPrepared,
                WorktreeCustodyStateKindV1::Materializing,
            ),
            (
                WorktreeCustodyStateKindV1::Materializing,
                WorktreeCustodyStateKindV1::LiveProtected,
            ),
        ] {
            assert!(
                crate::custody::transition_is_legal(from, to),
                "the writer must only publish edges 2a froze as legal: {from:?} -> {to:?}"
            );
        }
        assert!(residue(&worktree_root).is_empty());
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// §5.7 row 5's shape at the writer: a rename that took effect but whose parent sync did not
    /// complete is reported AMBIGUOUS, never as success and never as "nothing happened".
    /// Discriminates a writer that unwraps the publication outcome, or that maps every non-
    /// `Durable` arm to a plain failure the caller would read as "the record was not written".
    ///
    /// RENAMED in the 2b2 repair round (opus W-4a): this test never asserted "the residue is
    /// kept", and could not — the rename COMMITTED here, so the source name is already free and
    /// the residue assertion below is `== 0`. The residue-kept rule is pinned by
    /// `a_foreign_record_refuses_the_first_publication_and_the_temp_is_quarantined` (the `Err`
    /// arm) and `a_durable_publication_never_unlinks_the_staging_pathname` (the identity hazard).
    #[test]
    fn an_unsynced_parent_is_reported_ambiguous_with_the_record_already_durable() {
        let worktree_root = root("parent-sync");
        let target = worktree_root.join("ownr-run7-abc");
        let custodian =
            WorktreeCustodianV1::enter(&worktree_root, &target.to_string_lossy(), binding(&target))
                .unwrap();
        custodian.pinned_root().fail_sync_on_nth_call_for_test(1);

        let outcome = custodian.publish_protection_prepared();

        assert!(
            matches!(outcome, Err(CustodyWriteRefusalV1::Ambiguous(_))),
            "unexpected outcome: {outcome:?}"
        );
        assert_eq!(
            record_state(&worktree_root, &target),
            Some(WorktreeCustodyStateKindV1::ProtectionPrepared),
            "the rename DID take effect; the record is on disk and protective"
        );
        assert_eq!(
            residue(&worktree_root).len(),
            0,
            "the source name is free after a committed rename, so nothing was left to keep"
        );
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// The PARKED-1 review's fault-counting obligation (opus S-6), pinned as executable evidence:
    /// `PinnedDirectoryV1`'s publication-rename countdown is ONE counter shared by publish AND
    /// replace, so "call 3" in a publish→replace→replace sequence is the THIRD rename, not the
    /// third replace. Discriminates a crash-matrix test that counts only replaces — it would arm
    /// a fault that lands on a different transition than its own name claims.
    #[test]
    fn the_publication_fault_countdown_counts_publishes_and_replaces_together() {
        let worktree_root = root("fault-counting");
        let target = worktree_root.join("ownr-run7-abc");
        let custodian =
            WorktreeCustodianV1::enter(&worktree_root, &target.to_string_lossy(), binding(&target))
                .unwrap();
        custodian
            .pinned_root()
            .fail_publication_rename_on_nth_call_for_test(
                3,
                PublicationRenameFaultV1::BeforeEffect,
            );

        custodian
            .publish_protection_prepared()
            .expect("rename call 1 is not the armed one");
        custodian
            .replace_materializing()
            .expect("rename call 2 is not the armed one");
        std::fs::create_dir_all(&target).unwrap();
        let third = custodian.replace_live_protected(&identities(&target));

        assert!(
            third.is_err(),
            "the third RENAME must be the armed one, counting the publish"
        );
        assert_eq!(
            record_state(&worktree_root, &target),
            Some(WorktreeCustodyStateKindV1::Materializing),
            "a before-effect fault leaves the previous state authoritative"
        );
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// An error-after-effect rename (a network filesystem's retried RPC) must NOT be reported as
    /// "nothing happened". Discriminates a writer built on the raw errno instead of the
    /// identity-classified outcome the PARKED-1 fix introduced: the replacement really landed, so
    /// treating it as un-happened would leave the caller believing a stale state is current.
    #[test]
    fn an_error_after_effect_rename_is_reported_durable_not_failed() {
        let worktree_root = root("after-effect");
        let target = worktree_root.join("ownr-run7-abc");
        let custodian =
            WorktreeCustodianV1::enter(&worktree_root, &target.to_string_lossy(), binding(&target))
                .unwrap();
        custodian.publish_protection_prepared().unwrap();
        custodian
            .pinned_root()
            .fail_publication_rename_on_nth_call_for_test(1, PublicationRenameFaultV1::AfterEffect);

        custodian
            .replace_materializing()
            .expect("a rename that took effect is durable, whatever the syscall reported");

        assert_eq!(
            record_state(&worktree_root, &target),
            Some(WorktreeCustodyStateKindV1::Materializing)
        );
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// An undecidable rename is protective: the caller is told the outcome is unverified rather
    /// than being handed either "it landed" or "nothing happened".
    ///
    /// RENAMED in the 2b2 repair round (opus W-4a): the old name claimed this kept its residue,
    /// which it cannot — the injected `UnlinkSourceOnly` fault removes the source by definition,
    /// so there is nothing left to keep. What this test actually discriminates is a writer that
    /// collapses `RenameOutcomeUnverified` into either a clean `Err` (caller concludes the record
    /// was not written, and may keep treating a superseded state as current) or an `Ok`.
    #[test]
    fn an_undecidable_rename_refuses_ambiguously_without_claiming_an_effect() {
        let worktree_root = root("undecidable");
        let target = worktree_root.join("ownr-run7-abc");
        let custodian =
            WorktreeCustodianV1::enter(&worktree_root, &target.to_string_lossy(), binding(&target))
                .unwrap();
        custodian
            .pinned_root()
            .fail_publication_rename_on_nth_call_for_test(
                1,
                PublicationRenameFaultV1::UnlinkSourceOnly,
            );

        let outcome = custodian.publish_protection_prepared();

        assert!(
            matches!(outcome, Err(CustodyWriteRefusalV1::Ambiguous(_))),
            "an unverifiable rename must be ambiguous, not a clean failure: {outcome:?}"
        );
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// The custody cells really are held for the custodian's lifetime, and in the declared order.
    /// Discriminates a writer that takes the cells and drops them before publishing (leaving the
    /// probe→publish window the S7 gate wiring exists to close) and a field order whose `Drop`
    /// releases the outer cell first.
    #[test]
    fn both_cells_are_held_for_the_custodians_lifetime_and_released_on_drop() {
        let worktree_root = root("cells");
        let target = worktree_root.join("ownr-run7-abc");
        let held = binding(&target);
        let custodian =
            WorktreeCustodianV1::enter(&worktree_root, &target.to_string_lossy(), held.clone())
                .unwrap();

        assert!(
            crate::custody_lock::try_acquire_publication_lock_in(
                &worktree_root,
                &target.to_string_lossy()
            )
            .is_err(),
            "the publication cell must be held while a custodian exists"
        );
        assert!(
            crate::custody_lock::try_acquire_custody_lock_in(&worktree_root, &held.plan.custody_id)
                .is_err(),
            "the custody cell must be held while a custodian exists"
        );

        drop(custodian);
        assert!(crate::custody_lock::try_acquire_publication_lock_in(
            &worktree_root,
            &target.to_string_lossy()
        )
        .is_ok());
        assert!(crate::custody_lock::try_acquire_custody_lock_in(
            &worktree_root,
            &held.plan.custody_id
        )
        .is_ok());
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// `PreservationUnknown` carries the claim its own state rule REQUIRES, with the recovery
    /// locator the provider probe produced. Discriminates a writer that publishes the state with
    /// no claim (the decoder would refuse, so this would be a hard failure rather than a silent
    /// one) and one that hardcodes a locator instead of carrying the probe's answer — which is
    /// how `RegistrationUnproven` becomes unreachable in production.
    #[test]
    fn preservation_unknown_carries_its_required_claim_and_the_probed_locator() {
        let worktree_root = root("preservation-unknown");
        let target = worktree_root.join("ownr-run7-abc");
        std::fs::create_dir_all(&target).unwrap();
        let custodian =
            WorktreeCustodianV1::enter(&worktree_root, &target.to_string_lossy(), binding(&target))
                .unwrap();
        custodian.publish_protection_prepared().unwrap();
        custodian.replace_materializing().unwrap();

        custodian
            .replace_preservation_unknown(
                PreservationReasonV1::MaterializationInFlight,
                &identities(&target),
                RecoveryLocatorV1::RegistrationUnproven {},
                1_700_000_000_000,
            )
            .unwrap();

        let pinned = PinnedDirectoryV1::open(&worktree_root, "test").unwrap();
        let name: OsString = Path::new(&custody_record_path(&target.to_string_lossy()))
            .file_name()
            .unwrap()
            .to_os_string();
        let record = read_custody_record_in(&pinned, &name).unwrap();
        let claim = record.claim.clone().expect("this state requires a claim");
        assert_eq!(claim.reason, PreservationReasonV1::MaterializationInFlight);
        assert_eq!(
            claim.recovery_locator,
            RecoveryLocatorV1::RegistrationUnproven {}
        );
        assert_eq!(claim.custody_id, record.custody_id);
        assert!(
            !record.sweep_disposition().authorizes_checkout_removal(),
            "no custody state this writer publishes may authorize a removal"
        );
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    // ---- slice 2b2 repair R3: residue identity safety and honest policy ----

    /// R3's red test — the identity hazard, reproduced deterministically.
    ///
    /// The shipped durable arm did `remove_file(staged_path)` on the reasoning that a committed
    /// rename leaves the source name free, so the unlink is a harmless no-op. It is a no-op in
    /// exactly the case where it does nothing, and in the ONLY case where it does something —
    /// another actor created a file at that name after the rename — it deletes a foreign object
    /// whose identity was never checked. Here that actor is the test: the file at the staging
    /// name is emphatically not ours, and it must survive.
    ///
    /// Discriminates the shipped code precisely: re-add the unlink and this goes red.
    #[test]
    fn a_durable_publication_never_unlinks_the_staging_pathname() {
        let worktree_root = root("durable-no-unlink");
        let target = worktree_root.join("ownr-run7-abc");
        let custodian =
            WorktreeCustodianV1::enter(&worktree_root, &target.to_string_lossy(), binding(&target))
                .unwrap();
        // Someone else's file, sitting at the name our staging would have used.
        let foreign = worktree_root.join(format!(
            "ownr-run7-abc{CUSTODY_RECORD_SUFFIX}.staging-{}",
            "a".repeat(STAGING_NONCE_HEX_LEN)
        ));
        std::fs::write(&foreign, b"another actor's bytes").unwrap();

        custodian.settle_residue(
            &foreign,
            &CustodyPublicationV1::Durable {
                retried_rename: None,
            },
        );

        assert!(
            foreign.exists(),
            "a durable publication must not unlink whatever occupies the staging name"
        );
        assert_eq!(
            std::fs::read(&foreign).unwrap(),
            b"another actor's bytes",
            "and must certainly not have replaced its contents"
        );
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// The same rule for the ambiguous arm that can actually leave residue: an unverified rename
    /// whose source name is occupied by an object we cannot prove is ours. Leaving it is not
    /// merely conservative here, it is mandatory — unlinking would destroy a foreign file.
    #[test]
    fn an_unverified_rename_never_unlinks_what_occupies_the_staging_name() {
        let worktree_root = root("unverified-no-unlink");
        let target = worktree_root.join("ownr-run7-abc");
        let custodian =
            WorktreeCustodianV1::enter(&worktree_root, &target.to_string_lossy(), binding(&target))
                .unwrap();
        let foreign = worktree_root.join(format!(
            "ownr-run7-abc{CUSTODY_RECORD_SUFFIX}.staging-{}",
            "b".repeat(STAGING_NONCE_HEX_LEN)
        ));
        std::fs::write(&foreign, b"not ours either").unwrap();

        for outcome in [
            CustodyPublicationV1::RenameOutcomeUnverified("undecidable".into()),
            CustodyPublicationV1::ParentSyncAmbiguous("sync".into()),
            CustodyPublicationV1::TargetIdentityUnverified("identity".into()),
        ] {
            custodian.settle_residue(&foreign, &outcome);
            assert!(
                foreign.exists(),
                "no publication arm may unlink the staging name: {outcome:?}"
            );
        }
        assert_eq!(std::fs::read(&foreign).unwrap(), b"not ours either");
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// Residue recognition is EXACT (sol W-7). The storage report labels whatever this accepts as
    /// bridge-owned `Evidence`, so an over-permissive predicate would present an operator's own
    /// file as bridge residue. Discriminates the shipped `is_ascii_hexdigit` + any-length check,
    /// which accepted uppercase and every wrong length.
    #[test]
    fn staged_residue_recognition_requires_exactly_thirty_two_lowercase_hex() {
        let stem = format!("wt{CUSTODY_RECORD_SUFFIX}.staging-");
        let ok = format!("{stem}{}", "0123456789abcdef".repeat(2));
        assert!(is_staged_custody_residue(&ok));
        assert!(is_staged_custody_residue(&format!("/wt-root/{ok}")));

        for bad in [
            format!("{stem}{}", "a".repeat(STAGING_NONCE_HEX_LEN - 1)), // short
            format!("{stem}{}", "a".repeat(STAGING_NONCE_HEX_LEN + 1)), // long
            format!("{stem}{}", "A".repeat(STAGING_NONCE_HEX_LEN)),     // uppercase
            format!("{stem}{}", "g".repeat(STAGING_NONCE_HEX_LEN)),     // non-hex
            stem.clone(),                                               // no nonce
            format!(
                "{CUSTODY_RECORD_SUFFIX}.staging-{}",
                "a".repeat(STAGING_NONCE_HEX_LEN)
            ), // no stem
            "wt.custody.v1.json".to_string(),                           // a real record
            "wt.meta.json".to_string(),                                 // a legacy sidecar
        ] {
            assert!(
                !is_staged_custody_residue(&bad),
                "must not be recognised as residue: {bad}"
            );
        }
    }

    // ---- slice 2c1: fail-closed preservation (P1, P7, §5.7 rows 5 and 12) ----

    /// Four FULLY OBSERVED identities. The shared `identities()` fixture leaves `common_dir`
    /// plan-derived, which is legal for `LiveProtected` (only the envelope's `worktree` is checked
    /// there) but is refused for every preserving state: 2a's `identity_completeness` requires
    /// observed `dev`/`ino` on all four claim identities, and the writer is checked by the reader's
    /// own rule. That refusal is correct and was the first thing these tests found.
    fn complete_identities(worktree_root: &Path, target: &Path) -> MaterializedIdentitiesV1 {
        let source = worktree_root.join("src");
        let common = source.join(".git");
        std::fs::create_dir_all(&common).unwrap();
        MaterializedIdentitiesV1 {
            source: observed_identity(&source.to_string_lossy()),
            root: observed_identity(&worktree_root.to_string_lossy()),
            worktree: observed_identity(&target.to_string_lossy()),
            common_dir: observed_identity(&common.to_string_lossy()),
        }
    }

    /// Drive a checkout to `LiveProtected` the way the writer really does, then hand back a FRESH
    /// custodian so a preservation test starts its own fault countdown at 1.
    fn live_protected(
        worktree_root: &Path,
        target: &Path,
    ) -> (WorktreeCustodianV1, MaterializedIdentitiesV1) {
        std::fs::create_dir_all(target).unwrap();
        let bound = binding(target);
        let custodian =
            WorktreeCustodianV1::enter(worktree_root, &target.to_string_lossy(), bound.clone())
                .unwrap();
        custodian.publish_protection_prepared().unwrap();
        custodian.replace_materializing().unwrap();
        let identities = complete_identities(worktree_root, target);
        custodian.replace_live_protected(&identities).unwrap();
        drop(custodian);
        let custodian =
            WorktreeCustodianV1::enter(worktree_root, &target.to_string_lossy(), bound).unwrap();
        (custodian, identities)
    }

    /// P1's headline: a live checkout settles `Preserved` with the claim R2f2 disposes of, and the
    /// claim carries the trigger reason rather than a writer-chosen one.
    ///
    /// Discriminates a driver that settles `PreservationUnknown` for a perfectly verifiable
    /// checkout (which would strand recoverable work in the unknown bucket) and one that publishes
    /// a state carrying no claim.
    #[test]
    fn a_live_checkout_settles_preserved_with_its_required_claim() {
        let worktree_root = root("preserve-live");
        let target = worktree_root.join("ownr-run7-abc");
        let (custodian, identities) = live_protected(&worktree_root, &target);

        let outcome = custodian.preserve_after_cancel(
            PreservationReasonV1::NodeFailure,
            &identities,
            RecoveryLocatorV1::RegisteredWorktree {},
            1_700_000_000_000,
        );

        assert_eq!(outcome, PreservationOutcomeV1::Preserved);
        assert!(outcome.is_protective());
        assert_eq!(
            record_state(&worktree_root, &target),
            Some(WorktreeCustodyStateKindV1::Preserved)
        );
        let pinned = PinnedDirectoryV1::open(&worktree_root, "test").unwrap();
        let name: OsString = Path::new(&custody_record_path(&target.to_string_lossy()))
            .file_name()
            .unwrap()
            .to_os_string();
        let record = read_custody_record_in(&pinned, &name).unwrap();
        let claim = record.claim.clone().expect("Preserved requires a claim");
        assert_eq!(claim.reason, PreservationReasonV1::NodeFailure);
        assert_eq!(claim.source, identities.source);
        assert_eq!(claim.common_dir, identities.common_dir);
        assert!(!record.sweep_disposition().authorizes_checkout_removal());
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// The EDGE ORDER, pinned by injection rather than by reading the code: arm the SECOND rename
    /// of the preservation sequence to fail before taking effect. If the driver publishes
    /// `PreservationPrepared` first (§5.1 steps 3 then 4), rename 1 lands it and rename 2 — the
    /// terminal replace — is the armed one, so the record is left `PreservationPrepared`.
    ///
    /// Discriminates a driver that shortcuts `LiveProtected -> Preserved`: rename 1 would then BE
    /// the terminal publication and would succeed, leaving `preserved` on disk. It also
    /// discriminates a driver that publishes an edge 2a's frozen table does not contain, since
    /// `LiveProtected -> PreservationUnknown` is asserted illegal below.
    #[test]
    fn preservation_publishes_prepared_before_its_terminal_state() {
        let worktree_root = root("preserve-edge-order");
        let target = worktree_root.join("ownr-run7-abc");
        let (custodian, identities) = live_protected(&worktree_root, &target);
        custodian
            .pinned_root()
            .fail_publication_rename_on_nth_call_for_test(
                2,
                PublicationRenameFaultV1::BeforeEffect,
            );

        let outcome = custodian.preserve_after_cancel(
            PreservationReasonV1::Cancellation,
            &identities,
            RecoveryLocatorV1::RegisteredWorktree {},
            1_700_000_000_000,
        );

        assert!(
            matches!(outcome, PreservationOutcomeV1::Refused(_)),
            "a rename that provably did not happen is a refusal, not an ambiguity: {outcome:?}"
        );
        assert_eq!(
            record_state(&worktree_root, &target),
            Some(WorktreeCustodyStateKindV1::PreservationPrepared),
            "rename 1 must have been the PreservationPrepared replace"
        );
        assert!(
            !crate::custody::transition_is_legal(
                WorktreeCustodyStateKindV1::LiveProtected,
                WorktreeCustodyStateKindV1::PreservationUnknown
            ),
            "the shortcut this test forbids is not a legal edge either"
        );
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// §5.7 row 5 — "claim renamed, parent sync ambiguous: prior prepared state or ambiguous claim
    /// remains protective; report unknown."
    ///
    /// Discriminates a driver that reads a non-`Durable` publication as success and marches on to
    /// the terminal replace (it would publish an edge from a state it cannot know it is in), and
    /// one that reads it as "nothing happened" and reports a plain refusal while a claim is in
    /// fact on disk.
    #[test]
    fn claim_renamed_with_ambiguous_parent_sync_stays_protective() {
        let worktree_root = root("preserve-row5");
        let target = worktree_root.join("ownr-run7-abc");
        let (custodian, identities) = live_protected(&worktree_root, &target);
        custodian.pinned_root().fail_sync_on_nth_call_for_test(1);

        let outcome = custodian.preserve_after_cancel(
            PreservationReasonV1::Cancellation,
            &identities,
            RecoveryLocatorV1::RegisteredWorktree {},
            1_700_000_000_000,
        );

        assert!(
            matches!(outcome, PreservationOutcomeV1::Ambiguous(_)),
            "unexpected outcome: {outcome:?}"
        );
        assert!(outcome.is_protective());
        assert!(
            !outcome.is_terminal_preservation(),
            "an ambiguous transition must not be projected as a settled preservation"
        );
        let state = record_state(&worktree_root, &target);
        assert_eq!(
            state,
            Some(WorktreeCustodyStateKindV1::PreservationPrepared),
            "the rename DID commit; the claim is on disk and protective"
        );
        assert!(!state
            .expect("state read above")
            .sweep_disposition()
            .authorizes_checkout_removal());
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// §5.7 row 12 — "crash after preserved terminal: no automatic provider replay; claim awaits
    /// R2f2." Re-running the barrier over a terminal record is a NO-OP, byte for byte.
    ///
    /// Discriminates a driver that re-publishes on every request: `Preserved` has no outgoing edge
    /// in 2a's table, and a re-publication would both rewrite `created_wall_ms` (destroying the
    /// only record of when the work was preserved) and re-run a rename over a settled claim.
    #[test]
    fn preserved_claim_awaits_r2f2_with_no_provider_replay() {
        let worktree_root = root("preserve-row12");
        let target = worktree_root.join("ownr-run7-abc");
        let (custodian, identities) = live_protected(&worktree_root, &target);
        custodian
            .preserve_after_cancel(
                PreservationReasonV1::NodeFailure,
                &identities,
                RecoveryLocatorV1::RegisteredWorktree {},
                1_700_000_000_000,
            )
            .is_protective()
            .then_some(())
            .expect("the first preservation settles");
        let settled = std::fs::read(custody_record_path(&target.to_string_lossy())).unwrap();

        let again = custodian.preserve_after_cancel(
            PreservationReasonV1::Cancellation,
            &identities,
            RecoveryLocatorV1::RegisteredWorktree {},
            1_800_000_000_000,
        );

        assert_eq!(again, PreservationOutcomeV1::AlreadyPreserved);
        assert!(again.is_terminal_preservation());
        assert_eq!(
            std::fs::read(custody_record_path(&target.to_string_lossy())).unwrap(),
            settled,
            "a terminal claim is not rewritten, so its wall clock and reason survive"
        );
        assert!(WorktreeCustodyStateKindV1::Preserved.is_terminal());
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// P7 (2b2 opus S-9 / sol S-3) — the RED case. Swap the source directory for a same-named
    /// replacement after materialization: preservation must refuse to claim the replacement.
    ///
    /// Discriminates the shipped 2b2 behaviour exactly. Without retained identities the only thing
    /// a claim could carry is a fresh `observed_identity(...)` of each path, which would happily
    /// record the REPLACEMENT's `dev`/`ino` and assert it is the object this custody protected.
    /// Here the claim must instead carry the retained identity and the state must be
    /// `PreservationUnknown`, so an R2f2 consumer sees "we cannot vouch for this" rather than a
    /// confident claim about the wrong object.
    #[test]
    fn preservation_refuses_to_claim_a_swapped_source_and_settles_protective() {
        let worktree_root = root("preserve-swap");
        let target = worktree_root.join("ownr-run7-abc");
        let source = worktree_root.join("src");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        let bound = binding(&target);
        let custodian =
            WorktreeCustodianV1::enter(&worktree_root, &target.to_string_lossy(), bound.clone())
                .unwrap();
        custodian.publish_protection_prepared().unwrap();
        custodian.replace_materializing().unwrap();
        let common = worktree_root.join("common");
        std::fs::create_dir_all(&common).unwrap();
        let retained = MaterializedIdentitiesV1 {
            source: observed_identity(&source.to_string_lossy()),
            root: observed_identity(&worktree_root.to_string_lossy()),
            worktree: observed_identity(&target.to_string_lossy()),
            common_dir: observed_identity(&common.to_string_lossy()),
        };
        custodian.replace_live_protected(&retained).unwrap();
        drop(custodian);
        // Same NAME, different object — the substitution a path-based check cannot see.
        std::fs::remove_dir_all(&source).unwrap();
        std::fs::create_dir_all(&source).unwrap();
        let replacement = observed_identity(&source.to_string_lossy());
        assert_ne!(replacement, retained.source, "the swap must change dev/ino");
        let custodian =
            WorktreeCustodianV1::enter(&worktree_root, &target.to_string_lossy(), bound).unwrap();

        let outcome = custodian.preserve_after_cancel(
            PreservationReasonV1::NodeFailure,
            &retained,
            RecoveryLocatorV1::RegisteredWorktree {},
            1_700_000_000_000,
        );

        assert_eq!(
            outcome,
            PreservationOutcomeV1::PreservationUnknown(PreservationReasonV1::AmbiguousCleanup)
        );
        assert_eq!(
            record_state(&worktree_root, &target),
            Some(WorktreeCustodyStateKindV1::PreservationUnknown)
        );
        let pinned = PinnedDirectoryV1::open(&worktree_root, "test").unwrap();
        let name: OsString = Path::new(&custody_record_path(&target.to_string_lossy()))
            .file_name()
            .unwrap()
            .to_os_string();
        let claim = read_custody_record_in(&pinned, &name)
            .unwrap()
            .claim
            .expect("PreservationUnknown requires a claim");
        assert_eq!(
            claim.source, retained.source,
            "the claim records what we materialized, never the replacement"
        );
        assert_ne!(claim.source, replacement);
        assert_eq!(
            claim.recovery_locator,
            RecoveryLocatorV1::RegistrationUnproven {},
            "repair RB: a failed reverification must DOWNGRADE the locator — the caller passed \
             `RegisteredWorktree`, and keeping it would durably assert registration of an object \
             graph we just proved we cannot recognise"
        );
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// Repair RB, the VANISH arm — the same downgrade when an object is gone rather than swapped.
    ///
    /// Split from the swap test deliberately: a swapped object still produces a complete
    /// re-observation (it just names a different inode), while a vanished one produces the degraded
    /// shape, and those travel different branches of `identities_reverify`. Both must downgrade.
    #[test]
    fn a_vanished_object_also_downgrades_the_claims_recovery_locator() {
        let worktree_root = root("preserve-vanish-locator");
        let target = worktree_root.join("ownr-run7-abc");
        let (custodian, retained) = live_protected(&worktree_root, &target);
        // The common dir is one of the four reverified objects, and it is exactly the object the
        // locator makes a claim about.
        std::fs::remove_dir_all(&retained.common_dir.canonical_path).unwrap();

        let outcome = custodian.preserve_after_cancel(
            PreservationReasonV1::Cancellation,
            &retained,
            RecoveryLocatorV1::RegisteredWorktree {},
            1_700_000_000_000,
        );

        assert_eq!(
            outcome,
            PreservationOutcomeV1::PreservationUnknown(PreservationReasonV1::AmbiguousCleanup)
        );
        let pinned = PinnedDirectoryV1::open(&worktree_root, "test").unwrap();
        let name: OsString = Path::new(&custody_record_path(&target.to_string_lossy()))
            .file_name()
            .unwrap()
            .to_os_string();
        let claim = read_custody_record_in(&pinned, &name)
            .unwrap()
            .claim
            .expect("PreservationUnknown requires a claim");
        assert_eq!(
            claim.recovery_locator,
            RecoveryLocatorV1::RegistrationUnproven {}
        );
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// Strand a record in `PreservationPrepared` the way a crash between the two renames does:
    /// let the prepared publication land and fail the terminal one before it takes effect.
    fn stranded_prepared(
        worktree_root: &Path,
        target: &Path,
    ) -> (WorktreeCustodianV1, MaterializedIdentitiesV1) {
        let (custodian, identities) = live_protected(worktree_root, target);
        custodian
            .pinned_root()
            .fail_publication_rename_on_nth_call_for_test(
                2,
                PublicationRenameFaultV1::BeforeEffect,
            );
        let stranded = custodian.preserve_after_cancel(
            PreservationReasonV1::NodeFailure,
            &identities,
            RecoveryLocatorV1::RegisteredWorktree {},
            1_700_000_000_000,
        );
        assert!(
            matches!(stranded, PreservationOutcomeV1::Refused(_)),
            "the setup must strand, not settle: {stranded:?}"
        );
        assert_eq!(
            record_state(worktree_root, target),
            Some(WorktreeCustodyStateKindV1::PreservationPrepared)
        );
        drop(custodian);
        let custodian =
            WorktreeCustodianV1::enter(worktree_root, &target.to_string_lossy(), binding(target))
                .unwrap();
        (custodian, identities)
    }

    /// Repair RA (opus W2 / sol B-1) — a stranded `PreservationPrepared` record RESUMES to its
    /// terminal state, and resumes in ONE rename rather than re-publishing the prepared edge.
    ///
    /// The rename count is the discriminator, and it is why the fault is armed at call 2 rather
    /// than call 1. A writer that resumes directly performs exactly one rename (the terminal
    /// replace), so the armed call-2 fault never fires and the record settles `Preserved`. A writer
    /// that re-publishes `PreservationPrepared` first performs two, so call 2 IS its terminal
    /// replace, the fault fires, and the record is left stranded again — an infinite loop of
    /// prepared re-publications, and a `PreservationPrepared -> PreservationPrepared` self-loop
    /// that 2a's frozen table does not contain.
    ///
    /// Before this repair the barrier refused this state outright with "no legal preservation edge
    /// from preservation_prepared", which both stranded the checkout permanently and left the two
    /// real outgoing edges of the frozen table with zero producers.
    #[test]
    fn a_stranded_prepared_record_resumes_to_exactly_one_terminal_state() {
        let worktree_root = root("preserve-resume");
        let target = worktree_root.join("ownr-run7-abc");
        let (custodian, identities) = stranded_prepared(&worktree_root, &target);
        custodian
            .pinned_root()
            .fail_publication_rename_on_nth_call_for_test(
                2,
                PublicationRenameFaultV1::BeforeEffect,
            );

        let resumed = custodian.preserve_after_cancel(
            PreservationReasonV1::NodeFailure,
            &identities,
            RecoveryLocatorV1::RegisteredWorktree {},
            1_700_000_000_000,
        );

        assert_eq!(
            resumed,
            PreservationOutcomeV1::Preserved,
            "the resume must settle, and in one rename: the call-2 fault must never fire"
        );
        assert_eq!(
            record_state(&worktree_root, &target),
            Some(WorktreeCustodyStateKindV1::Preserved)
        );
        assert!(crate::custody::transition_is_legal(
            WorktreeCustodyStateKindV1::PreservationPrepared,
            WorktreeCustodyStateKindV1::Preserved
        ));
        assert!(
            !crate::custody::transition_is_legal(
                WorktreeCustodyStateKindV1::PreservationPrepared,
                WorktreeCustodyStateKindV1::PreservationPrepared
            ),
            "the edge a re-publishing resume would take does not exist"
        );
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// Repair RA's second half: the resume RE-DERIVES `verified` from the live objects instead of
    /// trusting the stranded record's claim.
    ///
    /// The stranded claim was minted while the object graph still verified. If the resume read its
    /// identities back — the obvious shortcut, since the record is right there and already carries
    /// a complete claim — a substitution performed after the strand would be laundered into a
    /// confident terminal `Preserved` claim over the replacement. That is P7's hazard, reachable
    /// through a path P7's own test does not cover.
    #[test]
    fn a_resume_reverifies_the_live_objects_and_never_trusts_the_stranded_claim() {
        let worktree_root = root("preserve-resume-reverify");
        let target = worktree_root.join("ownr-run7-abc");
        let (custodian, identities) = stranded_prepared(&worktree_root, &target);
        // The strand's claim already asserts these identities; swap one AFTER it was written.
        let source = &identities.source.canonical_path;
        std::fs::remove_dir_all(source).unwrap();
        std::fs::create_dir_all(source).unwrap();

        let resumed = custodian.preserve_after_cancel(
            PreservationReasonV1::NodeFailure,
            &identities,
            RecoveryLocatorV1::RegisteredWorktree {},
            1_700_000_000_000,
        );

        assert_eq!(
            resumed,
            PreservationOutcomeV1::PreservationUnknown(PreservationReasonV1::AmbiguousCleanup),
            "the resume must reverify live objects, not read the stranded claim back"
        );
        let pinned = PinnedDirectoryV1::open(&worktree_root, "test").unwrap();
        let name: OsString = Path::new(&custody_record_path(&target.to_string_lossy()))
            .file_name()
            .unwrap()
            .to_os_string();
        let claim = read_custody_record_in(&pinned, &name)
            .unwrap()
            .claim
            .expect("PreservationUnknown requires a claim");
        assert_eq!(
            claim.source, identities.source,
            "still never the replacement"
        );
        assert_eq!(
            claim.recovery_locator,
            RecoveryLocatorV1::RegistrationUnproven {}
        );
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// The positive control for the reverification predicate itself: an untouched object graph
    /// verifies, and a single swapped member is enough to fail it. Without this, the test above
    /// could pass against a predicate that is simply always false. The replacement is pre-created
    /// so two live objects guarantee distinct inodes before the rename swap.
    #[test]
    fn identity_reverification_passes_untouched_objects_and_fails_one_swap() {
        let worktree_root = root("preserve-reverify");
        let target = worktree_root.join("ownr-run7-abc");
        std::fs::create_dir_all(&target).unwrap();
        let retained = complete_identities(&worktree_root, &target);

        assert!(WorktreeCustodianV1::identities_reverify(&retained));

        let _displaced = replace_directory_with_precreated_sibling(&target);
        assert!(
            !WorktreeCustodianV1::identities_reverify(&retained),
            "a same-name replacement must not reverify"
        );
        std::fs::remove_dir_all(&target).unwrap();
        assert!(
            !WorktreeCustodianV1::identities_reverify(&retained),
            "a VANISHED object must not reverify either: the degraded re-observation falls back \
             to the plan-derived shape, which must never match a complete retained identity"
        );
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// No preservation edge exists from `Materializing`, so the barrier refuses rather than
    /// inventing one. Discriminates a driver that treats "any non-terminal record" as
    /// preservable, which would publish an edge outside 2a's frozen table and make the
    /// add-failure arm's `Materializing -> PreservationUnknown` race a second producer.
    #[test]
    fn preservation_refuses_a_from_state_with_no_legal_edge() {
        let worktree_root = root("preserve-illegal-edge");
        let target = worktree_root.join("ownr-run7-abc");
        std::fs::create_dir_all(&target).unwrap();
        let custodian =
            WorktreeCustodianV1::enter(&worktree_root, &target.to_string_lossy(), binding(&target))
                .unwrap();
        custodian.publish_protection_prepared().unwrap();
        custodian.replace_materializing().unwrap();

        let outcome = custodian.preserve_after_cancel(
            PreservationReasonV1::NodeFailure,
            &complete_identities(&worktree_root, &target),
            RecoveryLocatorV1::RegisteredWorktree {},
            1_700_000_000_000,
        );

        assert!(
            matches!(outcome, PreservationOutcomeV1::Refused(_)),
            "unexpected outcome: {outcome:?}"
        );
        assert_eq!(
            record_state(&worktree_root, &target),
            Some(WorktreeCustodyStateKindV1::Materializing),
            "a refused barrier writes nothing"
        );
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    // ---- slice 2c2: the deletion capability (P1, P2, P7) ----

    /// P1's headline: the `LiveProtected -> DeleteAuthorized` CAS, and the capability it mints.
    ///
    /// Also pins the two things that make the capability a capability rather than a struct: it
    /// names the exact checkout it authorizes (so it can never be pointed at another), and the
    /// record it leaves behind is `DeleteAuthorized` — a state 2a classifies `Recover`, so a crash
    /// here is recovery-owned rather than deletable.
    ///
    /// Discriminates a mint that publishes `Removed` directly (which would tombstone a checkout
    /// still on disk) and one that authorizes without transitioning (which would leave nothing
    /// durable for recovery to find).
    #[test]
    fn a_foreign_live_record_never_authorizes_deletion() {
        let worktree_root = root("authorize-foreign-record");
        let target = worktree_root.join("ownr-run7-abc");

        let (owner, identities) = live_protected(&worktree_root, &target);
        let original = std::fs::read(custody_record_path(&target.to_string_lossy())).unwrap();
        drop(owner);

        let foreign =
            WorktreeCustodianV1::enter(&worktree_root, &target.to_string_lossy(), binding(&target))
                .unwrap();
        let authorization = foreign.authorize_deletion("/src", &identities);

        assert!(
            matches!(authorization, DeletionAuthorizationV1::Refused(_)),
            "a foreign record must not be claimed: {authorization:?}"
        );
        assert_eq!(
            record_state(&worktree_root, &target),
            Some(WorktreeCustodyStateKindV1::LiveProtected),
            "the foreign record remains protective"
        );
        assert_eq!(
            std::fs::read(custody_record_path(&target.to_string_lossy())).unwrap(),
            original,
            "refusal is effect-free"
        );
        drop(foreign);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    #[test]
    fn a_live_checkout_authorizes_deletion_and_mints_its_capability() {
        let worktree_root = root("authorize-live");
        let target = worktree_root.join("ownr-run7-abc");
        let (custodian, identities) = live_protected(&worktree_root, &target);

        let authorization = custodian.authorize_deletion("/src", &identities);

        assert!(authorization.is_authorized(), "{authorization:?}");
        let DeletionAuthorizationV1::Authorized(capability) = authorization else {
            unreachable!("asserted authorized above")
        };
        assert_eq!(capability.worktree_path(), target.to_string_lossy());
        assert_eq!(capability.canonical_source(), "/src");
        assert_eq!(
            capability.custody_id().as_str(),
            custodian.custody_id().as_str()
        );
        assert_eq!(
            record_state(&worktree_root, &target),
            Some(WorktreeCustodyStateKindV1::DeleteAuthorized)
        );
        assert_eq!(
            WorktreeCustodyStateKindV1::DeleteAuthorized.sweep_disposition(),
            crate::custody::CustodySweepDispositionV1::Recover,
            "a crash after authorization is recovery-owned, never sweep-deletable"
        );
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// §5.1's monotonicity on the DURABLE side, and the reason it is a refusal in the writer as
    /// well as an `Ord` in the backend: "once a preserved claim exists ... no later healthy
    /// projection or TTL can mint deletion authority."
    ///
    /// The terminal and successor-owned non-`LiveProtected` states are driven, because they are
    /// distinct disk states and a mint that checked only `Preserved` would authorize deletion
    /// over a stranded `PreservationPrepared` or `RecoveredLive` claim exchange.
    ///
    /// Discriminates a mint that keys on "not already removed" rather than on the frozen table's
    /// single `LiveProtected -> DeleteAuthorized` edge.
    #[test]
    fn a_protected_non_live_checkout_can_never_be_authorized_for_deletion() {
        for name in ["preserved", "prepared", "recovered"] {
            let worktree_root = root(&format!("authorize-refuses-{name}"));
            let target = worktree_root.join("ownr-run7-abc");
            let (custodian, identities) = live_protected(&worktree_root, &target);
            match name {
                "preserved" => {
                    let outcome = custodian.preserve_after_cancel(
                        PreservationReasonV1::NodeFailure,
                        &identities,
                        RecoveryLocatorV1::RegisteredWorktree {},
                        1_700_000_000_000,
                    );
                    assert_eq!(outcome, PreservationOutcomeV1::Preserved);
                }
                "prepared" => {
                    custodian
                        .replace_preservation_prepared(
                            PreservationReasonV1::NodeFailure,
                            &identities,
                            RecoveryLocatorV1::RegisteredWorktree {},
                            1_700_000_000_000,
                        )
                        .unwrap();
                }
                "recovered" => {
                    let record =
                        custodian
                            .record(
                                WorktreeCustodyStateV1::RecoveredLive {
                                    predecessor_claim_digest:
                                        bridge_core::execution_policy::Sha256HexV1::digest(
                                            b"recovered",
                                        ),
                                },
                                Some(identities.worktree.clone()),
                                None,
                            )
                            .unwrap();
                    custodian
                        .stage_and_settle(&record, PublicationModeV1::Replace)
                        .unwrap();
                }
                _ => unreachable!(),
            }
            let before = record_state(&worktree_root, &target);

            let authorization = custodian.authorize_deletion("/src", &identities);

            assert!(
                matches!(authorization, DeletionAuthorizationV1::Refused(_)),
                "a protected non-live record must never mint deletion authority ({name}): {authorization:?}"
            );
            assert_eq!(
                record_state(&worktree_root, &target),
                before,
                "a refused mint writes nothing ({name})"
            );
            drop(custodian);
            std::fs::remove_dir_all(&worktree_root).unwrap();
        }
    }

    /// P7 boundary 1, the writer half: **no re-mint from a stale capability.** A crash between the
    /// CAS and the removal leaves `DeleteAuthorized` on disk; a second authorization attempt must
    /// refuse, so the checkout stays recovery-owned rather than acquiring a fresh licence to be
    /// deleted by whoever asks next.
    ///
    /// Discriminates a mint whose from-state check is "anything but a preservation".
    #[test]
    fn an_already_authorized_record_refuses_a_second_mint() {
        let worktree_root = root("authorize-no-remint");
        let target = worktree_root.join("ownr-run7-abc");
        let (custodian, identities) = live_protected(&worktree_root, &target);
        assert!(custodian
            .authorize_deletion("/src", &identities)
            .is_authorized());

        let second = custodian.authorize_deletion("/src", &identities);

        assert!(
            matches!(second, DeletionAuthorizationV1::Refused(_)),
            "{second:?}"
        );
        assert_eq!(
            record_state(&worktree_root, &target),
            Some(WorktreeCustodyStateKindV1::DeleteAuthorized),
            "and the record is still the recovery-owned one"
        );
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// P1's identity rule at MINT time: a swapped object graph must never be authorized for
    /// deletion. The target is replaced by a different directory at the same path — the exact
    /// substitution `identities_reverify` exists to catch — and the CAS must not run.
    ///
    /// Discriminates a mint that reverifies after the CAS, or not at all: either would leave a
    /// durable `DeleteAuthorized` naming an object graph nobody can vouch for. The replacement is
    /// pre-created so two live objects guarantee distinct inodes before the rename swap.
    #[test]
    fn a_swapped_object_graph_is_never_authorized_for_deletion() {
        let worktree_root = root("authorize-swap");
        let target = worktree_root.join("ownr-run7-abc");
        let (custodian, identities) = live_protected(&worktree_root, &target);
        let _displaced = replace_directory_with_precreated_sibling(&target);

        let authorization = custodian.authorize_deletion("/src", &identities);

        assert!(
            matches!(authorization, DeletionAuthorizationV1::Refused(_)),
            "{authorization:?}"
        );
        assert_eq!(
            record_state(&worktree_root, &target),
            Some(WorktreeCustodyStateKindV1::LiveProtected),
            "a refused mint leaves the checkout live and protected"
        );
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// P2's use-time revalidation, isolated: a capability minted over a graph that is then swapped
    /// cannot be turned into an `AuthorizedRemovalV1`, so `remove_v2` is unreachable for it.
    ///
    /// This is the SECOND identity check, and the test drives the window the mint's own check
    /// cannot cover — the swap happens after the CAS. Discriminates a `revalidate_for_removal`
    /// that merely rewraps the capability. The replacement is pre-created so two live objects
    /// guarantee distinct inodes before the rename swap.
    #[test]
    fn a_capability_whose_objects_changed_cannot_authorize_a_removal() {
        let worktree_root = root("capability-revalidate");
        let target = worktree_root.join("ownr-run7-abc");
        let (custodian, identities) = live_protected(&worktree_root, &target);
        let DeletionAuthorizationV1::Authorized(capability) =
            custodian.authorize_deletion("/src", &identities)
        else {
            panic!("a live checkout authorizes")
        };
        let _displaced = replace_directory_with_precreated_sibling(&target);

        let authorized = capability.revalidate_for_removal();

        assert!(
            authorized.is_err(),
            "a changed object graph must not reach a git removal"
        );
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// The positive control for the two identity tests: an untouched graph revalidates, so the
    /// refusals above are discriminating a real check rather than a permanently-false one.
    #[test]
    fn an_untouched_capability_revalidates_into_an_authorized_removal() {
        let worktree_root = root("capability-revalidate-ok");
        let target = worktree_root.join("ownr-run7-abc");
        let (custodian, identities) = live_protected(&worktree_root, &target);
        let DeletionAuthorizationV1::Authorized(capability) =
            custodian.authorize_deletion("/src", &identities)
        else {
            panic!("a live checkout authorizes")
        };

        let authorized = capability
            .revalidate_for_removal()
            .expect("an untouched object graph revalidates");

        assert_eq!(authorized.worktree_path(), target.to_string_lossy());
        assert_eq!(authorized.canonical_source(), "/src");
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// §5.1's last step and P7 boundary 4: the tombstone is legal ONLY from `DeleteAuthorized`.
    ///
    /// The `Removed` arm records the RETAINED worktree identity, not a fresh observation, because
    /// the object is gone by then and 2a's completeness rule would reject a degraded one — so this
    /// also pins that a tombstone can be published at all after the checkout vanishes.
    ///
    /// Discriminates a `record_removed` that publishes from any state, which would let a
    /// `LiveProtected` checkout acquire a tombstone while its work is still on disk — and a
    /// tombstone is the one custody state whose sweep disposition permits marker reclamation.
    #[test]
    fn the_removal_tombstone_is_legal_only_from_delete_authorized() {
        let worktree_root = root("tombstone-edge");
        let target = worktree_root.join("ownr-run7-abc");
        let (custodian, identities) = live_protected(&worktree_root, &target);

        let too_early = custodian.record_removed(&identities);
        assert!(
            matches!(too_early, RemovalRecordV1::Refused(_)),
            "a live checkout may not be tombstoned: {too_early:?}"
        );
        assert_eq!(
            record_state(&worktree_root, &target),
            Some(WorktreeCustodyStateKindV1::LiveProtected)
        );

        assert!(custodian
            .authorize_deletion("/src", &identities)
            .is_authorized());
        std::fs::remove_dir_all(&target).unwrap();
        assert_eq!(
            custodian.record_removed(&identities),
            RemovalRecordV1::Recorded
        );
        assert_eq!(
            record_state(&worktree_root, &target),
            Some(WorktreeCustodyStateKindV1::Removed)
        );

        let again = custodian.record_removed(&identities);
        assert!(
            matches!(again, RemovalRecordV1::Refused(_)),
            "`Removed` is terminal and has no self-loop: {again:?}"
        );
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// P7 boundary 5: an ambiguous parent sync while recording the tombstone stays protective —
    /// the outcome is reported `Ambiguous`, never folded into `Recorded`.
    ///
    /// The fault is armed on the rename that publishes `Removed`. Both candidate records
    /// (`DeleteAuthorized`, `Removed`) are non-preserving and neither can lose work, but a caller
    /// that read this as a settled tombstone would report a clean removal for a record it cannot
    /// prove landed.
    ///
    /// Discriminates the `From`-style collapse of ambiguity into success.
    #[test]
    fn an_ambiguous_tombstone_publication_stays_ambiguous() {
        let worktree_root = root("tombstone-ambiguous");
        let target = worktree_root.join("ownr-run7-abc");
        let (custodian, identities) = live_protected(&worktree_root, &target);
        assert!(custodian
            .authorize_deletion("/src", &identities)
            .is_authorized());
        // Armed AFTER the authorizing replace, so the next parent sync — the tombstone's — is the
        // one that fails. The rename commits, the parent sync does not: §5.7's "claim renamed,
        // parent sync ambiguous" shape, applied to the tombstone.
        custodian.pinned_root().fail_sync_on_nth_call_for_test(1);

        let recorded = custodian.record_removed(&identities);

        assert!(
            matches!(recorded, RemovalRecordV1::Ambiguous(_)),
            "an unverified tombstone publication must not report as recorded: {recorded:?}"
        );
        drop(custodian);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// The frozen transition table is UNCHANGED by this slice: both edges this slice publishes
    /// were already legal in 2a, and no new one was added.
    ///
    /// Discriminates the non-goal directly — a slice that "needed" a new edge (say
    /// `DeleteAuthorized -> PreservationPrepared` for the git-removal-failure boundary) and quietly
    /// added one instead of parking it.
    #[test]
    fn the_deletion_edges_were_already_legal_and_no_new_edge_was_added() {
        use crate::custody::{transition_is_legal, LEGAL_CUSTODY_TRANSITIONS_V1};
        use WorktreeCustodyStateKindV1 as K;

        assert!(transition_is_legal(K::LiveProtected, K::DeleteAuthorized));
        assert!(transition_is_legal(K::DeleteAuthorized, K::Removed));
        assert_eq!(
            LEGAL_CUSTODY_TRANSITIONS_V1.len(),
            10,
            "2a froze ten edges; this slice adds none"
        );
        // The failure boundaries deliberately have NO escape edge: a failed git removal leaves the
        // record `DeleteAuthorized` (sweep `Recover`), because preserving from there is not an
        // edge the frozen table contains.
        assert!(!transition_is_legal(
            K::DeleteAuthorized,
            K::PreservationPrepared
        ));
        assert!(!transition_is_legal(K::DeleteAuthorized, K::Preserved));
        assert!(!transition_is_legal(K::DeleteAuthorized, K::LiveProtected));
    }

    /// The names this writer really mints satisfy the tightened predicate. Without this, the
    /// exactness test above could pass against a generator that produces something else entirely.
    #[test]
    fn every_minted_staging_name_satisfies_the_exact_predicate() {
        let name = record_file_name("/wt-root/ownr-run7-abc").unwrap();
        for _ in 0..16 {
            let staged = staged_record_name(&name).unwrap();
            assert!(
                is_staged_custody_residue(&staged.to_string_lossy()),
                "minted name must be recognised: {staged:?}"
            );
        }
    }
}
