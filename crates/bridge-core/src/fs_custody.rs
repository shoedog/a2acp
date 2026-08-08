//! Descriptor-relative filesystem custody primitives extracted for R2f1b.
//!
//! The existing binary `local_file` module remains the production caller for compatibility
//! evidence. This core module contains only generic descriptor identity, sync barriers, and
//! no-replace publication needed by later custody tests/slices.

use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::ffi::CString;
use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryIdentityV1 {
    pub canonical_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dev: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ino: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegularFileIdentityV1 {
    pub dev: Option<u64>,
    pub ino: Option<u64>,
    pub len: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum FsCustodyError {
    #[error("{0}: invalid child name")]
    InvalidChildName(String),
    #[error("{0}: unsupported filesystem custody operation")]
    Unsupported(String),
    #[error("{0}: {1}")]
    Io(String, #[source] std::io::Error),
    #[error("{0}: identity changed")]
    IdentityChanged(String),
    #[error("{0}: target already exists")]
    TargetExists(String),
    #[error("{0}: injected sync failure")]
    InjectedSync(String),
}

pub struct PinnedDirectoryV1 {
    file: File,
    canonical_path: PathBuf,
    identity: DirectoryIdentityV1,
    sync_failure_countdown: AtomicUsize,
}

pub struct RegularChildRefV1<'a> {
    pub name: &'a OsStr,
    pub file: &'a File,
}

impl<'a> RegularChildRefV1<'a> {
    #[must_use]
    pub fn new(name: &'a OsStr, file: &'a File) -> Self {
        Self { name, file }
    }
}

impl std::fmt::Debug for PinnedDirectoryV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PinnedDirectoryV1")
            .field("canonical_path", &self.canonical_path)
            .field("identity", &self.identity)
            .finish()
    }
}

impl PinnedDirectoryV1 {
    pub fn open(path: &Path, label: &str) -> Result<Self, FsCustodyError> {
        let canonical_path = path
            .canonicalize()
            .map_err(|error| FsCustodyError::Io(label.to_owned(), error))?;
        let before = directory_path_identity(&canonical_path, label)?;
        let file = open_directory_no_follow(&canonical_path, label)?;
        let identity = directory_identity(&canonical_path, &file, label)?;
        let after = directory_path_identity(&canonical_path, label)?;
        if identity != before || identity != after {
            return Err(FsCustodyError::IdentityChanged(label.to_owned()));
        }
        Ok(Self {
            file,
            canonical_path,
            identity,
            sync_failure_countdown: AtomicUsize::new(0),
        })
    }

    #[must_use]
    pub fn identity(&self) -> &DirectoryIdentityV1 {
        &self.identity
    }

    #[must_use]
    pub fn canonical_path(&self) -> &Path {
        &self.canonical_path
    }

    pub fn sync(&self, label: &str) -> Result<(), FsCustodyError> {
        let mut remaining = self.sync_failure_countdown.load(Ordering::SeqCst);
        while remaining != 0 {
            match self.sync_failure_countdown.compare_exchange(
                remaining,
                remaining - 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) if remaining == 1 => {
                    return Err(FsCustodyError::InjectedSync(label.to_owned()));
                }
                Ok(_) => break,
                Err(observed) => remaining = observed,
            }
        }
        self.file
            .sync_all()
            .map_err(|error| FsCustodyError::Io(label.to_owned(), error))
    }

    pub fn sync_journal_recovery_barrier(&self, label: &str) -> Result<(), FsCustodyError> {
        self.sync(label)
    }

    pub fn fail_sync_on_nth_call_for_test(&self, call: usize) {
        assert!(call > 0, "sync failure injection call must be positive");
        self.sync_failure_countdown.store(call, Ordering::SeqCst);
    }

    pub fn open_regular_file(&self, name: &OsStr, label: &str) -> Result<File, FsCustodyError> {
        open_regular_child(&self.file, name, label)
    }

    pub fn publish_new_regular_child(
        &self,
        source: RegularChildRefV1<'_>,
        target_name: &OsStr,
        label: &str,
    ) -> Result<(), FsCustodyError> {
        self.publish_new_regular_child_with_before_rename(source, target_name, label, || Ok(()))
    }

    pub fn publish_new_regular_child_with_before_rename<F>(
        &self,
        source: RegularChildRefV1<'_>,
        target_name: &OsStr,
        label: &str,
        before_rename: F,
    ) -> Result<(), FsCustodyError>
    where
        F: FnOnce() -> Result<(), FsCustodyError>,
    {
        publish_new_regular_child_impl(&self.file, &source, target_name, label, before_rename)?;
        self.sync(label).map_err(|error| match error {
            FsCustodyError::InjectedSync(_) => FsCustodyError::InjectedSync(format!(
                "{label}: publication renamed but parent sync is ambiguous"
            )),
            FsCustodyError::Io(_, source) => FsCustodyError::Io(
                format!("{label}: publication renamed but parent sync is ambiguous"),
                source,
            ),
            other => other,
        })?;
        let opened_target = self.open_regular_file(target_name, label)?;
        if !same_regular_file(&opened_target, source.file, label)? {
            return Err(FsCustodyError::IdentityChanged(label.to_owned()));
        }
        Ok(())
    }
}

#[cfg(unix)]
fn directory_path_identity(
    canonical_path: &Path,
    label: &str,
) -> Result<DirectoryIdentityV1, FsCustodyError> {
    use std::os::unix::fs::MetadataExt as _;
    let metadata = std::fs::symlink_metadata(canonical_path)
        .map_err(|error| FsCustodyError::Io(label.to_owned(), error))?;
    if !metadata.is_dir() {
        return Err(FsCustodyError::Unsupported(format!(
            "{label}: path is not a directory"
        )));
    }
    Ok(DirectoryIdentityV1 {
        canonical_path: canonical_path.to_string_lossy().into_owned(),
        dev: Some(metadata.dev()),
        ino: Some(metadata.ino()),
    })
}

