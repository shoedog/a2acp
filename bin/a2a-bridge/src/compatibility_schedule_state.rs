//! Owner-private local state and lock capabilities for R3d2 admission.
//!
//! The default-off boundary may only probe the fixed production root read-only. Tests inject an
//! existing temporary root; R3d5 is the sole owner of production initialization and activation on
//! the operator account's local APFS volume.

#![cfg_attr(not(test), allow(dead_code))]

use std::ffi::OsStr;
use std::fs::File;
use std::io::{Seek as _, Write as _};
use std::os::fd::AsRawFd as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::local_file::{self, PinnedDirectory};

const STATE_DIRECTORY_MODE: u32 = 0o700;
const STATE_FILE_MODE: u32 = 0o600;
const MAX_LOCK_HOLDER_BYTES: usize = 512;
const MAX_PASSWD_BUFFER_BYTES: usize = 1024 * 1024;
const STATE_SUBDIRECTORIES: [&str; 5] = ["authority", "admission", "ledger", "supervisor", "locks"];

#[derive(Debug)]
pub(super) enum SchedulerStateError {
    Invalid(String),
    LockBusy(&'static str),
    LockOrder,
    Io {
        context: &'static str,
        source: std::io::Error,
    },
}

impl std::fmt::Display for SchedulerStateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => formatter.write_str(message),
            Self::LockBusy(label) => write!(formatter, "{label} is busy"),
            Self::LockOrder => formatter.write_str(
                "scheduler state lock order violation: owner-wide must precede authority-state",
            ),
            Self::Io { context, source } => write!(formatter, "{context}: {source}"),
        }
    }
}

impl std::error::Error for SchedulerStateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

struct StateRootInner {
    root: PinnedDirectory,
    authority: PinnedDirectory,
    admission: PinnedDirectory,
    ledger: PinnedDirectory,
    supervisor: PinnedDirectory,
    locks: PinnedDirectory,
    admission_holders: AtomicUsize,
    authority_only_holders: AtomicUsize,
}

#[derive(Clone)]
pub(super) struct SchedulerStateRoot {
    inner: Arc<StateRootInner>,
}

pub(super) struct OwnerAdmissionLock {
    inner: Arc<StateRootInner>,
    file: File,
}

pub(super) struct AdmissionAuthorityLocks {
    authority_file: File,
    _owner: OwnerAdmissionLock,
}

pub(super) struct AuthorityMutationLock {
    inner: Arc<StateRootInner>,
    file: File,
}

pub(super) trait AuthorityStateCapability {
    fn authority_directory(&self) -> &PinnedDirectory;
}

/// Capability for state that participates in the single owner-wide admission transaction.
/// It is deliberately implemented only by guards that retain the owner admission lock.
#[allow(dead_code)] // Admission/supervisor journals are wired together in R3d2e.
pub(super) trait AdmissionStateCapability {
    fn admission_directory(&self) -> &PinnedDirectory;
    fn ledger_directory(&self) -> &PinnedDirectory;
    fn supervisor_directory(&self) -> &PinnedDirectory;
}

fn invalid(error: impl std::fmt::Display) -> SchedulerStateError {
    SchedulerStateError::Invalid(error.to_string())
}

fn validate_holder(holder: &str) -> Result<(), SchedulerStateError> {
    if holder.is_empty()
        || holder.len() > MAX_LOCK_HOLDER_BYTES
        || !holder.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'-' | b'_' | b':' | b'/')
        })
    {
        return Err(SchedulerStateError::Invalid(
            "scheduler lock holder is not a bounded stable identity".into(),
        ));
    }
    Ok(())
}

