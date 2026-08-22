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
};
use crate::custody_lock::{
    try_acquire_custody_lock_in, try_acquire_publication_lock_in, CustodyLockGuardV1,
    CustodyLockRefusalV1, PublicationLockGuardV1,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custody::{WorktreeCustodyStateV1, WORKTREE_CUSTODY_RECORD_SCHEMA_V1};
    use crate::custody_lock::{
        custody_lock_dir, custody_lock_id, custody_publication_lock_id,
        try_acquire_custody_lock_in, try_acquire_publication_lock_in,
    };
    use crate::custody_writer::planned_identity;
    use bridge_core::execution_policy::Sha256HexV1;
    use bridge_core::ids::{AttemptId, AttemptIdentity, ExecutionId};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::time::Duration;

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
}
