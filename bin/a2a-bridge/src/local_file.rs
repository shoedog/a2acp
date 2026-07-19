//! Pinned, bounded local-file snapshots for security-sensitive CLI evidence.
//!
//! The descriptor is opened before type/size inspection, so a FIFO cannot park the process before the
//! regular-file gate. On Unix, `O_NOFOLLOW` rejects a final symlink and `O_NONBLOCK` makes every special
//! file return promptly; descriptor/path identity is then compared before the canonical path is trusted.

use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bridge_core::session_cwd::SessionCwd;

use crate::BoxError;

#[derive(Debug)]
pub(crate) struct LocalFileSnapshot {
    pub(crate) canonical_path: PathBuf,
    pub(crate) bytes: Vec<u8>,
    pub(crate) sha256: String,
    pub(crate) identity: RegularFileIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RegularFileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    length: u64,
    #[cfg(unix)]
    modified_seconds: i64,
    #[cfg(unix)]
    modified_nanoseconds: i64,
    #[cfg(unix)]
    changed_seconds: i64,
    #[cfg(unix)]
    changed_nanoseconds: i64,
    #[cfg(not(unix))]
    modified: Option<std::time::SystemTime>,
    #[cfg(not(unix))]
    created: Option<std::time::SystemTime>,
}

impl RegularFileIdentity {
    fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            Self {
                device: metadata.dev(),
                inode: metadata.ino(),
                length: metadata.len(),
                modified_seconds: metadata.mtime(),
                modified_nanoseconds: metadata.mtime_nsec(),
                changed_seconds: metadata.ctime(),
                changed_nanoseconds: metadata.ctime_nsec(),
            }
        }
        #[cfg(not(unix))]
        {
            Self {
                length: metadata.len(),
                modified: metadata.modified().ok(),
                created: metadata.created().ok(),
            }
        }
    }

    pub(crate) fn matches_metadata(&self, metadata: &std::fs::Metadata) -> bool {
        self == &Self::from_metadata(metadata)
    }
}

#[derive(Debug)]
pub(crate) struct OpenFileSnapshot {
    pub(crate) bytes: Vec<u8>,
    pub(crate) sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DirectoryIdentity {
    pub(crate) device: u64,
    pub(crate) inode: u64,
    pub(crate) object_sha256: String,
}

impl DirectoryIdentity {
    #[cfg(unix)]
    fn from_open_directory(
        file: &File,
        metadata: &std::fs::Metadata,
        label: &str,
    ) -> Result<Self, BoxError> {
        use std::os::unix::fs::MetadataExt as _;
        let mut material = durable_directory_object_material(file, label)?;
        material.extend_from_slice(&metadata.dev().to_be_bytes());
        material.extend_from_slice(&metadata.ino().to_be_bytes());
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            object_sha256: sha256_hex(&material),
        })
    }

    #[cfg(unix)]
    fn matches_metadata(&self, metadata: &std::fs::Metadata) -> bool {
        use std::os::unix::fs::MetadataExt as _;
        self.device == metadata.dev() && self.inode == metadata.ino()
    }
}

#[cfg(target_os = "macos")]
fn macos_attrlist(commonattr: libc::attrgroup_t, volattr: libc::attrgroup_t) -> libc::attrlist {
    libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr,
        volattr,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    }
}