fn verify_private_directory(
    directory: &PinnedDirectory,
    label: &str,
) -> Result<(), SchedulerStateError> {
    use std::os::unix::fs::MetadataExt as _;

    let metadata =
        directory
            .file_handle()
            .metadata()
            .map_err(|source| SchedulerStateError::Io {
                context: "cannot inspect scheduler state directory",
                source,
            })?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o777 != STATE_DIRECTORY_MODE
    {
        return Err(SchedulerStateError::Invalid(format!(
            "{label} must be an owner-owned mode-0700 directory"
        )));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn verify_local_apfs(directory: &PinnedDirectory) -> Result<(), SchedulerStateError> {
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::zeroed();
    // SAFETY: stat is correctly sized writable storage and the retained directory fd is live.
    if unsafe { libc::fstatfs(directory.file_handle().as_raw_fd(), stat.as_mut_ptr()) } == -1 {
        return Err(SchedulerStateError::Io {
            context: "cannot inspect scheduler state filesystem",
            source: std::io::Error::last_os_error(),
        });
    }
    // SAFETY: fstatfs initialized the complete structure.
    let stat = unsafe { stat.assume_init() };
    // SAFETY: f_fstypename is a fixed NUL-terminated C buffer returned by the kernel.
    let filesystem =
        unsafe { std::ffi::CStr::from_ptr(stat.f_fstypename.as_ptr()) }.to_string_lossy();
    if stat.f_flags & libc::MNT_LOCAL as u32 == 0 || filesystem.as_ref() != "apfs" {
        return Err(SchedulerStateError::Invalid(
            "scheduler state must reside on the local APFS filesystem".into(),
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn verify_local_apfs(_directory: &PinnedDirectory) -> Result<(), SchedulerStateError> {
    Err(SchedulerStateError::Invalid(
        "production scheduler state is supported only on macOS APFS".into(),
    ))
}

fn open_existing_root(path: &Path) -> Result<PinnedDirectory, SchedulerStateError> {
    use std::os::unix::fs::MetadataExt as _;

    let lexical_metadata =
        std::fs::symlink_metadata(path).map_err(|source| SchedulerStateError::Io {
            context: "cannot inspect scheduler state root",
            source,
        })?;
    if !lexical_metadata.is_dir() || lexical_metadata.file_type().is_symlink() {
        return Err(SchedulerStateError::Invalid(
            "scheduler state root must be a non-symlink directory".into(),
        ));
    }
    let snapshot = local_file::snapshot_directory(path, "scheduler state root").map_err(invalid)?;
    let root = PinnedDirectory::open(
        path,
        &snapshot.canonical_cwd,
        &snapshot.identity,
        "scheduler state root",
    )
    .map_err(invalid)?;
    let opened_metadata =
        root.file_handle()
            .metadata()
            .map_err(|source| SchedulerStateError::Io {
                context: "cannot inspect opened scheduler state root",
                source,
            })?;
    if lexical_metadata.dev() != opened_metadata.dev()
        || lexical_metadata.ino() != opened_metadata.ino()
    {
        return Err(SchedulerStateError::Invalid(
            "scheduler state root changed while it was being opened".into(),
        ));
    }
    verify_private_directory(&root, "scheduler state root")?;
    Ok(root)
}

fn current_operator_home() -> Result<PathBuf, SchedulerStateError> {
    use std::os::unix::ffi::OsStringExt as _;

    // SAFETY: sysconf reads one process-wide configuration value and has no pointer preconditions.
    let configured = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    let mut capacity = if configured > 0 {
        usize::try_from(configured).unwrap_or(16 * 1024)
    } else {
        16 * 1024
    }
    .min(MAX_PASSWD_BUFFER_BYTES);
    loop {
        let mut passwd = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        let mut buffer = vec![0_u8; capacity];
        // SAFETY: passwd and result are writable outputs, buffer is live for the call, and geteuid
        // has no preconditions. getpwuid_r returns only pointers into passwd/buffer before either
        // allocation is dropped.
        let status = unsafe {
            libc::getpwuid_r(
                libc::geteuid(),
                passwd.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if status == libc::ERANGE && capacity < MAX_PASSWD_BUFFER_BYTES {
            capacity = capacity.saturating_mul(2).min(MAX_PASSWD_BUFFER_BYTES);
            continue;
        }
        if status != 0 {
            return Err(SchedulerStateError::Io {
                context: "cannot resolve effective operator account",
                source: std::io::Error::from_raw_os_error(status),
            });
        }
        if result.is_null() {
            return Err(SchedulerStateError::Invalid(
                "effective operator account has no passwd entry".into(),
            ));
        }
        // SAFETY: a non-null getpwuid_r result initialized passwd and pw_dir points into the still-
        // live buffer for this iteration.
        let passwd = unsafe { passwd.assume_init() };
        if passwd.pw_dir.is_null() {
            return Err(SchedulerStateError::Invalid(
                "effective operator account has no home directory".into(),
            ));
        }
        let bytes = unsafe { std::ffi::CStr::from_ptr(passwd.pw_dir) }.to_bytes();
        if bytes.is_empty() {
            return Err(SchedulerStateError::Invalid(
                "effective operator account has an empty home directory".into(),
            ));
        }
        let home = PathBuf::from(std::ffi::OsString::from_vec(bytes.to_vec()));
        if !home.is_absolute() {
            return Err(SchedulerStateError::Invalid(
                "effective operator home directory is not absolute".into(),
            ));
        }
        return Ok(home);
    }
}

fn production_state_path(operator_home: &Path) -> PathBuf {
    operator_home
        .join("Library")
        .join("Application Support")
        .join("a2a-bridge")
        .join("operator")
        .join("compatibility-scheduler")
}

fn production_scheduler_state_present_at(
    operator_home: &Path,
    require_local_apfs: bool,
) -> Result<bool, SchedulerStateError> {
    let path = production_state_path(operator_home);
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(SchedulerStateError::Io {
                context: "cannot inspect fixed production scheduler state",
                source,
            })
        }
        Ok(_) => {}
    }
    let root = open_existing_root(&path)?;
    if require_local_apfs {
        verify_local_apfs(&root)?;
    }
    Ok(true)
}

/// Read-only default-off guard for the legacy manual compatibility path. The effective account's
/// passwd home is authoritative; caller-controlled environment and CLI paths cannot redirect it.
pub(super) fn production_scheduler_state_present() -> Result<bool, SchedulerStateError> {
    production_scheduler_state_present_at(&current_operator_home()?, true)
}

fn open_or_create_private_child(
    parent: &PinnedDirectory,
    name: &str,
) -> Result<PinnedDirectory, SchedulerStateError> {
    let child = parent
        .open_or_create_child_directory(
            OsStr::new(name),
            STATE_DIRECTORY_MODE,
            "scheduler state directory",
        )
        .map_err(invalid)?;
    verify_private_directory(&child, &format!("scheduler state {name}"))?;
    Ok(child)
}

fn open_lock_file(directory: &PinnedDirectory, name: &str) -> Result<File, SchedulerStateError> {
    use std::os::fd::FromRawFd as _;
    use std::os::unix::fs::MetadataExt as _;

    let c_name = std::ffi::CString::new(name).expect("fixed lock file names contain no NUL");
    let parent = directory.file_handle();
    // SAFETY: the retained parent and fixed single-component name are live. First attempt create-new
    // so only this creator may establish owner/mode; an existing object is reopened without repair.
    let mut fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            c_name.as_ptr(),
            libc::O_RDWR
                | libc::O_CREAT
                | libc::O_EXCL
                | libc::O_CLOEXEC
                | libc::O_NOFOLLOW
                | libc::O_NONBLOCK,
            STATE_FILE_MODE as libc::c_uint,
        )
    };
    let created = fd != -1;
    if fd == -1 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EEXIST) {
            return Err(SchedulerStateError::Io {
                context: "cannot create scheduler lock file",
                source: error,
            });
        }
        // SAFETY: the same retained parent/name pair is live and O_NOFOLLOW rejects a link.
        fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                c_name.as_ptr(),
                libc::O_RDWR | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            )
        };
        if fd == -1 {
            return Err(SchedulerStateError::Io {
                context: "cannot open scheduler lock file",
                source: std::io::Error::last_os_error(),
            });
        }
    }
    // SAFETY: fd was returned uniquely by openat.
    let file = unsafe { File::from_raw_fd(fd) };
    if created {
        // SAFETY: the create-new file descriptor is exclusively owned and live.
        if unsafe { libc::fchown(file.as_raw_fd(), libc::geteuid(), libc::getegid()) } == -1
            || unsafe { libc::fchmod(file.as_raw_fd(), STATE_FILE_MODE as libc::mode_t) } == -1
        {
            return Err(SchedulerStateError::Io {
                context: "cannot bind scheduler lock file owner/mode",
                source: std::io::Error::last_os_error(),
            });
        }
        directory.sync().map_err(invalid)?;
    }
    let metadata = file.metadata().map_err(|source| SchedulerStateError::Io {
        context: "cannot inspect scheduler lock file",
        source,
    })?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o777 != STATE_FILE_MODE
    {
        return Err(SchedulerStateError::Invalid(
            "scheduler lock must be an owner-owned single-link mode-0600 regular file".into(),
        ));
    }
    Ok(file)
}

