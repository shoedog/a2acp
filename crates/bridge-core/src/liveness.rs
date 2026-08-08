//! flock-based liveness for managed containers (Increment A). A run holds an exclusive `flock` on a per-run
//! lease file for its whole life; the OS releases it when the process dies (clean OR crash). A sweeper that
//! can ACQUIRE the lock ⇒ the owner is gone. This is PID-reuse-, clock-drift-, and reboot-safe (unlike
//! probing PID start-times) and needs no new deps — `libc::flock`.

use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

/// Non-blocking flock. `Ok(true)` = acquired, `Ok(false)` = held by another open file description,
/// `Err` = a real error.
fn flock_nb(file: &std::fs::File, exclusive: bool) -> std::io::Result<bool> {
    let op = (if exclusive {
        libc::LOCK_EX
    } else {
        libc::LOCK_SH
    }) | libc::LOCK_NB;
    let rc = unsafe { libc::flock(file.as_raw_fd(), op) };
    if rc == 0 {
        return Ok(true);
    }
    let e = std::io::Error::last_os_error();
    if e.raw_os_error() == Some(libc::EWOULDBLOCK) {
        return Ok(false);
    }
    Err(e)
}

/// Blocking exclusive flock (`LOCK_EX` WITHOUT `LOCK_NB`): waits until the current holder releases.
/// `EINTR` is retried — a signal (SIGCHLD from any spawned child, SIGWINCH from a resize) must not be
/// read as "the lock is unavailable", which would silently drop the exclusion this wait exists to provide.
fn flock_blocking_exclusive(file: &std::fs::File) -> std::io::Result<()> {
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        return Err(e);
    }
}

/// Explicit release, generalized over any display-able lease/lock identifier — a filesystem path
/// (pass `.display()`), an evidence id, a named lock label: whatever a call site can name the held
/// resource by, for the failure log below. `pub` because every flock guard in the binary shares
/// this one release-then-report path, not just this crate's own two: [`LeaseGuard::drop`],
/// [`PersistentLockGuard::drop`], the bin crate's `EvidenceLeaseGuardV1`, and the bin crate's
/// `OwnerAdmissionLock` / `AdmissionAuthorityLocks` / `AuthorityMutationLock` (all released a raw
/// `libc::flock(..., LOCK_UN)` with the result silently discarded before this consolidation — on a
/// filesystem where `LOCK_UN` can fail, that silence would resurrect the inherited-descriptor bug
/// this release exists to prevent, undetected).
///
/// Load-bearing: every one of those guards releases HERE rather than at close (see
/// [`PersistentLockGuard::drop`]'s doc for why closing alone is not a release), so a filesystem
/// that cannot unlock — `ENOLCK`/`EOPNOTSUPP` are reachable on NFS, and `$HOME/.a2a-bridge/leases`
/// may well be NFS — would silently resurrect the spawn-window bug this release exists to prevent.
/// Report it instead of swallowing it; releasing is still best-effort because the close that
/// follows is the only remaining recourse.
pub fn flock_unlock(file: &std::fs::File, id: impl std::fmt::Display) {
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) } != 0 {
        report_unlock_failure(id, std::io::Error::last_os_error());
    }
}

/// The on-failure half of [`flock_unlock`], split out so a caller with a test-only seam for
/// injecting a release failure can reuse this EXACT log-then-assert behavior instead of
/// re-deriving it. A live file descriptor cannot be corrupted to force this path in a test: the
/// standard library's I/O-safety runtime check hard-aborts the process the moment a `File`'s own
/// descriptor is closed out from under it, so callers that need to prove "a release failure is
/// loud, not silent" (e.g. the bin crate's `compatibility_schedule_state` guards) synthesize the
/// `io::Error` instead of causing a real one.
pub fn report_unlock_failure(id: impl std::fmt::Display, err: std::io::Error) {
    tracing::error!(
        lease = %id,
        error = %err,
        "releasing an advisory lock failed; a concurrently spawned child may hold it until it execs"
    );
    // Fail loudly in debug builds — but never while unwinding, where a second panic aborts the process.
    debug_assert!(
        std::thread::panicking(),
        "flock(LOCK_UN) failed for {id}: {err}"
    );
}