#[cfg(target_os = "macos")]
fn macos_descriptor_attribute(
    file: &File,
    list: &mut libc::attrlist,
    buffer: &mut [u8],
    label: &str,
) -> Result<(), BoxError> {
    use std::os::fd::AsRawFd as _;

    // SAFETY: `list` and `buffer` are live writable objects for the duration of the call, the
    // descriptor is owned by `file`, and the buffer size exactly describes `buffer`.
    let result = unsafe {
        libc::fgetattrlist(
            file.as_raw_fd(),
            (list as *mut libc::attrlist).cast(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            0,
        )
    };
    if result != 0 {
        return Err(format!(
            "{label}: durable directory identity is unavailable: {}",
            std::io::Error::last_os_error()
        )
        .into());
    }
    let returned = u32::from_ne_bytes(
        buffer[0..4]
            .try_into()
            .expect("attribute buffer always has a length field"),
    ) as usize;
    if returned != buffer.len() {
        return Err(format!(
            "{label}: durable directory identity returned an unexpected attribute size"
        )
        .into());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn durable_directory_object_material(file: &File, label: &str) -> Result<Vec<u8>, BoxError> {
    // APFS and other modern Darwin filesystems use 64-bit file IDs, so the legacy stat/fsobj_id
    // generation field may be zero. Only accept a descriptor's file ID when the volume explicitly
    // advertises both persistent and 64-bit object IDs; otherwise the plan/action gap cannot safely
    // distinguish object replacement and must fail closed.
    let mut capabilities = [0_u8; 36];
    let mut capability_list = macos_attrlist(0, libc::ATTR_VOL_CAPABILITIES);
    macos_descriptor_attribute(file, &mut capability_list, &mut capabilities, label)?;
    let format_capabilities = u32::from_ne_bytes(
        capabilities[4..8]
            .try_into()
            .expect("capability field has a fixed width"),
    );
    let format_valid = u32::from_ne_bytes(
        capabilities[20..24]
            .try_into()
            .expect("capability-valid field has a fixed width"),
    );
    if !macos_has_persistent_64_bit_file_ids(format_capabilities, format_valid) {
        return Err(format!(
            "{label}: filesystem does not provide persistent 64-bit directory object IDs"
        )
        .into());
    }

    let mut volume_uuid = [0_u8; 20];
    let mut volume_uuid_list = macos_attrlist(0, libc::ATTR_VOL_UUID);
    macos_descriptor_attribute(file, &mut volume_uuid_list, &mut volume_uuid, label)?;
    let volume_uuid: [u8; 16] = volume_uuid[4..20]
        .try_into()
        .expect("volume UUID field has a fixed width");
    if !macos_valid_volume_uuid(&volume_uuid) {
        return Err(format!("{label}: filesystem returned an invalid volume UUID").into());
    }

    let mut file_id = [0_u8; 12];
    let mut file_id_list = macos_attrlist(libc::ATTR_CMN_FILEID, 0);
    macos_descriptor_attribute(file, &mut file_id_list, &mut file_id, label)?;
    let file_id = u64::from_ne_bytes(
        file_id[4..12]
            .try_into()
            .expect("file-id field has a fixed width"),
    );
    if file_id == 0 {
        return Err(format!("{label}: filesystem returned an invalid persistent object ID").into());
    }
    let mut material = b"a2a-bridge:darwin-volume-file-id:v2\0".to_vec();
    material.extend_from_slice(&volume_uuid);
    material.extend_from_slice(&file_id.to_be_bytes());
    Ok(material)
}

#[cfg(target_os = "macos")]
fn macos_has_persistent_64_bit_file_ids(format_capabilities: u32, format_valid: u32) -> bool {
    let required = libc::VOL_CAP_FMT_PERSISTENTOBJECTIDS | libc::VOL_CAP_FMT_64BIT_OBJECT_IDS;
    format_valid & required == required && format_capabilities & required == required
}

#[cfg(target_os = "macos")]
fn macos_valid_volume_uuid(volume_uuid: &[u8; 16]) -> bool {
    volume_uuid.iter().any(|byte| *byte != 0)
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
#[repr(C)]
struct LinuxFileHandleBuffer {
    handle_bytes: libc::c_uint,
    handle_type: libc::c_int,
    bytes: [libc::c_uchar; libc::MAX_HANDLE_SZ as usize],
}

#[cfg(target_os = "linux")]
const LINUX_AT_HANDLE_MNT_ID_UNIQUE: libc::c_int = 0x001;

#[cfg(target_os = "linux")]
fn acquire_linux_file_handle<F>(
    mut acquire: F,
    label: &str,
) -> Result<(LinuxFileHandleBuffer, u64, libc::c_int), BoxError>
where
    F: FnMut(libc::c_int) -> std::io::Result<(LinuxFileHandleBuffer, u64)>,
{
    // Linux 6.5's identity-only FID mode works on filesystems such as overlayfs that intentionally do
    // not provide an openable handle. Linux 6.12's unique mount ID is non-reused during the current
    // boot. Both modes require it: an older kernel may reject the flag, but falling back to a reusable
    // 32-bit mount ID would let a later remount collide with a previously authorized plan.
    let base_flags = libc::AT_EMPTY_PATH | LINUX_AT_HANDLE_MNT_ID_UNIQUE;
    let identity_flags = base_flags | libc::AT_HANDLE_FID;
    match acquire(identity_flags) {
        Ok((handle, mount_id)) => Ok((handle, mount_id, identity_flags)),
        Err(identity_error) => match acquire(base_flags) {
            Ok((handle, mount_id)) => Ok((handle, mount_id, base_flags)),
            Err(openable_error) => Err(format!(
                "{label}: filesystem cannot provide a durable directory object handle with a unique mount identity (identity-only: {identity_error}; compatible: {openable_error})"
            )
            .into()),
        },
    }
}

#[cfg(target_os = "linux")]
fn parse_linux_boot_id(raw: &[u8]) -> Option<[u8; 36]> {
    let value = raw.strip_suffix(b"\n").unwrap_or(raw);
    let value: [u8; 36] = value.try_into().ok()?;
    let mut has_nonzero_hex_digit = false;
    for (index, byte) in value.iter().enumerate() {
        if matches!(index, 8 | 13 | 18 | 23) {
            if *byte != b'-' {
                return None;
            }
        } else if !byte.is_ascii_digit() && !(b'a'..=b'f').contains(byte) {
            return None;
        } else if *byte != b'0' {
            has_nonzero_hex_digit = true;
        }
    }
    has_nonzero_hex_digit.then_some(value)
}

#[cfg(target_os = "linux")]
fn linux_boot_id(label: &str) -> Result<[u8; 36], BoxError> {
    let path = "/proc/sys/kernel/random/boot_id";
    let file = File::open(path)
        .map_err(|error| format!("{label}: cannot read Linux boot identity: {error}"))?;
    let mut bytes = Vec::with_capacity(37);
    file.take(38)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{label}: cannot read Linux boot identity: {error}"))?;
    parse_linux_boot_id(&bytes)
        .ok_or_else(|| format!("{label}: Linux boot identity is malformed").into())
}

#[cfg(target_os = "linux")]
fn durable_directory_object_material(file: &File, label: &str) -> Result<Vec<u8>, BoxError> {
    use std::os::fd::AsRawFd as _;

    let empty_path = b"\0";
    let acquire = |flags: libc::c_int| {
        let mut handle = LinuxFileHandleBuffer {
            handle_bytes: libc::MAX_HANDLE_SZ as libc::c_uint,
            handle_type: 0,
            bytes: [0; libc::MAX_HANDLE_SZ as usize],
        };
        let mut mount_id = 0_u64;
        // SAFETY: `file` owns a live descriptor; `empty_path` is a NUL-terminated empty C string;
        // `LinuxFileHandleBuffer` has the C `file_handle` prefix followed immediately by MAX_HANDLE_SZ
        // bytes; and `mount_id` is a live 64-bit output required by AT_HANDLE_MNT_ID_UNIQUE. The
        // kernel is bounded by `handle_bytes`, and libc supplies the architecture-specific syscall ID.
        let result = unsafe {
            libc::syscall(
                libc::SYS_name_to_handle_at,
                file.as_raw_fd(),
                empty_path.as_ptr().cast::<libc::c_char>(),
                (&mut handle as *mut LinuxFileHandleBuffer).cast::<libc::file_handle>(),
                &mut mount_id as *mut u64,
                flags,
            )
        };
        if result == 0 {
            Ok((handle, mount_id))
        } else {
            Err(std::io::Error::last_os_error())
        }
    };
    let (handle, mount_id, used_flags) = acquire_linux_file_handle(acquire, label)?;
    let handle_bytes = handle.handle_bytes as usize;
    if handle_bytes == 0 || handle_bytes > handle.bytes.len() {
        return Err(
            format!("{label}: filesystem returned an invalid directory object handle").into(),
        );
    }
    let boot_id = linux_boot_id(label)?;
    let mut material = b"a2a-bridge:linux-boot-mount-file-handle:v2\0".to_vec();
    material.extend_from_slice(&boot_id);
    material.extend_from_slice(&used_flags.to_be_bytes());
    material.extend_from_slice(&mount_id.to_be_bytes());
    material.extend_from_slice(&handle.handle_type.to_be_bytes());
    material.extend_from_slice(&(handle_bytes as u32).to_be_bytes());
    material.extend_from_slice(&handle.bytes[..handle_bytes]);
    Ok(material)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn durable_directory_object_material(_file: &File, label: &str) -> Result<Vec<u8>, BoxError> {
    Err(format!(
        "{label}: durable host-fallback directory identity is unsupported on this operating system"
    )
    .into())
}

#[derive(Debug)]
pub(crate) struct DirectorySnapshot {
    pub(crate) canonical_cwd: SessionCwd,
    pub(crate) identity: DirectoryIdentity,
}

/// An open directory object whose descriptor is retained through the host ACP
/// process lifetime. The parent descriptor remains close-on-exec; the forked
/// child binds its cwd with `fchdir` and may retain only its copy when the OS
/// stable absolute path is descriptor-backed.
#[derive(Clone, Debug)]
pub(crate) struct PinnedDirectory {
    file: Arc<File>,
    canonical_cwd: SessionCwd,
    identity: DirectoryIdentity,
    acp_session_cwd: PathBuf,
    retain_descriptor_after_exec: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct RegularChildRef<'a> {
    name: &'a OsStr,
    file: &'a File,
}

impl<'a> RegularChildRef<'a> {
    pub(crate) fn new(name: &'a OsStr, file: &'a File) -> Self {
        Self { name, file }
    }
}

fn open_read_only_nonblocking(path: &Path) -> Result<File, std::io::Error> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    options.open(path)
}

fn open_directory(path: &Path) -> Result<File, std::io::Error> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW);
    }
    options.open(path)
}

#[cfg(unix)]
fn same_file(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    left.dev() == right.dev() && left.ino() == right.ino()
}

fn open_directory_snapshot(
    path: &Path,
    label: &str,
) -> Result<(File, DirectorySnapshot), BoxError> {
    if path.as_os_str().is_empty() {
        return Err(format!("{label}: path must be non-empty").into());
    }
    let canonical_path = std::fs::canonicalize(path)
        .map_err(|error| format!("{label}: cannot resolve {}: {error}", path.display()))?;
    let file = open_directory(&canonical_path).map_err(|error| {
        format!(
            "{label}: cannot open resolved directory {}: {error}",
            canonical_path.display()
        )
    })?;
    let descriptor_metadata = file.metadata().map_err(|error| {
        format!(
            "{label}: cannot inspect resolved directory {}: {error}",
            canonical_path.display()
        )
    })?;
    let path_metadata = std::fs::metadata(&canonical_path).map_err(|error| {
        format!(
            "{label}: cannot re-inspect resolved directory {}: {error}",
            canonical_path.display()
        )
    })?;
    if !descriptor_metadata.is_dir()
        || !path_metadata.is_dir()
        || !same_file(&descriptor_metadata, &path_metadata)
    {
        return Err(format!("{label}: directory path changed while it was being opened").into());
    }
    let canonical_cwd = SessionCwd::parse(&canonical_path.to_string_lossy())
        .map_err(|_| format!("{label}: resolved directory is not a valid session cwd"))?;
    let identity = DirectoryIdentity::from_open_directory(&file, &descriptor_metadata, label)?;
    Ok((
        file,
        DirectorySnapshot {
            canonical_cwd,
            identity,
        },
    ))
}

pub(crate) fn snapshot_directory(path: &Path, label: &str) -> Result<DirectorySnapshot, BoxError> {
    let (_file, snapshot) = open_directory_snapshot(path, label)?;
    Ok(snapshot)
}

fn stable_directory_path(
    _file: &File,
    identity: &DirectoryIdentity,
    label: &str,
) -> Result<(PathBuf, bool), BoxError> {
    #[cfg(target_os = "macos")]
    let (path, retain_descriptor_after_exec) = (
        PathBuf::from(format!("/.vol/{}/{}", identity.device, identity.inode)),
        false,
    );
    #[cfg(target_os = "linux")]
    let (path, retain_descriptor_after_exec) = {
        use std::os::fd::AsRawFd as _;
        (
            PathBuf::from(format!("/proc/self/fd/{}", _file.as_raw_fd())),
            true,
        )
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    return Err(format!(
        "{label}: descriptor-pinned host fallback is unsupported on this operating system"
    )
    .into());

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let metadata = std::fs::metadata(&path).map_err(|error| {
            format!(
                "{label}: stable directory handle {} is unavailable: {error}",
                path.display()
            )
        })?;
        if !metadata.is_dir() || !identity.matches_metadata(&metadata) {
            return Err(format!(
                "{label}: stable directory handle {} does not identify the planned directory",
                path.display()
            )
            .into());
        }
        Ok((path, retain_descriptor_after_exec))
    }
}

impl PinnedDirectory {
    pub(crate) fn open(
        path: &Path,
        expected_cwd: &SessionCwd,
        expected_identity: &DirectoryIdentity,
        label: &str,
    ) -> Result<Self, BoxError> {
        let (file, snapshot) = open_directory_snapshot(path, label)?;
        if &snapshot.canonical_cwd != expected_cwd || &snapshot.identity != expected_identity {
            return Err(format!("{label}: directory identity changed after planning").into());
        }
        let (acp_session_cwd, retain_descriptor_after_exec) =
            stable_directory_path(&file, &snapshot.identity, label)?;
        Ok(Self {
            file: Arc::new(file),
            canonical_cwd: snapshot.canonical_cwd,
            identity: snapshot.identity,
            acp_session_cwd,
            retain_descriptor_after_exec,
        })
    }

    pub(crate) fn file_handle(&self) -> Arc<File> {
        Arc::clone(&self.file)
    }

    pub(crate) fn acp_session_cwd(&self) -> PathBuf {
        self.acp_session_cwd.clone()
    }

    pub(crate) fn canonical_path(&self) -> PathBuf {
        PathBuf::from(self.canonical_cwd.as_str())
    }

    pub(crate) fn sync(&self) -> Result<(), BoxError> {
        self.file
            .sync_all()
            .map_err(|error| format!("pinned directory: cannot sync: {error}").into())
    }

    pub(crate) fn retain_descriptor_after_exec(&self) -> bool {
        self.retain_descriptor_after_exec
    }

    pub(crate) fn current_path_matches(&self) -> bool {
        open_directory_snapshot(
            Path::new(self.canonical_cwd.as_str()),
            "fallback smoke cwd recheck",
        )
        .ok()
        .is_some_and(|(_file, snapshot)| snapshot.identity == self.identity)
    }

    /// Create one owner-private regular file relative to the retained directory object. The effect
    /// never re-resolves the directory's pathname, so a same-name replacement cannot redirect it.
    pub(crate) fn create_new_file(
        &self,
        name: &OsStr,
        mode: u32,
        label: &str,
    ) -> Result<File, BoxError> {
        #[cfg(unix)]
        {
            use std::os::fd::{AsRawFd as _, FromRawFd as _};

            let name = child_name_cstring(name, label)?;
            // SAFETY: the parent descriptor and NUL-terminated single-component name are live for
            // this call. O_EXCL prevents replacement, O_NOFOLLOW rejects a final symlink, and the
            // returned descriptor is uniquely adopted by File.
            let fd = unsafe {
                libc::openat(
                    self.file.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_CLOEXEC
                        | libc::O_NOFOLLOW,
                    mode as libc::c_uint,
                )
            };
            if fd == -1 {
                return Err(format!(
                    "{label}: cannot create descriptor-relative file: {}",
                    std::io::Error::last_os_error()
                )
                .into());
            }
            // SAFETY: `fd` was returned uniquely by openat above.
            let file = unsafe { File::from_raw_fd(fd) };
            if let Err(error) = set_effective_owner(&file, label) {
                drop(file);
                // SAFETY: the same retained parent/name pair identifies the just-created entry.
                unsafe {
                    libc::unlinkat(self.file.as_raw_fd(), name.as_ptr(), 0);
                }
                return Err(error);
            }
            // SAFETY: `file` owns a live descriptor and `mode` is a normal permission mask.
            if unsafe { libc::fchmod(file.as_raw_fd(), mode as libc::mode_t) } == -1 {
                let error = std::io::Error::last_os_error();
                drop(file);
                // SAFETY: the same live parent/name pair identifies the just-created entry. Cleanup
                // is best-effort because the original permission failure remains authoritative.
                unsafe {
                    libc::unlinkat(self.file.as_raw_fd(), name.as_ptr(), 0);
                }
                return Err(
                    format!("{label}: cannot set owner-only file permissions: {error}").into(),
                );
            }
            let metadata = file
                .metadata()
                .map_err(|error| format!("{label}: cannot inspect created file: {error}"))?;
            use std::os::unix::fs::MetadataExt as _;
            if !metadata.is_file() || metadata.nlink() != 1 {
                return Err(
                    format!("{label}: created entry is not a single-link regular file").into(),
                );
            }
            Ok(file)
        }
        #[cfg(not(unix))]
        {
            let _ = (name, mode);
            Err(format!("{label}: descriptor-relative file creation is unsupported").into())
        }
    }

    /// Create one symbolic link relative to the retained directory object. Callers remain
    /// responsible for proving that the target resolves within their owned capability.
    pub(crate) fn create_symlink(
        &self,
        name: &OsStr,
        target: &OsStr,
        label: &str,
    ) -> Result<(), BoxError> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd as _;
            use std::os::unix::ffi::OsStrExt as _;

            let name = child_name_cstring(name, label)?;
            let target = std::ffi::CString::new(target.as_bytes())
                .map_err(|_| format!("{label}: symlink target contains NUL"))?;
            if target.as_bytes().is_empty() {
                return Err(format!("{label}: symlink target must not be empty").into());
            }
            // SAFETY: the retained parent descriptor and both NUL-terminated strings are live for
            // this call. The caller has already validated the target lexically within its root.
            if unsafe { libc::symlinkat(target.as_ptr(), self.file.as_raw_fd(), name.as_ptr()) }
                == -1
            {
                return Err(format!(
                    "{label}: cannot create descriptor-relative symlink: {}",
                    std::io::Error::last_os_error()
                )
                .into());
            }
            // SAFETY: the retained parent/name pair identifies the new link, and
            // AT_SYMLINK_NOFOLLOW applies ownership to the link rather than its target.
            if unsafe {
                libc::fchownat(
                    self.file.as_raw_fd(),
                    name.as_ptr(),
                    libc::geteuid(),
                    libc::getegid(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            } == -1
            {
                let error = std::io::Error::last_os_error();
                // SAFETY: best-effort cleanup uses the same retained parent/name pair.
                unsafe {
                    libc::unlinkat(self.file.as_raw_fd(), name.as_ptr(), 0);
                }
                return Err(format!("{label}: cannot bind symlink ownership: {error}").into());
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = (name, target);
            Err(format!("{label}: descriptor-relative symlink creation is unsupported").into())
        }
    }

    /// Open one existing regular child relative to the retained directory object.
    pub(crate) fn open_regular_file(&self, name: &OsStr, label: &str) -> Result<File, BoxError> {
        #[cfg(unix)]
        {
            use std::os::fd::{AsRawFd as _, FromRawFd as _};

            let name = child_name_cstring(name, label)?;
            // SAFETY: the parent descriptor and single-component name are live. The returned
            // descriptor is uniquely adopted by File.
            let fd = unsafe {
                libc::openat(
                    self.file.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
                )
            };
            if fd == -1 {
                return Err(format!(
                    "{label}: cannot open descriptor-relative file: {}",
                    std::io::Error::last_os_error()
                )
                .into());
            }
            // SAFETY: `fd` was returned uniquely by openat above.
            let file = unsafe { File::from_raw_fd(fd) };
            let metadata = file
                .metadata()
                .map_err(|error| format!("{label}: cannot inspect opened file: {error}"))?;
            use std::os::unix::fs::MetadataExt as _;
            if !metadata.is_file() || metadata.nlink() != 1 {
                return Err(format!("{label}: child must be a single-link regular file").into());
            }
            Ok(file)
        }
        #[cfg(not(unix))]
        {
            let _ = name;
            Err(format!("{label}: descriptor-relative file opening is unsupported").into())
        }
    }

    /// Reopen one existing child directory relative to the retained parent object. Each path
    /// component is therefore traversed without following a replacement symlink.
    pub(crate) fn open_child_directory(&self, name: &OsStr, label: &str) -> Result<Self, BoxError> {
        #[cfg(unix)]
        {
            use std::os::fd::{AsRawFd as _, FromRawFd as _};

            let c_name = child_name_cstring(name, label)?;
            // SAFETY: the retained parent descriptor and single-component name are live; the
            // returned descriptor is uniquely adopted by File and O_NOFOLLOW rejects a link.
            let fd = unsafe {
                libc::openat(
                    self.file.as_raw_fd(),
                    c_name.as_ptr(),
                    libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
                )
            };
            if fd == -1 {
                return Err(format!(
                    "{label}: cannot open descriptor-relative directory: {}",
                    std::io::Error::last_os_error()
                )
                .into());
            }
            // SAFETY: `fd` was returned uniquely by openat above.
            let file = unsafe { File::from_raw_fd(fd) };
            let metadata = file
                .metadata()
                .map_err(|error| format!("{label}: cannot inspect child directory: {error}"))?;
            if !metadata.is_dir() {
                return Err(format!("{label}: child entry is not a directory").into());
            }
            let identity = DirectoryIdentity::from_open_directory(&file, &metadata, label)?;
            let (acp_session_cwd, retain_descriptor_after_exec) =
                stable_directory_path(&file, &identity, label)?;
            let canonical_child = Path::new(self.canonical_cwd.as_str()).join(name);
            let canonical_cwd = SessionCwd::parse(&canonical_child.to_string_lossy())
                .map_err(|_| format!("{label}: child directory is not a valid session cwd"))?;
            Ok(Self {
                file: Arc::new(file),
                canonical_cwd,
                identity,
                acp_session_cwd,
                retain_descriptor_after_exec,
            })
        }
        #[cfg(not(unix))]
        {
            let _ = name;
            Err(format!("{label}: descriptor-relative directory opening is unsupported").into())
        }
    }

    /// Create one child directory, then retain the opened object beneath this directory descriptor.
    pub(crate) fn create_child_directory(
        &self,
        name: &OsStr,
        mode: u32,
        label: &str,
    ) -> Result<Self, BoxError> {
        #[cfg(unix)]
        {
            use std::os::fd::{AsRawFd as _, FromRawFd as _};

            let c_name = child_name_cstring(name, label)?;
            // SAFETY: the parent descriptor and single-component name are live for mkdirat.
            if unsafe {
                libc::mkdirat(self.file.as_raw_fd(), c_name.as_ptr(), mode as libc::mode_t)
            } == -1
            {
                return Err(format!(
                    "{label}: cannot create descriptor-relative directory: {}",
                    std::io::Error::last_os_error()
                )
                .into());
            }
            // SAFETY: the parent descriptor and name remain live; the returned descriptor is
            // uniquely adopted by File.
            let fd = unsafe {
                libc::openat(
                    self.file.as_raw_fd(),
                    c_name.as_ptr(),
                    libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
                )
            };
            if fd == -1 {
                let error = std::io::Error::last_os_error();
                // SAFETY: best-effort removal of the directory just created beneath the same parent.
                unsafe {
                    libc::unlinkat(self.file.as_raw_fd(), c_name.as_ptr(), libc::AT_REMOVEDIR);
                }
                return Err(format!("{label}: cannot retain created directory: {error}").into());
            }
            // SAFETY: `fd` was returned uniquely by openat above.
            let file = unsafe { File::from_raw_fd(fd) };
            if let Err(error) = set_effective_owner(&file, label) {
                drop(file);
                // SAFETY: best-effort cleanup uses the same retained parent/name pair.
                unsafe {
                    libc::unlinkat(self.file.as_raw_fd(), c_name.as_ptr(), libc::AT_REMOVEDIR);
                }
                return Err(error);
            }
            // SAFETY: `file` owns a live descriptor and `mode` is a normal permission mask.
            if unsafe { libc::fchmod(file.as_raw_fd(), mode as libc::mode_t) } == -1 {
                return Err(format!(
                    "{label}: cannot set owner-only directory permissions: {}",
                    std::io::Error::last_os_error()
                )
                .into());
            }
            let metadata = file
                .metadata()
                .map_err(|error| format!("{label}: cannot inspect created directory: {error}"))?;
            if !metadata.is_dir() {
                return Err(format!("{label}: created entry is not a directory").into());
            }
            let identity = DirectoryIdentity::from_open_directory(&file, &metadata, label)?;
            let (acp_session_cwd, retain_descriptor_after_exec) =
                stable_directory_path(&file, &identity, label)?;
            let canonical_child = Path::new(self.canonical_cwd.as_str()).join(name);
            let canonical_cwd = SessionCwd::parse(&canonical_child.to_string_lossy())
                .map_err(|_| format!("{label}: created directory is not a valid session cwd"))?;
            Ok(Self {
                file: Arc::new(file),
                canonical_cwd,
                identity,
                acp_session_cwd,
                retain_descriptor_after_exec,
            })
        }
        #[cfg(not(unix))]
        {
            let _ = (name, mode);
            Err(format!("{label}: descriptor-relative directory creation is unsupported").into())
        }
    }

    /// Open an existing child directory or create it owner-private beneath this retained parent.
    /// Existing entries are never chmod-repaired: callers can inspect and reject a broadened mode.
    pub(crate) fn open_or_create_child_directory(
        &self,
        name: &OsStr,
        mode: u32,
        label: &str,
    ) -> Result<Self, BoxError> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd as _;

            let c_name = child_name_cstring(name, label)?;
            // SAFETY: the retained parent descriptor and single-component name are live.
            let created = if unsafe {
                libc::mkdirat(self.file.as_raw_fd(), c_name.as_ptr(), mode as libc::mode_t)
            } == 0
            {
                true
            } else {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::EEXIST) {
                    return Err(format!(
                        "{label}: cannot create descriptor-relative directory: {error}"
                    )
                    .into());
                }
                false
            };

            let child = match self.open_child_directory(name, label) {
                Ok(child) => child,
                Err(error) => {
                    if created {
                        // SAFETY: best-effort cleanup uses the same retained parent/name pair.
                        unsafe {
                            libc::unlinkat(
                                self.file.as_raw_fd(),
                                c_name.as_ptr(),
                                libc::AT_REMOVEDIR,
                            );
                        }
                    }
                    return Err(error);
                }
            };
            if created {
                let handle = child.file_handle();
                set_effective_owner(&handle, label)?;
                // SAFETY: the retained child descriptor is live and `mode` is a permission mask.
                if unsafe { libc::fchmod(handle.as_raw_fd(), mode as libc::mode_t) } == -1 {
                    return Err(format!(
                        "{label}: cannot set owner-only directory permissions: {}",
                        std::io::Error::last_os_error()
                    )
                    .into());
                }
                self.sync()?;
            }
            Ok(child)
        }
        #[cfg(not(unix))]
        {
            let _ = (name, mode);
            Err(format!("{label}: descriptor-relative directory creation is unsupported").into())
        }
    }

    pub(crate) fn remove_child(
        &self,
        name: &OsStr,
        directory: bool,
        label: &str,
    ) -> Result<(), BoxError> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd as _;

            let name = child_name_cstring(name, label)?;
            let flags = if directory { libc::AT_REMOVEDIR } else { 0 };
            // SAFETY: the retained parent descriptor and single-component name are live.
            if unsafe { libc::unlinkat(self.file.as_raw_fd(), name.as_ptr(), flags) } == -1 {
                return Err(format!(
                    "{label}: cannot remove descriptor-relative child: {}",
                    std::io::Error::last_os_error()
                )
                .into());
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = (name, directory);
            Err(format!("{label}: descriptor-relative removal is unsupported").into())
        }
    }

    /// Atomically publish one already-synced regular child over another, but only while both names
    /// still identify the caller's retained file objects beneath this exact directory descriptor.
    pub(crate) fn replace_regular_child(
        &self,
        target: RegularChildRef<'_>,
        replacement: RegularChildRef<'_>,
        rollback: RegularChildRef<'_>,
        label: &str,
    ) -> Result<(), BoxError> {
        self.replace_regular_child_with_sync(target, replacement, rollback, label, || {
            self.file.sync_all()
        })
    }

    fn replace_regular_child_with_sync<F>(
        &self,
        target: RegularChildRef<'_>,
        replacement: RegularChildRef<'_>,
        rollback: RegularChildRef<'_>,
        label: &str,
        sync_directory: F,
    ) -> Result<(), BoxError>
    where
        F: FnOnce() -> std::io::Result<()>,
    {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd as _;

            let opened_target = self.open_regular_file(target.name, label)?;
            let opened_replacement = self.open_regular_file(replacement.name, label)?;
            let opened_rollback = self.open_regular_file(rollback.name, label)?;
            let target_metadata = target
                .file
                .metadata()
                .map_err(|error| format!("{label}: cannot inspect retained target: {error}"))?;
            let replacement_metadata = replacement.file.metadata().map_err(|error| {
                format!("{label}: cannot inspect retained replacement: {error}")
            })?;
            let rollback_metadata = rollback
                .file
                .metadata()
                .map_err(|error| format!("{label}: cannot inspect retained rollback: {error}"))?;
            if !same_file(&opened_target.metadata()?, &target_metadata) {
                return Err(format!("{label}: target identity changed before replacement").into());
            }
            if !same_file(&opened_replacement.metadata()?, &replacement_metadata) {
                return Err(
                    format!("{label}: replacement identity changed before publication").into(),
                );
            }
            if !same_file(&opened_rollback.metadata()?, &rollback_metadata) {
                return Err(
                    format!("{label}: rollback identity changed before publication").into(),
                );
            }

            let target_name = child_name_cstring(target.name, label)?;
            let replacement_name = child_name_cstring(replacement.name, label)?;
            let rollback_name = child_name_cstring(rollback.name, label)?;
            // SAFETY: target and replacement are validated single components bound to the retained
            // open files above. POSIX rename atomically publishes the synced replacement.
            if unsafe {
                libc::renameat(
                    self.file.as_raw_fd(),
                    replacement_name.as_ptr(),
                    self.file.as_raw_fd(),
                    target_name.as_ptr(),
                )
            } == -1
            {
                // SAFETY: best-effort cleanup of the still-separate rollback copy.
                unsafe {
                    libc::unlinkat(self.file.as_raw_fd(), rollback_name.as_ptr(), 0);
                }
                return Err(format!(
                    "{label}: cannot atomically publish descriptor-relative replacement: {}",
                    std::io::Error::last_os_error()
                )
                .into());
            }
            if let Err(sync_error) = sync_directory() {
                // SAFETY: rollback is a separately synced copy of the blocking setup aggregate.
                if unsafe {
                    libc::renameat(
                        self.file.as_raw_fd(),
                        rollback_name.as_ptr(),
                        self.file.as_raw_fd(),
                        target_name.as_ptr(),
                    )
                } == -1
                {
                    let rollback_error = std::io::Error::last_os_error();
                    // SAFETY: if restoration itself fails, best-effort removal prevents a green
                    // artifact from remaining authoritative at the requested output name.
                    unsafe {
                        libc::unlinkat(self.file.as_raw_fd(), target_name.as_ptr(), 0);
                    }
                    return Err(format!(
                        "{label}: cannot sync replacement directory: {sync_error}; cannot restore blocking target: {rollback_error}"
                    )
                    .into());
                }
                let _ = self.file.sync_all();
                return Err(format!(
                    "{label}: cannot sync replacement directory: {sync_error}; blocking target restored"
                )
                .into());
            }
            // The output name now durably identifies the final file. Rollback-copy cleanup is
            // best-effort because a cleanup failure must not turn a valid green artifact into a
            // command failure; any residue remains owner-only setup evidence outside repositories.
            // SAFETY: rollback remains the validated setup-copy component created by the caller.
            unsafe {
                libc::unlinkat(self.file.as_raw_fd(), rollback_name.as_ptr(), 0);
            }
            let _ = self.file.sync_all();
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = (target, replacement, rollback);
            let _ = sync_directory;
            Err(format!("{label}: atomic descriptor-relative replacement is unsupported").into())
        }
    }
}

