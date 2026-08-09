//! The R2f1b custody lock — the single mutual-exclusion cell shared by creation, every custody
//! transition, preservation, deletion, and sweep selection (focused boundary §2.5 step 5, §5.1
//! step 1).
//!
//! This module invents no locking mechanism. It is a typed, single-derivation facade over
//! `bridge_core::liveness`'s existing persistent lock (`acquire_persistent_lock_in`,
//! `acquire_persistent_lock_blocking_in`, [`PersistentLockGuard`]), whose `Drop` releases with
//! `LOCK_UN` *without* unlinking the path — the F-3 consolidation, and the property that makes it
//! safe to reuse one stable lock path across many acquirers. What this module adds is the five
//! things a shared cell has to pin down before two subsystems can agree they are using it:
//!
//! **(a) The lock id.** [`custody_lock_id`] — the [`WorktreeCustodyIdV1`] verbatim. That type is
//! `"custody-"` plus exactly 64 lowercase hex digits, so it is a single path component by
//! construction: no separator, no `.`/`..`, no NUL, fixed length. Deriving the key from the typed
//! id rather than from a path or a session string is what keeps a writer and a sweeper — which
//! know entirely different things about a checkout — computing the same key.
//!
//! **(b) The directory.** [`custody_lock_dir`] — `<worktree root>/.custody-locks`. Three reasons
//! it is there and not elsewhere: it is on the same filesystem as the checkouts it guards, so it
//! cannot outlive them on a different volume; a dotted subdirectory matches *neither* sweep
//! pattern (`*.meta.json`, `*.custody.v1.json`), so no scanner can mistake a lock for a record;
//! and it is deliberately NOT inside any checkout, because a lock stored inside the directory
//! whose deletion it guards is destroyed by the very act it is meant to serialize.
//!
//! **(c) Acquisition order.** `acquire_persistent_lock_blocking_in` documents a global obligation:
//! never hold a lock the current holder needs in order to release yours. The custody lock is the
//! **innermost** file lock in this workspace. The declared total order is
//!
//! ```text
//! run lease (liveness::acquire_lease, held for the whole process run)
//!   → operation lock (implement-resume, verify, run-id: liveness::acquire_persistent_lock*)
//!     → custody lock (this module, one per checkout)
//!       → in-process backend mutexes (WorktreeBackend map / cleanup cells / cell state)
//! ```
//!
//! A custody-lock holder must therefore never acquire a run lease or an operation lock, and must
//! never hold the lock across an `.await` that can only complete once another task takes it.
//!
//! **(d) Contention.** Two acquirers, and which one a caller may use is part of the contract,
//! not a preference:
//!
//! * [`try_acquire_custody_lock_in`] refuses with [`CustodyLockRefusalV1::Contended`]. **Every
//!   deletion-side and sweep-side caller must use this one.** A sweep that blocks behind a writer
//!   is a sweep that can stall a boot; and a refusal is exactly the right answer for a
//!   destructive caller, because a custody cell it cannot inspect is *unknown*, and unknown never
//!   licenses deletion (§5.2).
//! * [`acquire_custody_lock_blocking_in`] waits, and is for the transition writer itself, whose
//!   critical section is bounded by one publish/replace/sync sequence.
//!
//! **(e) Who retains the guard, and for how long.** The guard is held by *one custody transition*
//! — open, write the temp, sync it, replace, parent-sync, reverify — and released at its end. It
//! is **not** a lifetime lease on the checkout: liveness of a live checkout is the run lease's
//! job (`liveness::acquire_lease`), and the durable protective state is the record on disk, not
//! the lock. A destructive caller holds it across its whole decide-and-remove sequence, so no
//! transition can publish between the decision and the act.
//!
//! Slice 2b1 ships the primitive and its contract. Wiring transitions to it is 2b2's (writer) and
//! 2c's (preservation, deletion); this module mints no deletion authority of its own.

use bridge_core::execution_policy::WorktreeCustodyIdV1;
use bridge_core::liveness::{
    acquire_persistent_lock_blocking_in, acquire_persistent_lock_in, PersistentLockGuard,
};
use std::path::{Path, PathBuf};

/// The lock directory's name beneath the worktree root. Dotted so neither sweep pattern selects
/// it, and stable so two processes derive the same path.
pub const CUSTODY_LOCK_DIR_NAME: &str = ".custody-locks";