#[cfg(not(unix))]
fn directory_path_identity(
    canonical_path: &Path,
    label: &str,
) -> Result<DirectoryIdentityV1, FsCustodyError> {
    let metadata = std::fs::symlink_metadata(canonical_path)
        .map_err(|error| FsCustodyError::Io(label.to_owned(), error))?;
    if !metadata.is_dir() {
        return Err(FsCustodyError::Unsupported(format!(
            "{label}: path is not a directory"
        )));
    }
    Ok(DirectoryIdentityV1 {
        canonical_path: canonical_path.to_string_lossy().into_owned(),
        dev: None,
        ino: None,
    })
}

#[cfg(unix)]
fn open_directory_no_follow(path: &Path, label: &str) -> Result<File, FsCustodyError> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    options
        .open(path)
        .map_err(|error| FsCustodyError::Io(label.to_owned(), error))
}

#[cfg(not(unix))]
fn open_directory_no_follow(path: &Path, label: &str) -> Result<File, FsCustodyError> {
    File::open(path).map_err(|error| FsCustodyError::Io(label.to_owned(), error))
}

#[cfg(unix)]
fn directory_identity(
    canonical_path: &Path,
    file: &File,
    label: &str,
) -> Result<DirectoryIdentityV1, FsCustodyError> {
    use std::os::unix::fs::MetadataExt as _;
    let metadata = file
        .metadata()
        .map_err(|error| FsCustodyError::Io(label.to_owned(), error))?;
    if !metadata.is_dir() {
        return Err(FsCustodyError::Unsupported(format!(
            "{label}: path is not a directory"
        )));
    }
    Ok(DirectoryIdentityV1 {
        canonical_path: canonical_path.to_string_lossy().into_owned(),
        dev: Some(metadata.dev()),
        ino: Some(metadata.ino()),
    })
}

#[cfg(not(unix))]
fn directory_identity(
    canonical_path: &Path,
    file: &File,
    label: &str,
) -> Result<DirectoryIdentityV1, FsCustodyError> {
    let metadata = file
        .metadata()
        .map_err(|error| FsCustodyError::Io(label.to_owned(), error))?;
    if !metadata.is_dir() {
        return Err(FsCustodyError::Unsupported(format!(
            "{label}: path is not a directory"
        )));
    }
    Ok(DirectoryIdentityV1 {
        canonical_path: canonical_path.to_string_lossy().into_owned(),
        dev: None,
        ino: None,
    })
}

#[cfg(unix)]
fn child_name_cstring(name: &OsStr, label: &str) -> Result<CString, FsCustodyError> {
    use std::os::unix::ffi::OsStrExt as _;
    let bytes = name.as_bytes();
    if bytes.is_empty()
        || bytes.contains(&0)
        || bytes.contains(&b'/')
        || bytes == b"."
        || bytes == b".."
    {
        return Err(FsCustodyError::InvalidChildName(label.to_owned()));
    }
    CString::new(bytes).map_err(|_| FsCustodyError::InvalidChildName(label.to_owned()))
}

#[cfg(unix)]
fn open_regular_child(parent: &File, name: &OsStr, label: &str) -> Result<File, FsCustodyError> {
    use std::os::fd::{AsRawFd as _, FromRawFd as _};
    let name = child_name_cstring(name, label)?;
    // SAFETY: parent is a live directory descriptor and name is a validated single component.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd == -1 {
        return Err(FsCustodyError::Io(
            label.to_owned(),
            std::io::Error::last_os_error(),
        ));
    }
    // SAFETY: successful openat returned an owned fd.
    let file = unsafe { File::from_raw_fd(fd) };
    let metadata = file
        .metadata()
        .map_err(|error| FsCustodyError::Io(label.to_owned(), error))?;
    if !metadata.is_file() {
        return Err(FsCustodyError::Unsupported(format!(
            "{label}: child is not a regular file"
        )));
    }
    Ok(file)
}

#[cfg(not(unix))]
fn open_regular_child(_parent: &File, _name: &OsStr, label: &str) -> Result<File, FsCustodyError> {
    Err(FsCustodyError::Unsupported(label.to_owned()))
}

#[cfg(unix)]
fn same_regular_file(left: &File, right: &File, label: &str) -> Result<bool, FsCustodyError> {
    use std::os::unix::fs::MetadataExt as _;
    let left = left
        .metadata()
        .map_err(|error| FsCustodyError::Io(label.to_owned(), error))?;
    let right = right
        .metadata()
        .map_err(|error| FsCustodyError::Io(label.to_owned(), error))?;
    Ok(left.dev() == right.dev() && left.ino() == right.ino())
}

#[cfg(not(unix))]
fn same_regular_file(_left: &File, _right: &File, _label: &str) -> Result<bool, FsCustodyError> {
    Ok(false)
}