#[cfg(unix)]
fn set_effective_owner(file: &File, label: &str) -> Result<(), BoxError> {
    use std::os::fd::AsRawFd as _;

    // SAFETY: `file` owns a live descriptor; geteuid/getegid have no preconditions; and fchown
    // applies only to that retained object rather than re-resolving a path.
    if unsafe { libc::fchown(file.as_raw_fd(), libc::geteuid(), libc::getegid()) } == -1 {
        return Err(format!(
            "{label}: cannot bind effective ownership: {}",
            std::io::Error::last_os_error()
        )
        .into());
    }
    Ok(())
}

#[cfg(unix)]
fn child_name_cstring(name: &OsStr, label: &str) -> Result<std::ffi::CString, BoxError> {
    use std::os::unix::ffi::OsStrExt as _;

    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes == b"." || bytes == b".." || bytes.contains(&b'/') {
        return Err(format!("{label}: child name must be one non-special path component").into());
    }
    std::ffi::CString::new(bytes).map_err(|_| format!("{label}: child name contains NUL").into())
}

#[cfg(not(unix))]
fn same_file(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    left.len() == right.len()
        && left.modified().ok() == right.modified().ok()
        && left.created().ok() == right.created().ok()
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
    let mut out = String::with_capacity(digest.as_ref().len() * 2);
    for byte in digest.as_ref() {
        use std::fmt::Write as _;
        write!(&mut out, "{byte:02x}").expect("writing to String cannot fail");
    }
    out
}

