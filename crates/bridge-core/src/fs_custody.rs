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
