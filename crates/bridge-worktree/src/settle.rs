//! The T3b refusing settlement window over exactly one checkout custody record.
//!
//! This is the third acquirer class. Alongside the writer (which blocks while holding both
//! cells) and the sweep/deletion gate (which refuses while holding only the publication cell),
//! this window takes both cells with the refusing acquirers. The parked blocking-acquisition
//! policy is not activated by this slice or any later T3b slice: a settlement path never waits
//! for either cell.
//!
//! Opening is necessarily two-phase. The custody cell key lives inside the record, so the window
//! first reads the record under the publication cell to learn the key, then reads it again under
//! both cells. A changed record means a writer that this window did not exclude was
//! mid-transition, so the window refuses rather than selecting either version.
//!
//! The held window spans decide-and-act, preventing a transition from publishing between the
//! decision and the effect. Slices 2–5 add the proof, transition, and retirement inside it; this
//! slice adds none of them.

use crate::custody::{
    custody_record_path, read_custody_record_in, CustodyReadRefusalV1, WorktreeCustodyRecordV1,
    WorktreeCustodyStateV1,
};
use crate::custody_lock::{
    try_acquire_custody_lock_in, try_acquire_publication_lock_in, CustodyLockGuardV1,
    CustodyLockRefusalV1, PublicationLockGuardV1,
};
use crate::sweep::{
    reprove_exact_absence_entry, ExactAbsenceProbeV1, ExactAbsenceRecordAssessmentV1,
    ExactAbsenceSweepEntryV1, ExactAbsenceSweepReportV1, ReprovedExactAbsenceOutcomeV1,
    UnusedCandidateDecisionV1,
};
use bridge_core::execution_policy::WorktreeCustodyIdV1;
use bridge_core::fs_custody::PinnedDirectoryV1;
use std::ffi::{OsStr, OsString};
use std::path::Path;

/// Why a settlement window could not be opened. Every arm is a refusal; none is authority to
/// decide, transition, or act on the checkout.
#[derive(Debug, thiserror::Error)]
pub enum SettlementWindowRefusalV1 {
    /// Another actor already holds one of the cells needed for this window.
    #[error("settlement cell is contended: {0}")]
    CellContended(String),
    /// A required cell could not be opened or created.
    #[error("settlement cell is unavailable: {0}")]
    CellUnavailable(String),
    /// The worktree root disappeared or could not be pinned.
    #[error("settlement root is unavailable: {0}")]
    RootUnavailable(String),
    /// The requested checkout cannot be turned into this root's one-record subject.
    #[error("settlement subject is not constructible: {0}")]
    SubjectNotConstructible(String),
    /// The record is absent, not exclusively owned, oversized, malformed, or otherwise unreadable.
    #[error("settlement record is unreadable: {0}")]
    RecordUnreadable(#[from] CustodyReadRefusalV1),
    /// The record changed between its read under the publication cell and its read under both cells.
    #[error("settlement record changed under the held window: {0}")]
    RecordChangedUnderWindow(String),
}

/// A held settlement window for one decoded custody record.
///
/// The guards are fields so their lifetime is exactly this value's. They are declared in
/// acquisition order; Rust drops fields in declaration order, so the publication cell (outer)
/// releases after the custody cell (inner). That reverse-of-acquisition release order is required
/// by the nested lock discipline.
pub struct SettlementWindowV1 {
    record: WorktreeCustodyRecordV1,
    root: PinnedDirectoryV1,
    record_name: OsString,
    custody_id: WorktreeCustodyIdV1,
    _custody_cell: CustodyLockGuardV1,
    _publication_cell: PublicationLockGuardV1,
}

impl std::fmt::Debug for SettlementWindowV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SettlementWindowV1")
            .field("worktree_path", &self.worktree_path())
            .field("record_name", &self.record_name)
            .field("custody_id", &self.custody_id.as_str())
            .finish()
    }
}

impl SettlementWindowV1 {
    /// Enter the publication then custody cells without waiting, and bind both to one stable,
    /// decoded record.
    pub fn open(
        worktree_root: &Path,
        canonical_worktree_path: &str,
    ) -> Result<Self, SettlementWindowRefusalV1> {
        Self::open_with_after_first_read(worktree_root, canonical_worktree_path, || {})
    }

