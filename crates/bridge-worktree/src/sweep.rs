use crate::custody::{
    custody_record_path, CustodyReadRefusalV1, CustodySweepDispositionV1, PreservationReasonV1,
    WorktreeCustodyRecordV1, WorktreeCustodyStateV1, CUSTODY_RECORD_SUFFIX,
};
use crate::provider::{prune_argv, remove_argv};
use crate::provider_path::{canonicalize_lenient, sidecar_path};
use bridge_core::error::BridgeError;
use bridge_core::execution_policy::WorktreeObjectIdentityV1;
#[cfg(unix)]
use bridge_core::fs_custody::BirthTimeV1;
use bridge_core::fs_custody::{verify_payload_directory_identity, DirectoryIdentityV1};
use bridge_core::liveness::LeaseProbe;
use bridge_core::run_identity::{classify, Verdict};
use bridge_core::SessionCwd;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::Path;

mod checked_scan;
mod report;

use checked_scan::{
    CheckedScanCompletedV1, CheckedScanOpenRefusalV1, CheckedScanRowV1, CompatibilityPinOpenerV1,
    FilesystemCompatibilityPinOpenerV1, RootObservationSetV1,
};

pub use report::{
    CannotConstructSubjectV1, ClaimAuthorityObjectV1, ClaimAuthorityUnavailableReasonV1,
    ClaimAuthorityUnavailableV1, CustodyExactAbsenceAssessmentV1, CustodyRecordAssessmentV1,
    CustodyRootObservationV1, CustodyStateSnapshotV1, ExactAbsenceEnumerationV1,
    ExactAbsenceRecordAssessmentV1, ExactAbsenceRootRefusalV1, ExactAbsenceScanStatusV1,
    ExactAbsenceSweepEntryV1, ExactAbsenceSweepReportV1, IneligiblePopulationV1,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExactAbsenceCandidateV1 {
    pub canonical_source: String,
    source_identity: DirectoryIdentityV1,
    common_dir: String,
    common_dir_identity: DirectoryIdentityV1,
    custody_root_identity: Option<DirectoryIdentityV1>,
    pub worktree_path: String,
}
impl ExactAbsenceCandidateV1 {
    pub fn from_legacy(
        source: impl AsRef<str>,
        common_dir: impl AsRef<str>,
        worktree: impl AsRef<str>,
        probe: &dyn ExactAbsenceProbeV1,
    ) -> Result<Self, BridgeError> {
        let source = capture_directory_identity(Path::new(source.as_ref()), "source")?;
        let common_dir =
            capture_directory_identity(Path::new(common_dir.as_ref()), "source common directory")?;
        if !common_dir.matches(&source_common_dir_identity(&source.canonical_path, probe)?) {
            return Err(legacy_claim_authority_error(claim_authority_unavailable(
                ClaimAuthorityObjectV1::SourceCommonDirectoryBinding,
                ClaimAuthorityUnavailableReasonV1::OwnershipUnproven,
            )));
        }
        Self::from_bound(source, common_dir, None, worktree.as_ref())
            .map_err(legacy_claim_authority_error)
    }

    pub(crate) fn from_claim(
        source: &WorktreeObjectIdentityV1,
        root: &WorktreeObjectIdentityV1,
        common_dir: &WorktreeObjectIdentityV1,
        worktree: &WorktreeObjectIdentityV1,
        retained_root: &RetainedCustodyRootAuthorityV1,
        probe: &dyn ExactAbsenceProbeV1,
    ) -> Result<Self, ClaimAuthorityUnavailableV1> {
        for (object, claim) in [
            (ClaimAuthorityObjectV1::Source, source),
            (ClaimAuthorityObjectV1::Root, root),
            (ClaimAuthorityObjectV1::Worktree, worktree),
            (ClaimAuthorityObjectV1::CommonDirectory, common_dir),
        ] {
            if claim.canonical_path != claim.directory_identity.canonical_path {
                return Err(claim_authority_unavailable(
                    object,
                    ClaimAuthorityUnavailableReasonV1::PathMismatch,
                ));
            }
        }
        for (object, claim) in [
            (ClaimAuthorityObjectV1::Source, source),
            (ClaimAuthorityObjectV1::Root, root),
            (ClaimAuthorityObjectV1::Worktree, worktree),
            (ClaimAuthorityObjectV1::CommonDirectory, common_dir),
        ] {
            if !Path::new(&claim.canonical_path).is_absolute() {
                return Err(claim_authority_unavailable(
                    object,
                    ClaimAuthorityUnavailableReasonV1::NotAbsolute,
                ));
            }
        }
        for (object, identity) in [
            (ClaimAuthorityObjectV1::Source, &source.directory_identity),
            (ClaimAuthorityObjectV1::Root, &root.directory_identity),
            (
                ClaimAuthorityObjectV1::CommonDirectory,
                &common_dir.directory_identity,
            ),
        ] {
            if identity.dev.is_none() || identity.ino.is_none() {
                return Err(claim_authority_unavailable(
                    object,
                    ClaimAuthorityUnavailableReasonV1::IdentityIncomplete,
                ));
            }
        }
        let custody_root_identity = match retained_root {
            RetainedCustodyRootAuthorityV1::Stable(identity) => identity,
            RetainedCustodyRootAuthorityV1::Unavailable => {
                return Err(claim_authority_unavailable(
                    ClaimAuthorityObjectV1::Root,
                    ClaimAuthorityUnavailableReasonV1::ObservationUnavailable,
                ));
            }
            RetainedCustodyRootAuthorityV1::IdentityChanged => {
                return Err(claim_authority_unavailable(
                    ClaimAuthorityObjectV1::Root,
                    ClaimAuthorityUnavailableReasonV1::IdentityChanged,
                ));
            }
        };
        if !root.directory_identity.matches(custody_root_identity) {
            return Err(claim_authority_unavailable(
                ClaimAuthorityObjectV1::Root,
                ClaimAuthorityUnavailableReasonV1::IdentityChanged,
            ));
        }
        let observed_source = observe_claim_directory_identity(
            &source.canonical_path,
            ClaimAuthorityObjectV1::Source,
        )?;
        if !source.directory_identity.matches(&observed_source) {
            return Err(claim_authority_unavailable(
                ClaimAuthorityObjectV1::Source,
                ClaimAuthorityUnavailableReasonV1::IdentityChanged,
            ));
        }
        let observed_common = observe_claim_directory_identity(
            &common_dir.canonical_path,
            ClaimAuthorityObjectV1::CommonDirectory,
        )?;
        if !common_dir.directory_identity.matches(&observed_common) {
            return Err(claim_authority_unavailable(
                ClaimAuthorityObjectV1::CommonDirectory,
                ClaimAuthorityUnavailableReasonV1::IdentityChanged,
            ));
        }
        let observed_authority = source_common_dir_identity(&observed_source.canonical_path, probe)
            .map_err(|_| {
                claim_authority_unavailable(
                    ClaimAuthorityObjectV1::SourceCommonDirectoryBinding,
                    ClaimAuthorityUnavailableReasonV1::ObservationUnavailable,
                )
            })?;
        if !observed_common.matches(&observed_authority) {
            return Err(claim_authority_unavailable(
                ClaimAuthorityObjectV1::SourceCommonDirectoryBinding,
                ClaimAuthorityUnavailableReasonV1::OwnershipUnproven,
            ));
        }
        Self::from_bound(
            observed_source,
            observed_common,
            Some(custody_root_identity.clone()),
            &worktree.canonical_path,
        )
    }

    fn from_bound(
        source_identity: DirectoryIdentityV1,
        common_dir_identity: DirectoryIdentityV1,
        custody_root_identity: Option<DirectoryIdentityV1>,
        worktree: &str,
    ) -> Result<Self, ClaimAuthorityUnavailableV1> {
        if !Path::new(worktree).is_absolute() {
            return Err(claim_authority_unavailable(
                ClaimAuthorityObjectV1::Worktree,
                ClaimAuthorityUnavailableReasonV1::NotAbsolute,
            ));
        }
        if source_identity.dev.is_none() || source_identity.ino.is_none() {
            return Err(claim_authority_unavailable(
                ClaimAuthorityObjectV1::Source,
                ClaimAuthorityUnavailableReasonV1::IdentityIncomplete,
            ));
        }
        if common_dir_identity.dev.is_none() || common_dir_identity.ino.is_none() {
            return Err(claim_authority_unavailable(
                ClaimAuthorityObjectV1::CommonDirectory,
                ClaimAuthorityUnavailableReasonV1::IdentityIncomplete,
            ));
        }
        Ok(Self {
            canonical_source: source_identity.canonical_path.clone(),
            source_identity,
            common_dir: common_dir_identity.canonical_path.clone(),
            common_dir_identity,
            custody_root_identity,
            worktree_path: worktree.to_owned(),
        })
    }

    pub(crate) fn revalidate_filesystem_identities(&self) -> Result<(), BridgeError> {
        let source = capture_directory_identity(Path::new(&self.canonical_source), "source")?;
        let common_dir =
            capture_directory_identity(Path::new(&self.common_dir), "source common directory")?;
        if !(self.source_identity.matches(&source) && self.common_dir_identity.matches(&common_dir))
        {
            return Err(invalid(
                "exact-absence source or common-directory identity changed",
            ));
        }
        if let Some(custody_root_identity) = &self.custody_root_identity {
            let custody_root = capture_directory_identity(
                Path::new(&custody_root_identity.canonical_path),
                "custody root",
            )?;
            if !custody_root_identity.matches(&custody_root) {
                return Err(invalid("exact-absence custody-root identity changed"));
            }
        }
        Ok(())
    }

    /// Compares a fresh Host Git common-directory observation supplied by the caller.
    ///
    /// This is intentionally a pure comparison; Host Git performs the observation.
    pub(crate) fn revalidate_repository_authority(
        &self,
        observed_common_dir: &DirectoryIdentityV1,
    ) -> Result<(), BridgeError> {
        if self.common_dir_identity.matches(observed_common_dir) {
            Ok(())
        } else {
            Err(invalid(
                "exact-absence repository-authority common-directory identity changed",
            ))
        }
    }
}

pub(crate) enum RetainedCustodyRootAuthorityV1 {
    Stable(DirectoryIdentityV1),
    Unavailable,
    IdentityChanged,
}
fn claim_authority_unavailable(
    object: ClaimAuthorityObjectV1,
    reason: ClaimAuthorityUnavailableReasonV1,
) -> ClaimAuthorityUnavailableV1 {
    ClaimAuthorityUnavailableV1::new(object, reason)
}

fn legacy_claim_authority_error(refusal: ClaimAuthorityUnavailableV1) -> BridgeError {
    if refusal.object() == ClaimAuthorityObjectV1::SourceCommonDirectoryBinding
        && refusal.reason() == ClaimAuthorityUnavailableReasonV1::OwnershipUnproven
    {
        return invalid("source does not own the recorded common directory");
    }
    invalid(format!("legacy claim authority unavailable: {refusal:?}"))
}

fn invalid(reason: impl Into<String>) -> BridgeError {
    BridgeError::ConfigInvalid {
        reason: reason.into(),
    }
}

fn observe_claim_directory_identity(
    path: &str,
    object: ClaimAuthorityObjectV1,
) -> Result<DirectoryIdentityV1, ClaimAuthorityUnavailableV1> {
    if !Path::new(path).is_absolute() {
        return Err(claim_authority_unavailable(
            object,
            ClaimAuthorityUnavailableReasonV1::NotAbsolute,
        ));
    }
    let canonical = std::fs::canonicalize(path).map_err(|_| {
        claim_authority_unavailable(
            object,
            ClaimAuthorityUnavailableReasonV1::ObservationUnavailable,
        )
    })?;
    let identity = verify_payload_directory_identity(&canonical).map_err(|_| {
        claim_authority_unavailable(
            object,
            ClaimAuthorityUnavailableReasonV1::ObservationUnavailable,
        )
    })?;
    if identity.dev.is_none() || identity.ino.is_none() {
        return Err(claim_authority_unavailable(
            object,
            ClaimAuthorityUnavailableReasonV1::IdentityIncomplete,
        ));
    }
    Ok(identity)
}
pub(crate) fn capture_directory_identity(
    path: &Path,
    kind: &str,
) -> Result<DirectoryIdentityV1, BridgeError> {
    if !path.is_absolute() {
        return Err(invalid(format!("{kind} path has no absolute identity")));
    }
    let canonical = std::fs::canonicalize(path)
        .map_err(|error| invalid(format!("{kind} identity probe failed: {error}")))?;
    let identity = verify_payload_directory_identity(&canonical)
        .map_err(|refusal| invalid(format!("{kind} identity probe failed: {refusal:?}")))?;
    if identity.dev.is_none() || identity.ino.is_none() {
        return Err(invalid(format!("{kind} has no bound object identity")));
    }
    Ok(identity)
}
fn source_common_dir_identity(
    source: &str,
    probe: &dyn ExactAbsenceProbeV1,
) -> Result<DirectoryIdentityV1, BridgeError> {
    probe.observe_source_common_dir_identity(source)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExactAbsenceObservationV1 {
    TargetPresent,
    RegisteredButAbsent,
    BothAbsent,
}
pub trait ExactAbsenceProbeV1: Send + Sync {
    fn observe_source_common_dir_identity(
        &self,
        source: &str,
    ) -> Result<DirectoryIdentityV1, BridgeError>;

    fn observe_exact_absence(
        &self,
        candidate: &ExactAbsenceCandidateV1,
    ) -> Result<ExactAbsenceObservationV1, BridgeError>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnusedCandidateDecisionV1 {
    Authorized,
    Refused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExactAbsenceEvidenceV1 {
    Authorized,
    Refused,
    CannotProve,
}

impl ExactAbsenceEvidenceV1 {
    fn report_decision(self) -> UnusedCandidateDecisionV1 {
        match self {
            Self::Authorized => UnusedCandidateDecisionV1::Authorized,
            Self::Refused | Self::CannotProve => UnusedCandidateDecisionV1::Refused,
        }
    }
}

fn decide_unused_candidate_evidence(
    candidate: &ExactAbsenceCandidateV1,
    recovery_owned: bool,
    probe: &dyn ExactAbsenceProbeV1,
) -> ExactAbsenceEvidenceV1 {
    if recovery_owned {
        return ExactAbsenceEvidenceV1::Refused;
    }
    match probe.observe_exact_absence(candidate) {
        Ok(ExactAbsenceObservationV1::BothAbsent) => ExactAbsenceEvidenceV1::Authorized,
        Ok(
            ExactAbsenceObservationV1::TargetPresent
            | ExactAbsenceObservationV1::RegisteredButAbsent,
        ) => ExactAbsenceEvidenceV1::Refused,
        Err(_) => ExactAbsenceEvidenceV1::CannotProve,
    }
}

#[must_use]
pub fn decide_unused_candidate(
    candidate: &ExactAbsenceCandidateV1,
    recovery_owned: bool,
    probe: &dyn ExactAbsenceProbeV1,
) -> UnusedCandidateDecisionV1 {
    decide_unused_candidate_evidence(candidate, recovery_owned, probe).report_decision()
}
// Stays sync (not de-blocked like host_git.rs's run_git): this call runs inside
// `WorktreeRunEndGuard::drop` (a `Drop` impl cannot await) and during the
// startup/boot sweep — not a per-turn path. See spec
// docs/superpowers/specs/2026-07-03-wave-1-hardening.md §W1-C.
fn run_git_sync(argv: &[&str]) {
    let _ = std::process::Command::new("git").args(argv).output();
}

/// Best-effort remove a worktree + its sidecar.
fn remove_worktree(canonical_source: &str, common_dir: &str, worktree_path: &str) {
    run_git_sync(&remove_argv(canonical_source, worktree_path));
    run_git_sync(&prune_argv(canonical_source));
    if !common_dir.is_empty() {
        run_git_sync(&["--git-dir", common_dir, "worktree", "prune"]);
    }
    let _ = std::fs::remove_dir_all(worktree_path);
    let _ = std::fs::remove_file(sidecar_path(worktree_path));
}

fn sidecar_file_matches(sidecar_file: &str, worktree_path: &str) -> bool {
    let Ok(sidecar_file) = std::fs::canonicalize(Path::new(sidecar_file)) else {
        return false;
    };
    let Ok(expected) = std::fs::canonicalize(Path::new(&sidecar_path(worktree_path))) else {
        return false;
    };
    sidecar_file == expected
}

fn worktree_under_root(root: &SessionCwd, worktree_path: &str) -> bool {
    canonicalize_lenient(worktree_path)
        .map(|wt| wt.is_under(root))
        .unwrap_or(false)
}

fn remove_worktree_if_safe(
    root: &SessionCwd,
    sidecar_file: &str,
    s: &crate::provider_path::WorktreeSidecar,
) {
    // ---- STEP 1: the two forgery guards, FIRST ----------------------------------------------
    // Order matters, and this is a repair (2b2 review, opus S-8): the custody probe used to run
    // ahead of these, so a FORGED sidecar naming an arbitrary path made the sweep stat that path
    // — and, once the publication cell landed below, would have made it create a lock directory
    // beside it. Nothing may touch a path these two guards have not yet vouched for.
    if !sidecar_file_matches(sidecar_file, &s.worktree_path) {
        tracing::warn!(
            sidecar = sidecar_file,
            worktree_path = s.worktree_path,
            "skipping worktree sidecar whose file does not match its worktree sibling"
        );
        return;
    }
    if !worktree_under_root(root, &s.worktree_path) {
        tracing::warn!(
            sidecar = sidecar_file,
            worktree_path = s.worktree_path,
            root = root.as_str(),
            "skipping worktree sidecar outside sweep root"
        );
        return;
    }

    // ---- STEP 2: enter the checkout's publication cell, REFUSING -----------------------------
    // `custody_lock.rs`'s contract says "every deletion-side and sweep-side caller" must take
    // this cell with the refusing acquirer, and until this repair the sweep did not — it probed
    // and removed with nothing serializing the two. The race is the same one the backend gate's
    // window closes, and just as reachable: the sweep sees no record, a writer publishes
    // `ProtectionPrepared` while holding the cell, and the sweep deletes a protected checkout.
    //
    // Contention and unavailability both SKIP. A cell this sweep cannot enter is a custody state
    // it cannot inspect, and unknown never licenses deletion (§5.2). Skipping is also free of
    // consequence here: the next boot sweep retries.
    //
    // The cell is entered only AFTER the forgery guards pass, so the lock directory is created
    // under the sweep root only when a removal of a vouched-for sibling is actually imminent.
    let _cell = match crate::custody_lock::try_acquire_publication_lock_in(
        Path::new(root.as_str()),
        &s.worktree_path,
    ) {
        Ok(cell) => cell,
        Err(refusal) => {
            tracing::info!(
                sidecar = sidecar_file,
                worktree_path = s.worktree_path,
                refusal = %refusal,
                "skipping a worktree reclaim whose custody publication cell could not be entered"
            );
            return;
        }
    };

    // ---- STEP 3: coexistence guard, inside the cell ------------------------------------------
    // A checkout carrying BOTH records must be reclaimed by neither arm. Two halves, and the
    // first is not sufficient on its own:
    //
    // 1. the V3 writer never emits `.meta.json` (`v3_path_writes_no_legacy_meta_json`), but
    // 2. 2b1's deletion gate MANUFACTURES coexistence anyway: a refused cleanup RETAINS the
    //    legacy sidecar beside a custody record. That state needs no crash and no exotic input —
    //    it is the gate's ordinary output — and both this function's callers would then delete
    //    the checkout, including `WorktreeRunEndGuard`'s CLEAN arm on every normal run.
    //
    // Presence, never content, exactly as the backend gate: a corrupt or unreadable record must
    // still protect, so a decode-based answer (which would read damage as absence) is the one
    // shape this must not have.
    let presence = crate::custody::probe_custody_record_presence(&s.worktree_path);
    if !presence.authorizes_checkout_removal() {
        tracing::info!(
            sidecar = sidecar_file,
            worktree_path = s.worktree_path,
            "leaving a legacy sidecar whose checkout also carries an R2f1b custody record"
        );
        return;
    }

    // ---- STEP 4: remove, still holding the cell ---------------------------------------------
    remove_worktree(&s.canonical_source, &s.common_dir, &s.worktree_path);
}

/// A record enumerated by the dual-pattern scan.
///
/// Focused boundary §2.2 requires the boot sweep to scan **both** patterns: legacy
/// `*.meta.json` under the existing bounded policy, and `*.custody.v1.json` under §5
/// policy. Without the second pattern V3 checkouts would leak unreclaimed forever.
#[derive(Debug)]
pub enum ScannedWorktreeRecordV1 {
    /// A readable legacy V2 sidecar.
    Legacy(crate::provider_path::WorktreeSidecar),
    /// A readable, canonically-decoded V3 custody record.
    Custody(Box<WorktreeCustodyRecordV1>),
    /// A V3-named entry that could not be read under descriptor custody, or that did not
    /// decode. Never actionable — it classifies as unknown.
    UnreadableCustody(CustodyReadRefusalV1),
}

/// Iterate the readable worktree records directly under `root`, in both patterns.
///
/// V3 entries are read through a single pinned handle on `root`, so the record open is
/// descriptor-relative, no-follow, regular-file-only, single-link-only and byte-bounded.
pub fn scan_worktree_records(root: &str) -> Vec<(String, ScannedWorktreeRecordV1)> {
    scan_worktree_records_with_pin_opener(root, FilesystemCompatibilityPinOpenerV1)
}

fn project_action_scan_result(
    result: Result<CheckedScanCompletedV1, CheckedScanOpenRefusalV1>,
) -> Vec<(String, ScannedWorktreeRecordV1)> {
    result
        .map(CheckedScanCompletedV1::into_action_rows)
        .unwrap_or_default()
}

fn scan_worktree_records_with_pin_opener<P>(
    root: &str,
    pin_opener: P,
) -> Vec<(String, ScannedWorktreeRecordV1)>
where
    P: CompatibilityPinOpenerV1,
{
    project_action_scan_result(checked_scan::scan_compatibility_with_pin_opener(
        Path::new(root),
        None,
        pin_opener,
    ))
}

/// Does `record_file` name the custody record of its own existing worktree sibling?
///
/// The V3 twin of `sidecar_file_matches`, with one addition the legacy check does not
/// need: the sibling directory must exist. A record naming a vanished checkout is a
/// *missing* pair (§5.2), and a missing pair is unknown, not actionable.
fn custody_record_file_matches(record_file: &str, worktree_path: &str) -> bool {
    if !Path::new(worktree_path).is_dir() {
        return false;
    }
    let Ok(record_file) = std::fs::canonicalize(Path::new(record_file)) else {
        return false;
    };
    let Ok(expected) = std::fs::canonicalize(Path::new(&custody_record_path(worktree_path))) else {
        return false;
    };
    record_file == expected
}

/// Classify one scanned V3 entry. **Recovery-only: no result authorizes deletion.**
///
/// Per §5.2 the state is parsed before any run id or lease is examined, and both existing
/// custody guards apply to the V3 arm exactly as they do to the legacy arm: the
/// record↔sibling match defeats a forged record pointing at another directory, and the
/// under-root check defeats one pointing outside the sweep root.
///
/// Mutation-checked (both reverted before commit): deleting `custody_record_file_matches`'s
/// `is_dir` precondition turned `sweep_treats_mismatched_and_missing_v3_pairs_as_unknown`
/// red on its *missing*-pair case only; weakening `read_custody_record_in`'s `nlink != 1`
/// to `nlink > 99` turned `sweep_treats_multi_link_v3_record_as_unknown_and_never_deletes`
/// red, with the record classifying `Recover` instead of `Unknown`.
#[must_use]
pub fn custody_entry_disposition(
    root: &SessionCwd,
    record_file: &str,
    record: Result<&WorktreeCustodyRecordV1, &CustodyReadRefusalV1>,
) -> CustodySweepDispositionV1 {
    let Ok(record) = record else {
        // Corrupt, missing, symlinked, multiply-linked, or over-bound: unknown.
        return CustodySweepDispositionV1::Unknown;
    };
    let worktree_path = record.worktree.canonical_path.as_str();
    if !custody_record_file_matches(record_file, worktree_path) {
        return CustodySweepDispositionV1::Unknown;
    }
    if !worktree_under_root(root, worktree_path) {
        return CustodySweepDispositionV1::Refused;
    }
    if recorded_identity_matches_sibling(record) == Some(false) {
        // The record is well-formed and correctly placed, but the directory now
        // present is not the one whose identity it recorded. That is ambiguous
        // evidence, not a licence: fall back to the protective classification.
        tracing::warn!(
            record = record_file,
            worktree_path,
            "worktree custody record does not match the directory identity now present"
        );
        return CustodySweepDispositionV1::Recover;
    }
    record.sweep_disposition()
}

/// Compare the record's recorded object identity against the directory that is actually
/// there, **by descriptor** — §2.2: "Identity is checked by descriptor, not by
/// re-canonicalizing a string, at every decision point."
///
/// `None` means there is nothing to check: either the record is degraded (P2 — a
/// pre-materialization writer records plan-derived paths with no `dev`/`ino`), or the
/// platform has no such evidence (brief risk R-10, non-unix).
fn recorded_identity_matches_sibling(record: &WorktreeCustodyRecordV1) -> Option<bool> {
    let recorded = &record.worktree.directory_identity;
    if recorded.dev.is_none() || recorded.ino.is_none() {
        return None;
    }
    let file = bridge_core::fs_custody::open_directory_no_follow_raw(Path::new(
        record.worktree.canonical_path.as_str(),
    ))
    .ok()?;
    let metadata = file.metadata().ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let observed = DirectoryIdentityV1 {
            canonical_path: record.worktree.canonical_path.clone(),
            dev: Some(metadata.dev()),
            ino: Some(metadata.ino()),
            btime: BirthTimeV1::from_metadata(&metadata),
        };
        Some(recorded.matches(&observed))
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
}

/// Report one classified V3 entry. Slice 2a acts on none of them: it never runs `git`,
/// `remove_dir_all`, reset, clean, or checkout for a V3 record.
fn report_custody_entry(
    record_file: &str,
    disposition: CustodySweepDispositionV1,
    state_tag: Option<String>,
    refusal: Option<&CustodyReadRefusalV1>,
) {
    debug_assert!(
        !disposition.authorizes_checkout_removal(),
        "slice 2a mints no deletion authority for V3 custody records"
    );
    match refusal {
        Some(refusal) => tracing::warn!(
            record = record_file,
            category = disposition.report_category(),
            refusal = %refusal,
            "worktree custody record is unreadable; leaving it for recovery"
        ),
        None => tracing::info!(
            record = record_file,
            category = disposition.report_category(),
            state = state_tag.unwrap_or_default(),
            "worktree custody record is protected from the sweep"
        ),
    }
}

fn decide_unused_legacy_sidecar(
    root: &SessionCwd,
    sidecar_file: &str,
    sidecar: &crate::provider_path::WorktreeSidecar,
    probe: &dyn ExactAbsenceProbeV1,
) -> UnusedCandidateDecisionV1 {
    if !sidecar_file_matches(sidecar_file, &sidecar.worktree_path)
        || !worktree_under_root(root, &sidecar.worktree_path)
    {
        return UnusedCandidateDecisionV1::Refused;
    }
    ExactAbsenceCandidateV1::from_legacy(
        &sidecar.canonical_source,
        &sidecar.common_dir,
        &sidecar.worktree_path,
        probe,
    )
    .map(|candidate| decide_unused_candidate(&candidate, false, probe))
    .unwrap_or(UnusedCandidateDecisionV1::Refused)
}
/// Reject records whose target cannot be constructed as this root's child.
///
/// The order is significant: containment and sibling questions only make sense
/// after the recorded target is known to be absolute.
fn construction_guards(
    canonical_root: &SessionCwd,
    enumerated_name: &OsStr,
    recorded_worktree: &str,
) -> Result<(), CannotConstructSubjectV1> {
    let worktree = Path::new(recorded_worktree);
    if !worktree.is_absolute() {
        return Err(CannotConstructSubjectV1::RecordedWorktreePathNotAbsolute);
    }
    if !worktree_under_root(canonical_root, recorded_worktree) {
        return Err(CannotConstructSubjectV1::OutsideSweepRoot);
    }
    let (Some(parent), Some(stem)) = (worktree.parent(), worktree.file_name()) else {
        return Err(CannotConstructSubjectV1::RecordFileNotExpectedSibling);
    };
    let mut expected = stem.to_os_string();
    expected.push(CUSTODY_RECORD_SUFFIX);
    if parent != Path::new(canonical_root.as_str()) || enumerated_name != expected.as_os_str() {
        return Err(CannotConstructSubjectV1::RecordFileNotExpectedSibling);
    }
    Ok(())
}

/// `Ok(())` admits the record to construction; `Err(..)` is the reported population.
fn admit_custody_population(
    state: &WorktreeCustodyStateV1,
    claim_present: bool,
) -> Result<(), IneligiblePopulationV1> {
    match (state, claim_present) {
        (WorktreeCustodyStateV1::ProtectionPrepared {}, true) => Ok(()),
        (WorktreeCustodyStateV1::ProtectionPrepared {}, false) => {
            Err(IneligiblePopulationV1::BareProtectionPrepared)
        }
        (
            WorktreeCustodyStateV1::PreservationUnknown {
                reason: PreservationReasonV1::MaterializationInFlight,
            },
            true,
        ) => Ok(()),
        (
            WorktreeCustodyStateV1::PreservationUnknown {
                reason: PreservationReasonV1::MaterializationInFlight,
            },
            false,
        )
        | (
            WorktreeCustodyStateV1::PreservationUnknown {
                reason:
                    PreservationReasonV1::NodeFailure
                    | PreservationReasonV1::Cancellation
                    | PreservationReasonV1::AmbiguousCleanup
                    | PreservationReasonV1::PostConditionDisagreement
                    | PreservationReasonV1::RemovalFailed,
            },
            _,
        )
        | (WorktreeCustodyStateV1::PreservationPrepared {}, _)
        | (WorktreeCustodyStateV1::Preserved {}, _)
        | (WorktreeCustodyStateV1::UnusedSettled {}, _)
        | (WorktreeCustodyStateV1::Materializing {}, _)
        | (WorktreeCustodyStateV1::LiveProtected {}, _)
        | (WorktreeCustodyStateV1::DeleteAuthorized {}, _)
        | (WorktreeCustodyStateV1::Removed {}, _)
        | (WorktreeCustodyStateV1::RecoveredLive { .. }, _) => {
            Err(IneligiblePopulationV1::StateNotCandidate)
        }
    }
}

struct CustodyAssessmentProjectionV1 {
    assessment: CustodyExactAbsenceAssessmentV1,
    evidence: ExactAbsenceEvidenceV1,
}

fn assess_custody_record(
    canonical_root: &SessionCwd,
    enumerated_name: &OsStr,
    record: &WorktreeCustodyRecordV1,
    retained_root: &RetainedCustodyRootAuthorityV1,
    probe: &dyn ExactAbsenceProbeV1,
) -> CustodyAssessmentProjectionV1 {
    if let Err(refusal) = construction_guards(
        canonical_root,
        enumerated_name,
        &record.worktree.canonical_path,
    ) {
        return CustodyAssessmentProjectionV1 {
            assessment: CustodyExactAbsenceAssessmentV1::CannotConstructSubject(refusal),
            evidence: ExactAbsenceEvidenceV1::Refused,
        };
    }
    if let Err(population) = admit_custody_population(&record.state, record.claim.is_some()) {
        return CustodyAssessmentProjectionV1 {
            assessment: CustodyExactAbsenceAssessmentV1::IneligiblePopulation(population),
            evidence: ExactAbsenceEvidenceV1::Refused,
        };
    }
    let Some(claim) = record.claim.as_ref() else {
        return CustodyAssessmentProjectionV1 {
            assessment: CustodyExactAbsenceAssessmentV1::Assessed(
                UnusedCandidateDecisionV1::Refused,
            ),
            evidence: ExactAbsenceEvidenceV1::Refused,
        };
    };
    match ExactAbsenceCandidateV1::from_claim(
        &claim.source,
        &claim.root,
        &claim.common_dir,
        &claim.worktree,
        retained_root,
        probe,
    ) {
        Ok(candidate) => {
            let evidence = decide_unused_candidate_evidence(&candidate, false, probe);
            CustodyAssessmentProjectionV1 {
                assessment: CustodyExactAbsenceAssessmentV1::Assessed(evidence.report_decision()),
                evidence,
            }
        }
        Err(refusal) => CustodyAssessmentProjectionV1 {
            assessment: CustodyExactAbsenceAssessmentV1::CannotConstructSubject(
                CannotConstructSubjectV1::ClaimAuthorityUnavailable(refusal),
            ),
            evidence: ExactAbsenceEvidenceV1::CannotProve,
        },
    }
}
struct ExactScanProjectionRowV1 {
    checked: CheckedScanRowV1,
    assessment: ExactAbsenceRecordAssessmentV1,
    evidence: ExactAbsenceEvidenceV1,
}

struct ExactScanCompleteV1 {
    canonical_root: SessionCwd,
    iterator_error_count: usize,
    root_observations: RootObservationSetV1,
    rows: Vec<ExactScanProjectionRowV1>,
}

enum ExactScanOutcomeV1 {
    Refused {
        canonical_root: Option<SessionCwd>,
        refusal: ExactAbsenceRootRefusalV1,
    },
    Complete(ExactScanCompleteV1),
}

#[cfg(test)]
type ExactScanCompletePartsV1 = (
    SessionCwd,
    usize,
    RootObservationSetV1,
    Vec<ExactScanProjectionRowV1>,
);
#[cfg(test)]
type ExactScanRefusalPartsV1 = (Option<SessionCwd>, ExactAbsenceRootRefusalV1);

impl ExactScanOutcomeV1 {
    #[cfg(test)]
    fn into_exact_parts(self) -> Result<ExactScanCompletePartsV1, ExactScanRefusalPartsV1> {
        match self {
            Self::Refused {
                canonical_root,
                refusal,
            } => Err((canonical_root, refusal)),
            Self::Complete(complete) => Ok((
                complete.canonical_root,
                complete.iterator_error_count,
                complete.root_observations,
                complete.rows,
            )),
        }
    }

    fn into_report(self, requested_root: String) -> ExactAbsenceSweepReportV1 {
        match self {
            Self::Refused {
                canonical_root,
                refusal,
            } => ExactAbsenceSweepReportV1::new(
                requested_root,
                canonical_root.map(|root| root.as_str().to_owned()),
                ExactAbsenceScanStatusV1::new(
                    ExactAbsenceEnumerationV1::Refused(refusal),
                    CustodyRootObservationV1::Unavailable,
                ),
                Vec::new(),
            ),
            Self::Complete(complete) => {
                let enumeration = if complete.iterator_error_count == 0 {
                    ExactAbsenceEnumerationV1::Complete
                } else {
                    ExactAbsenceEnumerationV1::Incomplete {
                        skipped_entries: complete.iterator_error_count,
                    }
                };
                let entries = complete
                    .rows
                    .into_iter()
                    .map(report_exact_scan_projection_row)
                    .collect();
                ExactAbsenceSweepReportV1::new(
                    requested_root,
                    Some(complete.canonical_root.as_str().to_owned()),
                    ExactAbsenceScanStatusV1::new(
                        enumeration,
                        classify_root_observations(complete.root_observations),
                    ),
                    entries,
                )
            }
        }
    }
}

fn classify_root_observations(observations: RootObservationSetV1) -> CustodyRootObservationV1 {
    let (Some(retained_enumeration_object), Some(pinned_custody_directory), Some(final_named_root)) = (
        observations.retained_enumeration_object,
        observations.pinned_custody_directory,
        observations.final_named_root,
    ) else {
        return CustodyRootObservationV1::Unavailable;
    };
    if !(root_capture_has_object_identity(retained_enumeration_object)
        && root_capture_has_object_identity(pinned_custody_directory)
        && root_capture_has_object_identity(final_named_root))
    {
        return CustodyRootObservationV1::Unavailable;
    }
    if retained_enumeration_object == pinned_custody_directory
        && pinned_custody_directory == final_named_root
    {
        CustodyRootObservationV1::Pinned
    } else {
        CustodyRootObservationV1::IdentityChanged
    }
}

fn retained_custody_root_authority(
    observations: RootObservationSetV1,
    canonical_root: &SessionCwd,
) -> RetainedCustodyRootAuthorityV1 {
    match classify_root_observations(observations) {
        CustodyRootObservationV1::Pinned => {
            let Some(retained_enumeration_object) = observations.retained_enumeration_object else {
                return RetainedCustodyRootAuthorityV1::Unavailable;
            };
            RetainedCustodyRootAuthorityV1::Stable(DirectoryIdentityV1 {
                canonical_path: canonical_root.as_str().to_owned(),
                dev: retained_enumeration_object.dev,
                ino: retained_enumeration_object.ino,
                btime: retained_enumeration_object.birthtime,
            })
        }
        CustodyRootObservationV1::Unavailable => RetainedCustodyRootAuthorityV1::Unavailable,
        CustodyRootObservationV1::IdentityChanged => {
            RetainedCustodyRootAuthorityV1::IdentityChanged
        }
    }
}

fn root_capture_has_object_identity(capture: checked_scan::RootIdentityCaptureV1) -> bool {
    capture.dev.is_some() && capture.ino.is_some()
}

fn report_exact_scan_projection_row(row: ExactScanProjectionRowV1) -> ExactAbsenceSweepEntryV1 {
    let ExactScanProjectionRowV1 {
        checked,
        assessment,
        ..
    } = row;
    let (record_path, enumerated_name, scanned) = checked.parts();
    let custody_record_bytes = match scanned {
        ScannedWorktreeRecordV1::Custody(record) => record.encode_canonical().ok(),
        ScannedWorktreeRecordV1::Legacy(_) | ScannedWorktreeRecordV1::UnreadableCustody(_) => None,
    };
    ExactAbsenceSweepEntryV1::new(
        record_path.to_owned(),
        enumerated_name.to_os_string(),
        assessment,
        custody_record_bytes,
    )
}

/// Re-run the report's exact scan and projection for one report-selected record.
///
/// The returned tri-state outcome is freshly observed evidence only. It is deliberately not a report and
/// carries no settlement authority; `settle::reprove_under_window` keeps the held cells and
/// decides whether this evidence can mint a proved capability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReprovedExactAbsenceOutcomeV1 {
    Authorized,
    Refused(&'static str),
    CannotProve(&'static str),
}

pub(crate) fn reprove_exact_absence_entry(
    root: &Path,
    held_record: &WorktreeCustodyRecordV1,
    report_entry: &ExactAbsenceSweepEntryV1,
    probe: &dyn ExactAbsenceProbeV1,
) -> ReprovedExactAbsenceOutcomeV1 {
    let canonical_root = match canonicalize_lenient(&root.to_string_lossy()) {
        Ok(canonical_root) => canonical_root,
        Err(_) => return ReprovedExactAbsenceOutcomeV1::CannotProve("root cannot canonicalize"),
    };
    let outcome = project_exact_scan_result(
        canonical_root.clone(),
        checked_scan::scan_compatibility_with_pin_opener(
            Path::new(canonical_root.as_str()),
            Some(report_entry.enumerated_name()),
            FilesystemCompatibilityPinOpenerV1,
        ),
        probe,
    );
    let ExactScanOutcomeV1::Complete(complete) = outcome else {
        return ReprovedExactAbsenceOutcomeV1::CannotProve("root cannot enumerate");
    };
    if complete.iterator_error_count != 0
        || classify_root_observations(complete.root_observations)
            != CustodyRootObservationV1::Pinned
    {
        return ReprovedExactAbsenceOutcomeV1::CannotProve("root evidence is unavailable");
    }
    let Some(row) = complete.rows.into_iter().next() else {
        return ReprovedExactAbsenceOutcomeV1::CannotProve(
            "selected record was not observed in the fresh scan",
        );
    };
    let evidence = row.evidence;
    if evidence == ExactAbsenceEvidenceV1::CannotProve {
        return ReprovedExactAbsenceOutcomeV1::CannotProve(
            "fresh subject authority or exact-absence observation is unavailable",
        );
    }
    let refreshed = report_exact_scan_projection_row(row);
    let held_bytes = match held_record.encode_canonical() {
        Ok(held_bytes) => held_bytes,
        Err(_) => return ReprovedExactAbsenceOutcomeV1::CannotProve("held record cannot encode"),
    };
    let held_entry = ExactAbsenceSweepEntryV1::new(
        refreshed.record_path().to_owned(),
        refreshed.enumerated_name().to_os_string(),
        refreshed.assessment().clone(),
        Some(held_bytes),
    );
    if refreshed != *report_entry || refreshed != held_entry {
        return ReprovedExactAbsenceOutcomeV1::Refused("record or report evidence changed");
    }
    match evidence {
        ExactAbsenceEvidenceV1::Authorized => ReprovedExactAbsenceOutcomeV1::Authorized,
        ExactAbsenceEvidenceV1::Refused => {
            ReprovedExactAbsenceOutcomeV1::Refused("fresh exact-absence decision refused")
        }
        ExactAbsenceEvidenceV1::CannotProve => ReprovedExactAbsenceOutcomeV1::CannotProve(
            "fresh subject authority or exact-absence observation is unavailable",
        ),
    }
}

fn project_exact_scan_result(
    canonical_root: SessionCwd,
    result: Result<CheckedScanCompletedV1, CheckedScanOpenRefusalV1>,
    probe: &dyn ExactAbsenceProbeV1,
) -> ExactScanOutcomeV1 {
    let Ok((checked_rows, iterator_error_count, root_observations)) =
        result.map(CheckedScanCompletedV1::into_exact_parts)
    else {
        return ExactScanOutcomeV1::Refused {
            canonical_root: Some(canonical_root),
            refusal: ExactAbsenceRootRefusalV1::CannotEnumerate,
        };
    };
    let retained_root = retained_custody_root_authority(root_observations, &canonical_root);
    let mut rows = Vec::with_capacity(checked_rows.len());
    for checked in checked_rows {
        let (assessment, evidence) = match checked.parts() {
            (path, _, ScannedWorktreeRecordV1::Legacy(sidecar)) => {
                let decision = decide_unused_legacy_sidecar(&canonical_root, path, sidecar, probe);
                (
                    ExactAbsenceRecordAssessmentV1::Legacy(decision),
                    ExactAbsenceEvidenceV1::Refused,
                )
            }
            (_, enumerated_name, ScannedWorktreeRecordV1::Custody(record)) => {
                let custody = assess_custody_record(
                    &canonical_root,
                    enumerated_name,
                    record,
                    &retained_root,
                    probe,
                );
                (
                    ExactAbsenceRecordAssessmentV1::Custody(CustodyRecordAssessmentV1::new(
                        CustodyStateSnapshotV1::from(&record.state),
                        custody.assessment,
                    )),
                    custody.evidence,
                )
            }
            (_, _, ScannedWorktreeRecordV1::UnreadableCustody(refusal)) => (
                ExactAbsenceRecordAssessmentV1::UnreadableCustody(refusal.clone()),
                ExactAbsenceEvidenceV1::CannotProve,
            ),
        };
        let projection_row = ExactScanProjectionRowV1 {
            checked,
            assessment,
            evidence,
        };
        let path = projection_row.checked.record_path();
        let decision = projection_row.assessment.decision();
        tracing::info!(record = path, ?decision, "made exact-absence decision");
        rows.push(projection_row);
    }
    ExactScanOutcomeV1::Complete(ExactScanCompleteV1 {
        canonical_root,
        iterator_error_count,
        root_observations,
        rows,
    })
}

fn sweep_orphans_with_exact_absence_with_pin_opener<P>(
    root: &str,
    probe: &dyn ExactAbsenceProbeV1,
    pin_opener: P,
) -> ExactScanOutcomeV1
where
    P: CompatibilityPinOpenerV1,
{
    let canonical_root = match canonicalize_lenient(root) {
        Ok(canonical_root) => canonical_root,
        Err(_) => {
            return ExactScanOutcomeV1::Refused {
                canonical_root: None,
                refusal: ExactAbsenceRootRefusalV1::CannotCanonicalize,
            };
        }
    };
    project_exact_scan_result(
        canonical_root.clone(),
        checked_scan::scan_compatibility_with_pin_opener(
            Path::new(canonical_root.as_str()),
            None,
            pin_opener,
        ),
        probe,
    )
}

pub fn sweep_orphans_with_exact_absence(
    root: &str,
    probe: &dyn ExactAbsenceProbeV1,
) -> ExactAbsenceSweepReportV1 {
    let outcome = sweep_orphans_with_exact_absence_with_pin_opener(
        root,
        probe,
        FilesystemCompatibilityPinOpenerV1,
    );
    outcome.into_report(root.to_owned())
}
/// Reap only same-host **legacy** worktrees whose owner lease is free.
///
/// V3 custody records are recognized and classified, never deleted (§5.2).
pub fn sweep_orphans(root: &str, my_host: &str, probe: &dyn LeaseProbe) {
    let _ = sweep_orphans_with_exact_absence(root, &crate::host_git::HostGitWorktree::new());
    let Ok(root_cwd) = canonicalize_lenient(root) else {
        tracing::warn!(root, "skipping worktree sweep with non-canonical root");
        return;
    };
    for (path, scanned) in scan_worktree_records(root) {
        match scanned {
            ScannedWorktreeRecordV1::Legacy(s) => {
                let labels = HashMap::from([
                    ("a2a.host".to_string(), s.host.clone()),
                    ("a2a.lease".to_string(), s.lease.clone()),
                ]);
                if classify(&labels, my_host, probe) == Verdict::Dead {
                    remove_worktree_if_safe(&root_cwd, &path, &s);
                }
            }
            ScannedWorktreeRecordV1::Custody(record) => {
                let disposition = custody_entry_disposition(&root_cwd, &path, Ok(&record));
                report_custody_entry(
                    &path,
                    disposition,
                    Some(record.state.kind().wire_tag()),
                    None,
                );
            }
            ScannedWorktreeRecordV1::UnreadableCustody(refusal) => {
                let disposition = custody_entry_disposition(&root_cwd, &path, Err(&refusal));
                report_custody_entry(&path, disposition, None, Some(&refusal));
            }
        }
    }
}

/// Run-end backstop for worktrees created by a single bridge process run.
///
/// **Unconditionally non-destructive for V3 custody records** (focused boundary §5.2
/// bullet 2). For legacy V2 sidecars it applies the slice-2 brief's R9 ruling where R9
/// actually bites: deletion authority is removed from the **unwind** path.
///
/// R9's thrust is that an abrupt `Drop` is the moment the process knows least about
/// whether the work is still wanted, so it must not delete. That argument is about the
/// abrupt path. Making a *clean* exit defer as well would not defer the reclaim — it
/// would leak it permanently, because the boot sweep provably cannot fire afterwards:
/// `LeaseGuard::drop` unlinks the lease file on a clean drop (`liveness.rs:130-136`, and
/// its own doc-comment at `:115`: "The file is removed on a clean drop; after a crash it
/// persists with the lock FREE (the recovery signal)"), `FsLeaseProbe::try_state` then
/// answers `None` (`:253-258`), and `classify` maps `None` to `Unknown`, never `Dead`
/// (`run_identity.rs:110`). The free-lease recovery signal exists only after a crash.
///
/// So: clean drop reclaims this run's own legacy entries exactly as before R9; a drop
/// during an unwind defers to the boot sweep, which *can* fire in that case because the
/// crashed process left its lease file behind with the lock free. Pinned by
/// `boot_sweep_cannot_reclaim_a_cleanly_exited_run`.
///
/// # Explicit settlement (slice 2b2, S5)
///
/// [`Self::settle`] is the run's normal terminal path: an explicit, idempotent call the owner
/// makes when it knows the run ended in a handled way. `Drop` is then a BACKSTOP, and the two are
/// distinguishable:
///
/// * settled → `Drop` does nothing at all, so nothing is done twice;
/// * unsettled + clean → `Drop` performs the legacy reclaim as before (R9 leaves the clean path
///   destructive, because the boot sweep provably cannot fire after a clean exit);
/// * unsettled + unwinding → `Drop` defers, and records that settlement did NOT occur rather
///   than logging as though it had. That distinction is the point of Sol 17: an abrupt drop is
///   the moment the process knows least, and a backstop that reports itself as a settlement
///   makes an unsettled run indistinguishable from a settled one in the record.
///
/// Settlement is **non-destructive for every V3 record**. Converting unresolved live V3 entries
/// to preserved/unknown needs preservation transitions, which are 2c1's; what this slice settles
/// is the run-end pass itself, and it reports each V3 record as still recovery-owned rather than
/// pretending it was disposed of.
pub struct WorktreeRunEndGuard {
    pub root: String,
    pub instance_id: String,
    settled: std::sync::atomic::AtomicBool,
}

impl WorktreeRunEndGuard {
    #[must_use]
    pub fn new(root: String, instance_id: String) -> Self {
        Self {
            root,
            instance_id,
            settled: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Settle this run's worktrees explicitly. Idempotent: the second call is a no-op, and so is
    /// the later `Drop`.
    pub fn settle(&self) {
        if self.settled.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        self.run_end_pass(false, "settle");
    }

    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.settled.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn run_end_pass(&self, unwinding: bool, phase: &'static str) {
        let root_cwd = canonicalize_lenient(&self.root);
        for (path, scanned) in scan_worktree_records(&self.root) {
            match scanned {
                ScannedWorktreeRecordV1::Legacy(s) if s.run_id == self.instance_id => {
                    match (&root_cwd, unwinding) {
                        (_, true) => tracing::info!(
                            sidecar = path,
                            worktree_path = s.worktree_path,
                            run_id = self.instance_id,
                            "deferring worktree reclaim of this run to the next boot sweep \
                             (dropping during an unwind)"
                        ),
                        (Ok(root_cwd), false) => remove_worktree_if_safe(root_cwd, &path, &s),
                        (Err(_), false) => tracing::warn!(
                            root = self.root,
                            "skipping worktree end sweep with non-canonical root"
                        ),
                    }
                }
                ScannedWorktreeRecordV1::Legacy(_) => {}
                ScannedWorktreeRecordV1::Custody(record) => tracing::info!(
                    record = path,
                    state = record.state.kind().wire_tag(),
                    run_id = self.instance_id,
                    phase,
                    settled = self.is_settled(),
                    "leaving custody-protected worktree record untouched at run end; its \
                     disposition stays recovery-owned"
                ),
                ScannedWorktreeRecordV1::UnreadableCustody(refusal) => tracing::warn!(
                    record = path,
                    refusal = %refusal,
                    run_id = self.instance_id,
                    phase,
                    settled = self.is_settled(),
                    "leaving unreadable worktree custody record untouched at run end"
                ),
            }
        }
    }
}

impl Drop for WorktreeRunEndGuard {
    fn drop(&mut self) {
        if self.is_settled() {
            // Already settled explicitly. Doing the pass again would be harmless but would also
            // make "settled" unobservable; skipping it is what makes `settle` meaningful.
            return;
        }
        let unwinding = std::thread::panicking();
        if unwinding {
            tracing::warn!(
                root = self.root,
                run_id = self.instance_id,
                "worktree run-end guard dropped during an unwind WITHOUT explicit settlement; \
                 deferring reclaim to the next boot sweep. This is a backstop, not a settlement."
            );
        } else {
            // A NON-panicking drop is a handled terminal — an ordinary `return`, an early `?`, a
            // match arm that forgot the epilogue. Marking it settled before the pass is what makes
            // `is_settled()` (and the `settled` log field) truthful: "unsettled" then means
            // exactly "panicked or otherwise unhandled", which is the distinction Sol 17 asks for.
            // Call sites still settle explicitly where the terminal is named; this is the floor,
            // not a substitute (slice 2b2 repair R5).
            self.settled
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        self.run_end_pass(unwinding, "drop");
    }
}

#[cfg(test)]
mod tests {
    use super::{ClaimAuthorityObjectV1 as Object, ClaimAuthorityUnavailableReasonV1 as Reason};
    use crate::provider_path::{sidecar_path, write_sidecar, WorktreeSidecar};
    use bridge_core::liveness::LeaseProbe;
    use std::collections::HashMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct FakeProbe(HashMap<String, Option<bool>>);

    impl LeaseProbe for FakeProbe {
        fn try_state(&self, lease_path: &str) -> Option<bool> {
            self.0.get(lease_path).copied().flatten()
        }
    }

    fn unique_temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "a2a-bridge-worktree-sweep-{name}-{}-{nanos}",
            std::process::id()
        ))
    }

    #[cfg(unix)]
    #[test]
    fn exact_absence_claim_refuses_a_replaced_common_directory() {
        let root = unique_temp_dir("replaced-common-dir");
        let source = root.join("source");
        fs::create_dir_all(&source).unwrap();
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&source)
            .args(["init", "-q"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let source_identity = super::capture_directory_identity(&source, "source").unwrap();
        let common_identity =
            super::capture_directory_identity(&source.join(".git"), "source common directory")
                .unwrap();
        let worktree = root.join("worktree");
        fs::create_dir(&worktree).unwrap();
        let worktree_identity = super::capture_directory_identity(&worktree, "worktree").unwrap();
        let root_identity = super::capture_directory_identity(&root, "custody root").unwrap();
        let source_claim = WorktreeObjectIdentityV1 {
            canonical_path: source_identity.canonical_path.clone(),
            directory_identity: source_identity,
        };
        let common_claim = WorktreeObjectIdentityV1 {
            canonical_path: common_identity.canonical_path.clone(),
            directory_identity: common_identity,
        };
        let root_claim = WorktreeObjectIdentityV1 {
            canonical_path: root_identity.canonical_path.clone(),
            directory_identity: root_identity.clone(),
        };
        let retained_root = super::RetainedCustodyRootAuthorityV1::Stable(root_identity);
        let worktree_claim = WorktreeObjectIdentityV1 {
            canonical_path: worktree_identity.canonical_path.clone(),
            directory_identity: worktree_identity,
        };
        let probe = crate::host_git::HostGitWorktree::new();
        let candidate = super::ExactAbsenceCandidateV1::from_claim(
            &source_claim,
            &root_claim,
            &common_claim,
            &worktree_claim,
            &retained_root,
            &probe,
        )
        .unwrap();
        fs::rename(source.join(".git"), root.join("original-common")).unwrap();
        let replacement = root.join("replacement");
        fs::create_dir(&replacement).unwrap();
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&replacement)
            .args(["init", "-q"])
            .output()
            .unwrap();
        assert!(output.status.success());
        fs::rename(replacement.join(".git"), source.join(".git")).unwrap();
        assert!(
            candidate.revalidate_filesystem_identities().is_err(),
            "a common-directory replacement must refuse while the source inode is unchanged"
        );
        fs::remove_dir_all(root).unwrap();
    }
    fn write_worktree_sidecar(
        root: &Path,
        name: &str,
        host: &str,
        lease: &str,
        run_id: &str,
    ) -> WorktreeSidecar {
        let worktree_path = root.join(name);
        fs::create_dir_all(&worktree_path).unwrap();
        let sidecar = WorktreeSidecar {
            canonical_source: root.join("source").to_string_lossy().into_owned(),
            common_dir: root.join("source/.git").to_string_lossy().into_owned(),
            worktree_path: worktree_path.to_string_lossy().into_owned(),
            owner: "owner".into(),
            run_id: run_id.into(),
            host: host.into(),
            lease: lease.into(),
        };
        write_sidecar(&sidecar).unwrap();
        sidecar
    }

    #[test]
    fn sweep_reaps_dead_owner_keeps_live() {
        let root = unique_temp_dir("orphans");
        fs::create_dir_all(&root).unwrap();
        let dead = write_worktree_sidecar(&root, "dead", "my-host", "/leases/dead.lock", "run-a");
        let live = write_worktree_sidecar(&root, "live", "my-host", "/leases/live.lock", "run-b");
        let other =
            write_worktree_sidecar(&root, "other", "other-host", "/leases/other.lock", "run-c");
        let probe = FakeProbe(HashMap::from([
            ("/leases/dead.lock".to_string(), Some(true)),
            ("/leases/live.lock".to_string(), Some(false)),
            ("/leases/other.lock".to_string(), Some(true)),
        ]));

        super::sweep_orphans(&root.to_string_lossy(), "my-host", &probe);

        assert!(!Path::new(&dead.worktree_path).exists());
        assert!(!Path::new(&sidecar_path(&dead.worktree_path)).exists());
        assert!(Path::new(&live.worktree_path).exists());
        assert!(Path::new(&sidecar_path(&live.worktree_path)).exists());
        assert!(Path::new(&other.worktree_path).exists());
        assert!(Path::new(&sidecar_path(&other.worktree_path)).exists());

        fs::remove_dir_all(&root).unwrap();
    }

    /// REVISED per the slice-2 brief's R9 ruling, then repaired: R9 removes the
    /// run-end guard's deletion authority from the **unwind** path, which is
    /// where an abrupt `Drop` knows least about whether the work is wanted. It
    /// does *not* make a clean exit defer, because the boot sweep provably
    /// cannot reclaim afterwards — see
    /// `boot_sweep_cannot_reclaim_a_cleanly_exited_run` below, which pins the
    /// mechanism. Discriminates: the clean-exit legacy reclaim being dropped
    /// (a permanent worktree + sidecar leak on every clean `[worktrees]` run),
    /// and the guard widening beyond its own run.
    #[test]
    fn end_guard_reclaims_only_this_run_on_a_clean_exit() {
        let root = unique_temp_dir("end-guard");
        fs::create_dir_all(&root).unwrap();
        let mine = write_worktree_sidecar(&root, "mine", "my-host", "/leases/mine.lock", "mine");
        let other =
            write_worktree_sidecar(&root, "other", "my-host", "/leases/other.lock", "other");

        {
            let _guard =
                super::WorktreeRunEndGuard::new(root.to_string_lossy().into_owned(), "mine".into());
        }

        assert!(!Path::new(&mine.worktree_path).exists());
        assert!(!Path::new(&sidecar_path(&mine.worktree_path)).exists());
        assert!(Path::new(&other.worktree_path).exists());
        assert!(Path::new(&sidecar_path(&other.worktree_path)).exists());

        fs::remove_dir_all(&root).unwrap();
    }

    /// The R9 half that survives: dropping **during an unwind** defers instead
    /// of deleting. Discriminates: `std::thread::panicking()` being dropped
    /// from the guard, which would restore deletion on exactly the path where
    /// the process cannot know whether the checkout still matters.
    #[test]
    fn end_guard_defers_reclaim_when_dropping_during_an_unwind() {
        let root = unique_temp_dir("end-guard-unwind");
        fs::create_dir_all(&root).unwrap();
        let mine = write_worktree_sidecar(&root, "mine", "my-host", "/leases/mine.lock", "mine");

        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard =
                super::WorktreeRunEndGuard::new(root.to_string_lossy().into_owned(), "mine".into());
            panic!("run failed after the worktree was configured");
        }));
        assert!(unwound.is_err(), "the harness must actually unwind");

        assert!(
            Path::new(&mine.worktree_path).exists(),
            "an unwinding drop must not delete this run's checkout"
        );
        assert!(Path::new(&sidecar_path(&mine.worktree_path)).exists());

        fs::remove_dir_all(&root).unwrap();
    }

    /// The mechanism behind `end_guard_reclaims_only_this_run_on_a_clean_exit`,
    /// pinned so a future "just defer to the boot sweep" simplification cannot
    /// be made without seeing the leak it causes. `LeaseGuard::drop` **unlinks**
    /// the lease file on a clean drop (`liveness.rs:130-136`, and its own
    /// doc-comment at `:115`), `FsLeaseProbe::try_state` then answers `None`
    /// (`liveness.rs:253-258`), and `classify` maps `None` to `Unknown`, never
    /// `Dead` (`run_identity.rs:110`) — so the boot sweep's legacy arm never
    /// fires for a cleanly exited run. Uses the real lease and probe, not
    /// `FakeProbe`: this is a claim about production wiring.
    #[test]
    fn boot_sweep_cannot_reclaim_a_cleanly_exited_run() {
        use bridge_core::liveness::{acquire_lease_in, FsLeaseProbe};

        let root = unique_temp_dir("clean-exit-leak");
        let leases = unique_temp_dir("clean-exit-leak-leases");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&leases).unwrap();

        let lease = acquire_lease_in(&leases, "run-clean").unwrap();
        let lease_path = lease.path().to_string_lossy().into_owned();
        let orphan = write_worktree_sidecar(&root, "orphan", "my-host", &lease_path, "run-clean");

        // Held ⇒ Alive: the sweep must not touch a live run's checkout.
        super::sweep_orphans(&root.to_string_lossy(), "my-host", &FsLeaseProbe);
        assert!(Path::new(&orphan.worktree_path).exists());

        // Clean exit unlinks the lease file, so the probe can no longer answer
        // "free" — the verdict is Unknown and the boot sweep is a no-op forever.
        drop(lease);
        assert_eq!(
            FsLeaseProbe.try_state(&lease_path),
            None,
            "a cleanly dropped lease leaves no evidence for the boot sweep"
        );
        super::sweep_orphans(&root.to_string_lossy(), "my-host", &FsLeaseProbe);
        assert!(
            Path::new(&orphan.worktree_path).exists(),
            "the boot sweep cannot reclaim a cleanly exited run; the run-end \
             guard is the only thing that can"
        );

        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&leases).unwrap();
    }

    #[test]
    fn sweep_skips_sidecar_that_points_at_non_sibling_worktree() {
        let root = unique_temp_dir("sidecar-mismatch");
        let victim = unique_temp_dir("sidecar-mismatch-victim");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&victim).unwrap();
        fs::write(victim.join("keep"), "do not delete").unwrap();
        let sidecar = WorktreeSidecar {
            canonical_source: root.join("source").to_string_lossy().into_owned(),
            common_dir: root.join("source/.git").to_string_lossy().into_owned(),
            worktree_path: victim.to_string_lossy().into_owned(),
            owner: "owner".into(),
            run_id: "run-a".into(),
            host: "my-host".into(),
            lease: "/leases/dead.lock".into(),
        };
        let forged = root.join("forged.meta.json");
        fs::write(&forged, serde_json::to_vec(&sidecar).unwrap()).unwrap();
        let probe = FakeProbe(HashMap::from([(
            "/leases/dead.lock".to_string(),
            Some(true),
        )]));

        super::sweep_orphans(&root.to_string_lossy(), "my-host", &probe);

        assert!(victim.join("keep").exists());
        assert!(forged.exists());

        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&victim).unwrap();
    }

    /// Still live, and still discriminating, after the R9 repair: the run-end
    /// guard reclaims on a clean drop, so this forged record really does reach
    /// `remove_worktree_if_safe` and really is stopped by its guards.
    ///
    /// Mutation-checked (all reverted before commit): neutering
    /// `sidecar_file_matches` alone leaves this green, and so does neutering
    /// `worktree_under_root` alone -- the two guards defend this input
    /// redundantly. Neutering **both** turns this test and its boot-sweep twin
    /// `sweep_skips_sidecar_that_points_at_non_sibling_worktree` red together.
    /// So the pair is genuine coverage of the guard *set*, not of either guard
    /// individually; a single-guard regression would slip past both.
    #[test]
    fn end_guard_skips_sidecar_that_points_at_non_sibling_worktree() {
        let root = unique_temp_dir("end-guard-mismatch");
        let victim = unique_temp_dir("end-guard-mismatch-victim");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&victim).unwrap();
        fs::write(victim.join("keep"), "do not delete").unwrap();
        let sidecar = WorktreeSidecar {
            canonical_source: root.join("source").to_string_lossy().into_owned(),
            common_dir: root.join("source/.git").to_string_lossy().into_owned(),
            worktree_path: victim.to_string_lossy().into_owned(),
            owner: "owner".into(),
            run_id: "mine".into(),
            host: "my-host".into(),
            lease: "/leases/mine.lock".into(),
        };
        let forged = root.join("forged.meta.json");
        fs::write(&forged, serde_json::to_vec(&sidecar).unwrap()).unwrap();

        {
            let _guard =
                super::WorktreeRunEndGuard::new(root.to_string_lossy().into_owned(), "mine".into());
        }

        assert!(victim.join("keep").exists());
        assert!(forged.exists());

        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&victim).unwrap();
    }

    // ---- R2f1b slice 2a: dual-pattern recognition, recovery-only V3 arm ----

    use crate::custody::{
        custody_record_path, CustodySweepDispositionV1, PreservationReasonV1,
        PreservedWorktreeClaimV1, RecoveryLocatorV1, WorktreeCustodyRecordV1,
        WorktreeCustodyStateKindV1, WorktreeCustodyStateV1, WORKTREE_CUSTODY_RECORD_SCHEMA_V1,
    };
    use bridge_core::execution_policy::{
        PolicyNodeRefV1, Sha256HexV1, WorktreeCustodyIdV1, WorktreeObjectIdentityV1,
    };
    use bridge_core::fs_custody::{
        verify_payload_directory_identity, BirthTimeV1, DirectoryIdentityV1,
    };
    use bridge_core::ids::{AttemptId, AttemptIdentity, ExecutionId};

    fn sha(digit: char) -> Sha256HexV1 {
        Sha256HexV1::parse(digit.to_string().repeat(64)).unwrap()
    }

    /// An object identity carrying the directory's **observed** `dev`/`ino` when the
    /// path exists, so records built here are the shape a real writer publishes and the
    /// sweep's descriptor comparison (P3) actually has evidence to check. When
    /// `degraded`, the plan-derived path is all that is recorded — the shape a
    /// pre-materialization writer can produce (P2).
    fn object_with(path: &str, degraded: bool) -> WorktreeObjectIdentityV1 {
        let observed = (!degraded)
            .then(|| {
                std::fs::symlink_metadata(path).ok().map(|meta| {
                    use std::os::unix::fs::MetadataExt as _;
                    (meta.dev(), meta.ino(), BirthTimeV1::from_metadata(&meta))
                })
            })
            .flatten();
        let fallback = if degraded { None } else { Some((1, 2, None)) };
        let identity = observed.or(fallback);
        WorktreeObjectIdentityV1 {
            canonical_path: path.to_string(),
            directory_identity: DirectoryIdentityV1 {
                canonical_path: path.to_string(),
                dev: identity.map(|(dev, _, _)| dev),
                ino: identity.map(|(_, ino, _)| ino),
                btime: identity.and_then(|(_, _, btime)| btime),
            },
        }
    }

    fn attempt_identity() -> AttemptIdentity {
        AttemptIdentity {
            execution_id: ExecutionId::parse(format!("exec-{}", "1".repeat(32))).unwrap(),
            attempt_id: AttemptId::parse(format!("attempt-{}", "2".repeat(32))).unwrap(),
            ordinal: 0,
            parent_attempt_id: None,
        }
    }

    fn custody_record(worktree: &str, state: WorktreeCustodyStateV1) -> WorktreeCustodyRecordV1 {
        let custody_id = WorktreeCustodyIdV1::parse(format!("custody-{}", "3".repeat(64))).unwrap();
        // Publish the identity shape this state is settled to carry (P2).
        let degraded =
            state.identity_completeness() == crate::custody::IdentityCompletenessV1::MayBeDegraded;
        let object = |path: &str| object_with(path, degraded);
        let claim =
            (state.claim_presence() == crate::custody::ClaimPresenceV1::Required).then(|| {
                PreservedWorktreeClaimV1 {
                    schema_version: WORKTREE_CUSTODY_RECORD_SCHEMA_V1,
                    custody_id: custody_id.clone(),
                    execution_id: ExecutionId::parse(format!("exec-{}", "1".repeat(32))).unwrap(),
                    // Ordinal 0: the delivery-origin attempt is the current one.
                    origin_attempt_id: attempt_identity().attempt_id,
                    current_attempt: attempt_identity(),
                    node: PolicyNodeRefV1 {
                        sorted_ordinal: 0,
                        id_sha256: sha('5'),
                    },
                    checkout_fingerprint: sha('6'),
                    source: object("/src"),
                    root: object("/root"),
                    worktree: object(worktree),
                    common_dir: object("/src/.git"),
                    reason: match &state {
                        WorktreeCustodyStateV1::PreservationUnknown { reason } => *reason,
                        _ => PreservationReasonV1::NodeFailure,
                    },
                    created_wall_ms: 1_700_000_000_000,
                    recovery_locator: RecoveryLocatorV1::RegisteredWorktree {},
                }
            });
        WorktreeCustodyRecordV1 {
            schema_version: WORKTREE_CUSTODY_RECORD_SCHEMA_V1,
            custody_id,
            checkout_fingerprint: sha('6'),
            current_attempt: attempt_identity(),
            worktree: object(worktree),
            state,
            claim,
        }
    }

    /// Materialize a V3 checkout: the worktree directory plus its sibling
    /// `.custody.v1.json` record. Returns `(worktree_path, record_path)`.
    fn write_custody_checkout(
        root: &Path,
        name: &str,
        state: WorktreeCustodyStateV1,
    ) -> (PathBuf, PathBuf) {
        let worktree_path = root.join(name);
        fs::create_dir_all(&worktree_path).unwrap();
        let canonical = fs::canonicalize(&worktree_path)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let record = custody_record(&canonical, state);
        let record_path = PathBuf::from(custody_record_path(&canonical));
        fs::write(&record_path, record.encode_canonical().unwrap()).unwrap();
        (PathBuf::from(canonical), record_path)
    }

    fn dead_probe(lease: &str) -> FakeProbe {
        FakeProbe(HashMap::from([(lease.to_string(), Some(true))]))
    }

    struct BothAbsentProbe;

    impl super::ExactAbsenceProbeV1 for BothAbsentProbe {
        fn observe_source_common_dir_identity(
            &self,
            source: &str,
        ) -> Result<DirectoryIdentityV1, bridge_core::error::BridgeError> {
            super::ExactAbsenceProbeV1::observe_source_common_dir_identity(
                &crate::host_git::HostGitWorktree::new(),
                source,
            )
        }

        fn observe_exact_absence(
            &self,
            _candidate: &super::ExactAbsenceCandidateV1,
        ) -> Result<super::ExactAbsenceObservationV1, bridge_core::error::BridgeError> {
            Ok(super::ExactAbsenceObservationV1::BothAbsent)
        }
    }

    struct RecordingProbe {
        calls: AtomicUsize,
        authority_calls: AtomicUsize,
        authority: Option<DirectoryIdentityV1>,
        authority_fails: bool,
        result: super::ExactAbsenceObservationV1,
    }

    impl RecordingProbe {
        fn both_absent() -> Self {
            Self {
                calls: AtomicUsize::new(0),
                authority_calls: AtomicUsize::new(0),
                authority: None,
                authority_fails: false,
                result: super::ExactAbsenceObservationV1::BothAbsent,
            }
        }

        fn with_authority(authority: DirectoryIdentityV1) -> Self {
            Self {
                authority: Some(authority),
                ..Self::both_absent()
            }
        }

        fn with_authority_failure() -> Self {
            Self {
                authority_fails: true,
                ..Self::both_absent()
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        fn authority_calls(&self) -> usize {
            self.authority_calls.load(Ordering::SeqCst)
        }
    }

    impl super::ExactAbsenceProbeV1 for RecordingProbe {
        fn observe_source_common_dir_identity(
            &self,
            source: &str,
        ) -> Result<DirectoryIdentityV1, bridge_core::error::BridgeError> {
            self.authority_calls.fetch_add(1, Ordering::SeqCst);
            if self.authority_fails {
                return Err(bridge_core::error::BridgeError::StoreFailure);
            }
            if let Some(authority) = &self.authority {
                return Ok(authority.clone());
            }
            super::ExactAbsenceProbeV1::observe_source_common_dir_identity(
                &crate::host_git::HostGitWorktree::new(),
                source,
            )
        }

        fn observe_exact_absence(
            &self,
            _candidate: &super::ExactAbsenceCandidateV1,
        ) -> Result<super::ExactAbsenceObservationV1, bridge_core::error::BridgeError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.result)
        }
    }

    struct RecordingHostGitProbe {
        exact_absence_calls: AtomicUsize,
        authority_calls: AtomicUsize,
    }

    impl RecordingHostGitProbe {
        const fn new() -> Self {
            Self {
                exact_absence_calls: AtomicUsize::new(0),
                authority_calls: AtomicUsize::new(0),
            }
        }
    }

    impl super::ExactAbsenceProbeV1 for RecordingHostGitProbe {
        fn observe_source_common_dir_identity(
            &self,
            source: &str,
        ) -> Result<DirectoryIdentityV1, bridge_core::error::BridgeError> {
            self.authority_calls.fetch_add(1, Ordering::SeqCst);
            super::ExactAbsenceProbeV1::observe_source_common_dir_identity(
                &crate::host_git::HostGitWorktree::new(),
                source,
            )
        }

        fn observe_exact_absence(
            &self,
            candidate: &super::ExactAbsenceCandidateV1,
        ) -> Result<super::ExactAbsenceObservationV1, bridge_core::error::BridgeError> {
            self.exact_absence_calls.fetch_add(1, Ordering::SeqCst);
            super::ExactAbsenceProbeV1::observe_exact_absence(
                &crate::host_git::HostGitWorktree::new(),
                candidate,
            )
        }
    }

    fn real_custody_record(
        root: &Path,
        target: &Path,
        state: WorktreeCustodyStateV1,
        claim_present: bool,
    ) -> WorktreeCustodyRecordV1 {
        let source = root.join("source");
        fs::create_dir_all(&source).unwrap();
        assert!(std::process::Command::new("git")
            .args(["-C", source.to_str().unwrap(), "init", "-q"])
            .status()
            .unwrap()
            .success());
        let target = fs::canonicalize(target).unwrap();
        let identity = |path: &Path| {
            let directory_identity =
                verify_payload_directory_identity(&fs::canonicalize(path).unwrap()).unwrap();
            WorktreeObjectIdentityV1 {
                canonical_path: directory_identity.canonical_path.clone(),
                directory_identity,
            }
        };
        let reason = match &state {
            WorktreeCustodyStateV1::PreservationUnknown { reason } => *reason,
            _ => PreservationReasonV1::NodeFailure,
        };
        let mut record = custody_record(&target.to_string_lossy(), state);
        record.worktree = identity(&target);
        record.claim = claim_present.then(|| PreservedWorktreeClaimV1 {
            schema_version: WORKTREE_CUSTODY_RECORD_SCHEMA_V1,
            custody_id: record.custody_id.clone(),
            execution_id: record.current_attempt.execution_id.clone(),
            origin_attempt_id: record.current_attempt.attempt_id.clone(),
            current_attempt: record.current_attempt.clone(),
            node: PolicyNodeRefV1 {
                sorted_ordinal: 0,
                id_sha256: sha('5'),
            },
            checkout_fingerprint: record.checkout_fingerprint.clone(),
            source: identity(&source),
            root: identity(root),
            worktree: record.worktree.clone(),
            common_dir: identity(&source.join(".git")),
            reason,
            created_wall_ms: 1_700_000_000_000,
            recovery_locator: RecoveryLocatorV1::RegisteredWorktree {},
        });
        record
    }

    fn write_real_custody_record(
        root: &Path,
        record_path: &Path,
        target: &Path,
        state: WorktreeCustodyStateV1,
        claim_present: bool,
    ) {
        let record = real_custody_record(root, target, state, claim_present);
        fs::write(record_path, record.encode_canonical().unwrap()).unwrap();
    }

    fn write_expected_custody_record(
        root: &Path,
        name: &str,
        state: WorktreeCustodyStateV1,
        claim_present: bool,
    ) -> PathBuf {
        let target = root.join(name);
        fs::create_dir_all(&target).unwrap();
        let target = fs::canonicalize(target).unwrap();
        let record_path = PathBuf::from(custody_record_path(&target.to_string_lossy()));
        write_real_custody_record(root, &record_path, &target, state, claim_present);
        record_path
    }

    fn write_host_git_custody_record(root: &Path, name: &str) -> (PathBuf, PathBuf, Vec<u8>) {
        fs::create_dir_all(root).unwrap();
        let target = root.join(name);
        fs::create_dir(&target).unwrap();
        let record = real_custody_record(
            root,
            &target,
            WorktreeCustodyStateV1::PreservationUnknown {
                reason: PreservationReasonV1::MaterializationInFlight,
            },
            true,
        );
        let source = root.join("source");
        for args in [
            ["config", "user.email", "a@b.c"].as_slice(),
            ["config", "user.name", "x"].as_slice(),
        ] {
            assert!(std::process::Command::new("git")
                .arg("-C")
                .arg(&source)
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        fs::write(source.join("file.txt"), "base\n").unwrap();
        for args in [
            ["add", "-A"].as_slice(),
            ["commit", "-q", "-m", "init"].as_slice(),
        ] {
            assert!(std::process::Command::new("git")
                .arg("-C")
                .arg(&source)
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        let record_path = PathBuf::from(custody_record_path(&record.worktree.canonical_path));
        let custody_bytes = record.encode_canonical().unwrap();
        fs::write(&record_path, custody_bytes).unwrap();
        let custody_bytes = fs::read(&record_path).unwrap();
        (target, record_path, custody_bytes)
    }

    fn custody_assessment<'a>(
        report: &'a super::ExactAbsenceSweepReportV1,
        record_path: &Path,
    ) -> &'a super::CustodyExactAbsenceAssessmentV1 {
        let entry = report
            .entries()
            .iter()
            .find(|entry| entry.record_path() == record_path.to_string_lossy())
            .expect("record must appear in the exact scan");
        match entry.assessment() {
            super::ExactAbsenceRecordAssessmentV1::Custody(assessment) => assessment.assessment(),
            _ => panic!("record must remain a readable custody row"),
        }
    }
    fn candidate_claim_record(root: &Path, name: &str) -> WorktreeCustodyRecordV1 {
        let target = root.join(name);
        fs::create_dir_all(&target).unwrap();
        real_custody_record(
            root,
            &target,
            WorktreeCustodyStateV1::ProtectionPrepared {},
            true,
        )
    }

    fn claim_object_mut(
        claim: &mut PreservedWorktreeClaimV1,
        object: super::ClaimAuthorityObjectV1,
    ) -> &mut WorktreeObjectIdentityV1 {
        match object {
            super::ClaimAuthorityObjectV1::Source => &mut claim.source,
            super::ClaimAuthorityObjectV1::Worktree => &mut claim.worktree,
            super::ClaimAuthorityObjectV1::Root => &mut claim.root,
            super::ClaimAuthorityObjectV1::CommonDirectory => &mut claim.common_dir,
            super::ClaimAuthorityObjectV1::SourceCommonDirectoryBinding => unreachable!(),
        }
    }

    fn claim_refusal(
        record: &WorktreeCustodyRecordV1,
        probe: &RecordingProbe,
    ) -> super::ClaimAuthorityUnavailableV1 {
        let claim = record.claim.as_ref().unwrap();
        let retained_root =
            super::RetainedCustodyRootAuthorityV1::Stable(claim.root.directory_identity.clone());
        super::ExactAbsenceCandidateV1::from_claim(
            &claim.source,
            &claim.root,
            &claim.common_dir,
            &claim.worktree,
            &retained_root,
            probe,
        )
        .unwrap_err()
    }

    fn assert_candidate_control(root: &Path) {
        let control = write_expected_custody_record(
            root,
            "control",
            WorktreeCustodyStateV1::ProtectionPrepared {},
            true,
        );
        let probe = RecordingProbe::both_absent();
        let report = super::sweep_orphans_with_exact_absence(&root.to_string_lossy(), &probe);
        assert_eq!(
            custody_assessment(&report, &control),
            &super::CustodyExactAbsenceAssessmentV1::Assessed(
                super::UnusedCandidateDecisionV1::Authorized,
            )
        );
        assert!(matches!(
            report.scan().custody_root(),
            super::CustodyRootObservationV1::Pinned
        ));
        assert_eq!(report.effective().count(), 0);
        assert_eq!(probe.calls(), 1);
        assert_eq!(probe.authority_calls(), 1);
    }

    #[test]
    fn claim_authority_mapping_covers_every_constructible_object_and_reason() {
        let root = unique_temp_dir("claim-authority-fields");
        fs::create_dir_all(&root).unwrap();
        for (name, object, reason) in [
            ("source-mismatch", Object::Source, Reason::PathMismatch),
            ("root-mismatch", Object::Root, Reason::PathMismatch),
            ("worktree-mismatch", Object::Worktree, Reason::PathMismatch),
            (
                "common-mismatch",
                Object::CommonDirectory,
                Reason::PathMismatch,
            ),
            ("source-relative", Object::Source, Reason::NotAbsolute),
            ("root-relative", Object::Root, Reason::NotAbsolute),
            ("worktree-relative", Object::Worktree, Reason::NotAbsolute),
            (
                "common-relative",
                Object::CommonDirectory,
                Reason::NotAbsolute,
            ),
            ("root-incomplete", Object::Root, Reason::IdentityIncomplete),
            (
                "source-incomplete",
                Object::Source,
                Reason::IdentityIncomplete,
            ),
            (
                "common-incomplete",
                Object::CommonDirectory,
                Reason::IdentityIncomplete,
            ),
            ("source-changed", Object::Source, Reason::IdentityChanged),
            (
                "common-changed",
                Object::CommonDirectory,
                Reason::IdentityChanged,
            ),
        ] {
            let mut record = candidate_claim_record(&root, name);
            let field = claim_object_mut(record.claim.as_mut().unwrap(), object);
            match reason {
                super::ClaimAuthorityUnavailableReasonV1::PathMismatch => {
                    field.canonical_path.push_str(".mismatch");
                }
                super::ClaimAuthorityUnavailableReasonV1::NotAbsolute => {
                    field.canonical_path = "relative".to_string();
                    field.directory_identity.canonical_path = "relative".to_string();
                }
                super::ClaimAuthorityUnavailableReasonV1::IdentityIncomplete => {
                    field.directory_identity.dev = None;
                    field.directory_identity.ino = None;
                    field.directory_identity.btime = None;
                }
                super::ClaimAuthorityUnavailableReasonV1::IdentityChanged => {
                    field.directory_identity.ino = Some(0);
                }
                _ => unreachable!(),
            }
            let probe = RecordingProbe::both_absent();
            assert_eq!(
                claim_refusal(&record, &probe),
                super::ClaimAuthorityUnavailableV1::new(object, reason),
                "{name}"
            );
            assert_eq!(probe.authority_calls(), 0, "{name}");
        }
        let mut root_changed = candidate_claim_record(&root, "root-changed");
        let claim = root_changed.claim.as_mut().unwrap();
        let retained_root =
            super::RetainedCustodyRootAuthorityV1::Stable(claim.root.directory_identity.clone());
        claim.root.directory_identity.ino = Some(0);
        let probe = RecordingProbe::both_absent();
        assert_eq!(
            super::ExactAbsenceCandidateV1::from_claim(
                &claim.source,
                &claim.root,
                &claim.common_dir,
                &claim.worktree,
                &retained_root,
                &probe,
            )
            .unwrap_err(),
            super::ClaimAuthorityUnavailableV1::new(Object::Root, Reason::IdentityChanged),
        );
        assert_eq!(probe.authority_calls(), 0);

        let unavailable = candidate_claim_record(&root, "root-unavailable");
        let claim = unavailable.claim.as_ref().unwrap();
        let probe = RecordingProbe::both_absent();
        assert_eq!(
            super::ExactAbsenceCandidateV1::from_claim(
                &claim.source,
                &claim.root,
                &claim.common_dir,
                &claim.worktree,
                &super::RetainedCustodyRootAuthorityV1::Unavailable,
                &probe,
            )
            .unwrap_err(),
            super::ClaimAuthorityUnavailableV1::new(Object::Root, Reason::ObservationUnavailable),
        );
        assert_eq!(probe.authority_calls(), 0);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn claim_authority_errors_are_typed_and_never_reported_as_assessed() {
        let root = unique_temp_dir("claim-authority-report");
        fs::create_dir_all(&root).unwrap();
        let mut record = candidate_claim_record(&root, "source-missing");
        let missing = root.join("missing-source").to_string_lossy().into_owned();
        let source = &mut record.claim.as_mut().unwrap().source;
        source.canonical_path = missing.clone();
        source.directory_identity.canonical_path = missing;
        let record_path = PathBuf::from(custody_record_path(&record.worktree.canonical_path));
        let custody_bytes = record.encode_canonical().unwrap();
        fs::write(&record_path, custody_bytes).unwrap();
        let custody_bytes = fs::read(&record_path).unwrap();
        let probe = RecordingProbe::both_absent();
        let report = super::sweep_orphans_with_exact_absence(&root.to_string_lossy(), &probe);
        assert!(matches!(
            custody_assessment(&report, &record_path),
            super::CustodyExactAbsenceAssessmentV1::CannotConstructSubject(
                super::CannotConstructSubjectV1::ClaimAuthorityUnavailable(refusal)
            ) if refusal.object() == Object::Source
                && refusal.reason() == Reason::ObservationUnavailable
        ));
        assert_eq!(probe.calls(), 0);
        assert_eq!(probe.authority_calls(), 0);
        assert_eq!(fs::read(&record_path).unwrap(), custody_bytes);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn claim_authority_maps_common_and_binding_observations() {
        let root = unique_temp_dir("claim-authority-observations");
        fs::create_dir_all(&root).unwrap();

        let mut common_missing = candidate_claim_record(&root, "common-missing");
        let missing = root.join("missing-common").to_string_lossy().into_owned();
        let common_dir = &mut common_missing.claim.as_mut().unwrap().common_dir;
        common_dir.canonical_path = missing.clone();
        common_dir.directory_identity.canonical_path = missing;
        let probe = RecordingProbe::both_absent();
        assert_eq!(
            claim_refusal(&common_missing, &probe),
            super::ClaimAuthorityUnavailableV1::new(
                Object::CommonDirectory,
                Reason::ObservationUnavailable,
            )
        );
        assert_eq!(probe.authority_calls(), 0);

        let authority_failure = candidate_claim_record(&root, "authority-failure");
        let probe = RecordingProbe::with_authority_failure();
        assert_eq!(
            claim_refusal(&authority_failure, &probe),
            super::ClaimAuthorityUnavailableV1::new(
                Object::SourceCommonDirectoryBinding,
                Reason::ObservationUnavailable,
            )
        );
        assert_eq!(probe.calls(), 0);
        assert_eq!(probe.authority_calls(), 1);

        let authority_mismatch = candidate_claim_record(&root, "authority-mismatch");
        let wrong_common_dir = authority_mismatch
            .claim
            .as_ref()
            .unwrap()
            .source
            .directory_identity
            .clone();
        let probe = RecordingProbe::with_authority(wrong_common_dir);
        assert_eq!(
            claim_refusal(&authority_mismatch, &probe),
            super::ClaimAuthorityUnavailableV1::new(
                Object::SourceCommonDirectoryBinding,
                Reason::OwnershipUnproven,
            )
        );
        assert_eq!(probe.calls(), 0);
        assert_eq!(probe.authority_calls(), 1);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn degraded_claim_authority_matrix_has_sixteen_rows_and_only_worktree_degradation_probes() {
        let rows = [
            (
                "1",
                false,
                false,
                false,
                false,
                Some(Object::Source),
                0,
                0,
                false,
            ),
            (
                "2",
                false,
                false,
                false,
                true,
                Some(Object::Source),
                0,
                0,
                false,
            ),
            (
                "3",
                false,
                false,
                true,
                false,
                Some(Object::Source),
                0,
                0,
                false,
            ),
            (
                "4",
                false,
                false,
                true,
                true,
                Some(Object::Source),
                0,
                0,
                false,
            ),
            (
                "5",
                false,
                true,
                false,
                false,
                Some(Object::Source),
                0,
                0,
                false,
            ),
            (
                "6",
                false,
                true,
                false,
                true,
                Some(Object::Source),
                0,
                0,
                false,
            ),
            (
                "7",
                false,
                true,
                true,
                false,
                Some(Object::Source),
                0,
                0,
                false,
            ),
            (
                "8",
                false,
                true,
                true,
                true,
                Some(Object::Source),
                0,
                0,
                false,
            ),
            (
                "9",
                true,
                false,
                false,
                false,
                Some(Object::Root),
                0,
                0,
                false,
            ),
            (
                "10",
                true,
                false,
                false,
                true,
                Some(Object::Root),
                0,
                0,
                false,
            ),
            (
                "11",
                true,
                false,
                true,
                false,
                Some(Object::Root),
                0,
                0,
                false,
            ),
            (
                "12",
                true,
                false,
                true,
                true,
                Some(Object::Root),
                0,
                0,
                false,
            ),
            (
                "13",
                true,
                true,
                false,
                false,
                Some(Object::CommonDirectory),
                0,
                0,
                false,
            ),
            (
                "14",
                true,
                true,
                false,
                true,
                Some(Object::CommonDirectory),
                0,
                0,
                false,
            ),
            ("15", true, true, true, false, None, 1, 1, false),
            ("16", true, true, true, true, None, 1, 1, true),
        ];

        for (
            name,
            source,
            root_complete,
            common_dir,
            worktree,
            expected,
            authority,
            exact,
            historical,
        ) in rows
        {
            let root = unique_temp_dir(&format!("degraded-matrix-{name}"));
            fs::create_dir_all(&root).unwrap();
            let target = root.join(format!("worktree-{name}"));
            fs::create_dir(&target).unwrap();
            let mut record = real_custody_record(
                &root,
                &target,
                WorktreeCustodyStateV1::PreservationUnknown {
                    reason: PreservationReasonV1::MaterializationInFlight,
                },
                true,
            );
            let degrade = |identity: &mut WorktreeObjectIdentityV1| {
                identity.directory_identity.dev = None;
                identity.directory_identity.ino = None;
                identity.directory_identity.btime = None;
            };
            if !worktree {
                degrade(&mut record.worktree);
            }
            let claim = record.claim.as_mut().unwrap();
            if !source {
                degrade(&mut claim.source);
            }
            if !root_complete {
                degrade(&mut claim.root);
            }
            if !common_dir {
                degrade(&mut claim.common_dir);
            }
            if !worktree {
                degrade(&mut claim.worktree);
            }
            let record_path = PathBuf::from(custody_record_path(&record.worktree.canonical_path));
            let custody_bytes = record.encode_canonical().unwrap();
            fs::write(&record_path, custody_bytes).unwrap();
            let custody_bytes = fs::read(&record_path).unwrap();
            if historical {
                fs::remove_dir_all(&target).unwrap();
            }

            let probe = RecordingProbe::both_absent();
            let report = super::sweep_orphans_with_exact_absence(&root.to_string_lossy(), &probe);
            let assessment = custody_assessment(&report, &record_path);
            match expected {
                Some(object) => assert!(
                    matches!(
                        assessment,
                        super::CustodyExactAbsenceAssessmentV1::CannotConstructSubject(
                            super::CannotConstructSubjectV1::ClaimAuthorityUnavailable(refusal)
                        ) if refusal.object() == object
                            && refusal.reason() == Reason::IdentityIncomplete
                    ),
                    "row {name} must be the requested typed construction refusal: {assessment:?}"
                ),
                None => assert_eq!(
                    assessment,
                    &super::CustodyExactAbsenceAssessmentV1::Assessed(
                        super::UnusedCandidateDecisionV1::Authorized,
                    ),
                    "row {name} must reach exact absence"
                ),
            }
            assert_eq!(probe.authority_calls(), authority, "row {name}");
            assert_eq!(probe.calls(), exact, "row {name}");
            assert_eq!(fs::read(&record_path).unwrap(), custody_bytes, "row {name}");
            fs::remove_dir_all(&root).unwrap();
        }
    }

    #[test]
    fn persisted_record_host_git_exact_absence_matrix_preserves_bytes() {
        enum Expected {
            Authorized,
            Refused,
        }

        for (name, registered, expected) in [
            ("target-present", false, Expected::Refused),
            ("registered-absent", true, Expected::Refused),
            ("both-absent", false, Expected::Authorized),
        ] {
            let root = unique_temp_dir(&format!("host-git-{name}"));
            let (target, record_path, custody_bytes) = write_host_git_custody_record(&root, name);
            if name != "target-present" {
                fs::remove_dir_all(&target).unwrap();
            }
            if registered {
                let source = root.join("source");
                assert!(std::process::Command::new("git")
                    .arg("-C")
                    .arg(&source)
                    .args(["worktree", "add", "--detach", target.to_str().unwrap()])
                    .status()
                    .unwrap()
                    .success());
                fs::remove_dir_all(&target).unwrap();
            }

            let report = super::sweep_orphans_with_exact_absence(
                &root.to_string_lossy(),
                &crate::host_git::HostGitWorktree::new(),
            );
            let assessment = custody_assessment(&report, &record_path);
            match expected {
                Expected::Authorized => assert_eq!(
                    assessment,
                    &super::CustodyExactAbsenceAssessmentV1::Assessed(
                        super::UnusedCandidateDecisionV1::Authorized,
                    ),
                    "{name} must retain only the real BothAbsent outcome"
                ),
                Expected::Refused => assert_eq!(
                    assessment,
                    &super::CustodyExactAbsenceAssessmentV1::Assessed(
                        super::UnusedCandidateDecisionV1::Refused,
                    ),
                    "{name} must not produce BothAbsent"
                ),
            }
            assert_eq!(fs::read(&record_path).unwrap(), custody_bytes, "{name}");
            fs::remove_dir_all(&root).unwrap();
        }
    }

    #[test]
    fn persisted_record_degraded_worktree_reaches_host_git_and_preserves_bytes() {
        let root = unique_temp_dir("host-git-degraded-worktree");
        let (target, record_path, _) = write_host_git_custody_record(&root, "worktree");
        let mut record =
            WorktreeCustodyRecordV1::decode_canonical(&fs::read(&record_path).unwrap()).unwrap();
        let degrade = |identity: &mut WorktreeObjectIdentityV1| {
            identity.directory_identity.dev = None;
            identity.directory_identity.ino = None;
            identity.directory_identity.btime = None;
        };
        degrade(&mut record.worktree);
        degrade(&mut record.claim.as_mut().unwrap().worktree);
        let custody_bytes = record.encode_canonical().unwrap();
        fs::write(&record_path, custody_bytes).unwrap();
        let custody_bytes = fs::read(&record_path).unwrap();
        fs::remove_dir_all(&target).unwrap();

        let report = super::sweep_orphans_with_exact_absence(
            &root.to_string_lossy(),
            &crate::host_git::HostGitWorktree::new(),
        );
        assert_eq!(
            custody_assessment(&report, &record_path),
            &super::CustodyExactAbsenceAssessmentV1::Assessed(
                super::UnusedCandidateDecisionV1::Authorized,
            )
        );
        assert_eq!(fs::read(&record_path).unwrap(), custody_bytes);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn persisted_record_historical_complete_worktree_reaches_host_git_and_preserves_bytes() {
        let root = unique_temp_dir("host-git-historical-worktree");
        let (target, record_path, custody_bytes) = write_host_git_custody_record(&root, "worktree");
        fs::remove_dir_all(&target).unwrap();

        let report = super::sweep_orphans_with_exact_absence(
            &root.to_string_lossy(),
            &crate::host_git::HostGitWorktree::new(),
        );
        assert_eq!(
            custody_assessment(&report, &record_path),
            &super::CustodyExactAbsenceAssessmentV1::Assessed(
                super::UnusedCandidateDecisionV1::Authorized,
            )
        );
        assert_eq!(fs::read(&record_path).unwrap(), custody_bytes);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn persisted_record_wrong_repository_binding_is_typed_and_preserves_bytes() {
        let root = unique_temp_dir("wrong-repository-binding");
        let (_, record_path, _) = write_host_git_custody_record(&root, "worktree");
        let repository_b = root.join("repository-b");
        fs::create_dir(&repository_b).unwrap();
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(&repository_b)
            .args(["init", "-q"])
            .status()
            .unwrap()
            .success());
        let mut record =
            WorktreeCustodyRecordV1::decode_canonical(&fs::read(&record_path).unwrap()).unwrap();
        let common_dir = verify_payload_directory_identity(
            &fs::canonicalize(repository_b.join(".git")).unwrap(),
        )
        .unwrap();
        record.claim.as_mut().unwrap().common_dir = WorktreeObjectIdentityV1 {
            canonical_path: common_dir.canonical_path.clone(),
            directory_identity: common_dir,
        };
        let custody_bytes = record.encode_canonical().unwrap();
        fs::write(&record_path, custody_bytes).unwrap();
        let custody_bytes = fs::read(&record_path).unwrap();

        let probe = RecordingHostGitProbe::new();
        let report = super::sweep_orphans_with_exact_absence(&root.to_string_lossy(), &probe);
        assert!(matches!(
            custody_assessment(&report, &record_path),
            super::CustodyExactAbsenceAssessmentV1::CannotConstructSubject(
                super::CannotConstructSubjectV1::ClaimAuthorityUnavailable(refusal)
            ) if refusal.object() == Object::SourceCommonDirectoryBinding
                && refusal.reason() == Reason::OwnershipUnproven
        ));
        assert_eq!(probe.authority_calls.load(Ordering::SeqCst), 1);
        assert_eq!(probe.exact_absence_calls.load(Ordering::SeqCst), 0);
        assert_eq!(fs::read(&record_path).unwrap(), custody_bytes);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn persisted_record_root_replacement_during_git_refuses_and_preserves_bytes() {
        let root = unique_temp_dir("root-replacement-during-git");
        let (target, record_path, custody_bytes) = write_host_git_custody_record(&root, "worktree");
        fs::remove_dir_all(&target).unwrap();
        let replacement_root = root.with_file_name(format!(
            "{}-original",
            root.file_name().unwrap().to_string_lossy()
        ));
        let record_name = record_path.file_name().unwrap().to_os_string();
        let record_name_after = record_name.clone();
        let original_root = replacement_root.clone();
        let root_for_hook = root.clone();
        crate::host_git::set_exact_absence_after_initial_revalidation_hook(move || {
            fs::rename(&root_for_hook, &replacement_root).unwrap();
            fs::create_dir(&root_for_hook).unwrap();
            fs::rename(
                replacement_root.join("source"),
                root_for_hook.join("source"),
            )
            .unwrap();
            fs::rename(
                replacement_root.join(&record_name),
                root_for_hook.join(&record_name),
            )
            .unwrap();
        });

        let report = super::sweep_orphans_with_exact_absence(
            &root.to_string_lossy(),
            &crate::host_git::HostGitWorktree::new(),
        );
        assert_eq!(
            custody_assessment(&report, &record_path),
            &super::CustodyExactAbsenceAssessmentV1::Assessed(
                super::UnusedCandidateDecisionV1::Refused,
            )
        );
        assert_eq!(
            fs::read(root.join(record_name_after)).unwrap(),
            custody_bytes
        );
        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&original_root).unwrap();
    }

    #[test]
    fn stale_source_root_and_common_directory_objects_refuse_without_exact_absence() {
        for (name, object) in [
            ("source", Object::Source),
            ("root", Object::Root),
            ("common-directory", Object::CommonDirectory),
        ] {
            let root = unique_temp_dir(&format!("stale-{name}"));
            let (_, record_path, custody_bytes) = write_host_git_custody_record(&root, "worktree");
            let mut original_root = None;
            match object {
                Object::Source => {
                    fs::rename(root.join("source"), root.join("original-source")).unwrap();
                    fs::create_dir(root.join("source")).unwrap();
                    assert!(std::process::Command::new("git")
                        .arg("-C")
                        .arg(root.join("source"))
                        .args(["init", "-q"])
                        .status()
                        .unwrap()
                        .success());
                }
                Object::Root => {
                    let old_root = root.with_file_name(format!(
                        "{}-original",
                        root.file_name().unwrap().to_string_lossy()
                    ));
                    let record_name = record_path.file_name().unwrap().to_os_string();
                    fs::rename(&root, &old_root).unwrap();
                    fs::create_dir(&root).unwrap();
                    fs::rename(old_root.join("source"), root.join("source")).unwrap();
                    fs::rename(old_root.join(record_name), &record_path).unwrap();
                    original_root = Some(old_root);
                }
                Object::CommonDirectory => {
                    fs::rename(root.join("source/.git"), root.join("original-common")).unwrap();
                    assert!(std::process::Command::new("git")
                        .arg("-C")
                        .arg(root.join("source"))
                        .args(["init", "-q"])
                        .status()
                        .unwrap()
                        .success());
                }
                _ => unreachable!(),
            }

            let probe = RecordingProbe::both_absent();
            let report = super::sweep_orphans_with_exact_absence(&root.to_string_lossy(), &probe);
            assert!(matches!(
                custody_assessment(&report, &record_path),
                super::CustodyExactAbsenceAssessmentV1::CannotConstructSubject(
                    super::CannotConstructSubjectV1::ClaimAuthorityUnavailable(refusal)
                ) if refusal.object() == object
                    && refusal.reason() == Reason::IdentityChanged
            ));
            assert_eq!(probe.authority_calls(), 0, "{name}");
            assert_eq!(probe.calls(), 0, "{name}");
            assert_eq!(fs::read(&record_path).unwrap(), custody_bytes, "{name}");
            fs::remove_dir_all(&root).unwrap();
            if let Some(original_root) = original_root {
                fs::remove_dir_all(original_root).unwrap();
            }
        }
    }

    fn root_capture(
        dev: Option<u64>,
        ino: Option<u64>,
        birthtime: Option<bridge_core::fs_custody::BirthTimeV1>,
    ) -> super::checked_scan::RootIdentityCaptureV1 {
        super::checked_scan::RootIdentityCaptureV1 {
            dev,
            ino,
            birthtime,
        }
    }

    fn root_observations(
        retained_enumeration_object: super::checked_scan::RootIdentityCaptureV1,
        pinned_custody_directory: super::checked_scan::RootIdentityCaptureV1,
        final_named_root: super::checked_scan::RootIdentityCaptureV1,
    ) -> super::RootObservationSetV1 {
        super::RootObservationSetV1 {
            retained_enumeration_object: Some(retained_enumeration_object),
            pinned_custody_directory: Some(pinned_custody_directory),
            final_named_root: Some(final_named_root),
        }
    }

    #[test]
    fn root_observation_classifier_reports_pinned_captures() {
        let capture = root_capture(Some(1), Some(2), None);
        assert_eq!(
            super::classify_root_observations(root_observations(capture, capture, capture)),
            super::CustodyRootObservationV1::Pinned
        );
    }

    #[test]
    fn root_observation_classifier_reports_identity_changes_including_birthtime() {
        let birthtime = bridge_core::fs_custody::BirthTimeV1::new(1, 2).unwrap();
        let capture = root_capture(Some(1), Some(2), Some(birthtime));
        assert_eq!(
            super::classify_root_observations(root_observations(
                capture,
                root_capture(Some(1), Some(3), Some(birthtime)),
                capture,
            )),
            super::CustodyRootObservationV1::IdentityChanged
        );
        assert_eq!(
            super::classify_root_observations(root_observations(
                capture,
                root_capture(Some(1), Some(2), None),
                capture,
            )),
            super::CustodyRootObservationV1::IdentityChanged
        );
    }

    #[test]
    fn root_observation_classifier_refuses_incomplete_captures() {
        let capture = root_capture(Some(1), Some(2), None);
        assert_eq!(
            super::classify_root_observations(root_observations(
                capture,
                root_capture(None, Some(2), None),
                capture,
            )),
            super::CustodyRootObservationV1::Unavailable
        );
    }

    #[test]
    fn exact_absence_sweep_reports_cannot_canonicalize() {
        let unique_root = unique_temp_dir("exact-absence-cannot-canonicalize");
        let root = PathBuf::from(unique_root.file_name().unwrap());
        assert!(!root.exists());
        let requested_root = root.to_string_lossy().into_owned();

        let report = super::sweep_orphans_with_exact_absence(&requested_root, &BothAbsentProbe);

        assert_eq!(report.requested_root(), requested_root);
        assert_eq!(report.canonical_root(), None);
        assert!(matches!(
            report.scan().enumeration(),
            super::ExactAbsenceEnumerationV1::Refused(
                super::ExactAbsenceRootRefusalV1::CannotCanonicalize
            )
        ));
        assert_eq!(
            report.scan().custody_root(),
            super::CustodyRootObservationV1::Unavailable
        );
        assert!(report.entries().is_empty());
    }

    // Evidence: genuine runtime report projection with a frozen base-tree control.
    #[test]
    fn exact_absence_sweep_reports_the_stored_runtime_decision() {
        let root = unique_temp_dir("exact-absence-report");
        fs::create_dir_all(root.join("source")).unwrap();
        assert!(std::process::Command::new("git")
            .args(["-C", root.join("source").to_str().unwrap(), "init", "-q"])
            .status()
            .unwrap()
            .success());
        let legacy = write_worktree_sidecar(&root, "legacy", "host", "/leases/live", "run");
        let (_, custody_record) =
            write_custody_checkout(&root, "custody", WorktreeCustodyStateV1::LiveProtected {});
        let unreadable_record = root.join("unreadable.custody.v1.json");
        fs::write(&unreadable_record, b"not canonical custody JSON").unwrap();
        let requested_root = root.join(".").to_string_lossy().into_owned();
        let canonical_root = fs::canonicalize(&root)
            .unwrap()
            .to_string_lossy()
            .into_owned();

        let report = super::sweep_orphans_with_exact_absence(&requested_root, &BothAbsentProbe);

        assert_eq!(report.requested_root(), requested_root);
        assert_eq!(report.canonical_root(), Some(canonical_root.as_str()));
        assert!(matches!(
            report.scan().enumeration(),
            super::ExactAbsenceEnumerationV1::Complete
        ));
        assert_eq!(
            report.scan().custody_root(),
            super::CustodyRootObservationV1::Pinned
        );
        assert!(report.has_authoritative_scan());
        assert_eq!(report.entries().len(), 3);
        assert_eq!(report.effective().count(), 0);

        let entry = |path: &Path| {
            let path = fs::canonicalize(path).unwrap();
            report
                .entries()
                .iter()
                .find(|entry| entry.record_path() == path.to_string_lossy())
                .unwrap()
        };
        let legacy_entry = entry(Path::new(&sidecar_path(&legacy.worktree_path)));
        assert!(matches!(
            legacy_entry.assessment(),
            super::ExactAbsenceRecordAssessmentV1::Legacy(
                super::UnusedCandidateDecisionV1::Authorized
            )
        ));
        let custody_entry = entry(&custody_record);
        assert!(matches!(
            custody_entry.assessment(),
            super::ExactAbsenceRecordAssessmentV1::Custody(assessment)
                if assessment.state().kind() == WorktreeCustodyStateKindV1::LiveProtected
                    && matches!(
                        assessment.assessment(),
                        super::CustodyExactAbsenceAssessmentV1::IneligiblePopulation(
                            super::IneligiblePopulationV1::StateNotCandidate
                        )
                    )
        ));
        let unreadable_entry = entry(&unreadable_record);
        assert!(matches!(
            unreadable_entry.assessment(),
            super::ExactAbsenceRecordAssessmentV1::UnreadableCustody(_)
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn population_admission_covers_every_decodable_population_and_probes_only_candidates() {
        enum Expected {
            Candidate,
            BareProtectionPrepared,
            StateNotCandidate,
        }

        let root = unique_temp_dir("population-admission");
        fs::create_dir_all(&root).unwrap();
        let mut records = Vec::new();
        for (name, state, claim_present, expected) in [
            (
                "protection-with-claim",
                WorktreeCustodyStateV1::ProtectionPrepared {},
                true,
                Expected::Candidate,
            ),
            (
                "protection-bare",
                WorktreeCustodyStateV1::ProtectionPrepared {},
                false,
                Expected::BareProtectionPrepared,
            ),
            (
                "unknown-in-flight",
                WorktreeCustodyStateV1::PreservationUnknown {
                    reason: PreservationReasonV1::MaterializationInFlight,
                },
                true,
                Expected::Candidate,
            ),
            (
                "unknown-node-failure",
                WorktreeCustodyStateV1::PreservationUnknown {
                    reason: PreservationReasonV1::NodeFailure,
                },
                true,
                Expected::StateNotCandidate,
            ),
            (
                "unknown-cancellation",
                WorktreeCustodyStateV1::PreservationUnknown {
                    reason: PreservationReasonV1::Cancellation,
                },
                true,
                Expected::StateNotCandidate,
            ),
            (
                "unknown-ambiguous",
                WorktreeCustodyStateV1::PreservationUnknown {
                    reason: PreservationReasonV1::AmbiguousCleanup,
                },
                true,
                Expected::StateNotCandidate,
            ),
            (
                "unknown-postcondition",
                WorktreeCustodyStateV1::PreservationUnknown {
                    reason: PreservationReasonV1::PostConditionDisagreement,
                },
                true,
                Expected::StateNotCandidate,
            ),
            (
                "unknown-removal-failed",
                WorktreeCustodyStateV1::PreservationUnknown {
                    reason: PreservationReasonV1::RemovalFailed,
                },
                true,
                Expected::StateNotCandidate,
            ),
            (
                "preservation-prepared",
                WorktreeCustodyStateV1::PreservationPrepared {},
                true,
                Expected::StateNotCandidate,
            ),
            (
                "preserved",
                WorktreeCustodyStateV1::Preserved {},
                true,
                Expected::StateNotCandidate,
            ),
            (
                "unused-settled",
                WorktreeCustodyStateV1::UnusedSettled {},
                false,
                Expected::StateNotCandidate,
            ),
            (
                "materializing",
                WorktreeCustodyStateV1::Materializing {},
                false,
                Expected::StateNotCandidate,
            ),
            (
                "live-protected",
                WorktreeCustodyStateV1::LiveProtected {},
                false,
                Expected::StateNotCandidate,
            ),
            (
                "delete-authorized",
                WorktreeCustodyStateV1::DeleteAuthorized {},
                false,
                Expected::StateNotCandidate,
            ),
            (
                "removed",
                WorktreeCustodyStateV1::Removed {},
                false,
                Expected::StateNotCandidate,
            ),
            (
                "recovered-live",
                WorktreeCustodyStateV1::RecoveredLive {
                    predecessor_claim_digest: sha('7'),
                },
                false,
                Expected::StateNotCandidate,
            ),
        ] {
            let expected = match expected {
                Expected::Candidate => super::CustodyExactAbsenceAssessmentV1::Assessed(
                    super::UnusedCandidateDecisionV1::Authorized,
                ),
                Expected::BareProtectionPrepared => {
                    super::CustodyExactAbsenceAssessmentV1::IneligiblePopulation(
                        super::IneligiblePopulationV1::BareProtectionPrepared,
                    )
                }
                Expected::StateNotCandidate => {
                    super::CustodyExactAbsenceAssessmentV1::IneligiblePopulation(
                        super::IneligiblePopulationV1::StateNotCandidate,
                    )
                }
            };
            records.push((
                write_expected_custody_record(&root, name, state, claim_present),
                expected,
            ));
        }
        let probe = RecordingProbe::both_absent();
        let report = super::sweep_orphans_with_exact_absence(&root.to_string_lossy(), &probe);

        for (record, expected) in &records {
            assert_eq!(custody_assessment(&report, record), expected, "{record:?}");
        }
        assert_eq!(probe.calls(), 2);
        assert_eq!(probe.authority_calls(), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_canonical_preserved_record_stops_before_the_probe_with_a_matched_control() {
        let root = unique_temp_dir("preserved-admission");
        fs::create_dir_all(&root).unwrap();
        let preserved = write_expected_custody_record(
            &root,
            "preserved",
            WorktreeCustodyStateV1::Preserved {},
            true,
        );
        let probe = RecordingProbe::both_absent();
        let report = super::sweep_orphans_with_exact_absence(&root.to_string_lossy(), &probe);

        assert_eq!(
            custody_assessment(&report, &preserved),
            &super::CustodyExactAbsenceAssessmentV1::IneligiblePopulation(
                super::IneligiblePopulationV1::StateNotCandidate,
            )
        );
        assert_eq!(probe.calls(), 0);
        assert_eq!(probe.authority_calls(), 0);
        assert_candidate_control(&root);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn an_expected_sibling_symlink_alias_is_refused_with_a_matched_control() {
        use std::os::unix::fs::symlink;

        let root = unique_temp_dir("sibling-symlink-alias");
        fs::create_dir_all(&root).unwrap();
        let canonical_root = fs::canonicalize(&root).unwrap();
        let target = canonical_root.join("wt");
        fs::create_dir_all(&target).unwrap();
        let alias = canonical_root.join("alias.custody.v1.json");
        write_real_custody_record(
            &root,
            &alias,
            &target,
            WorktreeCustodyStateV1::ProtectionPrepared {},
            true,
        );
        let expected = PathBuf::from(custody_record_path(&target.to_string_lossy()));
        symlink(&alias, &expected).unwrap();
        let probe = RecordingProbe::both_absent();
        let report = super::sweep_orphans_with_exact_absence(&root.to_string_lossy(), &probe);

        assert_eq!(
            custody_assessment(&report, &alias),
            &super::CustodyExactAbsenceAssessmentV1::CannotConstructSubject(
                super::CannotConstructSubjectV1::RecordFileNotExpectedSibling,
            )
        );
        let symlink_entry = report
            .entries()
            .iter()
            .find(|entry| entry.record_path() == expected.to_string_lossy())
            .expect("symlink must be retained as an unreadable custody row");
        assert!(matches!(
            symlink_entry.assessment(),
            super::ExactAbsenceRecordAssessmentV1::UnreadableCustody(_)
        ));
        assert_eq!(probe.calls(), 0);

        assert_candidate_control(&root);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_nested_target_whose_record_sits_at_the_root_is_not_an_expected_sibling() {
        let root = unique_temp_dir("nested-target-sibling");
        fs::create_dir_all(&root).unwrap();
        let canonical_root = fs::canonicalize(&root).unwrap();
        let target = canonical_root.join("sub/wt");
        fs::create_dir_all(&target).unwrap();
        let record = canonical_root.join("wt.custody.v1.json");
        write_real_custody_record(
            &root,
            &record,
            &target,
            WorktreeCustodyStateV1::ProtectionPrepared {},
            true,
        );
        let probe = RecordingProbe::both_absent();
        let report = super::sweep_orphans_with_exact_absence(&root.to_string_lossy(), &probe);

        assert_eq!(
            custody_assessment(&report, &record),
            &super::CustodyExactAbsenceAssessmentV1::CannotConstructSubject(
                super::CannotConstructSubjectV1::RecordFileNotExpectedSibling,
            )
        );
        assert_eq!(probe.calls(), 0);

        assert_candidate_control(&root);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_target_resolving_outside_the_sweep_root_is_typed_outside_root() {
        use std::os::unix::fs::symlink;

        let root = unique_temp_dir("outside-sweep-root");
        let outside = unique_temp_dir("outside-sweep-root-target");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let canonical_root = fs::canonicalize(&root).unwrap();
        let alias = canonical_root.join("alias");
        symlink(&outside, &alias).unwrap();
        let record = canonical_root.join("alias.custody.v1.json");
        write_real_custody_record(
            &root,
            &record,
            &alias,
            WorktreeCustodyStateV1::ProtectionPrepared {},
            true,
        );
        let probe = RecordingProbe::both_absent();
        let report = super::sweep_orphans_with_exact_absence(&root.to_string_lossy(), &probe);

        assert_eq!(
            custody_assessment(&report, &record),
            &super::CustodyExactAbsenceAssessmentV1::CannotConstructSubject(
                super::CannotConstructSubjectV1::OutsideSweepRoot,
            )
        );
        assert_eq!(probe.calls(), 0);

        assert_candidate_control(&root);
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn a_relative_recorded_target_reports_the_first_guard_only() {
        let root = unique_temp_dir("relative-target-guard");
        fs::create_dir_all(&root).unwrap();
        let canonical_root = fs::canonicalize(&root).unwrap();
        let record = canonical_root.join("p.custody.v1.json");
        let relative = custody_record(
            "relative/target",
            WorktreeCustodyStateV1::ProtectionPrepared {},
        );
        fs::write(&record, relative.encode_canonical().unwrap()).unwrap();
        let probe = RecordingProbe::both_absent();
        let report = super::sweep_orphans_with_exact_absence(&root.to_string_lossy(), &probe);

        assert_eq!(
            custody_assessment(&report, &record),
            &super::CustodyExactAbsenceAssessmentV1::CannotConstructSubject(
                super::CannotConstructSubjectV1::RecordedWorktreePathNotAbsolute,
            )
        );
        assert_eq!(probe.calls(), 0);

        assert_candidate_control(&root);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_preserved_record_outside_the_root_reports_the_guard_not_the_population() {
        use std::os::unix::fs::symlink;

        let root = unique_temp_dir("preserved-outside-root");
        let outside = unique_temp_dir("preserved-outside-root-target");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let canonical_root = fs::canonicalize(&root).unwrap();
        let alias = canonical_root.join("alias");
        symlink(&outside, &alias).unwrap();
        let record = canonical_root.join("alias.custody.v1.json");
        write_real_custody_record(
            &root,
            &record,
            &alias,
            WorktreeCustodyStateV1::Preserved {},
            true,
        );
        let probe = RecordingProbe::both_absent();
        let report = super::sweep_orphans_with_exact_absence(&root.to_string_lossy(), &probe);

        assert_eq!(
            custody_assessment(&report, &record),
            &super::CustodyExactAbsenceAssessmentV1::CannotConstructSubject(
                super::CannotConstructSubjectV1::OutsideSweepRoot,
            )
        );
        assert_eq!(probe.calls(), 0);

        assert_candidate_control(&root);
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn invalid_persisted_claim_pairs_stay_unreadable_and_never_probe() {
        let root = unique_temp_dir("invalid-claim-pairs");
        fs::create_dir_all(&root).unwrap();
        let canonical_root = fs::canonicalize(&root).unwrap();
        let forbidden_target = canonical_root.join("forbidden");
        let required_target = canonical_root.join("required");
        fs::create_dir_all(&forbidden_target).unwrap();
        fs::create_dir_all(&required_target).unwrap();
        let forbidden_path = PathBuf::from(custody_record_path(
            &fs::canonicalize(&forbidden_target)
                .unwrap()
                .to_string_lossy(),
        ));
        let required_path = PathBuf::from(custody_record_path(
            &fs::canonicalize(&required_target)
                .unwrap()
                .to_string_lossy(),
        ));
        let forbidden = real_custody_record(
            &root,
            &forbidden_target,
            WorktreeCustodyStateV1::LiveProtected {},
            true,
        );
        let mut required = real_custody_record(
            &root,
            &required_target,
            WorktreeCustodyStateV1::Preserved {},
            true,
        );
        required.claim = None;
        fs::write(&forbidden_path, serde_json::to_vec(&forbidden).unwrap()).unwrap();
        fs::write(&required_path, serde_json::to_vec(&required).unwrap()).unwrap();
        let probe = RecordingProbe::both_absent();
        let report = super::sweep_orphans_with_exact_absence(&root.to_string_lossy(), &probe);

        for (path, expected) in [
            (
                &forbidden_path,
                crate::custody::CustodyRecordDecodeErrorV1::ClaimForbidden,
            ),
            (
                &required_path,
                crate::custody::CustodyRecordDecodeErrorV1::ClaimRequired,
            ),
        ] {
            let entry = report
                .entries()
                .iter()
                .find(|entry| entry.record_path() == path.to_string_lossy())
                .expect("invalid record must be reported as unreadable");
            assert!(matches!(
                entry.assessment(),
                super::ExactAbsenceRecordAssessmentV1::UnreadableCustody(
                    crate::custody::CustodyReadRefusalV1::Decode(actual)
                ) if actual == &expected
            ));
        }
        assert_eq!(probe.calls(), 0);

        assert_candidate_control(&root);
        fs::remove_dir_all(root).unwrap();
    }

    fn scanned_disposition(root: &Path, record_path: &Path) -> CustodySweepDispositionV1 {
        let root_cwd = crate::provider_path::canonicalize_lenient(&root.to_string_lossy()).unwrap();
        let scanned = super::scan_worktree_records(&root.to_string_lossy());
        let target = fs::canonicalize(record_path)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| record_path.to_string_lossy().into_owned());
        let (path, entry) = scanned
            .into_iter()
            .find(|(path, _)| {
                fs::canonicalize(path)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| path.clone())
                    == target
            })
            .expect("the V3 record must be enumerated by the dual-pattern scan");
        match entry {
            super::ScannedWorktreeRecordV1::Legacy(_) => {
                panic!("a .custody.v1.json record must not scan as a legacy sidecar")
            }
            super::ScannedWorktreeRecordV1::Custody(record) => {
                super::custody_entry_disposition(&root_cwd, &path, Ok(&record))
            }
            super::ScannedWorktreeRecordV1::UnreadableCustody(refusal) => {
                super::custody_entry_disposition(&root_cwd, &path, Err(&refusal))
            }
        }
    }

    /// Discriminates: the boot sweep failing to enumerate the V3 pattern at all
    /// (focused boundary §2.2 -- without dual-pattern recognition "V3 checkouts
    /// would leak unreclaimed forever"), or classifying a live V3 record as
    /// anything other than recovery. The dead-lease probe is the exact input
    /// that reaps a legacy sidecar, so this also pins that lease liveness never
    /// authorizes a V3 deletion.
    #[test]
    fn sweep_recognizes_live_v3_record_as_recovery_and_never_deletes_it() {
        let root = unique_temp_dir("v3-live");
        fs::create_dir_all(&root).unwrap();
        let (worktree, record) =
            write_custody_checkout(&root, "live", WorktreeCustodyStateV1::LiveProtected {});

        assert_eq!(
            scanned_disposition(&root, &record),
            CustodySweepDispositionV1::Recover
        );

        super::sweep_orphans(
            &root.to_string_lossy(),
            "my-host",
            &dead_probe("/leases/dead.lock"),
        );

        assert!(worktree.exists(), "V3 checkout must survive the boot sweep");
        assert!(record.exists(), "V3 record must survive the boot sweep");

        fs::remove_dir_all(&root).unwrap();
    }

    /// Discriminates: a preserved (terminal, R2f2-owned) V3 record being
    /// classified as anything but preserved, or being removed.
    #[test]
    fn sweep_classifies_preserved_v3_record_as_preserved_and_never_deletes_it() {
        let root = unique_temp_dir("v3-preserved");
        fs::create_dir_all(&root).unwrap();
        let (worktree, record) =
            write_custody_checkout(&root, "kept", WorktreeCustodyStateV1::Preserved {});

        assert_eq!(
            scanned_disposition(&root, &record),
            CustodySweepDispositionV1::Preserved
        );

        super::sweep_orphans(
            &root.to_string_lossy(),
            "my-host",
            &dead_probe("/leases/dead.lock"),
        );

        assert!(worktree.exists());
        assert!(record.exists());

        fs::remove_dir_all(&root).unwrap();
    }

    /// Discriminates: an undecodable V3 record being treated as absent (and so
    /// as an ordinary orphan) rather than as unknown. §5.2: "every corrupt /
    /// missing / mismatched V3 pair" is ineligible for deletion.
    #[test]
    fn sweep_treats_corrupt_v3_record_as_unknown_and_never_deletes() {
        let root = unique_temp_dir("v3-corrupt");
        fs::create_dir_all(&root).unwrap();
        let worktree = root.join("corrupt");
        fs::create_dir_all(&worktree).unwrap();
        let canonical = fs::canonicalize(&worktree).unwrap();
        let record = PathBuf::from(custody_record_path(&canonical.to_string_lossy()));
        fs::write(&record, b"{not json").unwrap();

        assert_eq!(
            scanned_disposition(&root, &record),
            CustodySweepDispositionV1::Unknown
        );

        super::sweep_orphans(
            &root.to_string_lossy(),
            "my-host",
            &dead_probe("/leases/dead.lock"),
        );

        assert!(worktree.exists());
        assert!(record.exists());

        fs::remove_dir_all(&root).unwrap();
    }

    /// Discriminates: the reader following a symlinked custody record instead
    /// of refusing it. A symlink is the cheapest way to make a sweep read one
    /// checkout's record while acting on another's directory.
    #[test]
    #[cfg(unix)]
    fn sweep_treats_symlinked_v3_record_as_unknown_and_never_deletes() {
        let root = unique_temp_dir("v3-symlink");
        fs::create_dir_all(&root).unwrap();
        let worktree = root.join("linked");
        fs::create_dir_all(&worktree).unwrap();
        let canonical = fs::canonicalize(&worktree).unwrap();
        let elsewhere = root.join("real-record.json");
        fs::write(
            &elsewhere,
            custody_record(
                &canonical.to_string_lossy(),
                WorktreeCustodyStateV1::LiveProtected {},
            )
            .encode_canonical()
            .unwrap(),
        )
        .unwrap();
        let record = PathBuf::from(custody_record_path(&canonical.to_string_lossy()));
        std::os::unix::fs::symlink(&elsewhere, &record).unwrap();

        assert_eq!(
            scanned_disposition(&root, &record),
            CustodySweepDispositionV1::Unknown
        );

        super::sweep_orphans(
            &root.to_string_lossy(),
            "my-host",
            &dead_probe("/leases/dead.lock"),
        );

        assert!(worktree.exists());
        assert!(record.symlink_metadata().is_ok());

        fs::remove_dir_all(&root).unwrap();
    }

    /// Discriminates: the reader accepting a multiply-linked record. A second
    /// hard link means another name owns the same bytes, so exclusive custody
    /// of the record cannot be proved and its state cannot be trusted.
    #[test]
    #[cfg(unix)]
    fn sweep_treats_multi_link_v3_record_as_unknown_and_never_deletes() {
        let root = unique_temp_dir("v3-multilink");
        fs::create_dir_all(&root).unwrap();
        let (worktree, record) =
            write_custody_checkout(&root, "shared", WorktreeCustodyStateV1::LiveProtected {});
        fs::hard_link(&record, root.join("second-name.json")).unwrap();

        assert_eq!(
            scanned_disposition(&root, &record),
            CustodySweepDispositionV1::Unknown
        );

        super::sweep_orphans(
            &root.to_string_lossy(),
            "my-host",
            &dead_probe("/leases/dead.lock"),
        );

        assert!(worktree.exists());
        assert!(record.exists());

        fs::remove_dir_all(&root).unwrap();
    }

    /// Discriminates: the sidecar-sibling guard not being applied to the V3
    /// arm. A record whose `worktree` names a directory that is not its own
    /// sibling -- or that does not exist at all -- is unknown, never
    /// actionable.
    #[test]
    fn sweep_treats_mismatched_and_missing_v3_pairs_as_unknown() {
        let root = unique_temp_dir("v3-mismatch");
        let victim = unique_temp_dir("v3-mismatch-victim");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&victim).unwrap();
        fs::write(victim.join("keep"), "do not delete").unwrap();
        let victim_canonical = fs::canonicalize(&victim).unwrap();

        // Sibling mismatch: the record file is not `<its own worktree>.custody.v1.json`.
        let forged = root.join(format!("forged{}", crate::custody::CUSTODY_RECORD_SUFFIX));
        fs::write(
            &forged,
            custody_record(
                &victim_canonical.to_string_lossy(),
                WorktreeCustodyStateV1::LiveProtected {},
            )
            .encode_canonical()
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            scanned_disposition(&root, &forged),
            CustodySweepDispositionV1::Unknown
        );

        // Missing sibling: the record is correctly named but its worktree is gone.
        let gone = root.join("gone");
        let gone_record = PathBuf::from(custody_record_path(&gone.to_string_lossy()));
        fs::write(
            &gone_record,
            custody_record(
                &gone.to_string_lossy(),
                WorktreeCustodyStateV1::LiveProtected {},
            )
            .encode_canonical()
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            scanned_disposition(&root, &gone_record),
            CustodySweepDispositionV1::Unknown
        );

        super::sweep_orphans(
            &root.to_string_lossy(),
            "my-host",
            &dead_probe("/leases/dead.lock"),
        );

        assert!(victim.join("keep").exists());
        assert!(forged.exists());
        assert!(gone_record.exists());

        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&victim).unwrap();
    }

    /// Discriminates: the under-root guard not being applied to the V3 arm. A
    /// record that is its own well-formed sibling but sits outside the sweep
    /// root is refused, not classified by state.
    #[test]
    fn sweep_refuses_v3_record_pointing_outside_the_sweep_root() {
        let root = unique_temp_dir("v3-outside");
        let outside = unique_temp_dir("v3-outside-target");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let outside_canonical = fs::canonicalize(&outside).unwrap();
        let record = PathBuf::from(custody_record_path(&outside_canonical.to_string_lossy()));
        fs::write(
            &record,
            custody_record(
                &outside_canonical.to_string_lossy(),
                WorktreeCustodyStateV1::LiveProtected {},
            )
            .encode_canonical()
            .unwrap(),
        )
        .unwrap();

        let root_cwd = crate::provider_path::canonicalize_lenient(&root.to_string_lossy()).unwrap();
        let decoded = custody_record(
            &outside_canonical.to_string_lossy(),
            WorktreeCustodyStateV1::LiveProtected {},
        );
        assert_eq!(
            super::custody_entry_disposition(&root_cwd, &record.to_string_lossy(), Ok(&decoded)),
            CustodySweepDispositionV1::Refused
        );

        assert!(outside.exists());

        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&outside).unwrap();
    }

    /// P3. Discriminates: the V3 arm trusting the record's *path string*
    /// instead of comparing the identity it recorded against the directory that
    /// is actually there. §2.2: "Identity is checked by **descriptor**, not by
    /// re-canonicalizing a string, at every decision point." A directory
    /// swapped out from under a valid record is ambiguous evidence, so it falls
    /// back to `Recover` -- never actionable -- exactly like the corrupt arms.
    /// `Preserved` is used deliberately: its normal disposition is `Preserved`,
    /// so the fallback is observable (a `LiveProtected` record is `Recover`
    /// either way and would not discriminate).
    #[test]
    #[cfg(unix)]
    fn sweep_falls_back_to_recover_when_the_sibling_directory_was_swapped() {
        let root = unique_temp_dir("v3-swapped");
        fs::create_dir_all(&root).unwrap();
        let (worktree, record) =
            write_custody_checkout(&root, "swapped", WorktreeCustodyStateV1::Preserved {});

        // Control: the recorded identity matches the directory on disk.
        assert_eq!(
            scanned_disposition(&root, &record),
            CustodySweepDispositionV1::Preserved
        );

        // Pre-create the replacement while the recorded directory is still live, guaranteeing
        // a distinct inode even on ext4, then rename both objects to perform the same-name swap.
        let replacement = root.join("swapped.swap-replacement");
        let displaced = root.join("swapped.swap-original");
        fs::create_dir(&replacement).unwrap();
        let before = object_with(&worktree.to_string_lossy(), false).directory_identity;
        let candidate = object_with(&replacement.to_string_lossy(), false).directory_identity;
        assert!(
            !before.matches(&candidate),
            "precondition: simultaneously live original and replacement must have distinct identities"
        );
        fs::rename(&worktree, &displaced).unwrap();
        fs::rename(&replacement, &worktree).unwrap();
        let after = object_with(&worktree.to_string_lossy(), false).directory_identity;
        assert!(
            !before.matches(&after),
            "precondition: the same-name replacement must not match the recorded identity"
        );

        let disposition = scanned_disposition(&root, &record);
        assert_eq!(disposition, CustodySweepDispositionV1::Recover);
        assert!(!disposition.authorizes_checkout_removal());

        fs::remove_dir_all(&root).unwrap();
    }

    /// P3 + P2. Discriminates: the descriptor check firing on a record that
    /// legitimately has no identity to check. A degraded record (§5.1's
    /// materialization-unresolved case) carries plan-derived paths only, so
    /// there is nothing to compare and its classification must be unchanged.
    #[test]
    #[cfg(unix)]
    fn sweep_classification_of_a_degraded_record_ignores_the_directory_identity() {
        let root = unique_temp_dir("v3-degraded");
        fs::create_dir_all(&root).unwrap();
        let (worktree, record) = write_custody_checkout(
            &root,
            "degraded",
            WorktreeCustodyStateV1::PreservationUnknown {
                reason: PreservationReasonV1::MaterializationInFlight,
            },
        );

        assert_eq!(
            scanned_disposition(&root, &record),
            CustodySweepDispositionV1::Unknown
        );

        fs::remove_dir_all(&worktree).unwrap();
        fs::create_dir_all(&worktree).unwrap();
        assert_eq!(
            scanned_disposition(&root, &record),
            CustodySweepDispositionV1::Unknown,
            "a degraded record has no identity evidence to invalidate"
        );

        fs::remove_dir_all(&root).unwrap();
    }

    /// Discriminates: the legacy boot arm changing behaviour once the V3 arm
    /// exists. In a mixed root the legacy dead-lease reclaim must still fire,
    /// byte for byte, while the V3 record beside it survives.
    #[test]
    fn legacy_boot_arm_still_reclaims_alongside_a_v3_record() {
        let root = unique_temp_dir("mixed-root");
        fs::create_dir_all(&root).unwrap();
        let dead = write_worktree_sidecar(&root, "dead", "my-host", "/leases/dead.lock", "run-a");
        let (v3_worktree, v3_record) =
            write_custody_checkout(&root, "v3", WorktreeCustodyStateV1::LiveProtected {});

        super::sweep_orphans(
            &root.to_string_lossy(),
            "my-host",
            &dead_probe("/leases/dead.lock"),
        );

        assert!(!Path::new(&dead.worktree_path).exists());
        assert!(!Path::new(&sidecar_path(&dead.worktree_path)).exists());
        assert!(v3_worktree.exists());
        assert!(v3_record.exists());

        fs::remove_dir_all(&root).unwrap();
    }

    /// Discriminates: the run-end guard acquiring any authority over a V3
    /// record. §5.2: the guard's `Drop` backstop is non-destructive and the
    /// already-synced protection record is authoritative.
    #[test]
    fn end_guard_is_non_destructive_for_v3_records() {
        let root = unique_temp_dir("end-guard-v3");
        fs::create_dir_all(&root).unwrap();
        let (worktree, record) =
            write_custody_checkout(&root, "v3", WorktreeCustodyStateV1::LiveProtected {});

        {
            let _guard = super::WorktreeRunEndGuard::new(
                root.to_string_lossy().into_owned(),
                attempt_identity().run_id().to_string(),
            );
        }

        assert!(worktree.exists());
        assert!(record.exists());

        fs::remove_dir_all(&root).unwrap();
    }

    // ---- R2f1b slice 2b2: coexistence, per-guard discrimination, and explicit settlement ----

    /// The binding 2b2 obligation from the 2b1 dual review (opus W-1). ONE checkout carrying BOTH
    /// records must be reclaimed by NEITHER arm — and this state is not exotic: 2b1's deletion
    /// gate produces it on every refusal (the legacy sidecar is retained beside the custody
    /// record). The run-end guard's arm is the CLEAN-drop one, no crash required.
    ///
    /// Discriminates the guard being absent: without it the legacy arm sees a dead lease, sees a
    /// sidecar that matches its sibling and is under the root, and deletes a custody-protected
    /// checkout together with its record.
    #[test]
    fn a_checkout_carrying_both_records_is_reclaimed_by_neither_sweep_arm() {
        for (name, run_id, boot) in [
            ("coexist-boot", "run-a", true),
            ("coexist-end", "mine", false),
        ] {
            let root = unique_temp_dir(name);
            fs::create_dir_all(&root).unwrap();
            let sidecar =
                write_worktree_sidecar(&root, "both", "my-host", "/leases/dead.lock", run_id);
            let canonical = fs::canonicalize(&sidecar.worktree_path)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let record = custody_record(&canonical, WorktreeCustodyStateV1::LiveProtected {});
            fs::write(
                custody_record_path(&canonical),
                record.encode_canonical().unwrap(),
            )
            .unwrap();

            if boot {
                super::sweep_orphans(
                    &root.to_string_lossy(),
                    "my-host",
                    &dead_probe("/leases/dead.lock"),
                );
            } else {
                // The CLEAN drop, which is the destructive one for legacy entries after R9.
                drop(super::WorktreeRunEndGuard::new(
                    root.to_string_lossy().into_owned(),
                    "mine".into(),
                ));
            }

            assert!(
                Path::new(&sidecar.worktree_path).exists(),
                "{name}: the custody-protected checkout must survive"
            );
            assert!(
                Path::new(&custody_record_path(&canonical)).exists(),
                "{name}: its custody record must survive"
            );
            assert!(
                Path::new(&sidecar_path(&sidecar.worktree_path)).exists(),
                "{name}: and the retained legacy sidecar with it"
            );
            fs::remove_dir_all(&root).unwrap();
        }
    }

    /// Guard-set coverage, part 1 — isolates `sidecar_file_matches` (2a carried item, docstring'd
    /// on `end_guard_skips_sidecar_that_points_at_non_sibling_worktree`, which the pair above only
    /// covered REDUNDANTLY: neutering either guard alone left it green).
    ///
    /// The forged record names a victim INSIDE the sweep root, so `worktree_under_root` passes and
    /// cannot be what stops it. Only the sidecar↔sibling match can. Neuter that one guard and this
    /// test goes red on its own.
    #[test]
    fn sidecar_sibling_match_alone_stops_a_forged_in_root_sidecar() {
        let root = unique_temp_dir("guard-sibling-only");
        fs::create_dir_all(&root).unwrap();
        let victim = root.join("victim");
        fs::create_dir_all(&victim).unwrap();
        fs::write(victim.join("keep"), "do not delete").unwrap();
        let sidecar = WorktreeSidecar {
            canonical_source: root.join("source").to_string_lossy().into_owned(),
            common_dir: root.join("source/.git").to_string_lossy().into_owned(),
            worktree_path: victim.to_string_lossy().into_owned(),
            owner: "owner".into(),
            run_id: "run-a".into(),
            host: "my-host".into(),
            lease: "/leases/dead.lock".into(),
        };
        // Named `forged.meta.json`, NOT `victim.meta.json`: the file is not its target's sibling.
        let forged = root.join("forged.meta.json");
        fs::write(&forged, serde_json::to_vec(&sidecar).unwrap()).unwrap();
        assert!(
            super::worktree_under_root(
                &crate::provider_path::canonicalize_lenient(&root.to_string_lossy()).unwrap(),
                &sidecar.worktree_path,
            ),
            "the under-root guard must PASS here, so it cannot be what stops the removal"
        );

        super::sweep_orphans(
            &root.to_string_lossy(),
            "my-host",
            &dead_probe("/leases/dead.lock"),
        );

        assert!(victim.join("keep").exists());
        fs::remove_dir_all(&root).unwrap();
    }

    /// Guard-set coverage, part 2 — isolates `worktree_under_root`.
    ///
    /// The record IS its target's sibling by name (`victim.meta.json` beside `victim`), so
    /// `sidecar_file_matches` passes and cannot be what stops it; but `victim` is a SYMLINK whose
    /// canonical target lies outside the sweep root, which only the under-root check sees.
    /// Neuter that one guard and the sidecar is deleted, turning this red on its own.
    #[test]
    fn under_root_check_alone_stops_a_sibling_sidecar_pointing_outside_the_root() {
        let root = unique_temp_dir("guard-under-root-only");
        let outside = unique_temp_dir("guard-under-root-victim");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("keep"), "do not delete").unwrap();
        let link = root.join("victim");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let sidecar = WorktreeSidecar {
            canonical_source: root.join("source").to_string_lossy().into_owned(),
            common_dir: root.join("source/.git").to_string_lossy().into_owned(),
            worktree_path: link.to_string_lossy().into_owned(),
            owner: "owner".into(),
            run_id: "run-a".into(),
            host: "my-host".into(),
            lease: "/leases/dead.lock".into(),
        };
        write_sidecar(&sidecar).unwrap();
        assert!(
            super::sidecar_file_matches(
                &sidecar_path(&sidecar.worktree_path),
                &sidecar.worktree_path
            ),
            "the sibling-match guard must PASS here, so it cannot be what stops the removal"
        );

        super::sweep_orphans(
            &root.to_string_lossy(),
            "my-host",
            &dead_probe("/leases/dead.lock"),
        );

        assert!(
            Path::new(&sidecar_path(&sidecar.worktree_path)).exists(),
            "the under-root guard must stop the removal before the sidecar is unlinked"
        );
        assert!(outside.join("keep").exists());
        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&outside).unwrap();
    }

    /// S5: explicit settlement runs the run-end pass ONCE, and the later `Drop` is a no-op.
    /// Discriminates a `settle()` that does not mark itself settled (the pass would run twice —
    /// harmless here but unobservable, which defeats the whole point of distinguishing a
    /// settlement from a backstop).
    #[test]
    fn explicit_settlement_reclaims_once_and_makes_the_drop_a_no_op() {
        let root = unique_temp_dir("settle-once");
        fs::create_dir_all(&root).unwrap();
        let mine = write_worktree_sidecar(&root, "mine", "my-host", "/leases/mine.lock", "mine");
        let guard =
            super::WorktreeRunEndGuard::new(root.to_string_lossy().into_owned(), "mine".into());

        assert!(!guard.is_settled());
        guard.settle();
        assert!(guard.is_settled());
        assert!(!Path::new(&mine.worktree_path).exists());

        // A second settle and the eventual Drop must both be no-ops. Re-create the checkout so a
        // repeated pass would be observable if it happened.
        fs::create_dir_all(&mine.worktree_path).unwrap();
        write_sidecar(&mine).unwrap();
        guard.settle();
        drop(guard);
        assert!(
            Path::new(&mine.worktree_path).exists(),
            "a settled guard must not run its pass again on drop"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    /// S5, the abrupt half (Sol 17): an unwinding `Drop` on an UNSETTLED guard is protective and
    /// does not pretend a settlement occurred. Discriminates a backstop that reclaims on the panic
    /// path (destroying evidence at the moment the process understands least) and one that marks
    /// itself settled afterwards, which would make an unsettled run indistinguishable from a
    /// settled one.
    #[test]
    fn an_abrupt_drop_is_protective_and_does_not_claim_settlement() {
        let root = unique_temp_dir("settle-abrupt");
        fs::create_dir_all(&root).unwrap();
        let mine = write_worktree_sidecar(&root, "mine", "my-host", "/leases/mine.lock", "mine");
        let root_for_panic = root.to_string_lossy().into_owned();

        let settled_at_drop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let observed = settled_at_drop.clone();
        let unwound = std::panic::catch_unwind(move || {
            let guard = super::WorktreeRunEndGuard::new(root_for_panic, "mine".into());
            observed.store(guard.is_settled(), std::sync::atomic::Ordering::SeqCst);
            panic!("run failed abruptly");
        });

        assert!(unwound.is_err());
        assert!(
            !settled_at_drop.load(std::sync::atomic::Ordering::SeqCst),
            "the guard must not report itself settled merely because it was constructed"
        );
        assert!(
            Path::new(&mine.worktree_path).exists(),
            "an abrupt drop must defer the reclaim to the next boot sweep"
        );
        assert!(Path::new(&sidecar_path(&mine.worktree_path)).exists());
        fs::remove_dir_all(&root).unwrap();
    }

    // ---- slice 2b2 repair R2: sweep-side publication cell ----

    /// R2's red test, boot arm. A writer holding the checkout's publication cell must stop the boot
    /// sweep dead — and the record is deliberately ABSENT, so the coexistence guard cannot be what
    /// stops it and the cell is isolated as the only remaining protection.
    ///
    /// Discriminates the shipped defect exactly: without the cell the sweep probes (sees nothing),
    /// then removes, while a writer is mid-`ProtectionPrepared` on the same target. The release leg
    /// proves the refusal is the cell and not a permanent wedge.
    #[test]
    fn a_writer_holding_the_publication_cell_stops_the_boot_sweep_and_releases_it() {
        let root = unique_temp_dir("cell-boot-sweep");
        fs::create_dir_all(&root).unwrap();
        let orphan =
            write_worktree_sidecar(&root, "orphan", "my-host", "/leases/dead.lock", "run-a");
        assert!(
            !Path::new(&custody_record_path(&orphan.worktree_path)).exists(),
            "no custody record: the cell must be the only thing protecting this checkout"
        );

        let writer =
            crate::custody_lock::try_acquire_publication_lock_in(&root, &orphan.worktree_path)
                .unwrap();
        super::sweep_orphans(
            &root.to_string_lossy(),
            "my-host",
            &dead_probe("/leases/dead.lock"),
        );
        assert!(
            Path::new(&orphan.worktree_path).exists(),
            "the boot sweep must not delete a checkout whose publication cell it cannot enter"
        );
        assert!(Path::new(&sidecar_path(&orphan.worktree_path)).exists());

        drop(writer);
        super::sweep_orphans(
            &root.to_string_lossy(),
            "my-host",
            &dead_probe("/leases/dead.lock"),
        );
        assert!(
            !Path::new(&orphan.worktree_path).exists(),
            "once the cell is free the ordinary reclaim proceeds"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    /// R2's red test, run-end arm. Same race against explicit settlement, which is the arm that
    /// runs on EVERY clean run — no crash, no boot, no dead lease needed.
    #[test]
    fn a_writer_holding_the_publication_cell_stops_run_end_settlement() {
        let root = unique_temp_dir("cell-run-end");
        fs::create_dir_all(&root).unwrap();
        let mine = write_worktree_sidecar(&root, "mine", "my-host", "/leases/mine.lock", "mine");

        let writer =
            crate::custody_lock::try_acquire_publication_lock_in(&root, &mine.worktree_path)
                .unwrap();
        let guard =
            super::WorktreeRunEndGuard::new(root.to_string_lossy().into_owned(), "mine".into());
        guard.settle();
        assert!(
            Path::new(&mine.worktree_path).exists(),
            "run-end settlement must not delete a checkout whose cell is held"
        );
        drop(guard);

        drop(writer);
        let second =
            super::WorktreeRunEndGuard::new(root.to_string_lossy().into_owned(), "mine".into());
        second.settle();
        assert!(
            !Path::new(&mine.worktree_path).exists(),
            "with the cell free, settlement reclaims as before"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    /// The reverse order: the SWEEP holds the cell and a writer arriving must WAIT rather than
    /// publish into the middle of a removal. Discriminates a writer that takes the non-blocking
    /// acquirer (it would spuriously fail a legitimate transition) and, more importantly, one that
    /// takes no publication cell at all — it would publish `ProtectionPrepared` over a checkout
    /// whose deletion is already in flight.
    #[test]
    fn a_writer_waits_when_a_reclaim_already_holds_the_publication_cell() {
        let root = unique_temp_dir("cell-reverse-order");
        fs::create_dir_all(&root).unwrap();
        let target = root.join("ownr-run7-abc");
        fs::create_dir_all(&target).unwrap();
        let target_path = target.to_string_lossy().into_owned();

        let reclaiming =
            crate::custody_lock::try_acquire_publication_lock_in(&root, &target_path).unwrap();

        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let writer = std::thread::spawn({
            let root = root.clone();
            let target_path = target_path.clone();
            move || {
                let guard = crate::custody_lock::acquire_publication_lock_blocking_in(
                    &root,
                    &target_path,
                    &|| entered_tx.send(()).unwrap(),
                )
                .unwrap();
                done_tx.send(()).unwrap();
                drop(guard);
            }
        });

        entered_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the writer must report that it is waiting");
        assert!(
            done_rx
                .recv_timeout(std::time::Duration::from_millis(200))
                .is_err(),
            "the writer must not enter the cell while a reclaim holds it"
        );

        drop(reclaiming);
        done_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the writer proceeds once the reclaim releases");
        writer.join().unwrap();
        fs::remove_dir_all(&root).unwrap();
    }

    /// The forgery guards must run BEFORE anything touches the named path (opus S-8). A forged
    /// sidecar naming a path outside the root must not cause a custody probe there, and — now that
    /// the publication cell is taken on this path — must not cause a lock directory to be created
    /// beside the victim either.
    ///
    /// Discriminates the shipped order, where `probe_custody_record_presence` ran first.
    #[test]
    fn a_forged_sidecar_never_touches_its_named_path_before_the_guards_pass() {
        let root = unique_temp_dir("forged-no-touch");
        let victim = unique_temp_dir("forged-no-touch-victim");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&victim).unwrap();
        fs::write(victim.join("keep"), "do not delete").unwrap();
        let sidecar = WorktreeSidecar {
            canonical_source: root.join("source").to_string_lossy().into_owned(),
            common_dir: root.join("source/.git").to_string_lossy().into_owned(),
            worktree_path: victim.to_string_lossy().into_owned(),
            owner: "owner".into(),
            run_id: "run-a".into(),
            host: "my-host".into(),
            lease: "/leases/dead.lock".into(),
        };
        fs::write(
            root.join("forged.meta.json"),
            serde_json::to_vec(&sidecar).unwrap(),
        )
        .unwrap();

        super::sweep_orphans(
            &root.to_string_lossy(),
            "my-host",
            &dead_probe("/leases/dead.lock"),
        );

        assert!(victim.join("keep").exists());
        assert!(
            !victim
                .parent()
                .unwrap()
                .join(crate::custody_lock::CUSTODY_LOCK_DIR_NAME)
                .exists(),
            "a forged path must not have a lock directory created beside it"
        );
        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&victim).unwrap();
    }

    // ---- slice 2b2 repair R5: one settlement per handled terminal ----

    /// The branch table. Every HANDLED terminal — whatever the outcome — settles exactly once, and
    /// the guard reports itself settled afterwards.
    ///
    /// Models the four `implement::Action` arms plus run-workflow's two: before this repair only
    /// the `Commit` arm was wrapped, so three of implement's four terminals and run-workflow's
    /// output-write failure reached `Drop` unsettled. "Exactly once" is observed by re-creating
    /// the checkout after the settle: a second pass would reclaim it again.
    #[test]
    fn every_handled_outcome_settles_exactly_once_and_reports_it() {
        for outcome in [
            "abort",
            "no-commit-clean",
            "no-commit-dirty",
            "commit",
            "workflow-ok",
            "workflow-output-error",
        ] {
            let root = unique_temp_dir(&format!("branch-{outcome}"));
            fs::create_dir_all(&root).unwrap();
            let mine =
                write_worktree_sidecar(&root, "mine", "my-host", "/leases/mine.lock", "mine");
            let guard =
                super::WorktreeRunEndGuard::new(root.to_string_lossy().into_owned(), "mine".into());

            assert!(
                !guard.is_settled(),
                "{outcome}: unsettled before the epilogue"
            );
            guard.settle();
            assert!(guard.is_settled(), "{outcome}: settled after the epilogue");
            assert!(
                !Path::new(&mine.worktree_path).exists(),
                "{outcome}: the settlement pass ran"
            );

            // Exactly once: a repeated settle and the eventual drop must both be no-ops.
            fs::create_dir_all(&mine.worktree_path).unwrap();
            write_sidecar(&mine).unwrap();
            guard.settle();
            drop(guard);
            assert!(
                Path::new(&mine.worktree_path).exists(),
                "{outcome}: the pass must not run a second time"
            );
            fs::remove_dir_all(&root).unwrap();
        }
    }

    /// A handled terminal that forgot its epilogue still settles — a NON-panicking drop is by
    /// definition an ordinary return, so it is a handled exit and must record itself as one.
    /// Together with `an_abrupt_drop_is_protective_and_does_not_claim_settlement`, this makes
    /// "unsettled" mean exactly "panicked or otherwise unhandled".
    ///
    /// Discriminates the shipped bookkeeping, where a clean drop of an UNSETTLED guard ran the
    /// pass but left `is_settled()` false and logged `settled = !unwinding`, i.e. `true` — the
    /// field and the flag disagreeing in opposite directions at the same moment.
    #[test]
    fn a_clean_drop_of_an_unsettled_guard_counts_as_a_handled_settlement() {
        let root = unique_temp_dir("clean-drop-settles");
        fs::create_dir_all(&root).unwrap();
        let mine = write_worktree_sidecar(&root, "mine", "my-host", "/leases/mine.lock", "mine");

        {
            let guard =
                super::WorktreeRunEndGuard::new(root.to_string_lossy().into_owned(), "mine".into());
            assert!(!guard.is_settled());
        }

        assert!(
            !Path::new(&mine.worktree_path).exists(),
            "a clean drop still performs the reclaim"
        );
        fs::remove_dir_all(&root).unwrap();
    }
}