pub(crate) fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(crate) fn sha256_regular_file_bounded(
    file: &File,
    label: &str,
    max_bytes: u64,
) -> Result<String, BoxError> {
    Ok(read_open_regular_file_bounded(file, label, max_bytes)?.sha256)
}

pub(crate) fn read_open_regular_file_bounded(
    file: &File,
    label: &str,
    max_bytes: u64,
) -> Result<OpenFileSnapshot, BoxError> {
    let metadata = file
        .metadata()
        .map_err(|error| format!("{label}: cannot inspect open file: {error}"))?;
    if !metadata.is_file() || metadata.len() > max_bytes {
        return Err(format!("{label}: open file is not a bounded regular file").into());
    }
    let mut reader = file
        .try_clone()
        .map_err(|error| format!("{label}: cannot clone open file: {error}"))?;
    reader
        .seek(std::io::SeekFrom::Start(0))
        .map_err(|error| format!("{label}: cannot rewind open file: {error}"))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    reader
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{label}: read failed: {error}"))?;
    if bytes.len() as u64 > max_bytes {
        return Err(format!("{label}: content exceeds the {max_bytes}-byte limit").into());
    }
    let sha256 = sha256_hex(&bytes);
    Ok(OpenFileSnapshot { bytes, sha256 })
}