    fn open_with_after_first_read<F>(
        worktree_root: &Path,
        canonical_worktree_path: &str,
        after_first_read: F,
    ) -> Result<Self, SettlementWindowRefusalV1>
    where
        F: FnOnce(),
    {
        // This check must precede a cell attempt: entering a cell creates the `.custody-locks`
        // directory, so trying first could recreate a root that a teardown had already removed.
        match worktree_root.try_exists() {
            Ok(true) => {}
            Ok(false) => {
                return Err(SettlementWindowRefusalV1::RootUnavailable(format!(
                    "worktree root does not exist: {}",
                    worktree_root.display()
                )));
            }
            Err(error) => {
                return Err(SettlementWindowRefusalV1::RootUnavailable(format!(
                    "worktree root cannot be checked: {error}"
                )));
            }
        }

        let publication_cell =
            try_acquire_publication_lock_in(worktree_root, canonical_worktree_path)
                .map_err(map_cell_refusal)?;
        let root = PinnedDirectoryV1::open(worktree_root, "worktree custody root")
            .map_err(|error| SettlementWindowRefusalV1::RootUnavailable(error.to_string()))?;
        let record_name = custody_record_name(canonical_worktree_path)?;

        let first_record = read_custody_record_in(&root, &record_name)?;
        let first_bytes = canonical_bytes(&first_record)?;
        let custody_id = first_record.custody_id.clone();
        let custody_cell =
            try_acquire_custody_lock_in(worktree_root, &custody_id).map_err(map_cell_refusal)?;

        after_first_read();

        let record = read_custody_record_in(&root, &record_name)?;
        let second_bytes = canonical_bytes(&record)?;
        if first_bytes != second_bytes {
            return Err(SettlementWindowRefusalV1::RecordChangedUnderWindow(
                format!(
                    "{} changed between the publication-only and both-cell reads",
                    record_name.to_string_lossy()
                ),
            ));
        }
        if record.worktree.canonical_path != canonical_worktree_path {
            return Err(SettlementWindowRefusalV1::SubjectNotConstructible(format!(
                "record worktree path {} does not match requested path {canonical_worktree_path}",
                record.worktree.canonical_path
            )));
        }

        Ok(Self {
            record,
            root,
            record_name,
            custody_id,
            _custody_cell: custody_cell,
            _publication_cell: publication_cell,
        })
    }

    #[must_use]
    pub fn record(&self) -> &WorktreeCustodyRecordV1 {
        &self.record
    }

    #[must_use]
    pub fn pinned_root(&self) -> &PinnedDirectoryV1 {
        &self.root
    }

    #[must_use]
    pub fn record_name(&self) -> &OsStr {
        &self.record_name
    }

    #[must_use]
    pub fn custody_id(&self) -> &WorktreeCustodyIdV1 {
        &self.custody_id
    }

    #[must_use]
    pub fn worktree_path(&self) -> &str {
        &self.record.worktree.canonical_path
    }
}

fn map_cell_refusal(refusal: CustodyLockRefusalV1) -> SettlementWindowRefusalV1 {
    match refusal {
        CustodyLockRefusalV1::Contended(id) => SettlementWindowRefusalV1::CellContended(id),
        CustodyLockRefusalV1::Unavailable(id, error) => {
            SettlementWindowRefusalV1::CellUnavailable(format!("{id}: {error}"))
        }
    }
}

fn custody_record_name(
    canonical_worktree_path: &str,
) -> Result<OsString, SettlementWindowRefusalV1> {
    Path::new(&custody_record_path(canonical_worktree_path))
        .file_name()
        .map(OsStr::to_os_string)
        .ok_or_else(|| {
            SettlementWindowRefusalV1::SubjectNotConstructible(format!(
                "worktree target has no file name: {canonical_worktree_path}"
            ))
        })
}

fn canonical_bytes(record: &WorktreeCustodyRecordV1) -> Result<Vec<u8>, SettlementWindowRefusalV1> {
    record
        .encode_canonical()
        .map_err(CustodyReadRefusalV1::Decode)
        .map_err(SettlementWindowRefusalV1::RecordUnreadable)
}

/// Why a held settlement window could not establish current proof for its subject.
///
/// `Refused` is a proved-negative answer. `CannotProve` means the current evidence was
/// unavailable, so a later settlement slice must not treat it as a negative conclusion.
#[derive(Debug, thiserror::Error)]
pub enum SettlementProofRefusalV1 {
    /// The report entry, record, or renewed exact-absence observation proved the subject ineligible.
    #[error("settlement subject is refused: {0}")]
    Refused(String),
    /// A fresh scan could not establish the current evidence needed for a decision.
    #[error("settlement subject cannot be proved: {0}")]
    CannotProve(String),
}

/// A settlement subject proved under, and owning, its refusing settlement window.
///
/// There is no public constructor. The contained window keeps both cells alive until this
/// capability is dropped, so no later settlement action can retain proof after releasing them.
#[must_use]
pub struct ProvenSettlementV1 {
    window: SettlementWindowV1,
}

impl std::fmt::Debug for ProvenSettlementV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProvenSettlementV1")
            .field("worktree_path", &self.worktree_path())
            .field("record_name", &self.record_name())
            .field("custody_id", &self.custody_id().as_str())
            .finish()
    }
}