/// `<worktree root>/.custody-locks` — the one directory every custody-cell acquirer must use.
#[must_use]
pub fn custody_lock_dir(worktree_root: &Path) -> PathBuf {
    worktree_root.join(CUSTODY_LOCK_DIR_NAME)
}

/// The lock id for a custody cell: the custody id verbatim.
///
/// Taking the typed id rather than a `&str` is the point — the shape guarantee that makes this a
/// safe path component (`"custody-"` + 64 lowercase hex, validated by
/// `WorktreeCustodyIdV1::parse`) belongs to the type, and a stringly key would let a caller
/// derive one from a session id or a path and silently miss the cell everyone else is using.
#[must_use]
pub fn custody_lock_id(custody_id: &WorktreeCustodyIdV1) -> &str {
    custody_id.as_str()
}

/// Why a custody cell could not be entered.
#[derive(Debug, thiserror::Error)]
pub enum CustodyLockRefusalV1 {
    /// Another holder owns this exact custody cell. For a destructive or sweeping caller this is
    /// terminal: the cell is unknown, and unknown is never deleted.
    #[error("custody cell {0} is held by another owner")]
    Contended(String),
    /// The lock could not be created or opened at all (permissions, unwritable root, …). Also
    /// terminal for a destructive caller, and for the same reason.
    #[error("custody cell {0} is unavailable: {1}")]
    Unavailable(String, #[source] std::io::Error),
}

/// A held custody cell. Dropping it releases the cell (`LOCK_UN`) without unlinking the path.
pub struct CustodyLockGuardV1 {
    custody_id: WorktreeCustodyIdV1,
    guard: PersistentLockGuard,
}

// Hand-written because `PersistentLockGuard` is not `Debug` (it owns a `File`); the two fields a
// diagnostic needs are the cell it holds and where.
impl std::fmt::Debug for CustodyLockGuardV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CustodyLockGuardV1")
            .field("custody_id", &self.custody_id.as_str())
            .field("path", &self.guard.path())
            .finish()
    }
}

impl CustodyLockGuardV1 {
    /// Which cell this guard holds. Carried so a transition cannot publish a record under one
    /// custody id while holding the cell of another.
    #[must_use]
    pub fn custody_id(&self) -> &WorktreeCustodyIdV1 {
        &self.custody_id
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        self.guard.path()
    }
}

fn classify(custody_id: &WorktreeCustodyIdV1, error: std::io::Error) -> CustodyLockRefusalV1 {
    if error.kind() == std::io::ErrorKind::WouldBlock {
        CustodyLockRefusalV1::Contended(custody_id.as_str().to_owned())
    } else {
        CustodyLockRefusalV1::Unavailable(custody_id.as_str().to_owned(), error)
    }
}

/// Enter a custody cell, or refuse. **The required acquirer for every deletion-side and
/// sweep-side caller** — see the module docs, contention.
pub fn try_acquire_custody_lock_in(
    worktree_root: &Path,
    custody_id: &WorktreeCustodyIdV1,
) -> Result<CustodyLockGuardV1, CustodyLockRefusalV1> {
    acquire_persistent_lock_in(
        &custody_lock_dir(worktree_root),
        custody_lock_id(custody_id),
    )
    .map(|guard| CustodyLockGuardV1 {
        custody_id: custody_id.clone(),
        guard,
    })
    .map_err(|error| classify(custody_id, error))
}