fn try_lock(
    directory: &PinnedDirectory,
    name: &str,
    label: &'static str,
    holder: &str,
) -> Result<File, SchedulerStateError> {
    validate_holder(holder)?;
    let mut file = open_lock_file(directory, name)?;
    // SAFETY: the verified regular-file descriptor is live. LOCK_NB guarantees refusal, not queueing.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == -1 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
            return Err(SchedulerStateError::LockBusy(label));
        }
        return Err(SchedulerStateError::Io {
            context: "cannot acquire scheduler lock",
            source: error,
        });
    }
    file.set_len(0).map_err(|source| SchedulerStateError::Io {
        context: "cannot clear scheduler lock holder",
        source,
    })?;
    file.seek(std::io::SeekFrom::Start(0))
        .map_err(|source| SchedulerStateError::Io {
            context: "cannot seek scheduler lock holder",
            source,
        })?;
    file.write_all(holder.as_bytes())
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .map_err(|source| SchedulerStateError::Io {
            context: "cannot persist scheduler lock holder",
            source,
        })?;
    Ok(file)
}

impl SchedulerStateRoot {
    fn initialize(path: &Path, require_local_apfs: bool) -> Result<Self, SchedulerStateError> {
        let root = open_existing_root(path)?;
        if require_local_apfs {
            verify_local_apfs(&root)?;
        }
        let mut children = Vec::with_capacity(STATE_SUBDIRECTORIES.len());
        for name in STATE_SUBDIRECTORIES {
            children.push(open_or_create_private_child(&root, name)?);
        }
        let [authority, admission, ledger, supervisor, locks]: [PinnedDirectory; 5] =
            children.try_into().map_err(|_| {
                SchedulerStateError::Invalid("scheduler state layout is incomplete".into())
            })?;
        Ok(Self {
            inner: Arc::new(StateRootInner {
                root,
                authority,
                admission,
                ledger,
                supervisor,
                locks,
                admission_holders: AtomicUsize::new(0),
                authority_only_holders: AtomicUsize::new(0),
            }),
        })
    }