#[cfg(unix)]
fn publish_new_regular_child_impl<F>(
    parent: &File,
    source: &RegularChildRefV1<'_>,
    target_name: &OsStr,
    label: &str,
    before_rename: F,
) -> Result<(), FsCustodyError>
where
    F: FnOnce() -> Result<(), FsCustodyError>,
{
    use std::mem::MaybeUninit;
    use std::os::fd::AsRawFd as _;
    let source_name = child_name_cstring(source.name, label)?;
    let target_name = child_name_cstring(target_name, label)?;
    if source_name.as_bytes() == target_name.as_bytes() {
        return Err(FsCustodyError::InvalidChildName(label.to_owned()));
    }
    let opened_source = open_regular_child(parent, source.name, label)?;
    if !same_regular_file(&opened_source, source.file, label)? {
        return Err(FsCustodyError::IdentityChanged(label.to_owned()));
    }

    let mut target_stat = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: retained parent descriptor and target pointer are valid; no-follow checks the entry.
    let target_result = unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            target_name.as_ptr(),
            target_stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if target_result == 0 {
        return Err(FsCustodyError::TargetExists(label.to_owned()));
    }
    let target_error = std::io::Error::last_os_error();
    if target_error.raw_os_error() != Some(libc::ENOENT) {
        return Err(FsCustodyError::Io(label.to_owned(), target_error));
    }

    before_rename()?;

    #[cfg(target_os = "macos")]
    let rename_result = unsafe {
        libc::renameatx_np(
            parent.as_raw_fd(),
            source_name.as_ptr(),
            parent.as_raw_fd(),
            target_name.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    #[cfg(target_os = "linux")]
    let rename_result = unsafe {
        libc::renameat2(
            parent.as_raw_fd(),
            source_name.as_ptr(),
            parent.as_raw_fd(),
            target_name.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let rename_result = -1;
    if rename_result == -1 {
        return Err(FsCustodyError::Io(
            label.to_owned(),
            std::io::Error::last_os_error(),
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn publish_new_regular_child_impl<F>(
    _parent: &File,
    _source: &RegularChildRefV1<'_>,
    _target_name: &OsStr,
    label: &str,
    _before_rename: F,
) -> Result<(), FsCustodyError>
where
    F: FnOnce() -> Result<(), FsCustodyError>,
{
    Err(FsCustodyError::Unsupported(label.to_owned()))
}

#[must_use]
pub fn open_options_create_new_owner_private() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true).read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    }
    options
}

/// R2f1b A1 safety net: `fs_custody.rs` had zero tests and zero production callers for its
/// operational surface at the time this module was added (only `DirectoryIdentityV1` was
/// referenced elsewhere, purely as a data field). These tests exist to lock down the intended
/// custody invariants before the upcoming `fs_custody`/`local_file` extraction (A4) starts
/// depending on this code.
///
/// Every test documents, in its own doc comment, the specific incorrect implementation it is
/// meant to catch. Where practical the discrimination was verified by temporarily inverting the
/// guarded production condition and re-running the test to confirm it goes red, then reverting —
/// see the PR report for which tests were mutation-checked this way.
///
/// HONEST LIMIT — fsync durability is unverifiable here: no test in this module can observe
/// whether `sync()`/`sync_journal_recovery_barrier()` actually issue a real `fsync(2)` that
/// flushes durably to disk, versus a no-op that simply returns `Ok(())` without touching the
/// underlying descriptor. In-process assertions can only see the typed `Result` these calls
/// return, never the storage state after a real crash. A4 must not treat this suite as proof of
/// durability — only as proof of the typed error/success plumbing and the higher-level atomicity
/// invariants (no-replace publish, before-rename ordering, post-rename identity) that hold
/// regardless of whether the underlying fsync is actually effective.
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// Guarantees a racer thread is signalled to stop and joined even if the calling test panics
    /// while the race is in flight. A bare `stop.store(...); handle.join()` placed only after
    /// the racing loop never runs on an early panic (e.g. an unexpected error variant, or a
    /// failed setup `.unwrap()`), leaking a spinning background thread for the rest of the test
    /// binary's process lifetime. Wrapping the handle in this guard makes cleanup run on every
    /// exit path, including unwinding.
    struct StopOnDrop {
        stop: Arc<AtomicBool>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl Drop for StopOnDrop {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    // ---------------------------------------------------------------------------------------
    // PinnedDirectoryV1::open
    // ---------------------------------------------------------------------------------------

    /// Discriminates a regression where `PinnedDirectoryV1::open` stores a wrong or missing
    /// `(dev, ino)` pair (e.g. stat'ing something other than the target, or leaving dev/ino as
    /// `None` on unix), which would silently defeat every later identity recheck that trusts
    /// `identity()`.
    #[cfg(unix)]
    #[test]
    fn pinned_directory_open_captures_the_real_directory_identity() {
        use std::os::unix::fs::MetadataExt as _;

        let dir = tempfile::tempdir().unwrap();
        let expected = fs::metadata(dir.path()).unwrap();

        let pinned = PinnedDirectoryV1::open(dir.path(), "happy path").unwrap();

        assert_eq!(pinned.identity().dev, Some(expected.dev()));
        assert_eq!(pinned.identity().ino, Some(expected.ino()));
        assert_eq!(
            pinned.canonical_path(),
            fs::canonicalize(dir.path()).unwrap()
        );
    }

    /// Discriminates a regression that drops the `identity != before || identity != after`
    /// recheck in `PinnedDirectoryV1::open` *entirely*: a background thread continuously
    /// replaces the target directory (via an atomic directory-to-directory rename, so the path
    /// is never momentarily absent) while the foreground repeatedly calls `open` until it
    /// observes the guard firing, bounded by a wall-clock budget.
    ///
    /// HONEST LIMIT: `FsCustodyError::IdentityChanged` carries only the caller's `label`, with no
    /// field distinguishing which side of the check tripped — the pre-open `before` stat or the
    /// post-open `after` stat. Because the swapper races continuously through the whole guarded
    /// window (often swapping more than once per `open` attempt), a *one-sided* weakening — e.g.
    /// keeping only `identity != before` and dropping `identity != after`, or vice versa — often
    /// still fires by chance on the surviving comparison across many attempts, so this test
    /// cannot reliably prove either half of the check is individually required. It reliably
    /// catches only complete removal of the guard (both comparisons dropped, as verified by
    /// mutation-testing `if false { ... }` in place of the real condition).
    #[cfg(unix)]
    #[test]
    fn pinned_directory_open_detects_replacement_racing_with_its_identity_recheck() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        fs::create_dir(&target).unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let swap_stop = Arc::clone(&stop);
        let swap_root = root.path().to_path_buf();
        let swap_target = target.clone();
        let swapper = std::thread::spawn(move || {
            let mut generation: u64 = 0;
            while !swap_stop.load(Ordering::SeqCst) {
                generation += 1;
                let replacement = swap_root.join(format!("gen-{generation}"));
                if fs::create_dir(&replacement).is_ok() {
                    let _ = fs::rename(&replacement, &swap_target);
                }
                // A small pace-setting sleep (rather than a max-speed spin) gives the scheduler
                // an explicit yield point, which empirically improves interleaving odds under
                // coverage instrumentation more than it costs in wall-clock budget.
                std::thread::sleep(Duration::from_micros(100));
            }
        });
        let _guard = StopOnDrop {
            stop: Arc::clone(&stop),
            handle: Some(swapper),
        };

        // Raised from an originally-tuned 4s: CI runs coverage-instrumented on 2-4 vCPU, which
        // both slows each attempt and reduces scheduler interleaving odds versus a local
        // uninstrumented run. The loop still exits the moment the guard fires; this only raises
        // the ceiling for a slow/loaded runner.
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut caught = false;
        while Instant::now() < deadline {
            match PinnedDirectoryV1::open(&target, "race target") {
                Ok(_pinned) => {}
                Err(FsCustodyError::IdentityChanged(label)) => {
                    assert_eq!(label, "race target");
                    caught = true;
                    break;
                }
                Err(other) => panic!("unexpected error during race: {other:?}"),
            }
        }

        assert!(
            caught,
            "expected PinnedDirectoryV1::open to detect at least one directory replacement \
             racing with its identity recheck within the time budget"
        );
    }

    /// Discriminates `PinnedDirectoryV1::open` failing to reject a non-directory path — e.g. an
    /// implementation that skips (or weakens) `directory_path_identity`'s `is_dir()` check and
    /// proceeds to treat a regular file as if it were a directory.
    #[cfg(unix)]
    #[test]
    fn pinned_directory_open_refuses_a_non_directory_path() {
        let dir = tempfile::tempdir().unwrap();
        let regular = dir.path().join("regular.txt");
        fs::write(&regular, b"not a directory").unwrap();

        let error = PinnedDirectoryV1::open(&regular, "not a directory").unwrap_err();
        assert!(matches!(error, FsCustodyError::Unsupported(_)));
    }

    /// Discriminates dropping `O_NOFOLLOW` (or `O_DIRECTORY`) from `open_directory_no_follow`,
    /// which would let a directory symlink be silently followed and opened instead of refused.
    ///
    /// This defends a window that looks dead from the *public* API: `PinnedDirectoryV1::open`
    /// calls `Path::canonicalize` first, which already resolves away any symlink in the
    /// caller-supplied path, so handing the public `open()` a symlinked path never reaches this
    /// function with a still-symlinked argument. The real target is the TOCTOU window *after*
    /// canonicalization: if the entry at the now-fixed canonical path is replaced by a symlink
    /// between `canonicalize()` returning and this call running, `O_NOFOLLOW` is what refuses to
    /// follow it. A4 should not read this test as covering dead code just because a direct
    /// public-API symlink argument never triggers it — see
    /// `pinned_directory_open_detects_replacement_racing_with_its_identity_recheck` for the
    /// race-based exercise of that exact window through the public API.
    ///
    /// The specific errno is platform-dependent and is asserted exactly per platform on purpose
    /// (not tolerated across both, so a change here is a tripwire rather than something this
    /// test silently absorbs): macOS reports `ENOTDIR` because it evaluates the `O_DIRECTORY`
    /// type mismatch on the symlink before the `O_NOFOLLOW` check; Linux reports `ELOOP` because
    /// it evaluates `O_NOFOLLOW` first.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn open_directory_no_follow_refuses_a_symlinked_directory() {
        #[cfg(target_os = "macos")]
        const EXPECTED_ERRNO: i32 = libc::ENOTDIR;
        #[cfg(target_os = "linux")]
        const EXPECTED_ERRNO: i32 = libc::ELOOP;

        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let error = open_directory_no_follow(&link, "symlinked directory").unwrap_err();
        match error {
            FsCustodyError::Io(_, io_error) => {
                assert_eq!(io_error.raw_os_error(), Some(EXPECTED_ERRNO));
            }
            other => panic!("expected an Io error, got {other:?}"),
        }
    }

    /// Discriminates dropping `O_DIRECTORY` from `open_directory_no_follow`: opening a *regular
    /// file* (not a symlink, so `O_NOFOLLOW` is not what is under test here) with `O_DIRECTORY`
    /// set must fail with `ENOTDIR`; without that flag the open would simply succeed (a regular
    /// file is a perfectly valid thing to open read-only). Unlike the symlink case above, this is
    /// fully deterministic on every unix and needs no platform split.
    ///
    /// We test against a regular file rather than a FIFO on purpose: opening a FIFO for
    /// read-only access without `O_NONBLOCK` blocks until a writer opens the other end, so if a
    /// FIFO were ever substituted for a directory, `O_DIRECTORY`'s early, non-blocking `ENOTDIR`
    /// refusal is what stops that open from hanging the whole process — not just a correctness
    /// nicety but a hang-avoidance guarantee that a test exercising it via a real FIFO would risk
    /// demonstrating the hard way.
    #[cfg(unix)]
    #[test]
    fn open_directory_no_follow_refuses_a_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let regular = dir.path().join("regular.txt");
        fs::write(&regular, b"not a directory").unwrap();

        let error = open_directory_no_follow(&regular, "regular file").unwrap_err();
        match error {
            FsCustodyError::Io(_, io_error) => {
                assert_eq!(io_error.raw_os_error(), Some(libc::ENOTDIR));
            }
            other => panic!("expected an Io/ENOTDIR error, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------------------------------
    // sync / sync_journal_recovery_barrier
    // ---------------------------------------------------------------------------------------

    /// Discriminates `sync`/`sync_journal_recovery_barrier` being wired to an always-erroring
    /// path (e.g. `sync()` unconditionally returning `Err` regardless of the injection
    /// countdown). See the module-level HONEST LIMIT note above: this does *not* prove `sync()`
    /// performs a real `fsync` — a no-op stub that simply returns `Ok(())` without touching the
    /// underlying descriptor would pass this test identically, and every other test in this
    /// module, since none of them can observe post-crash storage state.
    #[cfg(unix)]
    #[test]
    fn sync_succeeds_and_barrier_delegates_to_sync() {
        let dir = tempfile::tempdir().unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "sync happy path").unwrap();

        pinned.sync("sync happy path").unwrap();
        pinned
            .sync_journal_recovery_barrier("sync happy path")
            .unwrap();
    }

    /// Discriminates an off-by-one or non-resetting countdown in the failure-injection hook
    /// (e.g. failing every call once armed instead of exactly one, or firing on the wrong call
    /// number).
    #[cfg(unix)]
    #[test]
    fn fail_sync_on_nth_call_injects_a_typed_error_on_exactly_the_nth_call() {
        let dir = tempfile::tempdir().unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "injected sync").unwrap();

        pinned.fail_sync_on_nth_call_for_test(2);
        pinned.sync("injected sync").unwrap();
        let error = pinned.sync("injected sync").unwrap_err();
        assert!(matches!(
            error,
            FsCustodyError::InjectedSync(ref label) if label == "injected sync"
        ));
        // Countdown is single-use: the call after the injected failure must succeed normally.
        pinned.sync("injected sync").unwrap();
    }

    /// Discriminates `sync_journal_recovery_barrier` swallowing or transforming the injected
    /// failure instead of surfacing the same typed error `sync` produces.
    #[cfg(unix)]
    #[test]
    fn sync_journal_recovery_barrier_surfaces_the_injected_failure() {
        let dir = tempfile::tempdir().unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "barrier failure").unwrap();

        pinned.fail_sync_on_nth_call_for_test(1);
        let error = pinned
            .sync_journal_recovery_barrier("barrier failure")
            .unwrap_err();
        assert!(matches!(error, FsCustodyError::InjectedSync(_)));
    }

    /// Discriminates a countdown that does not disarm after firing once (a "sticky" failure), or
    /// a failed barrier leaving stray state that corrupts a later, unrelated publish. Proves one
    /// injected barrier failure produces exactly one failure and nothing else: a publish
    /// attempted afterward still completes as a single, clean, all-or-nothing operation (no
    /// partial publication left over from the earlier failure).
    #[cfg(unix)]
    #[test]
    fn publish_after_a_failed_recovery_barrier_still_completes_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "barrier then publish").unwrap();

        pinned.fail_sync_on_nth_call_for_test(1);
        let barrier_error = pinned
            .sync_journal_recovery_barrier("barrier then publish")
            .unwrap_err();
        assert!(matches!(barrier_error, FsCustodyError::InjectedSync(_)));

        let source_name = OsStr::new("source.tmp");
        let target_name = OsStr::new("target.final");
        fs::write(dir.path().join(source_name), b"payload").unwrap();
        let source_file = fs::File::open(dir.path().join(source_name)).unwrap();

        pinned
            .publish_new_regular_child(
                RegularChildRefV1::new(source_name, &source_file),
                target_name,
                "barrier then publish",
            )
            .unwrap();

        assert!(!dir.path().join(source_name).exists());
        assert_eq!(fs::read(dir.path().join(target_name)).unwrap(), b"payload");
    }

    // ---------------------------------------------------------------------------------------
    // open_regular_file
    // ---------------------------------------------------------------------------------------

    /// Discriminates `open_regular_file` failing to open an ordinary child, or opening the wrong
    /// one.
    #[cfg(unix)]
    #[test]
    fn open_regular_file_opens_an_existing_regular_child() {
        use std::io::Read as _;

        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("child.txt"), b"hello").unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "open child").unwrap();

        let mut file = pinned
            .open_regular_file(OsStr::new("child.txt"), "open child")
            .unwrap();
        let mut contents = Vec::new();
        file.read_to_end(&mut contents).unwrap();
        assert_eq!(contents, b"hello");
    }

    /// Discriminates dropping `O_NOFOLLOW` from `open_regular_child`, which would let a
    /// symlinked child be silently followed instead of refused.
    #[cfg(unix)]
    #[test]
    fn open_regular_file_refuses_a_symlinked_child() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.txt");
        fs::write(&target, b"real").unwrap();
        std::os::unix::fs::symlink(&target, dir.path().join("link.txt")).unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "symlink child").unwrap();

        let error = pinned
            .open_regular_file(OsStr::new("link.txt"), "symlink child")
            .unwrap_err();
        match error {
            FsCustodyError::Io(_, io_error) => {
                assert_eq!(io_error.raw_os_error(), Some(libc::ELOOP));
            }
            other => panic!("expected an Io/ELOOP error, got {other:?}"),
        }
    }

    /// Discriminates `open_regular_child` failing to reject a non-regular child (here a
    /// subdirectory), which would let a caller treat a directory as if it were a readable file.
    #[cfg(unix)]
    #[test]
    fn open_regular_file_refuses_a_subdirectory() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("subdir")).unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "subdirectory child").unwrap();

        let error = pinned
            .open_regular_file(OsStr::new("subdir"), "subdirectory child")
            .unwrap_err();
        assert!(matches!(error, FsCustodyError::Unsupported(_)));
    }

    /// Discriminates dropping the `same_regular_file` pre-check in
    /// `publish_new_regular_child_impl`, which would let publish silently operate on whatever is
    /// *currently* at `source_name` instead of the exact object the caller obtained a handle to
    /// — e.g. publishing swapped-in content under a trusted name. This is the identity re-check
    /// reachable from the public API without requiring a race: the caller's `source.file` handle
    /// goes stale the moment the entry at `source_name` is replaced.
    #[cfg(unix)]
    #[test]
    fn publish_refuses_when_the_callers_source_handle_is_stale() {
        use std::os::unix::fs::MetadataExt as _;

        let dir = tempfile::tempdir().unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "stale source").unwrap();
        let source_name = OsStr::new("source.tmp");
        let target_name = OsStr::new("target.final");
        fs::write(dir.path().join(source_name), b"original").unwrap();
        let stale_handle = fs::File::open(dir.path().join(source_name)).unwrap();
        let stale_ino = stale_handle.metadata().unwrap().ino();

        // Replace the entry at `source_name` with a different file (different inode) while the
        // caller's handle still points at the original, now-unlinked, inode.
        fs::remove_file(dir.path().join(source_name)).unwrap();
        fs::write(dir.path().join(source_name), b"swapped").unwrap();
        let replacement_ino = fs::metadata(dir.path().join(source_name)).unwrap().ino();
        assert_ne!(
            stale_ino, replacement_ino,
            "test setup must produce a genuinely different inode"
        );

        let error = pinned
            .publish_new_regular_child(
                RegularChildRefV1::new(source_name, &stale_handle),
                target_name,
                "stale source",
            )
            .unwrap_err();
        assert!(matches!(error, FsCustodyError::IdentityChanged(_)));
        assert!(!dir.path().join(target_name).exists());
        assert_eq!(fs::read(dir.path().join(source_name)).unwrap(), b"swapped");
    }

    // ---------------------------------------------------------------------------------------
    // publish_new_regular_child / publish_new_regular_child_with_before_rename
    // ---------------------------------------------------------------------------------------

    /// Discriminates a publish that leaves stray temporaries behind, publishes under the wrong
    /// name, or fails to move the source out of its original name.
    #[cfg(unix)]
    #[test]
    fn publish_new_regular_child_creates_exactly_the_target() {
        use std::os::unix::fs::MetadataExt as _;

        let dir = tempfile::tempdir().unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "publish happy path").unwrap();
        let source_name = OsStr::new("source.tmp");
        let target_name = OsStr::new("target.final");
        fs::write(dir.path().join(source_name), b"payload").unwrap();
        let source_file = fs::File::open(dir.path().join(source_name)).unwrap();
        let source_ino = source_file.metadata().unwrap().ino();

        pinned
            .publish_new_regular_child(
                RegularChildRefV1::new(source_name, &source_file),
                target_name,
                "publish happy path",
            )
            .unwrap();

        assert!(!dir.path().join(source_name).exists());
        let target_metadata = fs::metadata(dir.path().join(target_name)).unwrap();
        assert_eq!(target_metadata.ino(), source_ino);
        assert_eq!(fs::read(dir.path().join(target_name)).unwrap(), b"payload");
        let mut entries: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        entries.sort();
        assert_eq!(entries, vec![std::ffi::OsString::from("target.final")]);
    }

    /// Discriminates dropping (or weakening) the early `fstatat` pre-check in
    /// `publish_new_regular_child_impl` that observes an existing target before ever attempting
    /// the rename, leaving it untouched and the source unmoved.
    ///
    /// DOES NOT cover `RENAME_EXCL`/`RENAME_NOREPLACE` on the underlying rename call itself: with
    /// a target that already exists *before* `publish` is even called, this pre-check refuses
    /// and returns before the rename syscall ever runs (confirmed by mutation-testing the flag
    /// away here — this test still passes because the pre-check already caught it). The
    /// rename-call flag's own no-replace guarantee is isolated and covered exclusively by
    /// `publish_rename_itself_refuses_a_target_created_after_the_pre_check_but_before_the_rename`
    /// below — do not delete that test as "redundant" with this one; each covers a different
    /// line of defense.
    #[cfg(unix)]
    #[test]
    fn publish_refuses_to_replace_an_existing_target() {
        let dir = tempfile::tempdir().unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "publish over existing").unwrap();
        let source_name = OsStr::new("source.tmp");
        let target_name = OsStr::new("target.final");
        fs::write(dir.path().join(source_name), b"new bytes").unwrap();
        fs::write(dir.path().join(target_name), b"old bytes").unwrap();
        let source_file = fs::File::open(dir.path().join(source_name)).unwrap();

        let error = pinned
            .publish_new_regular_child(
                RegularChildRefV1::new(source_name, &source_file),
                target_name,
                "publish over existing",
            )
            .unwrap_err();

        assert!(matches!(error, FsCustodyError::TargetExists(_)));
        assert_eq!(
            fs::read(dir.path().join(target_name)).unwrap(),
            b"old bytes"
        );
        assert_eq!(
            fs::read(dir.path().join(source_name)).unwrap(),
            b"new bytes"
        );
    }

    /// Isolates the no-replace guarantee of the underlying `renameatx_np`/`renameat2` call
    /// itself, as distinct from the earlier `fstatat` pre-check that
    /// `publish_refuses_to_replace_an_existing_target` exercises: a target created *inside*
    /// `before_rename` appears strictly after the pre-check already observed an absent target
    /// but strictly before the real rename syscall runs, so only `RENAME_EXCL`/
    /// `RENAME_NOREPLACE` on the rename call itself stands between this window and a silent
    /// clobber. Discriminates dropping that flag (falling back to a plain, replacing rename).
    /// This is the *sole* coverage for the rename-call flag itself — see the note on
    /// `publish_refuses_to_replace_an_existing_target` above.
    #[cfg(unix)]
    #[test]
    fn publish_rename_itself_refuses_a_target_created_after_the_pre_check_but_before_the_rename() {
        let dir = tempfile::tempdir().unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "flag isolation").unwrap();
        let source_name = OsStr::new("source.tmp");
        let target_name = OsStr::new("target.final");
        fs::write(dir.path().join(source_name), b"trusted").unwrap();
        let source_file = fs::File::open(dir.path().join(source_name)).unwrap();
        let target_path = dir.path().join(target_name);

        let error = pinned
            .publish_new_regular_child_with_before_rename(
                RegularChildRefV1::new(source_name, &source_file),
                target_name,
                "flag isolation",
                || {
                    fs::write(&target_path, b"appeared during the window").unwrap();
                    Ok(())
                },
            )
            .unwrap_err();

        assert!(matches!(error, FsCustodyError::Io(_, _)));
        assert_eq!(
            fs::read(&target_path).unwrap(),
            b"appeared during the window"
        );
        assert_eq!(fs::read(dir.path().join(source_name)).unwrap(), b"trusted");
    }

    /// Discriminates a regression that runs `before_rename` too early (before the source
    /// identity and target-absence pre-checks have been established) or too late (after the
    /// rename already made the target visible), which would break callers relying on the hook as
    /// a last-chance barrier with full knowledge that the rename is about to happen atomically.
    #[cfg(unix)]
    #[test]
    fn publish_before_rename_hook_runs_after_pre_checks_but_before_the_visible_rename() {
        let dir = tempfile::tempdir().unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "hook order").unwrap();
        let source_name = OsStr::new("source.tmp");
        let target_name = OsStr::new("target.final");
        fs::write(dir.path().join(source_name), b"trusted").unwrap();
        let source_file = fs::File::open(dir.path().join(source_name)).unwrap();

        let target_path = dir.path().join(target_name);
        let source_path = dir.path().join(source_name);
        let hook_called = std::cell::Cell::new(false);
        pinned
            .publish_new_regular_child_with_before_rename(
                RegularChildRefV1::new(source_name, &source_file),
                target_name,
                "hook order",
                || {
                    hook_called.set(true);
                    assert!(
                        !target_path.exists(),
                        "before_rename must run before the target becomes visible"
                    );
                    assert!(
                        source_path.exists(),
                        "before_rename must run before the source is moved away"
                    );
                    Ok(())
                },
            )
            .unwrap();

        assert!(hook_called.get());
        assert!(!source_path.exists());
        assert_eq!(fs::read(&target_path).unwrap(), b"trusted");
    }

    /// Discriminates hoisting `before_rename` to the very top of
    /// `publish_new_regular_child_impl`, ahead of the source-identity and target-absence
    /// pre-checks: with an existing target, a correct implementation's `fstatat` pre-check
    /// returns `TargetExists` before ever invoking the hook, so `hook_called` must stay false.
    /// The ordering assertions inside
    /// `publish_before_rename_hook_runs_after_pre_checks_but_before_the_visible_rename` above
    /// cannot catch a hoisted-to-the-top call by themselves, because nothing on disk changes
    /// before the pre-checks run in that test's (target-absent) scenario — this test supplies
    /// the missing negative case by making a pre-check fail.
    #[cfg(unix)]
    #[test]
    fn publish_before_rename_hook_does_not_run_when_the_target_precheck_already_refuses() {
        let dir = tempfile::tempdir().unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "hook precheck order").unwrap();
        let source_name = OsStr::new("source.tmp");
        let target_name = OsStr::new("target.final");
        fs::write(dir.path().join(source_name), b"new bytes").unwrap();
        fs::write(dir.path().join(target_name), b"old bytes").unwrap();
        let source_file = fs::File::open(dir.path().join(source_name)).unwrap();

        let hook_called = std::cell::Cell::new(false);
        let error = pinned
            .publish_new_regular_child_with_before_rename(
                RegularChildRefV1::new(source_name, &source_file),
                target_name,
                "hook precheck order",
                || {
                    hook_called.set(true);
                    Ok(())
                },
            )
            .unwrap_err();

        assert!(matches!(error, FsCustodyError::TargetExists(_)));
        assert!(
            !hook_called.get(),
            "before_rename must not run once the target-absence pre-check has already refused"
        );
        assert_eq!(
            fs::read(dir.path().join(target_name)).unwrap(),
            b"old bytes"
        );
    }

    /// Discriminates a regression that performs the rename before invoking `before_rename` (or
    /// ignores the hook's error), which would leave a visible target despite the hook's abort —
    /// the "no partial publication" guarantee callers use the hook to enforce.
    #[cfg(unix)]
    #[test]
    fn publish_leaves_no_visible_target_when_before_rename_hook_aborts() {
        let dir = tempfile::tempdir().unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "hook abort").unwrap();
        let source_name = OsStr::new("source.tmp");
        let target_name = OsStr::new("target.final");
        fs::write(dir.path().join(source_name), b"trusted").unwrap();
        let source_file = fs::File::open(dir.path().join(source_name)).unwrap();

        let error = pinned
            .publish_new_regular_child_with_before_rename(
                RegularChildRefV1::new(source_name, &source_file),
                target_name,
                "hook abort",
                || Err(FsCustodyError::Unsupported("aborted for test".to_owned())),
            )
            .unwrap_err();

        assert!(matches!(error, FsCustodyError::Unsupported(_)));
        assert!(!dir.path().join(target_name).exists());
        assert_eq!(fs::read(dir.path().join(source_name)).unwrap(), b"trusted");
    }

    /// Discriminates dropping the same-name guard in `publish_new_regular_child_impl`: renaming
    /// a name onto itself is not a meaningful publish, and the underlying
    /// `RENAME_EXCL`/`RENAME_NOREPLACE` semantics for a self-rename are not something this
    /// custody layer wants to depend on.
    #[cfg(unix)]
    #[test]
    fn publish_refuses_when_source_and_target_names_are_identical() {
        let dir = tempfile::tempdir().unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "same name").unwrap();
        let name = OsStr::new("same.tmp");
        fs::write(dir.path().join(name), b"payload").unwrap();
        let source_file = fs::File::open(dir.path().join(name)).unwrap();

        let error = pinned
            .publish_new_regular_child(
                RegularChildRefV1::new(name, &source_file),
                name,
                "same name",
            )
            .unwrap_err();
        assert!(matches!(error, FsCustodyError::InvalidChildName(_)));
        assert_eq!(fs::read(dir.path().join(name)).unwrap(), b"payload");
    }

    /// Discriminates a regression that drops or weakens the post-rename `same_regular_file`
    /// recheck in `publish_new_regular_child_with_before_rename` (comparing `opened_target`
    /// against `source.file` after the atomic rename), which would let publish report success
    /// while an impostor swapped into `target_name` immediately after the rename stays silently
    /// visible under the trusted name. Best-effort race: `before_rename` is used only to start
    /// the racer at the last possible moment (right before the real rename syscall), which is a
    /// legitimate use of the hook's documented seam — it is not a stand-in for a missing one.
    #[cfg(unix)]
    #[test]
    fn publish_detects_target_swapped_between_the_atomic_rename_and_the_post_rename_recheck() {
        // Raised from an originally-tuned 6s: CI runs coverage-instrumented on 2-4 vCPU.
        let deadline = Instant::now() + Duration::from_secs(25);
        let mut caught = false;
        while Instant::now() < deadline && !caught {
            let dir = tempfile::tempdir().unwrap();
            let pinned = PinnedDirectoryV1::open(dir.path(), "race publish").unwrap();
            let source_name = OsStr::new("source.tmp");
            let target_name = OsStr::new("target.final");
            fs::write(dir.path().join(source_name), b"trusted").unwrap();
            let source_file = fs::File::open(dir.path().join(source_name)).unwrap();

            let target_path = dir.path().join(target_name);
            let go = Arc::new(AtomicBool::new(false));
            let stop = Arc::new(AtomicBool::new(false));
            let go_reader = Arc::clone(&go);
            let stop_reader = Arc::clone(&stop);
            let swap_target = target_path.clone();
            let swap_dir = dir.path().to_path_buf();
            let swapper = std::thread::spawn(move || {
                // S1 fix: this initial wait must itself be bounded by `stop` *and* a hard
                // deadline. If the real rename never runs (e.g. `before_rename` is never reached
                // because an earlier pre-check failed under load — EMFILE/ENOMEM), `go` is never
                // set; without this bound the thread would spin here forever, and the outer
                // `join()` (or the `StopOnDrop` guard's join) would then hang indefinitely — CI
                // has no timeout-minutes configured, so this was a multi-hour-hang risk.
                let wait_deadline = Instant::now() + Duration::from_secs(2);
                while !go_reader.load(Ordering::SeqCst) {
                    if stop_reader.load(Ordering::SeqCst) || Instant::now() >= wait_deadline {
                        return;
                    }
                    std::thread::yield_now();
                }
                let mut generation: u64 = 0;
                while !stop_reader.load(Ordering::SeqCst) {
                    generation += 1;
                    let decoy = swap_dir.join(format!("impostor-{generation}"));
                    if fs::write(&decoy, b"impostor").is_ok() {
                        let _ = fs::rename(&decoy, &swap_target);
                    }
                    // See the pacing note in the sibling `open()` race test above.
                    std::thread::sleep(Duration::from_micros(100));
                }
            });
            let _guard = StopOnDrop {
                stop: Arc::clone(&stop),
                handle: Some(swapper),
            };

            let go_writer = Arc::clone(&go);
            let result = pinned.publish_new_regular_child_with_before_rename(
                RegularChildRefV1::new(source_name, &source_file),
                target_name,
                "race publish",
                move || {
                    go_writer.store(true, Ordering::SeqCst);
                    Ok(())
                },
            );

            if matches!(result, Err(FsCustodyError::IdentityChanged(_))) {
                caught = true;
            }
            // `_guard` drops here at the end of each iteration (and on any panic unwinding
            // through this scope), stopping and joining the swapper before the next
            // tempdir/thread pair is created.
        }

        assert!(
            caught,
            "expected publish_new_regular_child_with_before_rename to detect at least one \
             post-rename target swap within the time budget"
        );
    }

    // ---------------------------------------------------------------------------------------
    // child_name_cstring
    // ---------------------------------------------------------------------------------------

    /// Discriminates a weakened or missing child-name validator in `child_name_cstring` (zero
    /// coverage before this test): every one of these names must be refused rather than silently
    /// accepted and handed to a raw `openat`/`renameat*` call, where any of them could escape the
    /// intended single-path-component-child contract — `.`/`..` walk the namespace instead of
    /// naming a child, `/` names something outside the immediate directory, an embedded NUL is
    /// meaningless (and dangerous) to the underlying C string API, and empty is not a name at
    /// all.
    #[cfg(unix)]
    #[test]
    fn child_name_cstring_rejects_every_unsafe_name() {
        use std::os::unix::ffi::OsStrExt as _;

        let empty = OsStr::new("");
        let dot = OsStr::new(".");
        let dotdot = OsStr::new("..");
        let nested = OsStr::new("a/b");
        let embedded_nul = OsStr::from_bytes(b"a\0b");

        for name in [empty, dot, dotdot, nested, embedded_nul] {
            let error = child_name_cstring(name, "unsafe name").unwrap_err();
            assert!(
                matches!(error, FsCustodyError::InvalidChildName(_)),
                "expected InvalidChildName for {name:?}, got {error:?}"
            );
        }
    }

    // ---------------------------------------------------------------------------------------
    // open_options_create_new_owner_private
    // ---------------------------------------------------------------------------------------

    /// Discriminates dropping `.create_new(true)` (would silently overwrite an existing file
    /// instead of failing) or granting any group/world permission bit.
    ///
    /// HONEST LIMIT: the requested mode is exactly `0600`, but this test asserts only
    /// `mode & 0o077 == 0` (no group/world bits), not `mode == 0o600` exactly. The kernel applies
    /// `requested_mode & !umask` when creating a file, so under a sufficiently restrictive
    /// process umask (e.g. `0777`) the resulting mode could legitimately be more restrictive than
    /// `0600` (even `0000`) with no bug in this function. The security property this test
    /// actually needs to hold — no group/world access — survives any umask, since a umask can
    /// only ever remove permission bits, never add them. We deliberately do not mutate the
    /// process umask to force an exact `0600` observation here: `umask` is global process state,
    /// and mutating it would race every other test running concurrently in this binary.
    #[cfg(unix)]
    #[test]
    fn create_new_owner_private_enforces_o_excl_and_no_group_or_world_access() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owner-private.tmp");

        let file = open_options_create_new_owner_private().open(&path).unwrap();
        let mode = file.metadata().unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode & 0o077,
            0,
            "created file must not grant any group/world permission bit, got mode {mode:04o}"
        );
        drop(file);

        let error = open_options_create_new_owner_private()
            .open(&path)
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    }
}