/// Stable per-host id (best-effort). Labelled `a2a.host` so a sweep never reaps another machine's containers.
pub fn host_id() -> String {
    let raw = std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    parse_host(&raw)
}

fn parse_host(raw: &str) -> String {
    let h = raw.trim();
    if h.is_empty() {
        "localhost".into()
    } else {
        h.to_string()
    }
}

/// The host-global lock/lease directory (`$A2A_LEASE_DIR`, else `$HOME/.a2a-bridge/leases`). Public so
/// callers whose lock identity is host-global — not scoped to one bridge root — can name the same
/// namespace instead of inventing a second one.
pub fn lease_dir() -> PathBuf {
    if let Ok(d) = std::env::var("A2A_LEASE_DIR") {
        return PathBuf::from(d);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".a2a-bridge").join("leases")
}

/// Held for the owning process's life; the OS releases the flock when `_file` drops (clean OR crash). The
/// file is removed on a clean drop; after a crash it persists with the lock FREE (the recovery signal).
pub struct LeaseGuard {
    path: PathBuf,
    _file: std::fs::File,
}
impl LeaseGuard {
    pub fn path(&self) -> &Path {
        &self.path
    }
}
impl Drop for LeaseGuard {
    fn drop(&mut self) {
        // Unlink first to shrink the doomed-inode window; safe only because lease ids are unique per run —
        // do not reuse lease ids across acquirers. (A contender that opened this inode before the unlink and
        // flocks after the release below would acquire the doomed inode while a later `create` mints a fresh
        // one: two holders of one lease id on two inodes.)
        let _ = std::fs::remove_file(&self.path);
        flock_unlock(&self._file, self.path.display()); // see PersistentLockGuard::drop — closing alone is not a release
    }
}

/// Create + exclusively flock `<dir>/<run_id>.lock`. The returned guard MUST outlive the run.
pub fn acquire_lease_in(dir: &Path, run_id: &str) -> std::io::Result<LeaseGuard> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{run_id}.lock"));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false) // a lease file is a lock handle; never clobber its (irrelevant) content
        .open(&path)?;
    if !flock_nb(&file, true)? {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "lease already held",
        ));
    }
    Ok(LeaseGuard { path, _file: file })
}

/// A stable-path advisory mutex. Unlike [`LeaseGuard`], dropping this guard never removes the lock path:
/// a contender may already have opened that inode and be waiting to acquire it. Keeping the path stable until
/// every contender closes its descriptor prevents a later opener from locking a replacement inode concurrently.
pub struct PersistentLockGuard {
    path: PathBuf,
    _file: std::fs::File,
}
impl PersistentLockGuard {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Release explicitly instead of relying on the close. A `flock` belongs to the OPEN FILE DESCRIPTION, so
/// closing our descriptor frees the lock only once EVERY descriptor sharing that description is gone — and
/// every concurrent process spawn copies our whole descriptor table into the child, where `FD_CLOEXEC` does
/// not take effect until `exec`. Relying on the close therefore leaks the lock for the width of an unrelated
/// spawn, making the next `resume`/`merge` fail spuriously with "operation lock already held". `LOCK_UN`
/// drops the lock from the description itself, so no inherited copy can hold it open.
impl Drop for PersistentLockGuard {
    fn drop(&mut self) {
        flock_unlock(&self._file, self.path.display());
    }
}

/// Open (creating if absent) the stable lock path shared by both persistent-lock acquirers. Never
/// truncates: a lock file is a handle, and its content is irrelevant.
fn open_persistent_lock_file(
    dir: &Path,
    lock_id: &str,
) -> std::io::Result<(PathBuf, std::fs::File)> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{lock_id}.lock"));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)?;
    Ok((path, file))
}