pub(crate) fn stable_regular_file_path(
    file: &File,
    label: &str,
) -> Result<(PathBuf, bool), BoxError> {
    #[cfg(target_os = "macos")]
    let (path, retain_descriptor_after_exec) = {
        use std::os::unix::fs::MetadataExt as _;

        let metadata = file
            .metadata()
            .map_err(|error| format!("{label}: cannot inspect open file: {error}"))?;
        (
            PathBuf::from(format!("/.vol/{}/{}", metadata.dev(), metadata.ino())),
            false,
        )
    };
    #[cfg(target_os = "linux")]
    let (path, retain_descriptor_after_exec) = {
        use std::os::fd::AsRawFd as _;

        (
            PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd())),
            true,
        )
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    return Err(format!("{label}: stable executable object paths are unsupported").into());

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let descriptor_metadata = file
            .metadata()
            .map_err(|error| format!("{label}: cannot inspect open file: {error}"))?;
        let path_metadata = std::fs::metadata(&path).map_err(|error| {
            format!(
                "{label}: stable file handle {} is unavailable: {error}",
                path.display()
            )
        })?;
        if !descriptor_metadata.is_file()
            || !path_metadata.is_file()
            || !same_file(&descriptor_metadata, &path_metadata)
        {
            return Err(format!(
                "{label}: stable file handle {} does not identify the opened file",
                path.display()
            )
            .into());
        }
        Ok((path, retain_descriptor_after_exec))
    }
}