    #[allow(dead_code)] // R3d5 is the sole production initialization/activation owner.
    pub(super) fn initialize_production() -> Result<Self, SchedulerStateError> {
        let path = production_state_path(&current_operator_home()?);
        Self::initialize(&path, true)
    }

    #[cfg(test)]
    pub(super) fn initialize_for_test(path: &Path) -> Result<Self, SchedulerStateError> {
        Self::initialize(path, false)
    }

    pub(super) fn try_owner_admission(
        &self,
        holder: &str,
    ) -> Result<OwnerAdmissionLock, SchedulerStateError> {
        if self.inner.authority_only_holders.load(Ordering::SeqCst) != 0 {
            return Err(SchedulerStateError::LockOrder);
        }
        let file = try_lock(
            &self.inner.locks,
            "owner-admission.lock",
            "owner-wide compatibility admission lock",
            holder,
        )?;
        if self.inner.authority_only_holders.load(Ordering::SeqCst) != 0 {
            drop(file);
            return Err(SchedulerStateError::LockOrder);
        }
        self.inner.admission_holders.fetch_add(1, Ordering::SeqCst);
        Ok(OwnerAdmissionLock {
            inner: Arc::clone(&self.inner),
            file,
        })
    }

    pub(super) fn try_authority_mutation(
        &self,
        holder: &str,
    ) -> Result<AuthorityMutationLock, SchedulerStateError> {
        if self.inner.admission_holders.load(Ordering::SeqCst) != 0 {
            return Err(SchedulerStateError::LockOrder);
        }
        let file = try_lock(
            &self.inner.locks,
            "authority-state.lock",
            "authority-state lock",
            holder,
        )?;
        if self.inner.admission_holders.load(Ordering::SeqCst) != 0 {
            drop(file);
            return Err(SchedulerStateError::LockOrder);
        }
        self.inner
            .authority_only_holders
            .fetch_add(1, Ordering::SeqCst);
        Ok(AuthorityMutationLock {
            inner: Arc::clone(&self.inner),
            file,
        })
    }

