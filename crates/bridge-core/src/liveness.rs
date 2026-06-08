//! flock-based liveness for managed containers (Increment A). A run holds an exclusive `flock` on a per-run
//! lease file for its whole life; the OS releases it when the process dies (clean OR crash). A sweeper that
//! can ACQUIRE the lock ⇒ the owner is gone. This is PID-reuse-, clock-drift-, and reboot-safe (unlike
//! probing PID start-times) and needs no new deps — `libc::flock`.

use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

/// Non-blocking flock. `Ok(true)` = acquired, `Ok(false)` = held by another open file description,
/// `Err` = a real error.
fn flock_nb(file: &std::fs::File, exclusive: bool) -> std::io::Result<bool> {
    let op = (if exclusive { libc::LOCK_EX } else { libc::LOCK_SH }) | libc::LOCK_NB;
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

fn flock_unlock(file: &std::fs::File) {
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
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
        let _ = std::fs::remove_file(&self.path); // best-effort; the OS already freed the lock
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
                flock_unlock(&f); // acquired ⇒ free ⇒ owner dead; release so we don't claim it
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
        assert_eq!(probe.try_state(&path), None, "removed on clean drop ⇒ absent");
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
    fn parse_host_trims_and_falls_back() {
        assert_eq!(parse_host("  myhost \n"), "myhost");
        assert_eq!(parse_host(""), "localhost");
    }
}