impl ProvenSettlementV1 {
    #[must_use]
    pub fn record(&self) -> &WorktreeCustodyRecordV1 {
        self.window.record()
    }

    #[must_use]
    pub fn record_name(&self) -> &OsStr {
        self.window.record_name()
    }

    #[must_use]
    pub fn custody_id(&self) -> &WorktreeCustodyIdV1 {
        self.window.custody_id()
    }

    #[must_use]
    pub fn worktree_path(&self) -> &str {
        self.window.worktree_path()
    }
}

/// Re-prove a report entry under its held window before any later settlement effect.
///
/// A report is historical evidence only. This gate first rejects any report that was not an
/// authoritative snapshot, then binds the selected entry back to that report and re-runs the
/// report's scan and projection under the held cells. Only the freshly re-proved decision can
/// mint `ProvenSettlementV1`.
pub fn reprove_under_window(
    window: SettlementWindowV1,
    report: &ExactAbsenceSweepReportV1,
    entry: &ExactAbsenceSweepEntryV1,
    probe: &dyn ExactAbsenceProbeV1,
) -> Result<ProvenSettlementV1, SettlementProofRefusalV1> {
    if !report.has_authoritative_scan() {
        return Err(SettlementProofRefusalV1::Refused(
            "report scan was not authoritative".to_string(),
        ));
    }
    if !report.entries().iter().any(|reported| reported == entry) {
        return Err(SettlementProofRefusalV1::Refused(
            "selected entry does not belong to the report".to_string(),
        ));
    }
    if !matches!(
        &window.record().state,
        WorktreeCustodyStateV1::ProtectionPrepared {}
    ) || window.record().claim.is_none()
    {
        return Err(SettlementProofRefusalV1::Refused(
            "held record is not protection-prepared with its required claim population".to_string(),
        ));
    }
    if entry.assessment().decision() != UnusedCandidateDecisionV1::Authorized
        || matches!(
            entry.assessment(),
            ExactAbsenceRecordAssessmentV1::Legacy(_)
        )
    {
        return Err(SettlementProofRefusalV1::Refused(
            "report entry is not an authorized custody subject".to_string(),
        ));
    }
    match reprove_exact_absence_entry(
        window.pinned_root().canonical_path(),
        window.record(),
        entry,
        probe,
    ) {
        ReprovedExactAbsenceOutcomeV1::Authorized => Ok(ProvenSettlementV1 { window }),
        ReprovedExactAbsenceOutcomeV1::Refused(reason) => {
            Err(SettlementProofRefusalV1::Refused(reason.to_owned()))
        }
        ReprovedExactAbsenceOutcomeV1::CannotProve(reason) => {
            Err(SettlementProofRefusalV1::CannotProve(reason.to_owned()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custody::{
        PreservationReasonV1, PreservedWorktreeClaimV1, RecoveryLocatorV1,
        WORKTREE_CUSTODY_RECORD_SCHEMA_V1,
    };
    use crate::custody_lock::{
        custody_lock_dir, custody_lock_id, custody_publication_lock_id,
        try_acquire_custody_lock_in, try_acquire_publication_lock_in,
    };
    use crate::custody_writer::planned_identity;
    use crate::sweep::{
        sweep_orphans_with_exact_absence, CustodyRootObservationV1, ExactAbsenceEnumerationV1,
        ExactAbsenceScanStatusV1, ExactAbsenceSweepReportV1,
    };
    use bridge_core::execution_policy::{PolicyNodeRefV1, Sha256HexV1, WorktreeObjectIdentityV1};
    use bridge_core::fs_custody::{verify_payload_directory_identity, DirectoryIdentityV1};
    use bridge_core::ids::{AttemptId, AttemptIdentity, ExecutionId};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::time::Duration;

    use std::fs;
    fn root(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "a2a-bridge-settlement-window-{name}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        std::fs::canonicalize(path).unwrap()
    }

    fn record_for(target: &Path, custody_digit: char) -> WorktreeCustodyRecordV1 {
        WorktreeCustodyRecordV1 {
            schema_version: WORKTREE_CUSTODY_RECORD_SCHEMA_V1,
            custody_id: WorktreeCustodyIdV1::parse(format!(
                "custody-{}",
                custody_digit.to_string().repeat(64)
            ))
            .unwrap(),
            checkout_fingerprint: Sha256HexV1::parse("6".repeat(64)).unwrap(),
            current_attempt: AttemptIdentity {
                execution_id: ExecutionId::parse(format!("exec-{}", "1".repeat(32))).unwrap(),
                attempt_id: AttemptId::parse(format!("attempt-{}", "2".repeat(32))).unwrap(),
                ordinal: 0,
                parent_attempt_id: None,
            },
            worktree: planned_identity(&target.to_string_lossy()),
            state: WorktreeCustodyStateV1::ProtectionPrepared {},
            claim: None,
        }
    }

    fn write_record(root: &Path, target: &Path, custody_digit: char) -> WorktreeCustodyRecordV1 {
        let record = record_for(target, custody_digit);
        let record_name = custody_record_name(&target.to_string_lossy()).unwrap();
        std::fs::write(root.join(record_name), record.encode_canonical().unwrap()).unwrap();
        record
    }

    fn remove_root(root: &Path) {
        std::fs::remove_dir_all(root).unwrap();
    }

    fn observed_identity(path: &Path) -> WorktreeObjectIdentityV1 {
        let directory_identity =
            verify_payload_directory_identity(&fs::canonicalize(path).unwrap()).unwrap();
        WorktreeObjectIdentityV1 {
            canonical_path: directory_identity.canonical_path.clone(),
            directory_identity,
        }
    }

    fn authorized_record(
        root: &Path,
        target: &Path,
        custody_digit: char,
    ) -> WorktreeCustodyRecordV1 {
        let source = root.join(format!("source-{custody_digit}"));
        let common_dir = source.join("common");
        fs::create_dir_all(&common_dir).unwrap();
        let mut record = record_for(target, custody_digit);
        record.claim = Some(PreservedWorktreeClaimV1 {
            schema_version: WORKTREE_CUSTODY_RECORD_SCHEMA_V1,
            custody_id: record.custody_id.clone(),
            execution_id: record.current_attempt.execution_id.clone(),
            origin_attempt_id: record.current_attempt.attempt_id.clone(),
            current_attempt: record.current_attempt.clone(),
            node: PolicyNodeRefV1 {
                sorted_ordinal: 0,
                id_sha256: Sha256HexV1::parse("5".repeat(64)).unwrap(),
            },
            checkout_fingerprint: record.checkout_fingerprint.clone(),
            source: observed_identity(&source),
            root: observed_identity(root),
            worktree: record.worktree.clone(),
            common_dir: observed_identity(&common_dir),
            reason: PreservationReasonV1::NodeFailure,
            created_wall_ms: 1_700_000_000_000,
            recovery_locator: RecoveryLocatorV1::RegisteredWorktree {},
        });
        record
    }

    fn write_authorized_record(
        root: &Path,
        target: &Path,
        custody_digit: char,
    ) -> WorktreeCustodyRecordV1 {
        let record = authorized_record(root, target, custody_digit);
        let name = custody_record_name(&target.to_string_lossy()).unwrap();
        fs::write(root.join(name), record.encode_canonical().unwrap()).unwrap();
        record
    }

    struct CurrentAbsenceProbeV1 {
        source_common_dir: DirectoryIdentityV1,
    }

    impl ExactAbsenceProbeV1 for CurrentAbsenceProbeV1 {
        fn observe_source_common_dir_identity(
            &self,
            _: &str,
        ) -> Result<DirectoryIdentityV1, bridge_core::error::BridgeError> {
            Ok(self.source_common_dir.clone())
        }

        fn observe_exact_absence(
            &self,
            candidate: &crate::sweep::ExactAbsenceCandidateV1,
        ) -> Result<crate::sweep::ExactAbsenceObservationV1, bridge_core::error::BridgeError>
        {
            Ok(if Path::new(&candidate.worktree_path).exists() {
                crate::sweep::ExactAbsenceObservationV1::TargetPresent
            } else {
                crate::sweep::ExactAbsenceObservationV1::BothAbsent
            })
        }
    }

    fn current_probe(record: &WorktreeCustodyRecordV1) -> CurrentAbsenceProbeV1 {
        CurrentAbsenceProbeV1 {
            source_common_dir: record
                .claim
                .as_ref()
                .unwrap()
                .common_dir
                .directory_identity
                .clone(),
        }
    }

    fn authoritative_report(
        root: &Path,
        probe: &CurrentAbsenceProbeV1,
    ) -> ExactAbsenceSweepReportV1 {
        let report = sweep_orphans_with_exact_absence(&root.to_string_lossy(), probe);
        assert!(report.has_authoritative_scan());
        assert_eq!(report.entries().len(), 1);
        report
    }

    /// A settlement actor that uses the writer's fixed publication-then-custody ordering. This
    /// test-only shape reaches the primitive directly so the test can observe its `on_contended`
    /// callback; production transition entry remains `WorktreeCustodianV1`.
    fn enter_writer_cells_for_test(
        root: &Path,
        canonical_worktree_path: &str,
        custody_id: &WorktreeCustodyIdV1,
        on_contended: &dyn Fn(),
    ) {
        let publication_id = custody_publication_lock_id(canonical_worktree_path);
        let _publication = bridge_core::liveness::acquire_persistent_lock_blocking_in(
            &custody_lock_dir(root),
            &publication_id,
            on_contended,
        )
        .unwrap();
        let _custody = bridge_core::liveness::acquire_persistent_lock_blocking_in(
            &custody_lock_dir(root),
            custody_lock_id(custody_id),
            &|| panic!("the custody cell is free once the publication cell is entered"),
        )
        .unwrap();
    }

    #[test]
    fn the_window_refuses_a_held_publication_cell() {
        let root = root("held-publication");
        let target = root.join("ownr-run7-abc");
        write_record(&root, &target, '3');
        let held = try_acquire_publication_lock_in(&root, &target.to_string_lossy()).unwrap();

        let refusal = SettlementWindowV1::open(&root, &target.to_string_lossy()).unwrap_err();

        assert!(matches!(
            refusal,
            SettlementWindowRefusalV1::CellContended(_)
        ));
        drop(held);
        remove_root(&root);
    }

    #[test]
    fn the_window_refuses_a_held_custody_cell() {
        let root = root("held-custody");
        let target = root.join("ownr-run7-abc");
        let record = write_record(&root, &target, '4');
        let held = try_acquire_custody_lock_in(&root, &record.custody_id).unwrap();

        let refusal = SettlementWindowV1::open(&root, &target.to_string_lossy()).unwrap_err();

        assert!(matches!(
            refusal,
            SettlementWindowRefusalV1::CellContended(_)
        ));
        drop(held);
        remove_root(&root);
    }

    #[test]
    fn a_transition_writer_waits_for_an_open_settlement_window() {
        let root = root("window-then-writer");
        let target = root.join("ownr-run7-abc");
        let record = write_record(&root, &target, '5');
        let window = SettlementWindowV1::open(&root, &target.to_string_lossy()).unwrap();
        let (waited_tx, waited_rx) = mpsc::channel();
        let waited = Arc::new(AtomicBool::new(false));
        let writer = std::thread::spawn({
            let root = root.clone();
            let worktree_path = target.to_string_lossy().into_owned();
            let custody_id = record.custody_id.clone();
            let waited = Arc::clone(&waited);
            move || {
                enter_writer_cells_for_test(&root, &worktree_path, &custody_id, &|| {
                    waited.store(true, Ordering::SeqCst);
                    waited_tx.send(()).unwrap();
                });
            }
        });

        waited_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(!writer.is_finished());

        drop(window);
        writer.join().unwrap();
        assert!(waited.load(Ordering::SeqCst));
        remove_root(&root);
    }

    #[test]
    fn the_window_takes_the_publication_cell_before_the_custody_cell() {
        let root = root("custody-before-window");
        let target = root.join("ownr-run7-abc");
        let record = write_record(&root, &target, '6');
        let held = try_acquire_custody_lock_in(&root, &record.custody_id).unwrap();

        let refusal = SettlementWindowV1::open(&root, &target.to_string_lossy()).unwrap_err();

        assert!(matches!(
            refusal,
            SettlementWindowRefusalV1::CellContended(_)
        ));
        assert!(
            try_acquire_publication_lock_in(&root, &target.to_string_lossy()).is_ok(),
            "the publication cell must have been released on the custody-cell refusal"
        );
        drop(held);
        remove_root(&root);
    }

    #[test]
    fn the_window_refuses_a_record_that_changed_between_its_two_reads() {
        let root = root("changed-record");
        let target = root.join("ownr-run7-abc");
        write_record(&root, &target, '7');
        let record_name = custody_record_name(&target.to_string_lossy()).unwrap();
        let changed = record_for(&target, '8').encode_canonical().unwrap();

        let refusal = SettlementWindowV1::open_with_after_first_read(
            &root,
            &target.to_string_lossy(),
            || std::fs::write(root.join(record_name), changed).unwrap(),
        )
        .unwrap_err();

        assert!(matches!(
            refusal,
            SettlementWindowRefusalV1::RecordChangedUnderWindow(_)
        ));
        remove_root(&root);
    }

    #[test]
    fn the_window_refuses_a_record_with_a_mismatched_worktree_path() {
        let root = root("mismatched-record-path");
        let target = root.join("ownr-run7-abc");
        let mut record = record_for(&target, 'a');
        record.worktree = planned_identity("/a-different-worktree");
        let record_name = custody_record_name(&target.to_string_lossy()).unwrap();
        std::fs::write(root.join(record_name), record.encode_canonical().unwrap()).unwrap();

        let refusal = SettlementWindowV1::open(&root, &target.to_string_lossy()).unwrap_err();

        assert!(matches!(
            refusal,
            SettlementWindowRefusalV1::SubjectNotConstructible(_)
        ));
        remove_root(&root);
    }

    #[test]
    fn the_window_mints_no_effect() {
        let root = root("effect-audit");
        let target = root.join("ownr-run7-abc");
        write_record(&root, &target, '9');
        let record_name = custody_record_name(&target.to_string_lossy()).unwrap();
        let before = std::fs::read(root.join(&record_name)).unwrap();

        let window = SettlementWindowV1::open(&root, &target.to_string_lossy()).unwrap();

        assert_eq!(std::fs::read(root.join(record_name)).unwrap(), before);
        assert_eq!(window.worktree_path(), target.to_string_lossy());
        let production_source = include_str!("settle.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        for forbidden_edge in [
            "std::fs::rename(",
            "std::fs::remove_file(",
            "std::fs::remove_dir_all(",
            "std::process::Command",
            "custody_writer::",
            "provider::",
            ".publish_",
            ".replace_",
        ] {
            assert!(
                !production_source.contains(forbidden_edge),
                "the settlement path must not reach {forbidden_edge}"
            );
        }
        drop(window);
        remove_root(&root);
    }
    /// Discriminates a settlement actor accepting the report's historical absence after a target
    /// was recreated. The fresh scan must see the replacement and leave custody bytes untouched.
    #[test]
    fn a_stale_report_is_never_authority_the_window_reproves() {
        let root = root("stale-report");
        let target = root.join("ownr-run7-abc");
        let record = write_authorized_record(&root, &target, 'b');
        let probe = current_probe(&record);
        let report = authoritative_report(&root, &probe);
        let before =
            fs::read(root.join(custody_record_name(&target.to_string_lossy()).unwrap())).unwrap();
        fs::create_dir(&target).unwrap();

        let window = SettlementWindowV1::open(&root, &target.to_string_lossy()).unwrap();
        let refusal =
            reprove_under_window(window, &report, &report.entries()[0], &probe).unwrap_err();

        assert!(matches!(refusal, SettlementProofRefusalV1::Refused(_)));
        assert_eq!(
            fs::read(root.join(custody_record_name(&target.to_string_lossy()).unwrap())).unwrap(),
            before
        );
        remove_root(&root);
    }

    /// Discriminates proving a newly replaced decodable record merely because its new scan result
    /// is also authorized. The report's retained canonical bytes must still bind the subject.
    #[test]
    fn a_record_replaced_between_report_and_window_refuses() {
        let root = root("replaced-record");
        let target = root.join("ownr-run7-abc");
        let original = write_authorized_record(&root, &target, 'c');
        let probe = current_probe(&original);
        let report = authoritative_report(&root, &probe);
        let mut replacement = original.clone();
        replacement.checkout_fingerprint = Sha256HexV1::parse("7".repeat(64)).unwrap();
        replacement.claim.as_mut().unwrap().checkout_fingerprint =
            replacement.checkout_fingerprint.clone();
        fs::write(
            root.join(custody_record_name(&target.to_string_lossy()).unwrap()),
            replacement.encode_canonical().unwrap(),
        )
        .unwrap();

        let window = SettlementWindowV1::open(&root, &target.to_string_lossy()).unwrap();
        let refusal =
            reprove_under_window(window, &report, &report.entries()[0], &probe).unwrap_err();

        assert!(matches!(refusal, SettlementProofRefusalV1::Refused(_)));
        remove_root(&root);
    }

    /// Discriminates unavailable current scan evidence from a proved-negative eligibility result.
    #[test]
    fn an_unavailable_fresh_scan_is_cannot_prove() {
        let root = root("cannot-prove");
        let target = root.join("ownr-run7-abc");
        let record = write_authorized_record(&root, &target, 'e');
        let probe = current_probe(&record);
        let report = authoritative_report(&root, &probe);
        let window = SettlementWindowV1::open(&root, &target.to_string_lossy()).unwrap();
        let relocated = root.with_file_name(format!(
            "a2a-bridge-settlement-window-cannot-prove-relocated-{}",
            std::process::id()
        ));
        fs::rename(&root, &relocated).unwrap();

        let refusal =
            reprove_under_window(window, &report, &report.entries()[0], &probe).unwrap_err();

        assert!(matches!(refusal, SettlementProofRefusalV1::CannotProve(_)));
        remove_root(&relocated);
    }

    /// Discriminates row-level claim-authority loss from a fresh proved-negative decision.
    #[test]
    fn a_row_level_authority_failure_is_cannot_prove() {
        let root = root("row-level-cannot-prove");
        let target = root.join("ownr-run7-abc");
        let record = write_authorized_record(&root, &target, 'd');
        let probe = current_probe(&record);
        let report = authoritative_report(&root, &probe);
        let unrelated_common_dir = root.join("unrelated-common-dir");
        fs::create_dir(&unrelated_common_dir).unwrap();
        let unavailable_probe = CurrentAbsenceProbeV1 {
            source_common_dir: observed_identity(&unrelated_common_dir).directory_identity,
        };
        let window = SettlementWindowV1::open(&root, &target.to_string_lossy()).unwrap();
        let refusal =
            reprove_under_window(window, &report, &report.entries()[0], &unavailable_probe)
                .unwrap_err();

        assert!(matches!(refusal, SettlementProofRefusalV1::CannotProve(_)));
        remove_root(&root);
    }
    /// Discriminates a non-authoritative report from reaching a later settlement mutation.
    #[test]
    fn a_non_authoritative_scan_refuses() {
        let root = root("non-authoritative");
        let target = root.join("ownr-run7-abc");
        let record = write_authorized_record(&root, &target, 'f');
        let probe = current_probe(&record);
        let report = authoritative_report(&root, &probe);
        let non_authoritative = ExactAbsenceSweepReportV1::new(
            root.to_string_lossy().into_owned(),
            Some(root.to_string_lossy().into_owned()),
            ExactAbsenceScanStatusV1::new(
                ExactAbsenceEnumerationV1::Incomplete { skipped_entries: 1 },
                CustodyRootObservationV1::Pinned,
            ),
            vec![report.entries()[0].clone()],
        );
        let window = SettlementWindowV1::open(&root, &target.to_string_lossy()).unwrap();
        let refusal = reprove_under_window(
            window,
            &non_authoritative,
            &non_authoritative.entries()[0],
            &probe,
        )
        .unwrap_err();

        assert!(matches!(refusal, SettlementProofRefusalV1::Refused(_)));
        remove_root(&root);
    }

    /// Discriminates a legacy assessment sharing the raw authorized decision from a custody proof.
    #[test]
    fn a_legacy_assessment_refuses() {
        let root = root("legacy");
        let target = root.join("ownr-run7-abc");
        let record = write_authorized_record(&root, &target, '1');
        let probe = current_probe(&record);
        let name = custody_record_name(&target.to_string_lossy()).unwrap();
        let legacy = ExactAbsenceSweepEntryV1::new(
            root.join(&name).to_string_lossy().into_owned(),
            name,
            ExactAbsenceRecordAssessmentV1::Legacy(UnusedCandidateDecisionV1::Authorized),
            None,
        );
        let report = ExactAbsenceSweepReportV1::new(
            root.to_string_lossy().into_owned(),
            Some(root.to_string_lossy().into_owned()),
            ExactAbsenceScanStatusV1::new(
                ExactAbsenceEnumerationV1::Complete,
                CustodyRootObservationV1::Pinned,
            ),
            vec![legacy],
        );
        let window = SettlementWindowV1::open(&root, &target.to_string_lossy()).unwrap();
        let refusal =
            reprove_under_window(window, &report, &report.entries()[0], &probe).unwrap_err();

        assert!(matches!(refusal, SettlementProofRefusalV1::Refused(_)));
        remove_root(&root);
    }

    /// Discriminates a state accepted by the scan's candidate population from the narrower T3b
    /// settlement population, and separately rejects a bare protection-prepared record.
    #[test]
    fn a_wrong_state_or_missing_claim_refuses() {
        for missing_claim in [false, true] {
            let root = root(if missing_claim {
                "bare-claim"
            } else {
                "wrong-state"
            });
            let target = root.join("ownr-run7-abc");
            let baseline =
                write_authorized_record(&root, &target, if missing_claim { '2' } else { '3' });
            let probe = current_probe(&baseline);
            let mut record = baseline;
            if missing_claim {
                record.claim = None;
            } else {
                record.state = WorktreeCustodyStateV1::PreservationUnknown {
                    reason: PreservationReasonV1::MaterializationInFlight,
                };
                record.claim.as_mut().unwrap().reason =
                    PreservationReasonV1::MaterializationInFlight;
            }
            fs::write(
                root.join(custody_record_name(&target.to_string_lossy()).unwrap()),
                record.encode_canonical().unwrap(),
            )
            .unwrap();
            let report = authoritative_report(&root, &probe);
            let window = SettlementWindowV1::open(&root, &target.to_string_lossy()).unwrap();
            let refusal =
                reprove_under_window(window, &report, &report.entries()[0], &probe).unwrap_err();

            assert!(matches!(refusal, SettlementProofRefusalV1::Refused(_)));
            remove_root(&root);
        }
    }

    /// Discriminates a proof that releases its window before the proof itself is dropped.
    #[test]
    fn the_proof_cannot_outlive_its_window() {
        let root = root("proof-owns-window");
        let target = root.join("ownr-run7-abc");
        let record = write_authorized_record(&root, &target, '4');
        let probe = current_probe(&record);
        let report = authoritative_report(&root, &probe);
        let window = SettlementWindowV1::open(&root, &target.to_string_lossy()).unwrap();
        let proof = reprove_under_window(window, &report, &report.entries()[0], &probe).unwrap();

        assert!(matches!(
            SettlementWindowV1::open(&root, &target.to_string_lossy()),
            Err(SettlementWindowRefusalV1::CellContended(_))
        ));
        drop(proof);
        drop(SettlementWindowV1::open(&root, &target.to_string_lossy()).unwrap());
        remove_root(&root);
    }

    /// Discriminates a later proof path gaining a mutation edge while still returning a capability.
    /// Effect-freedom is conditional on the caller-supplied `ExactAbsenceProbeV1`; this slice has no
    /// production caller, and any reachable spawn can arrive only through that supplied probe.
    #[test]
    fn the_reproof_mints_no_effect() {
        let root = root("reproof-effect-audit");
        let target = root.join("ownr-run7-abc");
        let record = write_authorized_record(&root, &target, '5');
        let probe = current_probe(&record);
        let report = authoritative_report(&root, &probe);
        let name = custody_record_name(&target.to_string_lossy()).unwrap();
        let before = fs::read(root.join(&name)).unwrap();
        let window = SettlementWindowV1::open(&root, &target.to_string_lossy()).unwrap();
        let proof = reprove_under_window(window, &report, &report.entries()[0], &probe).unwrap();

        assert_eq!(fs::read(root.join(name)).unwrap(), before);
        assert_eq!(proof.worktree_path(), target.to_string_lossy());
        let settlement_source = include_str!("settle.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        let sweep_source = include_str!("sweep.rs")
            .split_once("pub(crate) fn reprove_exact_absence_entry")
            .unwrap()
            .1
            .split_once("fn project_exact_scan_result")
            .unwrap()
            .0;
        fn source_slice<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
            let after_start = source
                .split_once(start)
                .unwrap_or_else(|| panic!("source slice is missing start anchor: {start}"));
            after_start
                .1
                .split_once(end)
                .unwrap_or_else(|| panic!("source slice is missing end anchor: {end}"))
                .0
        }
        fn source_tail<'a>(source: &'a str, start: &str) -> &'a str {
            source
                .split_once(start)
                .unwrap_or_else(|| panic!("source slice is missing start anchor: {start}"))
                .1
        }
        let complete_sweep_source = include_str!("sweep.rs");
        let checked_scan_source = include_str!("sweep/checked_scan.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        let audited_sources = [
            settlement_source,
            sweep_source,
            source_slice(
                complete_sweep_source,
                "fn project_exact_scan_result",
                "fn sweep_orphans_with_exact_absence_with_pin_opener",
            ),
            source_slice(
                complete_sweep_source,
                "fn report_exact_scan_projection_row",
                "/// Re-run the report's exact scan",
            ),
            source_slice(
                complete_sweep_source,
                "fn assess_custody_record",
                "struct ExactScanProjectionRowV1",
            ),
            source_slice(
                complete_sweep_source,
                "fn decide_unused_legacy_sidecar",
                "/// Reject records whose target cannot be constructed",
            ),
            source_slice(
                complete_sweep_source,
                "fn decide_unused_candidate_evidence",
                "#[must_use]",
            ),
            source_tail(checked_scan_source, "fn scan_checked_rows_with_source"),
        ];
        assert!(
            settlement_source.contains("probe: &dyn ExactAbsenceProbeV1")
                && !settlement_source.contains("impl ExactAbsenceProbeV1 for")
                && !settlement_source.contains("HostGitWorktree"),
            "settle.rs must not construct an ExactAbsenceProbeV1 implementation; the probe is caller-supplied"
        );
        for forbidden_edge in [
            "std::fs::rename(",
            "std::fs::remove_file(",
            "std::fs::remove_dir_all(",
            "custody_writer::",
            "provider::",
            ".publish_",
            ".replace_",
            "remove_argv",
            "prune_argv",
        ] {
            assert!(
                audited_sources
                    .iter()
                    .all(|source| !source.contains(forbidden_edge)),
                "the re-proof path must not reach {forbidden_edge}"
            );
        }
        assert!(
            audited_sources
                .iter()
                .all(|source| !source.contains("std::process::Command")),
            "the re-proof code must not originate a process spawn; any reachable spawn belongs to the caller-supplied probe"
        );
        drop(proof);
        remove_root(&root);
    }
}