/// Create + exclusively flock `<dir>/<lock_id>.lock` without unlinking it on guard drop. This is for reusable
/// operation mutexes; crash-detecting run leases must continue to use [`acquire_lease_in`].
pub fn acquire_persistent_lock_in(
    dir: &Path,
    lock_id: &str,
) -> std::io::Result<PersistentLockGuard> {
    let (path, file) = open_persistent_lock_file(dir, lock_id)?;
    if !flock_nb(&file, true)? {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "operation lock already held",
        ));
    }
    Ok(PersistentLockGuard { path, _file: file })
}

/// Same lock, same create-no-unlink contract and same [`PersistentLockGuard::drop`] release — but WAITS
/// for the current holder instead of failing. For mutexes whose critical section is legitimately long
/// (a containerized verify runs for minutes), where [`acquire_persistent_lock_in`]'s `WouldBlock` would
/// turn a normal queue into a spurious refusal.
///
/// One non-blocking attempt runs first; `on_contended` fires ONLY when that attempt found the lock held,
/// i.e. exactly when this call is about to wait, so the caller can tell the operator why it is stalled.
/// It is called at most once, before the wait, on the calling thread.
///
/// Caller obligations: this blocks the calling thread with no timeout, so (1) never hold another lock that
/// the current holder needs in order to release this one — acquire in a fixed global order, and (2) do not
/// call it from a thread whose progress the holder depends on.
pub fn acquire_persistent_lock_blocking_in(
    dir: &Path,
    lock_id: &str,
    on_contended: &dyn Fn(),
) -> std::io::Result<PersistentLockGuard> {
    let (path, file) = open_persistent_lock_file(dir, lock_id)?;
    if !flock_nb(&file, true)? {
        on_contended();
        flock_blocking_exclusive(&file)?;
    }
    Ok(PersistentLockGuard { path, _file: file })
}

/// Production: acquire under the default lease dir (`$A2A_LEASE_DIR` else `$HOME/.a2a-bridge/leases`).
pub fn acquire_lease(run_id: &str) -> std::io::Result<LeaseGuard> {
    acquire_lease_in(&lease_dir(), run_id)
}

/// Probe a lease path WITHOUT holding it. `Some(true)` = free (owner dead); `Some(false)` = held (alive);
/// `None` = absent/unreadable (caller ⇒ Unknown ⇒ spare).
pub trait LeaseProbe: Send + Sync {
    fn try_state(&self, lease_path: &str) -> Option<bool>;
}