    #[cfg(test)]
    fn directory_paths(&self) -> [std::path::PathBuf; 6] {
        [
            self.inner.root.canonical_path(),
            self.inner.authority.canonical_path(),
            self.inner.admission.canonical_path(),
            self.inner.ledger.canonical_path(),
            self.inner.supervisor.canonical_path(),
            self.inner.locks.canonical_path(),
        ]
    }
}

impl OwnerAdmissionLock {
    pub(super) fn try_authority_state(
        self,
        holder: &str,
    ) -> Result<AdmissionAuthorityLocks, SchedulerStateError> {
        let authority_file = try_lock(
            &self.inner.locks,
            "authority-state.lock",
            "authority-state lock",
            holder,
        )?;
        Ok(AdmissionAuthorityLocks {
            authority_file,
            _owner: self,
        })
    }
}

impl AdmissionStateCapability for OwnerAdmissionLock {
    fn admission_directory(&self) -> &PinnedDirectory {
        &self.inner.admission
    }

    fn ledger_directory(&self) -> &PinnedDirectory {
        &self.inner.ledger
    }

    fn supervisor_directory(&self) -> &PinnedDirectory {
        &self.inner.supervisor
    }
}

impl Drop for OwnerAdmissionLock {
    fn drop(&mut self) {
        // SAFETY: this guard uniquely owns the locked descriptor.
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
        self.inner.admission_holders.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Drop for AdmissionAuthorityLocks {
    fn drop(&mut self) {
        // SAFETY: the combined guard uniquely owns the nested authority descriptor. The owner
        // guard remains a field until this drop completes, enforcing authority-before-owner release.
        unsafe {
            libc::flock(self.authority_file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

impl AuthorityStateCapability for AdmissionAuthorityLocks {
    fn authority_directory(&self) -> &PinnedDirectory {
        &self._owner.inner.authority
    }
}

impl AdmissionStateCapability for AdmissionAuthorityLocks {
    fn admission_directory(&self) -> &PinnedDirectory {
        &self._owner.inner.admission
    }

    fn ledger_directory(&self) -> &PinnedDirectory {
        &self._owner.inner.ledger
    }

    fn supervisor_directory(&self) -> &PinnedDirectory {
        &self._owner.inner.supervisor
    }
}

impl Drop for AuthorityMutationLock {
    fn drop(&mut self) {
        // SAFETY: this guard uniquely owns the locked descriptor.
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
        self.inner
            .authority_only_holders
            .fetch_sub(1, Ordering::SeqCst);
    }
}

impl AuthorityStateCapability for AuthorityMutationLock {
    fn authority_directory(&self) -> &PinnedDirectory {
        &self.inner.authority
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    fn root() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        root
    }

    fn create_production_root(operator_home: &Path) -> PathBuf {
        let path = production_state_path(operator_home);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[test]
    fn fixed_production_presence_is_read_only_and_fail_closed() {
        let operator_home = tempfile::tempdir().unwrap();
        assert!(!production_scheduler_state_present_at(operator_home.path(), false).unwrap());

        let root = create_production_root(operator_home.path());
        assert!(production_scheduler_state_present_at(operator_home.path(), false).unwrap());
        assert!(std::fs::read_dir(&root).unwrap().next().is_none());

        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(production_scheduler_state_present_at(operator_home.path(), false).is_err());
        assert_eq!(
            std::fs::metadata(root).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn fixed_production_presence_rejects_a_symlink_root() {
        let operator_home = tempfile::tempdir().unwrap();
        let path = production_state_path(operator_home.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let target = root();
        std::os::unix::fs::symlink(target.path(), &path).unwrap();

        assert!(production_scheduler_state_present_at(operator_home.path(), false).is_err());
    }

    #[test]
    fn effective_operator_home_is_absolute() {
        let passwd_home = current_operator_home().unwrap();
        assert!(passwd_home.is_absolute());
    }

    #[test]
    fn state_layout_and_locks_are_owner_private_and_nonblocking() {
        let root = root();
        let state = SchedulerStateRoot::initialize_for_test(root.path()).unwrap();
        assert!(matches!(
            state.try_owner_admission("not a stable holder"),
            Err(SchedulerStateError::Invalid(_))
        ));
        for path in state.directory_paths() {
            let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
        }

        let owner = state.try_owner_admission("run-1:daily").unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("locks/owner-admission.lock")).unwrap(),
            "run-1:daily\n"
        );
        assert!(matches!(
            state.try_owner_admission("run-2:manual"),
            Err(SchedulerStateError::LockBusy(_))
        ));
        let admission = owner.try_authority_state("run-1:authority").unwrap();
        assert!(matches!(
            state.try_authority_mutation("operator:revoke"),
            Err(SchedulerStateError::LockOrder)
        ));
        assert!(matches!(
            state.try_owner_admission("run-2:manual"),
            Err(SchedulerStateError::LockBusy(_))
        ));
        drop(admission);
        state.try_owner_admission("run-2:manual").unwrap();

        for name in ["owner-admission.lock", "authority-state.lock"] {
            let path = root.path().join("locks").join(name);
            let metadata = std::fs::metadata(path).unwrap();
            assert!(metadata.is_file());
            assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn owner_lock_contender_process_helper() {
        let Some(root) = std::env::var_os("A2A_R3D2_LOCK_HELPER_ROOT") else {
            return;
        };
        let state = SchedulerStateRoot::initialize_for_test(Path::new(&root)).unwrap();
        let result = state.try_owner_admission("child:daily");
        match std::env::var("A2A_R3D2_LOCK_HELPER_EXPECT").as_deref() {
            Ok("busy") => assert!(matches!(result, Err(SchedulerStateError::LockBusy(_)))),
            Ok("acquired") => assert!(result.is_ok()),
            Ok("exit_while_held") => {
                let _guard = result.unwrap();
                // SAFETY: this helper is a disposable child process. _exit intentionally skips
                // Rust drops to prove the kernel releases flock on abrupt process termination.
                unsafe { libc::_exit(0) }
            }
            other => panic!("unexpected helper expectation {other:?}"),
        }
    }

    #[test]
    fn owner_lock_is_exclusive_across_processes_and_released_on_drop() {
        let root = root();
        let state = SchedulerStateRoot::initialize_for_test(root.path()).unwrap();
        let owner = state.try_owner_admission("parent:daily").unwrap();
        let helper = |expect: &str| {
            std::process::Command::new(std::env::current_exe().unwrap())
                .arg("--exact")
                .arg("compatibility_schedule_state::tests::owner_lock_contender_process_helper")
                .env("A2A_R3D2_LOCK_HELPER_ROOT", root.path())
                .env("A2A_R3D2_LOCK_HELPER_EXPECT", expect)
                .output()
                .unwrap()
        };
        let busy = helper("busy");
        assert!(
            busy.status.success(),
            "child busy probe failed: {}",
            String::from_utf8_lossy(&busy.stderr)
        );
        drop(owner);
        let acquired = helper("acquired");
        assert!(
            acquired.status.success(),
            "child acquisition probe failed: {}",
            String::from_utf8_lossy(&acquired.stderr)
        );

        let abrupt = helper("exit_while_held");
        assert!(abrupt.status.success());
        state.try_owner_admission("parent:after-crash").unwrap();
    }

    #[test]
    fn authority_lock_contender_process_helper() {
        let Some(root) = std::env::var_os("A2A_R3D2_AUTHORITY_HELPER_ROOT") else {
            return;
        };
        let state = SchedulerStateRoot::initialize_for_test(Path::new(&root)).unwrap();
        let result = state.try_authority_mutation("child:issue");
        match std::env::var("A2A_R3D2_AUTHORITY_HELPER_EXPECT").as_deref() {
            Ok("busy") => assert!(matches!(result, Err(SchedulerStateError::LockBusy(_)))),
            Ok("acquired") => assert!(result.is_ok()),
            other => panic!("unexpected authority helper expectation {other:?}"),
        }
    }

    #[test]
    fn authority_issuance_is_exclusive_across_processes_without_queueing() {
        let root = root();
        let state = SchedulerStateRoot::initialize_for_test(root.path()).unwrap();
        let authority = state.try_authority_mutation("parent:issue").unwrap();
        let helper = |expect: &str| {
            std::process::Command::new(std::env::current_exe().unwrap())
                .arg("--exact")
                .arg("compatibility_schedule_state::tests::authority_lock_contender_process_helper")
                .env("A2A_R3D2_AUTHORITY_HELPER_ROOT", root.path())
                .env("A2A_R3D2_AUTHORITY_HELPER_EXPECT", expect)
                .output()
                .unwrap()
        };
        let busy = helper("busy");
        assert!(
            busy.status.success(),
            "child authority busy probe failed: {}",
            String::from_utf8_lossy(&busy.stderr)
        );
        drop(authority);
        let acquired = helper("acquired");
        assert!(
            acquired.status.success(),
            "child authority acquisition probe failed: {}",
            String::from_utf8_lossy(&acquired.stderr)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_state_root_is_on_local_apfs() {
        let root = root();
        let state = SchedulerStateRoot::initialize_for_test(root.path()).unwrap();
        verify_local_apfs(&state.inner.root).unwrap();
    }

    #[test]
    fn authority_only_then_owner_wide_is_rejected_as_reversed_order() {
        let root = root();
        let state = SchedulerStateRoot::initialize_for_test(root.path()).unwrap();
        let _authority = state.try_authority_mutation("operator:issue").unwrap();
        assert!(matches!(
            state.try_owner_admission("run-1:daily"),
            Err(SchedulerStateError::LockOrder)
        ));
    }

    #[test]
    fn nested_authority_capability_keeps_the_owner_wide_lock_live() {
        let root = root();
        let state = SchedulerStateRoot::initialize_for_test(root.path()).unwrap();
        let owner = state.try_owner_admission("run-1:daily").unwrap();
        let admission = owner.try_authority_state("run-1:authority").unwrap();

        assert!(matches!(
            state.try_owner_admission("run-2:daily"),
            Err(SchedulerStateError::LockBusy(_))
        ));

        drop(admission);
        state.try_owner_admission("run-2:daily").unwrap();
    }

    #[test]
    fn broadened_or_nonregular_state_objects_fail_closed() {
        let broad = tempfile::tempdir().unwrap();
        std::fs::set_permissions(broad.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(SchedulerStateRoot::initialize_for_test(broad.path()).is_err());

        let child_symlink_root = root();
        let child_target = root();
        std::os::unix::fs::symlink(
            child_target.path(),
            child_symlink_root.path().join("authority"),
        )
        .unwrap();
        assert!(SchedulerStateRoot::initialize_for_test(child_symlink_root.path()).is_err());

        let symlink_root = root();
        let state = SchedulerStateRoot::initialize_for_test(symlink_root.path()).unwrap();
        drop(state);
        std::os::unix::fs::symlink(
            "/dev/null",
            symlink_root
                .path()
                .join("locks")
                .join("owner-admission.lock"),
        )
        .unwrap();
        let state = SchedulerStateRoot::initialize_for_test(symlink_root.path()).unwrap();
        assert!(state.try_owner_admission("run-1:daily").is_err());

        let fifo_root = root();
        let state = SchedulerStateRoot::initialize_for_test(fifo_root.path()).unwrap();
        let fifo = fifo_root.path().join("locks").join("owner-admission.lock");
        let fifo_c = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).unwrap();
        // SAFETY: the temporary path is NUL-terminated and owned by this test.
        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
        assert!(state.try_owner_admission("run-1:daily").is_err());
    }

    #[test]
    fn existing_broadened_child_directory_is_not_repaired() {
        let root = root();
        let authority = root.path().join("authority");
        std::fs::create_dir(&authority).unwrap();
        std::fs::set_permissions(&authority, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(SchedulerStateRoot::initialize_for_test(root.path()).is_err());
        assert_eq!(
            std::fs::metadata(authority).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }
}