/// Enter a custody cell, waiting for the current holder. For the transition writer only.
///
/// `on_contended` fires at most once, on the calling thread, exactly when this call is about to
/// wait. This blocks the calling thread with no timeout: honour the module's acquisition order,
/// and never call it from an async task without offloading.
pub fn acquire_custody_lock_blocking_in(
    worktree_root: &Path,
    custody_id: &WorktreeCustodyIdV1,
    on_contended: &dyn Fn(),
) -> Result<CustodyLockGuardV1, CustodyLockRefusalV1> {
    acquire_persistent_lock_blocking_in(
        &custody_lock_dir(worktree_root),
        custody_lock_id(custody_id),
        on_contended,
    )
    .map(|guard| CustodyLockGuardV1 {
        custody_id: custody_id.clone(),
        guard,
    })
    .map_err(|error| classify(custody_id, error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::time::Duration;

    fn cid(digit: char) -> WorktreeCustodyIdV1 {
        WorktreeCustodyIdV1::parse(format!("custody-{}", digit.to_string().repeat(64))).unwrap()
    }

    fn root(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "a2a-bridge-custody-lock-{name}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    /// Discriminates a lock id derived from anything but the custody id — a path, a session id, a
    /// hash — which would leave a writer and a sweeper serializing on two different files while
    /// each believes it holds the cell. Also pins the single-component property the raw path join
    /// depends on.
    #[test]
    fn the_lock_id_is_the_custody_id_and_is_a_single_path_component() {
        let custody_id = cid('a');

        let id = custody_lock_id(&custody_id);

        assert_eq!(id, custody_id.as_str());
        assert!(!id.contains(std::path::MAIN_SEPARATOR));
        assert!(!id.contains('/') && !id.contains('\0'));
        assert!(id != "." && id != ".." && !id.is_empty());
    }

    /// The lock directory must live beside the checkouts, never inside one: a lock stored under
    /// `<checkout>/` is destroyed by the removal it exists to serialize, so a second actor would
    /// mint a fresh inode and both would "hold" the cell.
    #[test]
    fn the_lock_directory_is_a_sibling_of_the_checkouts_never_inside_one() {
        let worktree_root = Path::new("/wt-root");
        let checkout = worktree_root.join("ownr-run7-abc");

        let dir = custody_lock_dir(worktree_root);

        assert_eq!(dir, worktree_root.join(".custody-locks"));
        assert!(!dir.starts_with(&checkout));
        assert!(dir.starts_with(worktree_root));
    }

    /// Discriminates naming the lock directory (or its files) something either sweep pattern
    /// selects. A `<id>.custody.v1.json`-shaped lock would be scanned as an unreadable record on
    /// every boot; a `.meta.json`-shaped one would be handed to the legacy arm, which deletes.
    #[test]
    fn no_sweep_pattern_selects_the_custody_lock_directory_or_its_files() {
        let worktree_root = root("sweep-blind");
        let custody_id = cid('b');
        let _held = try_acquire_custody_lock_in(&worktree_root, &custody_id).unwrap();

        let scanned = crate::sweep::scan_worktree_records(&worktree_root.to_string_lossy());

        assert!(
            scanned.is_empty(),
            "the custody lock directory must be invisible to both sweep patterns: {scanned:?}"
        );
        let lock_dir = custody_lock_dir(&worktree_root);
        let lock_name = lock_dir.join(format!("{}.lock", custody_lock_id(&custody_id)));
        assert!(!crate::custody::is_custody_record_name(
            &lock_name.to_string_lossy()
        ));
        assert!(!lock_name.to_string_lossy().ends_with(".meta.json"));
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// Writer-vs-sweep, order 1: the writer holds the cell first. Discriminates a sweep-side
    /// acquirer that blocks (which would stall a boot behind a live transition) or, worse, one
    /// that succeeds anyway — which would let a sweep classify a record mid-replacement.
    #[test]
    fn a_sweep_is_refused_while_the_writer_holds_the_cell_and_admitted_after_it_releases() {
        let worktree_root = root("writer-then-sweep");
        let custody_id = cid('c');

        let writer = try_acquire_custody_lock_in(&worktree_root, &custody_id).unwrap();
        let refusal = try_acquire_custody_lock_in(&worktree_root, &custody_id).unwrap_err();
        assert!(
            matches!(&refusal, CustodyLockRefusalV1::Contended(id) if id == custody_id.as_str()),
            "unexpected refusal: {refusal:?}"
        );

        drop(writer);
        let sweep = try_acquire_custody_lock_in(&worktree_root, &custody_id).unwrap();
        assert_eq!(sweep.custody_id(), &custody_id);
        drop(sweep);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// Writer-vs-sweep, order 2: the sweep holds the cell first and the writer must WAIT rather
    /// than refuse — the asymmetry the module contract requires. Discriminates a blocking
    /// acquirer that is really the non-blocking one (the writer would spuriously fail a
    /// transition), and one whose `on_contended` never fires (an operator gets no reason for a
    /// stall). The `waiting` flag also proves the writer did not simply acquire immediately.
    #[test]
    fn the_writer_waits_for_a_sweep_holder_and_is_told_it_is_waiting() {
        let worktree_root = root("sweep-then-writer");
        let custody_id = cid('d');
        let sweep = try_acquire_custody_lock_in(&worktree_root, &custody_id).unwrap();

        let (entered_tx, entered_rx) = mpsc::channel();
        let waited = Arc::new(AtomicBool::new(false));
        let writer = std::thread::spawn({
            let worktree_root = worktree_root.clone();
            let custody_id = custody_id.clone();
            let waited = Arc::clone(&waited);
            move || {
                let guard = acquire_custody_lock_blocking_in(&worktree_root, &custody_id, &|| {
                    waited.store(true, Ordering::SeqCst);
                    entered_tx.send(()).unwrap();
                })
                .unwrap();
                assert_eq!(guard.custody_id(), &custody_id);
            }
        });

        // The writer reports it is about to wait; it cannot have the cell yet.
        entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(!writer.is_finished());

        drop(sweep);
        writer.join().unwrap();
        assert!(waited.load(Ordering::SeqCst));
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// Preserve-vs-delete, order 1: preservation holds the cell, deletion arrives. The deletion
    /// side must be refused outright — this is the interleaving §5.1 exists to prevent, where a
    /// context-free deletion runs between a preservation's record write and its parent sync.
    #[test]
    fn a_deletion_is_refused_while_preservation_holds_the_cell() {
        let worktree_root = root("preserve-then-delete");
        let custody_id = cid('e');

        let preserving = acquire_custody_lock_blocking_in(&worktree_root, &custody_id, &|| {
            panic!("an uncontended acquire must not report contention")
        })
        .unwrap();

        let refusal = try_acquire_custody_lock_in(&worktree_root, &custody_id).unwrap_err();

        assert!(matches!(refusal, CustodyLockRefusalV1::Contended(_)));
        drop(preserving);
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// Preserve-vs-delete, order 2: deletion holds the cell first and preservation must wait for
    /// the destructive act to finish rather than publish into the middle of it. Together with
    /// order 1 this pins that the cell is a real mutex in both directions, not a one-way guard.
    #[test]
    fn preservation_waits_for_a_deletion_holder_before_publishing() {
        let worktree_root = root("delete-then-preserve");
        let custody_id = cid('f');
        let deleting = try_acquire_custody_lock_in(&worktree_root, &custody_id).unwrap();

        let (entered_tx, entered_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let preserving = std::thread::spawn({
            let worktree_root = worktree_root.clone();
            let custody_id = custody_id.clone();
            move || {
                let _guard = acquire_custody_lock_blocking_in(&worktree_root, &custody_id, &|| {
                    entered_tx.send(()).unwrap();
                })
                .unwrap();
                done_tx.send(()).unwrap();
            }
        });

        entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(
            done_rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "preservation must not enter the cell while the deletion holds it"
        );

        drop(deleting);
        done_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        preserving.join().unwrap();
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// Discriminates a cell keyed on anything coarser than the custody id (the root, the owner,
    /// the run): two unrelated checkouts would serialize against each other, turning every
    /// concurrent workflow node into a queue.
    #[test]
    fn distinct_custody_cells_do_not_contend() {
        let worktree_root = root("distinct-cells");

        let first = try_acquire_custody_lock_in(&worktree_root, &cid('1')).unwrap();
        let second = try_acquire_custody_lock_in(&worktree_root, &cid('2')).unwrap();

        assert_ne!(first.path(), second.path());
        drop((first, second));
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }

    /// Discriminates a guard whose `Drop` unlinks the lock path (the `LeaseGuard` behaviour,
    /// which is wrong for a reusable cell): a contender that opened the doomed inode before the
    /// unlink would flock it while a later acquirer creates a fresh one, giving two holders of
    /// one cell on two inodes. The stable path is the invariant; assert it survives a release.
    #[test]
    fn releasing_a_cell_leaves_its_lock_path_in_place() {
        let worktree_root = root("stable-path");
        let custody_id = cid('9');

        let guard = try_acquire_custody_lock_in(&worktree_root, &custody_id).unwrap();
        let path = guard.path().to_path_buf();
        drop(guard);

        assert!(path.exists(), "the custody lock path must stay stable");
        std::fs::remove_dir_all(&worktree_root).unwrap();
    }
}