pub struct FsLeaseProbe;
impl LeaseProbe for FsLeaseProbe {
    fn try_state(&self, lease_path: &str) -> Option<bool> {
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(lease_path)
            .ok()?;
        match flock_nb(&f, true) {
            Ok(true) => {
                flock_unlock(&f, lease_path); // acquired ⇒ free ⇒ owner dead; release so we don't claim it
                Some(true)
            }
            Ok(false) => Some(false), // held ⇒ owner alive
            Err(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn held_lease_probes_alive_then_absent_after_drop() {
        let dir = tempfile::tempdir().unwrap();
        let probe = FsLeaseProbe;
        let guard = acquire_lease_in(dir.path(), "r1").unwrap();
        let path = guard.path().to_string_lossy().into_owned();
        assert_eq!(probe.try_state(&path), Some(false), "held ⇒ alive");
        drop(guard);
        assert_eq!(
            probe.try_state(&path),
            None,
            "removed on clean drop ⇒ absent"
        );
    }

    #[test]
    fn crashed_lease_file_persists_with_free_lock_probes_dead() {
        // Simulate a crash: lock acquired then the fd drops (OS releases), but the file is NOT removed.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crashed.lock");
        {
            let f = std::fs::OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .truncate(false)
                .open(&path)
                .unwrap();
            assert!(flock_nb(&f, true).unwrap());
        } // f drops → lock released; file persists
        assert_eq!(
            FsLeaseProbe.try_state(path.to_str().unwrap()),
            Some(true),
            "free lock on a persisted file ⇒ dead"
        );
    }

    #[test]
    fn second_acquire_of_held_lease_fails() {
        let dir = tempfile::tempdir().unwrap();
        let _g1 = acquire_lease_in(dir.path(), "x").unwrap();
        assert!(
            acquire_lease_in(dir.path(), "x").is_err(),
            "a held lease can't be acquired again"
        );
    }

    #[test]
    fn persistent_lock_keeps_one_inode_across_open_drop_reacquire_interleaving() {
        let dir = tempfile::tempdir().unwrap();
        let first = acquire_persistent_lock_in(dir.path(), "same-run").unwrap();
        let path = first.path().to_path_buf();

        // Contender B opens the path while A owns it, but has not tried flock yet.
        let opened_before_release = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        drop(first);

        // C opens after A releases. Because A did not unlink, B and C still address the same inode.
        let current = acquire_persistent_lock_in(dir.path(), "same-run").unwrap();
        assert!(
            !flock_nb(&opened_before_release, true).unwrap(),
            "an earlier opener must not acquire a detached predecessor inode beside the current lock"
        );
        drop(current);

        assert!(path.exists(), "a reusable operation-lock path must persist");
        let reacquired = acquire_persistent_lock_in(dir.path(), "same-run").unwrap();
        drop(reacquired);
    }

    /// `flock` is owned by the OPEN FILE DESCRIPTION, not by one descriptor, so closing our descriptor
    /// releases the lock only once EVERY descriptor sharing that description is gone. Any concurrent
    /// process spawn copies the whole descriptor table into the child, and `FD_CLOEXEC` does not take
    /// effect until the child reaches `exec` — so for the width of an unrelated spawn, a dropped guard's
    /// lock stays held and the next acquire fails with `WouldBlock`. `dup` reproduces exactly that
    /// sharing (one description, two descriptors) with no dependence on process timing.
    #[test]
    fn dropped_persistent_lock_is_free_even_while_an_inherited_descriptor_survives() {
        let dir = tempfile::tempdir().unwrap();
        let guard = acquire_persistent_lock_in(dir.path(), "run-b").unwrap();
        let inherited = unsafe { libc::dup(guard._file.as_raw_fd()) };
        assert!(inherited >= 0, "dup failed");

        // Negative half: while the guard is ALIVE the lock must still exclude. Without this, releasing
        // early (moving `LOCK_UN` out of `Drop`, or firing it sooner) would keep the positive half green.
        assert!(
            acquire_persistent_lock_in(dir.path(), "run-b").is_err(),
            "a live guard must still exclude a second acquirer"
        );
        drop(guard);

        let reacquired = acquire_persistent_lock_in(dir.path(), "run-b").map(|_| ());
        unsafe { libc::close(inherited) };
        assert!(
            reacquired.is_ok(),
            "dropping the guard must release the operation lock outright, not wait for a concurrently \
             spawned child to exec: {reacquired:?}"
        );
    }

    /// Same mechanism on the crash-detection lease. The misread needs one precondition: the probe must have
    /// OPENED the lease inode before the owner's unlink — a probe that opens afterwards just gets `None`
    /// (absent). Given that overlap, `FsLeaseProbe` holds an independent description on the same inode, and
    /// if the owner's release waits on a descriptor inherited by a concurrently spawned child, the probe
    /// reads `Some(false)` = "owner alive" for an owner that is already gone, suppressing a container reap.
    #[test]
    fn dropped_lease_probes_free_even_while_an_inherited_descriptor_survives() {
        let dir = tempfile::tempdir().unwrap();
        let guard = acquire_lease_in(dir.path(), "r1").unwrap();
        let observer = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(guard.path())
            .unwrap();
        let inherited = unsafe { libc::dup(guard._file.as_raw_fd()) };
        assert!(inherited >= 0, "dup failed");
        drop(guard);

        let free = flock_nb(&observer, true).unwrap();
        unsafe { libc::close(inherited) };
        assert!(
            free,
            "a dropped lease must read as free (owner gone) even while a concurrently spawned child \
             still holds an inherited descriptor"
        );
    }

    /// The blocking variant must WAIT for the holder rather than fail, must announce the wait exactly
    /// once before blocking, and must acquire once the holder releases. Ordering is carried by channels
    /// and by the flock itself — no sleeps: while `held` is alive the waiter provably cannot have
    /// acquired, so the `try_recv` below is an exact mutual-exclusion assertion, not a timing guess.
    #[test]
    fn blocking_persistent_lock_waits_for_the_holder_then_acquires() {
        use std::sync::mpsc;
        use std::time::Duration;

        let dir = tempfile::tempdir().unwrap();
        let held = acquire_persistent_lock_in(dir.path(), "vol-x").unwrap();

        let (contended_tx, contended_rx) = mpsc::channel();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let waiter_dir = dir.path().to_path_buf();
        let waiter = std::thread::spawn(move || {
            let guard = acquire_persistent_lock_blocking_in(&waiter_dir, "vol-x", &|| {
                contended_tx.send(()).unwrap();
            })
            .expect("blocking acquire must not fail on a merely-held lock");
            acquired_tx.send(()).unwrap();
            drop(guard);
        });

        contended_rx
            .recv_timeout(Duration::from_secs(60))
            .expect("a held lock must announce the wait before blocking");
        assert!(
            acquired_rx.try_recv().is_err(),
            "the waiter must not hold the lock while another guard is alive"
        );

        drop(held);
        acquired_rx
            .recv_timeout(Duration::from_secs(60))
            .expect("the waiter must acquire once the holder releases");
        waiter.join().unwrap();

        // The path persists (create-no-unlink) and is reacquirable after both guards are gone.
        let path = dir.path().join("vol-x.lock");
        assert!(
            path.exists(),
            "a persistent lock path must survive its guards"
        );
        drop(acquire_persistent_lock_in(dir.path(), "vol-x").unwrap());
    }

    #[test]
    fn blocking_persistent_lock_on_a_free_id_neither_waits_nor_announces() {
        let dir = tempfile::tempdir().unwrap();
        // A DIFFERENT id is held: distinct ids must not serialize against each other.
        let _other = acquire_persistent_lock_in(dir.path(), "vol-a").unwrap();
        let announced = std::sync::atomic::AtomicBool::new(false);
        let guard = acquire_persistent_lock_blocking_in(dir.path(), "vol-b", &|| {
            announced.store(true, std::sync::atomic::Ordering::SeqCst);
        })
        .unwrap();
        assert!(
            !announced.load(std::sync::atomic::Ordering::SeqCst),
            "an uncontended acquire must not announce a wait"
        );
        assert!(guard.path().ends_with("vol-b.lock"));
        drop(guard);
    }

    /// The blocking acquirer must not weaken the NON-blocking one: a lock taken by the blocking variant
    /// still excludes `acquire_persistent_lock_in`, and releases outright on drop.
    #[test]
    fn blocking_and_nonblocking_acquirers_share_one_exclusion() {
        let dir = tempfile::tempdir().unwrap();
        let guard = acquire_persistent_lock_blocking_in(dir.path(), "shared", &|| {
            panic!("a free lock must not report contention")
        })
        .unwrap();
        assert!(
            acquire_persistent_lock_in(dir.path(), "shared").is_err(),
            "a blocking-acquired lock must exclude the non-blocking acquirer"
        );
        drop(guard);
        drop(acquire_persistent_lock_in(dir.path(), "shared").unwrap());
    }

    #[test]
    fn parse_host_trims_and_falls_back() {
        assert_eq!(parse_host("  myhost \n"), "myhost");
        assert_eq!(parse_host(""), "localhost");
    }
}