pub(crate) fn read_regular_file_bounded(
    path: &Path,
    label: &str,
    max_bytes: u64,
) -> Result<LocalFileSnapshot, BoxError> {
    read_regular_file_bounded_after_open(path, label, max_bytes, || {})
}

fn read_regular_file_bounded_after_open<F>(
    path: &Path,
    label: &str,
    max_bytes: u64,
    after_open: F,
) -> Result<LocalFileSnapshot, BoxError>
where
    F: FnOnce(),
{
    if path.as_os_str().is_empty() {
        return Err(format!("{label}: path must be non-empty").into());
    }
    let mut file = open_read_only_nonblocking(path)
        .map_err(|error| format!("{label}: cannot open {}: {error}", path.display()))?;
    after_open();
    let descriptor_metadata = file
        .metadata()
        .map_err(|error| format!("{label}: cannot inspect {}: {error}", path.display()))?;
    if !descriptor_metadata.is_file() {
        return Err(format!("{label}: {} must be a regular file", path.display()).into());
    }
    if descriptor_metadata.len() > max_bytes {
        return Err(format!(
            "{label}: {} exceeds the {max_bytes}-byte limit",
            path.display()
        )
        .into());
    }

    let identity = RegularFileIdentity::from_metadata(&descriptor_metadata);
    let canonical_path = std::fs::canonicalize(path)
        .map_err(|error| format!("{label}: cannot resolve {}: {error}", path.display()))?;
    let path_metadata = std::fs::metadata(&canonical_path).map_err(|error| {
        format!(
            "{label}: cannot inspect resolved path {}: {error}",
            canonical_path.display()
        )
    })?;
    if !path_metadata.is_file() || !same_file(&descriptor_metadata, &path_metadata) {
        return Err(format!("{label}: path changed while it was being opened").into());
    }

    let mut bytes = Vec::with_capacity(descriptor_metadata.len() as usize);
    file.by_ref()
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{label}: read failed: {error}"))?;
    if bytes.len() as u64 > max_bytes {
        return Err(format!("{label}: content exceeds the {max_bytes}-byte limit").into());
    }
    let final_descriptor_metadata = file
        .metadata()
        .map_err(|error| format!("{label}: cannot reinspect {}: {error}", path.display()))?;
    let final_path_metadata = std::fs::metadata(&canonical_path).map_err(|error| {
        format!(
            "{label}: cannot reinspect resolved path {}: {error}",
            canonical_path.display()
        )
    })?;
    if !identity.matches_metadata(&final_descriptor_metadata)
        || !identity.matches_metadata(&final_path_metadata)
        || !same_file(&final_descriptor_metadata, &final_path_metadata)
    {
        return Err(format!("{label}: file changed while it was being read").into());
    }
    let sha256 = sha256_hex(&bytes);
    Ok(LocalFileSnapshot {
        canonical_path,
        bytes,
        sha256,
        identity,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn bounded_reader_accepts_regular_file_and_hashes_exact_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("evidence.json");
        fs::write(&path, b"abc").unwrap();
        let snapshot = read_regular_file_bounded(&path, "evidence", 3).unwrap();
        assert_eq!(snapshot.bytes, b"abc");
        assert_eq!(
            snapshot.sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(snapshot.canonical_path, fs::canonicalize(path).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn bounded_reader_rejects_symlink_fifo_device_and_socket_without_blocking() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt as _;
        use std::os::unix::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let regular = dir.path().join("regular");
        fs::write(&regular, b"ok").unwrap();
        let symlink = dir.path().join("symlink");
        std::os::unix::fs::symlink(&regular, &symlink).unwrap();
        assert!(read_regular_file_bounded(&symlink, "source", 1024).is_err());

        let fifo = dir.path().join("fifo");
        let fifo_c = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: `fifo_c` is a valid NUL-terminated path and the mode is a normal permission mask.
        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
        let started = std::time::Instant::now();
        assert!(read_regular_file_bounded(&fifo, "source", 1024).is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(1));

        assert!(read_regular_file_bounded(Path::new("/dev/null"), "source", 1024).is_err());

        let socket = dir.path().join("socket");
        let _listener = UnixListener::bind(&socket).unwrap();
        assert!(read_regular_file_bounded(&socket, "source", 1024).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn bounded_reader_rejects_path_replacement_after_descriptor_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("evidence.json");
        let original = dir.path().join("original.json");
        fs::write(&path, b"trusted").unwrap();

        let error = read_regular_file_bounded_after_open(&path, "evidence", 1024, || {
            fs::rename(&path, &original).unwrap();
            fs::write(&path, b"replacement").unwrap();
        })
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("path changed while it was being opened"));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn pinned_directory_exposes_an_absolute_object_path_across_same_name_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let planned = dir.path().join("planned");
        let moved = dir.path().join("moved");
        fs::create_dir(&planned).unwrap();
        fs::write(planned.join("original-marker"), b"original").unwrap();
        let snapshot = snapshot_directory(&planned, "test cwd").unwrap();
        let pin = PinnedDirectory::open(
            &planned,
            &snapshot.canonical_cwd,
            &snapshot.identity,
            "test cwd",
        )
        .unwrap();
        let stable = pin.acp_session_cwd();
        assert!(stable.is_absolute());

        fs::rename(&planned, &moved).unwrap();
        fs::create_dir(&planned).unwrap();

        let stable_metadata = fs::metadata(&stable).unwrap();
        assert!(snapshot.identity.matches_metadata(&stable_metadata));
        assert_eq!(
            fs::read(stable.join("original-marker")).unwrap(),
            b"original"
        );
        assert!(!pin.current_path_matches());
        #[cfg(target_os = "macos")]
        assert!(!pin.retain_descriptor_after_exec());
        #[cfg(target_os = "linux")]
        assert!(pin.retain_descriptor_after_exec());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn descriptor_relative_children_stay_on_the_pinned_parent_and_reject_escape_names() {
        use std::io::Write as _;

        let dir = tempfile::tempdir().unwrap();
        let planned = dir.path().join("planned");
        let moved = dir.path().join("moved");
        fs::create_dir(&planned).unwrap();
        let snapshot = snapshot_directory(&planned, "test parent").unwrap();
        let pin = PinnedDirectory::open(
            &planned,
            &snapshot.canonical_cwd,
            &snapshot.identity,
            "test parent",
        )
        .unwrap();

        fs::rename(&planned, &moved).unwrap();
        fs::create_dir(&planned).unwrap();
        fs::create_dir(planned.join(".git")).unwrap();

        let mut evidence = pin
            .create_new_file(OsStr::new("evidence.json"), 0o600, "test evidence")
            .unwrap();
        evidence.write_all(b"original-parent").unwrap();
        drop(evidence);
        let scratch = pin
            .create_child_directory(OsStr::new("scratch"), 0o700, "test scratch")
            .unwrap();
        let mut marker = scratch
            .create_new_file(OsStr::new("marker"), 0o600, "test marker")
            .unwrap();
        marker.write_all(b"pinned-child").unwrap();
        drop(marker);

        assert_eq!(
            fs::read(moved.join("evidence.json")).unwrap(),
            b"original-parent"
        );
        assert_eq!(
            fs::read(moved.join("scratch/marker")).unwrap(),
            b"pinned-child"
        );
        assert!(!planned.join("evidence.json").exists());
        assert!(!planned.join("scratch").exists());
        assert!(pin
            .create_new_file(OsStr::new("../escape"), 0o600, "escape")
            .is_err());
        assert!(!dir.path().join("escape").exists());

        fs::hard_link(moved.join("evidence.json"), moved.join("evidence-link")).unwrap();
        assert!(pin
            .open_regular_file(OsStr::new("evidence.json"), "hard-linked evidence")
            .unwrap_err()
            .to_string()
            .contains("single-link"));
        fs::remove_file(moved.join("evidence-link")).unwrap();
        pin.open_regular_file(OsStr::new("evidence.json"), "single-link evidence")
            .unwrap();

        scratch
            .remove_child(OsStr::new("marker"), false, "test marker cleanup")
            .unwrap();
        drop(scratch);
        pin.remove_child(OsStr::new("scratch"), true, "test scratch cleanup")
            .unwrap();
        pin.remove_child(OsStr::new("evidence.json"), false, "test evidence cleanup")
            .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn atomic_child_replacement_restores_target_when_directory_sync_fails() {
        use std::io::Write as _;

        let dir = tempfile::tempdir().unwrap();
        let snapshot = snapshot_directory(dir.path(), "test publication directory").unwrap();
        let pin = PinnedDirectory::open(
            dir.path(),
            &snapshot.canonical_cwd,
            &snapshot.identity,
            "test publication directory",
        )
        .unwrap();
        let mut setup = pin
            .create_new_file(OsStr::new("aggregate.json"), 0o600, "test setup aggregate")
            .unwrap();
        setup.write_all(b"blocking setup").unwrap();
        setup.sync_all().unwrap();
        let mut replacement = pin
            .create_new_file(OsStr::new("final.json"), 0o600, "test final aggregate")
            .unwrap();
        replacement.write_all(b"green final").unwrap();
        replacement.sync_all().unwrap();
        let mut rollback = pin
            .create_new_file(
                OsStr::new("setup-rollback.json"),
                0o600,
                "test rollback aggregate",
            )
            .unwrap();
        rollback.write_all(b"blocking setup").unwrap();
        rollback.sync_all().unwrap();

        let error = pin
            .replace_regular_child_with_sync(
                RegularChildRef::new(OsStr::new("aggregate.json"), &setup),
                RegularChildRef::new(OsStr::new("final.json"), &replacement),
                RegularChildRef::new(OsStr::new("setup-rollback.json"), &rollback),
                "test final publication",
                || Err(std::io::Error::other("injected directory sync failure")),
            )
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("injected directory sync failure"));
        assert_eq!(
            fs::read(dir.path().join("aggregate.json")).unwrap(),
            b"blocking setup"
        );
        assert!(!dir.path().join("final.json").exists());
        assert!(!dir.path().join("setup-rollback.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn atomic_child_replacement_refuses_same_name_rebound_replacement() {
        use std::io::Write as _;

        let dir = tempfile::tempdir().unwrap();
        let snapshot = snapshot_directory(dir.path(), "test publication directory").unwrap();
        let pin = PinnedDirectory::open(
            dir.path(),
            &snapshot.canonical_cwd,
            &snapshot.identity,
            "test publication directory",
        )
        .unwrap();
        let mut setup = pin
            .create_new_file(OsStr::new("aggregate.json"), 0o600, "test setup aggregate")
            .unwrap();
        setup.write_all(b"blocking setup").unwrap();
        setup.sync_all().unwrap();
        let mut replacement = pin
            .create_new_file(OsStr::new("final.json"), 0o600, "test final aggregate")
            .unwrap();
        replacement.write_all(b"green final").unwrap();
        replacement.sync_all().unwrap();
        let mut rollback = pin
            .create_new_file(
                OsStr::new("setup-rollback.json"),
                0o600,
                "test rollback aggregate",
            )
            .unwrap();
        rollback.write_all(b"blocking setup").unwrap();
        rollback.sync_all().unwrap();

        fs::rename(
            dir.path().join("final.json"),
            dir.path().join("retained-final.json"),
        )
        .unwrap();
        fs::write(dir.path().join("final.json"), b"same-name rebound final").unwrap();

        let error = pin
            .replace_regular_child(
                RegularChildRef::new(OsStr::new("aggregate.json"), &setup),
                RegularChildRef::new(OsStr::new("final.json"), &replacement),
                RegularChildRef::new(OsStr::new("setup-rollback.json"), &rollback),
                "test final publication",
            )
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("replacement identity changed before publication"));
        assert_eq!(
            fs::read(dir.path().join("aggregate.json")).unwrap(),
            b"blocking setup"
        );
        assert_eq!(
            fs::read(dir.path().join("final.json")).unwrap(),
            b"same-name rebound final"
        );
        assert_eq!(
            fs::read(dir.path().join("retained-final.json")).unwrap(),
            b"green final"
        );
    }

    #[cfg(unix)]
    #[test]
    fn atomic_child_replacement_removes_green_target_when_rollback_fails() {
        use std::io::Write as _;

        let dir = tempfile::tempdir().unwrap();
        let snapshot = snapshot_directory(dir.path(), "test publication directory").unwrap();
        let pin = PinnedDirectory::open(
            dir.path(),
            &snapshot.canonical_cwd,
            &snapshot.identity,
            "test publication directory",
        )
        .unwrap();
        let mut setup = pin
            .create_new_file(OsStr::new("aggregate.json"), 0o600, "test setup aggregate")
            .unwrap();
        setup.write_all(b"blocking setup").unwrap();
        setup.sync_all().unwrap();
        let mut replacement = pin
            .create_new_file(OsStr::new("final.json"), 0o600, "test final aggregate")
            .unwrap();
        replacement.write_all(b"green final").unwrap();
        replacement.sync_all().unwrap();
        let mut rollback = pin
            .create_new_file(
                OsStr::new("setup-rollback.json"),
                0o600,
                "test rollback aggregate",
            )
            .unwrap();
        rollback.write_all(b"blocking setup").unwrap();
        rollback.sync_all().unwrap();

        let error = pin
            .replace_regular_child_with_sync(
                RegularChildRef::new(OsStr::new("aggregate.json"), &setup),
                RegularChildRef::new(OsStr::new("final.json"), &replacement),
                RegularChildRef::new(OsStr::new("setup-rollback.json"), &rollback),
                "test final publication",
                || {
                    fs::remove_file(dir.path().join("setup-rollback.json")).unwrap();
                    Err(std::io::Error::other("injected directory sync failure"))
                },
            )
            .unwrap_err();

        assert!(error.to_string().contains("cannot restore blocking target"));
        assert!(!dir.path().join("aggregate.json").exists());
        assert!(!dir.path().join("final.json").exists());
        assert!(!dir.path().join("setup-rollback.json").exists());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn pinned_directory_rejects_a_wrong_object_fingerprint_with_matching_dev_and_inode() {
        let dir = tempfile::tempdir().unwrap();
        let planned = dir.path().join("planned");
        fs::create_dir(&planned).unwrap();
        let snapshot = snapshot_directory(&planned, "test cwd").unwrap();
        let mut wrong_identity = snapshot.identity.clone();
        wrong_identity.object_sha256 = "0".repeat(64);

        let error = PinnedDirectory::open(
            &planned,
            &snapshot.canonical_cwd,
            &wrong_identity,
            "test cwd",
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("directory identity changed after planning"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn durable_directory_identity_fails_closed_when_both_handle_modes_are_unavailable() {
        let mut observed_flags = Vec::new();
        let error = acquire_linux_file_handle(
            |flags| {
                observed_flags.push(flags);
                Err(std::io::Error::from_raw_os_error(libc::EOPNOTSUPP))
            },
            "unsupported cwd",
        )
        .unwrap_err();
        assert_eq!(
            observed_flags,
            [
                libc::AT_EMPTY_PATH | libc::AT_HANDLE_FID | LINUX_AT_HANDLE_MNT_ID_UNIQUE,
                libc::AT_EMPTY_PATH | LINUX_AT_HANDLE_MNT_ID_UNIQUE
            ]
        );
        assert!(error
            .to_string()
            .contains("cannot provide a durable directory object handle"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_boot_identity_parser_is_strict_and_bounded() {
        let valid = b"00112233-4455-6677-8899-aabbccddeeff\n";
        assert_eq!(parse_linux_boot_id(valid).unwrap().as_slice(), &valid[..36]);
        assert!(parse_linux_boot_id(b"00112233-4455-6677-8899-AABBCCDDEEFF\n").is_none());
        assert!(parse_linux_boot_id(b"001122334455-6677-8899-aabbccddeeff\n").is_none());
        assert!(parse_linux_boot_id(b"00112233-4455-6677-8899-aabbccddeeffxx").is_none());
        assert!(parse_linux_boot_id(b"00000000-0000-0000-0000-000000000000\n").is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn darwin_durable_identity_requires_both_persistent_64_bit_capabilities() {
        let persistent = libc::VOL_CAP_FMT_PERSISTENTOBJECTIDS;
        let wide = libc::VOL_CAP_FMT_64BIT_OBJECT_IDS;
        assert!(macos_has_persistent_64_bit_file_ids(
            persistent | wide,
            persistent | wide
        ));
        assert!(!macos_has_persistent_64_bit_file_ids(
            persistent,
            persistent | wide
        ));
        assert!(!macos_has_persistent_64_bit_file_ids(
            persistent | wide,
            persistent
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn darwin_durable_identity_is_scoped_to_the_volume_uuid() {
        let dir = tempfile::tempdir().unwrap();
        let file = open_directory(dir.path()).unwrap();
        let material = durable_directory_object_material(&file, "test cwd").unwrap();
        let prefix = b"a2a-bridge:darwin-volume-file-id:v2\0";

        assert!(material.starts_with(prefix));
        assert_eq!(material.len(), prefix.len() + 16 + 8);
        assert!(!macos_valid_volume_uuid(&[0; 16]));
        assert!(macos_valid_volume_uuid(&[1; 16]));
    }
}
