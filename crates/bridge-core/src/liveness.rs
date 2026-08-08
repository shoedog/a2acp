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

/// Explicit release. Load-bearing: both guards release HERE rather than at close (see
/// [`PersistentLockGuard::drop`]), so a filesystem that cannot unlock — `ENOLCK`/`EOPNOTSUPP` are reachable
/// on NFS, and `$HOME/.a2a-bridge/leases` may well be NFS — would silently resurrect the spawn-window bug
/// this release exists to prevent. Report it instead of swallowing it; releasing is still best-effort
/// because the close that follows is the only remaining recourse.
fn flock_unlock(file: &std::fs::File, path: &Path) {
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) } != 0 {
        let err = std::io::Error::last_os_error();
        tracing::error!(
            path = %path.display(),
            error = %err,
            "releasing an advisory lock failed; a concurrently spawned child may hold it until it execs"
        );
        // Fail loudly in debug builds — but never while unwinding, where a second panic aborts the process.
        debug_assert!(
            std::thread::panicking(),
            "flock(LOCK_UN) failed for {}: {err}",
            path.display()
        );
    }
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

fn lease_dir() -> PathBuf {
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
        flock_unlock(&self._file, &self.path); // see PersistentLockGuard::drop — closing alone is not a release
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
        flock_unlock(&self._file, &self.path);
    }
}

/// Create + exclusively flock `<dir>/<lock_id>.lock` without unlinking it on guard drop. This is for reusable
/// operation mutexes; crash-detecting run leases must continue to use [`acquire_lease_in`].
pub fn acquire_persistent_lock_in(
    dir: &Path,
    lock_id: &str,
) -> std::io::Result<PersistentLockGuard> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{lock_id}.lock"));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)?;
    if !flock_nb(&file, true)? {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "operation lock already held",
        ));
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
                flock_unlock(&f, Path::new(lease_path)); // acquired ⇒ free ⇒ owner dead; release so we don't claim it
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

    #[test]
    fn parse_host_trims_and_falls_back() {
        assert_eq!(parse_host("  myhost \n"), "myhost");
        assert_eq!(parse_host(""), "localhost");
    }
}
