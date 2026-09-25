//! Closed, descriptor-rooted Git execution for the isolated custody exporter.
//!
//! This module deliberately has no public API and no production caller.  It is the narrow effect
//! seam that the later exporter will use after it has selected a caller-pinned Git installation.

use crate::fs_custody::{DirectoryIdentityV1, FsCustodyError, PinnedDirectoryV1};
use crate::process::{ProcessAuthorityPortV1, SystemProcessAuthorityV1};
use ring::digest;
#[cfg(test)]
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::{CString, OsString};
use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const GIT_MINIMUM_VERSION: (u32, u32, u32) = (2, 54, 0);
const VERSION_STDOUT_LIMIT: usize = 16 * 1024;
const VERSION_STDERR_LIMIT: usize = 16 * 1024;
const FIXED_PATH: &str = "/usr/bin:/bin";

type BuiltGitCommandV1 = (
    Command,
    Vec<OsString>,
    BTreeMap<String, String>,
    Option<GitObjectStoreEvidenceV1>,
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ExpectedGitDigestV1([u8; 32]);

impl ExpectedGitDigestV1 {
    #[must_use]
    pub(crate) const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub(crate) const fn bytes(self) -> [u8; 32] {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GitObjectFormatV1 {
    Sha1,
    Sha256,
}

impl GitObjectFormatV1 {
    fn argument(self) -> &'static str {
        match self {
            Self::Sha1 => "sha1",
            Self::Sha256 => "sha256",
        }
    }

    fn hash_length(self) -> usize {
        match self {
            Self::Sha1 => 40,
            Self::Sha256 => 64,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GitCommandV1 {
    Version,
    InitBare {
        dir: String,
        object_format: GitObjectFormatV1,
    },
    CatFileBatchCheck,
    PackObjectsStdout,
    IndexPackStrictStdin,
    VerifyPack {
        git_dir: String,
        pack_hash: String,
        object_format: GitObjectFormatV1,
    },
    CatFileAllObjects,
    RevListMissingPrint,
    FsckStrict,
}

impl GitCommandV1 {
    pub(crate) fn arguments(&self) -> Result<(Vec<OsString>, bool), CustodyGitError> {
        let arguments = match self {
            Self::Version => vec!["version".into()],
            Self::InitBare { dir, object_format } => {
                validate_component(dir, "init bare directory")?;
                vec![
                    "init".into(),
                    "--bare".into(),
                    "--template=".into(),
                    format!("--object-format={}", object_format.argument()).into(),
                    dir.into(),
                ]
            }
            Self::CatFileBatchCheck => vec!["cat-file".into(), "--batch-check".into()],
            Self::PackObjectsStdout => vec!["pack-objects".into(), "--stdout".into()],
            Self::IndexPackStrictStdin => {
                vec!["index-pack".into(), "--strict".into(), "--stdin".into()]
            }
            Self::VerifyPack {
                git_dir,
                pack_hash,
                object_format,
            } => {
                validate_component(git_dir, "verify pack git directory")?;
                if pack_hash.len() != object_format.hash_length()
                    || !pack_hash
                        .bytes()
                        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
                {
                    return Err(CustodyGitError::InvalidCommand(
                        "verify pack hash is not a lowercase object id".into(),
                    ));
                }
                vec![
                    "verify-pack".into(),
                    "-v".into(),
                    format!("{git_dir}/objects/pack/pack-{pack_hash}.idx").into(),
                ]
            }
            Self::CatFileAllObjects => vec![
                "cat-file".into(),
                "--batch-all-objects".into(),
                "--batch-check=%(objectname) %(objecttype)".into(),
            ],
            Self::RevListMissingPrint => vec![
                "rev-list".into(),
                "--objects".into(),
                "--no-object-names".into(),
                "--missing=print".into(),
                "--stdin".into(),
            ],
            Self::FsckStrict => vec![
                "fsck".into(),
                "--strict".into(),
                "--full".into(),
                "--no-reflogs".into(),
                "--no-dangling".into(),
                "--no-progress".into(),
            ],
        };
        Ok((arguments, !matches!(self, Self::InitBare { .. })))
    }

    /// Only the read-only source-reading subcommands may be pointed at a caller object store.
    ///
    /// An object-store route is absolute by necessity (HL1), so it is the one input that can name
    /// a location outside the caller's pinned root. A mutating subcommand handed such a route would
    /// write repository, pack, or index data there — `index-pack --stdin` writes its pack and index
    /// straight into `GIT_OBJECT_DIRECTORY` — which would defeat the root-confined effect boundary.
    fn permits_object_store_route(&self) -> bool {
        match self {
            Self::CatFileBatchCheck
            | Self::PackObjectsStdout
            | Self::VerifyPack { .. }
            | Self::CatFileAllObjects
            | Self::RevListMissingPrint
            | Self::FsckStrict => true,
            Self::Version | Self::InitBare { .. } | Self::IndexPackStrictStdin => false,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Version => "version",
            Self::InitBare { .. } => "init --bare",
            Self::CatFileBatchCheck => "cat-file --batch-check",
            Self::PackObjectsStdout => "pack-objects --stdout",
            Self::IndexPackStrictStdin => "index-pack --strict --stdin",
            Self::VerifyPack { .. } => "verify-pack",
            Self::CatFileAllObjects => "cat-file --batch-all-objects",
            Self::RevListMissingPrint => "rev-list --missing=print",
            Self::FsckStrict => "fsck --strict",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GitRootNamesV1 {
    home: String,
    xdg_config_home: String,
    git_dir: String,
}

impl GitRootNamesV1 {
    pub(crate) fn new(
        home: impl Into<String>,
        xdg_config_home: impl Into<String>,
        git_dir: impl Into<String>,
    ) -> Result<Self, CustodyGitError> {
        let result = Self {
            home: home.into(),
            xdg_config_home: xdg_config_home.into(),
            git_dir: git_dir.into(),
        };
        validate_component(&result.home, "HOME")?;
        validate_component(&result.xdg_config_home, "XDG_CONFIG_HOME")?;
        validate_component(&result.git_dir, "GIT_DIR")?;
        Ok(result)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct GitObjectStoreRouteV1 {
    primary: PathBuf,
    alternates: Vec<PathBuf>,
}

impl GitObjectStoreRouteV1 {
    pub(crate) fn new(primary: PathBuf, alternates: Vec<PathBuf>) -> Result<Self, CustodyGitError> {
        validate_object_store_path(&primary)?;
        for alternate in &alternates {
            validate_object_store_path(alternate)?;
        }
        Ok(Self {
            primary,
            alternates,
        })
    }

    fn alternate_value(&self) -> OsString {
        let mut bytes = Vec::new();
        for (index, path) in self.alternates.iter().enumerate() {
            if index != 0 {
                bytes.push(b':');
            }
            bytes.extend_from_slice(path.as_os_str().as_bytes());
        }
        OsString::from_vec(bytes)
    }

    pub(crate) fn evidence(&self) -> GitObjectStoreEvidenceV1 {
        GitObjectStoreEvidenceV1 {
            primary: digest_path(b"git-object-dir", self.primary.as_os_str().as_bytes()),
            alternates: self
                .alternates
                .iter()
                .map(|path| digest_path(b"git-alternate-dir", path.as_os_str().as_bytes()))
                .collect(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct GitRouteRequestV1 {
    path: PathBuf,
    expected_digest: ExpectedGitDigestV1,
    profile: GitAdmissionProfileV1,
}

impl GitRouteRequestV1 {
    pub(crate) fn production(
        path: PathBuf,
        expected_digest: ExpectedGitDigestV1,
    ) -> Result<Self, CustodyGitError> {
        if !path.is_absolute() {
            return Err(CustodyGitError::InvalidRoute(
                "Git route must be absolute".into(),
            ));
        }
        Ok(Self {
            path,
            expected_digest,
            profile: GitAdmissionProfileV1::Production,
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test_fixture(
        path: PathBuf,
        expected_digest: ExpectedGitDigestV1,
        trust_anchor: PathBuf,
    ) -> Result<Self, CustodyGitError> {
        if !path.is_absolute() || !trust_anchor.is_absolute() {
            return Err(CustodyGitError::InvalidRoute(
                "test Git route and anchor must be absolute".into(),
            ));
        }
        Ok(Self {
            path,
            expected_digest,
            profile: GitAdmissionProfileV1::TestFixture { trust_anchor },
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test_system(
        path: PathBuf,
        expected_digest: ExpectedGitDigestV1,
    ) -> Result<Self, CustodyGitError> {
        if !path.is_absolute() {
            return Err(CustodyGitError::InvalidRoute(
                "test Git route must be absolute".into(),
            ));
        }
        Ok(Self {
            path,
            expected_digest,
            profile: GitAdmissionProfileV1::TestSystem,
        })
    }
}

#[derive(Clone, Debug)]
enum GitAdmissionProfileV1 {
    Production,
    #[cfg(test)]
    TestFixture {
        trust_anchor: PathBuf,
    },
    #[cfg(test)]
    TestSystem,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WriteCheckV1 {
    Faccessat,
    ModeBitsAsRoot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RouteFactsV1 {
    pub(crate) dev: u64,
    pub(crate) ino: u64,
    pub(crate) file_type: u32,
    pub(crate) uid: u32,
    pub(crate) gid: u32,
    pub(crate) mode: u32,
    pub(crate) size: u64,
    pub(crate) mtime_secs: i64,
    pub(crate) mtime_nanos: i64,
    pub(crate) ctime_secs: i64,
    pub(crate) ctime_nanos: i64,
}

impl RouteFactsV1 {
    pub(crate) fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
            // The POSIX file-type mask is written as a literal rather than `libc::S_IFMT`, whose
            // `mode_t` is `u16` on macOS and `u32` on Linux; a cast either way trips clippy on one
            // of the two lanes.
            file_type: metadata.mode() & 0o170000,
            uid: metadata.uid(),
            gid: metadata.gid(),
            mode: metadata.mode(),
            size: metadata.size(),
            mtime_secs: metadata.mtime(),
            mtime_nanos: metadata.mtime_nsec(),
            ctime_secs: metadata.ctime(),
            ctime_nanos: metadata.ctime_nsec(),
        }
    }
}

pub(crate) fn same_route_facts(left: &RouteFactsV1, right: &RouteFactsV1) -> bool {
    left.dev == right.dev
        && left.ino == right.ino
        && left.file_type == right.file_type
        && left.uid == right.uid
        && left.gid == right.gid
        && left.mode == right.mode
        && left.size == right.size
        && left.mtime_secs == right.mtime_secs
        && left.mtime_nanos == right.mtime_nanos
        && left.ctime_secs == right.ctime_secs
        && left.ctime_nanos == right.ctime_nanos
}

#[cfg(test)]
thread_local! {
    static RECHECK_FACTS_OVERRIDE: RefCell<bool> = const { RefCell::new(false) };
}

#[cfg(test)]
pub(crate) struct RecheckFactsOverrideGuardV1(bool);

#[cfg(test)]
impl Drop for RecheckFactsOverrideGuardV1 {
    fn drop(&mut self) {
        RECHECK_FACTS_OVERRIDE.with(|slot| *slot.borrow_mut() = self.0);
    }
}

#[cfg(test)]
pub(crate) fn hold_recheck_facts_constant_for_test() -> RecheckFactsOverrideGuardV1 {
    let previous = RECHECK_FACTS_OVERRIDE.with(|slot| slot.replace(true));
    RecheckFactsOverrideGuardV1(previous)
}

#[cfg(test)]
fn facts_for_recheck(observed: RouteFactsV1, admitted: RouteFactsV1) -> RouteFactsV1 {
    RECHECK_FACTS_OVERRIDE.with(|slot| if *slot.borrow() { admitted } else { observed })
}

#[cfg(not(test))]
fn facts_for_recheck(observed: RouteFactsV1, _: RouteFactsV1) -> RouteFactsV1 {
    observed
}

#[cfg(test)]
thread_local! {
    static RECHECK_DIGEST_BYPASS: RefCell<bool> = const { RefCell::new(false) };
}

#[cfg(test)]
pub(crate) struct RecheckDigestBypassGuardV1(bool);

#[cfg(test)]
impl Drop for RecheckDigestBypassGuardV1 {
    fn drop(&mut self) {
        RECHECK_DIGEST_BYPASS.with(|slot| *slot.borrow_mut() = self.0);
    }
}

/// Isolates A5a. The mandatory pre-spawn recheck repeats the digest comparison, so it masks
/// admission's own comparison: with this bypass held, step 4 of admission is the only thing between
/// a mismatched pin and an executed binary.
#[cfg(test)]
pub(crate) fn bypass_recheck_digest_for_test() -> RecheckDigestBypassGuardV1 {
    let previous = RECHECK_DIGEST_BYPASS.with(|slot| slot.replace(true));
    RecheckDigestBypassGuardV1(previous)
}

#[cfg(test)]
fn recheck_digest_bypassed() -> bool {
    RECHECK_DIGEST_BYPASS.with(|slot| *slot.borrow())
}

#[cfg(not(test))]
fn recheck_digest_bypassed() -> bool {
    false
}

#[cfg(test)]
type RouteAuditHookV1 = Box<dyn Fn(&Path)>;

#[cfg(test)]
thread_local! {
    static ROUTE_AUDIT_HOOK: RefCell<Option<RouteAuditHookV1>> = const { RefCell::new(None) };
}

#[cfg(test)]
pub(crate) struct RouteAuditHookGuardV1(Option<RouteAuditHookV1>);

#[cfg(test)]
impl Drop for RouteAuditHookGuardV1 {
    fn drop(&mut self) {
        ROUTE_AUDIT_HOOK.with(|slot| *slot.borrow_mut() = self.0.take());
    }
}

#[cfg(test)]
pub(crate) fn install_route_audit_hook_for_test(
    hook: impl Fn(&Path) + 'static,
) -> RouteAuditHookGuardV1 {
    let previous = ROUTE_AUDIT_HOOK.with(|slot| slot.borrow_mut().replace(Box::new(hook)));
    RouteAuditHookGuardV1(previous)
}

#[cfg(test)]
fn run_route_audit_hook(canonical_path: &Path) {
    ROUTE_AUDIT_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow().as_ref() {
            hook(canonical_path);
        }
    });
}

#[cfg(test)]
thread_local! {
    static FINAL_SYMLINK_REFUSAL_BYPASS: RefCell<bool> = const { RefCell::new(false) };
}

#[cfg(test)]
pub(crate) struct FinalSymlinkRefusalBypassGuardV1(bool);

#[cfg(test)]
impl Drop for FinalSymlinkRefusalBypassGuardV1 {
    fn drop(&mut self) {
        FINAL_SYMLINK_REFUSAL_BYPASS.with(|slot| *slot.borrow_mut() = self.0);
    }
}

/// A compiling mutation for A5: skip the supplied-path lstat rejection before canonicalization.
#[cfg(test)]
pub(crate) fn bypass_final_symlink_refusal_for_test() -> FinalSymlinkRefusalBypassGuardV1 {
    let previous = FINAL_SYMLINK_REFUSAL_BYPASS.with(|slot| slot.replace(true));
    FinalSymlinkRefusalBypassGuardV1(previous)
}

#[cfg(test)]
fn final_symlink_refusal_bypassed() -> bool {
    FINAL_SYMLINK_REFUSAL_BYPASS.with(|slot| *slot.borrow())
}

#[cfg(not(test))]
fn final_symlink_refusal_bypassed() -> bool {
    false
}

#[derive(Clone, Debug)]
struct AdmittedGitRouteV1 {
    canonical_path: PathBuf,
    facts: RouteFactsV1,
    expected_digest: ExpectedGitDigestV1,
    observed_digest: [u8; 32],
    write_check: WriteCheckV1,
    profile: GitAdmissionProfileV1,
}

pub(crate) enum GitStdinSourceV1 {
    Bytes(Vec<u8>),
    File { file: File, max_bytes: u64 },
}

enum GitStdoutTargetV1 {
    Capture,
    File(File),
}

/// The child-facing data sources are descriptor-backed when a later exporter streams a pack or
/// ledger. `Bytes` remains only for bounded control inputs such as a Git object-id line.
pub(crate) struct GitRunRequestV1 {
    pub(crate) command: GitCommandV1,
    pub(crate) object_store: Option<GitObjectStoreRouteV1>,
    stdin: GitStdinSourceV1,
    stdout: GitStdoutTargetV1,
    stdout_root_identity: Option<DirectoryIdentityV1>,
    pub(crate) stdout_limit: usize,
    pub(crate) stderr_limit: usize,
    pub(crate) deadline: Instant,
    #[cfg(test)]
    pub(crate) guard_bypass: GitGuardBypassV1,
}

impl GitRunRequestV1 {
    pub(crate) fn new(
        command: GitCommandV1,
        stdin: Vec<u8>,
        stdout_limit: usize,
        stderr_limit: usize,
        deadline: Instant,
    ) -> Self {
        Self::with_stdin_source(
            command,
            GitStdinSourceV1::Bytes(stdin),
            stdout_limit,
            stderr_limit,
            deadline,
        )
    }

    /// Build a request whose stdin is a regular file, bounded by `max_stdin_bytes`.
    ///
    /// The writer thread is joined after the deadline path, so a descriptor whose read can block
    /// indefinitely — a FIFO, a device, a socket — would outlive the mandatory deadline however the
    /// child itself is terminated. Construction is therefore fallible and fail-closed: it `fstat`s
    /// the caller's descriptor and refuses anything that is not a regular file, and it refuses a
    /// regular file longer than the caller's explicit bound. Both refusals precede any spawn, and
    /// the writer never reads past the bound even if the file grows afterwards.
    pub(crate) fn from_file(
        command: GitCommandV1,
        stdin: File,
        max_stdin_bytes: u64,
        stdout_limit: usize,
        stderr_limit: usize,
        deadline: Instant,
    ) -> Result<Self, CustodyGitError> {
        let metadata = stdin.metadata().map_err(CustodyGitError::Stdin)?;
        let file_type = metadata.file_type();
        if !file_type.is_file() {
            return Err(CustodyGitError::StdinNotRegular {
                observed: describe_file_type(&file_type),
            });
        }
        if metadata.len() > max_stdin_bytes {
            return Err(CustodyGitError::StdinLimit {
                limit: max_stdin_bytes,
            });
        }
        Ok(Self::with_stdin_source(
            command,
            GitStdinSourceV1::File {
                file: stdin,
                max_bytes: max_stdin_bytes,
            },
            stdout_limit,
            stderr_limit,
            deadline,
        ))
    }

    fn with_stdin_source(
        command: GitCommandV1,
        stdin: GitStdinSourceV1,
        stdout_limit: usize,
        stderr_limit: usize,
        deadline: Instant,
    ) -> Self {
        Self {
            command,
            object_store: None,
            stdin,
            stdout: GitStdoutTargetV1::Capture,
            stdout_root_identity: None,
            stdout_limit,
            stderr_limit,
            deadline,
            #[cfg(test)]
            guard_bypass: GitGuardBypassV1::default(),
        }
    }

    /// Send stdout to a **new** owner-private child of the caller's pinned root and retain only
    /// its bounded stream evidence.
    ///
    /// The runner creates the target itself, through the root's retained descriptor and from a
    /// validated single component, so no caller can hand the seam a descriptor that writes outside
    /// the root it supplied. An existing entry is refused by the descriptor-relative creation.
    pub(crate) fn stream_stdout_to_new_child(
        &mut self,
        root: &PinnedDirectoryV1,
        name: &str,
    ) -> Result<(), CustodyGitError> {
        validate_component(name, "stdout target name")?;
        let file =
            root.create_new_regular_child(std::ffi::OsStr::new(name), "custody Git stdout target")?;
        self.stdout = GitStdoutTargetV1::File(file);
        self.stdout_root_identity = Some(root.identity().clone());
        Ok(())
    }
}

/// How the stdin writer and the two stream readers are scheduled around one child.
///
/// Production always uses `Concurrent`. The other three are A10's serialization mutations, kept as
/// separate values so the control discriminates each one instead of one shared branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GitIoScheduleV1 {
    Concurrent,
    StdinBeforeReaders,
    StdoutAfterStdin,
    StderrAfterExit,
}

#[cfg(test)]
thread_local! {
    static LAST_IO_SCHEDULE: RefCell<GitIoScheduleV1> =
        const { RefCell::new(GitIoScheduleV1::Concurrent) };
}

#[cfg(test)]
fn record_io_schedule_for_test(schedule: GitIoScheduleV1) {
    LAST_IO_SCHEDULE.with(|slot| *slot.borrow_mut() = schedule);
}

/// The schedule the most recent `run` on this thread actually used, so A10 can prove that its three
/// mutations take three different paths.
#[cfg(test)]
pub(crate) fn last_io_schedule_for_test() -> GitIoScheduleV1 {
    LAST_IO_SCHEDULE.with(|slot| *slot.borrow())
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct GitGuardBypassV1 {
    pub(crate) no_lazy_fetch_flag: bool,
    pub(crate) no_lazy_fetch_environment: bool,
    pub(crate) protocol_allow: bool,
    pub(crate) stdin_before_readers: bool,
    pub(crate) stdout_after_stdin: bool,
    pub(crate) stderr_after_exit: bool,
    pub(crate) add_environment_variable: bool,
    pub(crate) init_sets_git_dir: bool,
    pub(crate) init_uses_template: bool,
    pub(crate) skip_stdout_limit: bool,
    pub(crate) skip_stderr_limit: bool,
}

#[cfg(test)]
impl GitGuardBypassV1 {
    fn io_schedule(self) -> GitIoScheduleV1 {
        if self.stdin_before_readers {
            GitIoScheduleV1::StdinBeforeReaders
        } else if self.stdout_after_stdin {
            GitIoScheduleV1::StdoutAfterStdin
        } else if self.stderr_after_exit {
            GitIoScheduleV1::StderrAfterExit
        } else {
            GitIoScheduleV1::Concurrent
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GitPathDigestV1([u8; 32]);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GitObjectStoreEvidenceV1 {
    pub(crate) primary: GitPathDigestV1,
    pub(crate) alternates: Vec<GitPathDigestV1>,
}

#[derive(Clone, Debug)]
pub(crate) struct GitRouteEvidenceV1 {
    pub(crate) canonical_path: PathBuf,
    pub(crate) expected_digest: ExpectedGitDigestV1,
    pub(crate) observed_digest: [u8; 32],
    pub(crate) facts: RouteFactsV1,
    pub(crate) write_check: &'static str,
}

#[derive(Clone, Debug)]
pub(crate) struct GitStreamEvidenceV1 {
    pub(crate) length: usize,
    pub(crate) sha256: [u8; 32],
}

#[derive(Clone, Debug)]
pub(crate) struct GitRunEvidenceV1 {
    pub(crate) argv: Vec<OsString>,
    pub(crate) environment: BTreeMap<String, String>,
    pub(crate) object_store: Option<GitObjectStoreEvidenceV1>,
    pub(crate) route: GitRouteEvidenceV1,
    pub(crate) version: (u32, u32, u32),
    pub(crate) root_identity: DirectoryIdentityV1,
    pub(crate) exit_status: Option<i32>,
    pub(crate) stdout: GitStreamEvidenceV1,
    pub(crate) stderr: GitStreamEvidenceV1,
}

#[derive(Clone, Debug)]
pub(crate) enum GitStdoutV1 {
    Captured(Vec<u8>),
    Streamed(GitStreamEvidenceV1),
}

impl GitStdoutV1 {
    fn evidence(&self) -> GitStreamEvidenceV1 {
        match self {
            Self::Captured(bytes) => stream_evidence(bytes),
            Self::Streamed(evidence) => evidence.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct GitRunResultV1 {
    pub(crate) stdout: GitStdoutV1,
    pub(crate) stderr: Vec<u8>,
    pub(crate) status: ExitStatus,
    pub(crate) evidence: GitRunEvidenceV1,
}

impl GitRunResultV1 {
    pub(crate) fn captured_stdout(&self) -> Option<&[u8]> {
        match &self.stdout {
            GitStdoutV1::Captured(bytes) => Some(bytes),
            GitStdoutV1::Streamed(_) => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct GitDigestMismatchV1 {
    pub(crate) expected: ExpectedGitDigestV1,
    pub(crate) observed: [u8; 32],
    pub(crate) canonical_path: PathBuf,
    pub(crate) facts: RouteFactsV1,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CustodyGitError {
    #[error("invalid Git route: {0}")]
    InvalidRoute(String),
    #[error("invalid Git command: {0}")]
    InvalidCommand(String),
    #[error("invalid Git object-store route: {0}")]
    InvalidObjectStoreRoute(String),
    #[error("Git command {command} may not receive a caller object-store route")]
    ObjectStoreRouteRefused { command: &'static str },
    #[error("Git route refused: {0}")]
    RouteRefusal(String),
    #[error("Git route identity changed")]
    RouteIdentityChanged,
    #[error("Git digest does not match the caller pin")]
    DigestMismatch { details: Box<GitDigestMismatchV1> },
    #[error("Git binary drifted while a child ran: {0}")]
    BinaryDrift(String),
    #[error("Git version is unsupported: {0}")]
    UnsupportedVersion(String),
    #[error("Git stdout exceeded its {limit}-byte cap")]
    StdoutLimit { limit: usize },
    #[error("Git stderr exceeded its {limit}-byte cap")]
    StderrLimit { limit: usize },
    #[error("Git child exceeded its deadline")]
    Timeout,
    #[error("Git child stdin must be a regular file, not {observed}")]
    StdinNotRegular { observed: &'static str },
    #[error("Git child stdin exceeded its {limit}-byte bound")]
    StdinLimit { limit: u64 },
    #[error("Git child stdin failed: {0}")]
    Stdin(std::io::Error),
    #[error("Git child stream failed: {0}")]
    Stream(std::io::Error),
    #[error(transparent)]
    Fs(#[from] FsCustodyError),
    #[error("Git process failed to start: {0}")]
    Spawn(std::io::Error),
}

pub(crate) struct GitRunnerV1 {
    route: AdmittedGitRouteV1,
    version: (u32, u32, u32),
}

impl GitRunnerV1 {
    pub(crate) const MINIMUM_VERSION: (u32, u32, u32) = GIT_MINIMUM_VERSION;

    pub(crate) fn admit(
        request: GitRouteRequestV1,
        root: &PinnedDirectoryV1,
        names: &GitRootNamesV1,
        deadline: Instant,
    ) -> Result<Self, CustodyGitError> {
        let route = admit_route(request)?;
        let provisional = Self {
            route,
            version: GIT_MINIMUM_VERSION,
        };
        let version_result = provisional.run(
            root,
            names,
            GitRunRequestV1::new(
                GitCommandV1::Version,
                Vec::new(),
                VERSION_STDOUT_LIMIT,
                VERSION_STDERR_LIMIT,
                deadline,
            ),
            || Ok(()),
            || Ok(()),
        )?;
        if !version_result.status.success() {
            return Err(CustodyGitError::UnsupportedVersion(format!(
                "Git version exited {:?}",
                version_result.status.code()
            )));
        }
        let version = parse_git_version(
            version_result
                .captured_stdout()
                .expect("version stdout is captured"),
        )?;
        if version < GIT_MINIMUM_VERSION {
            return Err(CustodyGitError::UnsupportedVersion(format!(
                "{version:?} is below {GIT_MINIMUM_VERSION:?}"
            )));
        }
        Ok(Self {
            route: provisional.route,
            version,
        })
    }

    pub(crate) fn run<P, Q>(
        &self,
        root: &PinnedDirectoryV1,
        names: &GitRootNamesV1,
        request: GitRunRequestV1,
        before_spawn: P,
        after_exit: Q,
    ) -> Result<GitRunResultV1, CustodyGitError>
    where
        P: FnOnce() -> Result<(), CustodyGitError>,
        Q: FnOnce() -> Result<(), CustodyGitError>,
    {
        self.recheck()?;
        before_spawn()?;
        if Instant::now() >= request.deadline {
            return Err(CustodyGitError::Timeout);
        }
        let (mut command, argv, environment, object_store) = self.command(root, names, &request)?;
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // The group leader is the direct child. Its descendants retain the same group, so a
            // cap or deadline closes every inherited pipe endpoint before the I/O joins.
            .process_group(0);
        let mut child = command.spawn().map_err(CustodyGitError::Spawn)?;
        let child_stdin = child.stdin.take().expect("piped stdin is present");
        let mut stdout = Some(child.stdout.take().expect("piped stdout is present"));
        let mut stderr = Some(child.stderr.take().expect("piped stderr is present"));
        let child = Arc::new(Mutex::new(child));
        let overflow = Arc::new(AtomicBool::new(false));
        #[cfg(test)]
        let guard_bypass = request.guard_bypass;
        let GitRunRequestV1 {
            stdin,
            stdout: stdout_target,
            stdout_limit,
            stderr_limit,
            deadline,
            ..
        } = request;
        #[cfg(test)]
        let schedule = guard_bypass.io_schedule();
        #[cfg(not(test))]
        let schedule = GitIoScheduleV1::Concurrent;
        #[cfg(test)]
        record_io_schedule_for_test(schedule);
        #[cfg(test)]
        let stdout_reader_limit = if guard_bypass.skip_stdout_limit {
            usize::MAX
        } else {
            stdout_limit
        };
        #[cfg(not(test))]
        let stdout_reader_limit = stdout_limit;
        #[cfg(test)]
        let stderr_reader_limit = if guard_bypass.skip_stderr_limit {
            usize::MAX
        } else {
            stderr_limit
        };
        #[cfg(not(test))]
        let stderr_reader_limit = stderr_limit;
        let drain_complete = Arc::new(AtomicBool::new(false));
        let drain_timed_out = Arc::new(AtomicBool::new(false));
        arm_drain_deadline(
            child.clone(),
            deadline,
            drain_complete.clone(),
            drain_timed_out.clone(),
        );
        // The default schedule runs the stdin writer, the stdout reader, and the stderr reader
        // concurrently, so no full pipe can deadlock. The three `#[cfg(test)]` schedules below are
        // A10's mutations, and each one is a distinct serialization rather than a shared branch:
        // `StdinBeforeReaders` completes the caller's input before any reader exists,
        // `StdoutAfterStdin` keeps stderr concurrent but defers stdout until the writer is done, and
        // `StderrAfterExit` defers stderr until the child has been reaped.
        let mut writer = Some(thread::spawn(move || copy_input(stdin, child_stdin)));
        let mut write = None;
        let mut stdout_target = Some(stdout_target);
        let mut stdout_reader = None;
        let mut stderr_reader = None;
        if schedule == GitIoScheduleV1::StdinBeforeReaders {
            write = Some(join_writer(&mut writer)?);
        }
        if schedule != GitIoScheduleV1::StdoutAfterStdin {
            stdout_reader = Some(spawn_reader(
                stdout.take().expect("stdout reader owns its pipe"),
                stdout_reader_limit,
                stdout_target.take().expect("stdout reader owns its target"),
                child.clone(),
                overflow.clone(),
            ));
        }
        if schedule != GitIoScheduleV1::StderrAfterExit {
            stderr_reader = Some(spawn_reader(
                stderr.take().expect("stderr reader owns its pipe"),
                stderr_reader_limit,
                GitStdoutTargetV1::Capture,
                child.clone(),
                overflow.clone(),
            ));
        }
        if schedule == GitIoScheduleV1::StdoutAfterStdin {
            write = Some(join_writer(&mut writer)?);
            stdout_reader = Some(spawn_reader(
                stdout.take().expect("deferred stdout reader owns its pipe"),
                stdout_reader_limit,
                stdout_target
                    .take()
                    .expect("deferred stdout reader owns its target"),
                child.clone(),
                overflow.clone(),
            ));
        }

        let (status, timed_out) = wait_for_child(&child, &overflow, deadline)?;
        if stderr_reader.is_none() {
            stderr_reader = Some(spawn_reader(
                stderr.take().expect("deferred stderr reader owns its pipe"),
                stderr_reader_limit,
                GitStdoutTargetV1::Capture,
                child.clone(),
                overflow.clone(),
            ));
        }
        let stdout = stdout_reader
            .expect("the stdout reader is started before the child is reaped")
            .join()
            .map_err(|_| CustodyGitError::Stream(thread_panic_error()))?;
        let stderr = stderr_reader
            .expect("the stderr reader is started")
            .join()
            .map_err(|_| CustodyGitError::Stream(thread_panic_error()))?;
        let write = match write {
            Some(write) => write,
            None => join_writer(&mut writer)?,
        };
        drain_complete.store(true, Ordering::SeqCst);

        // A completed child is always followed by both custody rechecks, even when it overflowed
        // a cap or timed out, and even when the binary drifted: the caller's post-exit check is
        // how source, alternate, scratch, and work identity are evaluated and recorded for a child
        // that already ran. Both are therefore evaluated before either is reported, and detected
        // binary drift then dominates every other outcome.
        let drift = self
            .recheck()
            .map_err(|error| CustodyGitError::BinaryDrift(error.to_string()));
        let caller_check = after_exit();
        drift?;
        caller_check?;

        if timed_out || drain_timed_out.load(Ordering::SeqCst) {
            return Err(CustodyGitError::Timeout);
        }
        let stdout = match stdout {
            Ok(stream) => stream,
            Err(StreamReaderError::Limit) => {
                return Err(CustodyGitError::StdoutLimit {
                    limit: stdout_limit,
                });
            }
            Err(StreamReaderError::Io(error)) => return Err(CustodyGitError::Stream(error)),
        };
        let stderr = match stderr {
            Ok(GitStdoutV1::Captured(bytes)) => bytes,
            Ok(GitStdoutV1::Streamed(_)) => unreachable!("stderr is always captured"),
            Err(StreamReaderError::Limit) => {
                return Err(CustodyGitError::StderrLimit {
                    limit: stderr_limit,
                });
            }
            Err(StreamReaderError::Io(error)) => return Err(CustodyGitError::Stream(error)),
        };
        if let Err(error) = write {
            return Err(CustodyGitError::Stdin(error));
        }

        let evidence = GitRunEvidenceV1 {
            argv,
            environment,
            object_store,
            route: self.route_evidence(),
            version: self.version,
            root_identity: root.identity().clone(),
            exit_status: status.code(),
            stdout: stdout.evidence(),
            stderr: stream_evidence(&stderr),
        };
        Ok(GitRunResultV1 {
            stdout,
            stderr,
            status,
            evidence,
        })
    }

    fn command(
        &self,
        root: &PinnedDirectoryV1,
        names: &GitRootNamesV1,
        request: &GitRunRequestV1,
    ) -> Result<BuiltGitCommandV1, CustodyGitError> {
        if let Some(identity) = &request.stdout_root_identity {
            // The evidence record names the root this child is rooted at, so a stdout child created
            // under a different pin would misdescribe where the output went.
            if !identity.matches(root.identity()) {
                return Err(CustodyGitError::InvalidCommand(
                    "the stdout target was created under a different pinned root".into(),
                ));
            }
        }
        if request.object_store.is_some() && !request.command.permits_object_store_route() {
            return Err(CustodyGitError::ObjectStoreRouteRefused {
                command: request.command.label(),
            });
        }
        let (subcommand, include_git_dir) = request.command.arguments()?;
        #[cfg(test)]
        let (subcommand, include_git_dir) = {
            let mut subcommand = subcommand;
            let mut include_git_dir = include_git_dir;
            if matches!(request.command, GitCommandV1::InitBare { .. }) {
                include_git_dir |= request.guard_bypass.init_sets_git_dir;
                if request.guard_bypass.init_uses_template {
                    let template = subcommand
                        .iter_mut()
                        .find(|argument| *argument == "--template=")
                        .expect("InitBare has the fixed empty template argument");
                    *template = "--template=planted".into();
                }
            }
            (subcommand, include_git_dir)
        };
        let mut argv = vec!["--no-optional-locks".into(), "--no-replace-objects".into()];
        #[cfg(not(test))]
        argv.push("--no-lazy-fetch".into());
        #[cfg(test)]
        if !request.guard_bypass.no_lazy_fetch_flag {
            argv.push("--no-lazy-fetch".into());
        }
        argv.extend([
            "-c".into(),
            "core.hooksPath=/dev/null".into(),
            "-c".into(),
            "core.fsmonitor=false".into(),
        ]);
        #[cfg(not(test))]
        argv.extend(["-c".into(), "protocol.allow=never".into()]);
        #[cfg(test)]
        if !request.guard_bypass.protocol_allow {
            argv.extend(["-c".into(), "protocol.allow=never".into()]);
        }
        argv.extend(subcommand);

        let mut command = Command::new(&self.route.canonical_path);
        command.env_clear();
        let mut environment = BTreeMap::new();
        set_environment(&mut command, &mut environment, "GIT_OPTIONAL_LOCKS", "0");
        set_environment(
            &mut command,
            &mut environment,
            "GIT_NO_REPLACE_OBJECTS",
            "1",
        );
        #[cfg(not(test))]
        set_environment(&mut command, &mut environment, "GIT_NO_LAZY_FETCH", "1");
        #[cfg(test)]
        if !request.guard_bypass.no_lazy_fetch_environment {
            set_environment(&mut command, &mut environment, "GIT_NO_LAZY_FETCH", "1");
        }
        set_environment(&mut command, &mut environment, "GIT_TERMINAL_PROMPT", "0");
        set_environment(&mut command, &mut environment, "GIT_CONFIG_NOSYSTEM", "1");
        set_environment(
            &mut command,
            &mut environment,
            "GIT_CONFIG_GLOBAL",
            "/dev/null",
        );
        set_environment(&mut command, &mut environment, "HOME", &names.home);
        set_environment(
            &mut command,
            &mut environment,
            "XDG_CONFIG_HOME",
            &names.xdg_config_home,
        );
        if include_git_dir {
            set_environment(&mut command, &mut environment, "GIT_DIR", &names.git_dir);
        }
        let object_store_evidence = request
            .object_store
            .as_ref()
            .map(GitObjectStoreRouteV1::evidence);
        if let Some(route) = &request.object_store {
            command.env("GIT_OBJECT_DIRECTORY", &route.primary);
            command.env("GIT_ALTERNATE_OBJECT_DIRECTORIES", route.alternate_value());
            environment.insert(
                "GIT_OBJECT_DIRECTORY".into(),
                "redacted:git-object-dir".into(),
            );
            environment.insert(
                "GIT_ALTERNATE_OBJECT_DIRECTORIES".into(),
                "redacted:git-alternate-dir".into(),
            );
        }
        set_environment(&mut command, &mut environment, "PATH", FIXED_PATH);
        set_environment(&mut command, &mut environment, "LC_ALL", "C");
        set_environment(&mut command, &mut environment, "LANG", "C");
        #[cfg(test)]
        if request.guard_bypass.add_environment_variable {
            set_environment(
                &mut command,
                &mut environment,
                "A2A_GIT_TEST_INHERITED",
                "1",
            );
        }
        command.args(&argv);
        root.root_command(&mut command, "custody Git rooted command")?;
        Ok((command, argv, environment, object_store_evidence))
    }

    fn recheck(&self) -> Result<(), CustodyGitError> {
        let (file, facts) = open_route_file(&self.route.canonical_path)?;
        let _write_check = audit_and_bind(
            &self.route.canonical_path,
            &file,
            facts,
            &self.route.profile,
        )?;
        if !same_route_facts(
            &facts_for_recheck(facts, self.route.facts),
            &self.route.facts,
        ) {
            return Err(CustodyGitError::RouteIdentityChanged);
        }
        let observed = digest_file(file)?;
        if observed != self.route.expected_digest.bytes() && !recheck_digest_bypassed() {
            return Err(CustodyGitError::DigestMismatch {
                details: Box::new(GitDigestMismatchV1 {
                    expected: self.route.expected_digest,
                    observed,
                    canonical_path: self.route.canonical_path.clone(),
                    facts,
                }),
            });
        }
        Ok(())
    }

    fn route_evidence(&self) -> GitRouteEvidenceV1 {
        GitRouteEvidenceV1 {
            canonical_path: self.route.canonical_path.clone(),
            expected_digest: self.route.expected_digest,
            observed_digest: self.route.observed_digest,
            facts: self.route.facts,
            write_check: match self.route.write_check {
                WriteCheckV1::Faccessat => "faccessat_at_eaccess",
                WriteCheckV1::ModeBitsAsRoot => "mode_bits_as_root",
            },
        }
    }
}

fn admit_route(request: GitRouteRequestV1) -> Result<AdmittedGitRouteV1, CustodyGitError> {
    let supplied = std::fs::symlink_metadata(&request.path).map_err(|error| {
        CustodyGitError::RouteRefusal(format!("cannot lstat supplied Git route: {error}"))
    })?;
    if supplied.file_type().is_symlink() && !final_symlink_refusal_bypassed() {
        return Err(CustodyGitError::RouteRefusal(
            "supplied Git route has a final symlink".into(),
        ));
    }
    let canonical_path = request.path.canonicalize().map_err(|error| {
        CustodyGitError::RouteRefusal(format!("cannot canonicalize Git route: {error}"))
    })?;
    let (file, facts) = open_route_file(&canonical_path)?;
    let write_check = audit_and_bind(&canonical_path, &file, facts, &request.profile)?;
    let observed_digest = digest_file(file)?;
    if observed_digest != request.expected_digest.bytes() {
        return Err(CustodyGitError::DigestMismatch {
            details: Box::new(GitDigestMismatchV1 {
                expected: request.expected_digest,
                observed: observed_digest,
                canonical_path,
                facts,
            }),
        });
    }
    Ok(AdmittedGitRouteV1 {
        canonical_path,
        facts,
        expected_digest: request.expected_digest,
        observed_digest,
        write_check,
        profile: request.profile,
    })
}

fn open_route_file(path: &Path) -> Result<(File, RouteFactsV1), CustodyGitError> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let file = options.open(path).map_err(|error| {
        CustodyGitError::RouteRefusal(format!("cannot open Git route: {error}"))
    })?;
    let metadata = file.metadata().map_err(|error| {
        CustodyGitError::RouteRefusal(format!("cannot fstat Git route: {error}"))
    })?;
    if !metadata.is_file() {
        return Err(CustodyGitError::RouteRefusal(
            "Git route is not a regular file".into(),
        ));
    }
    Ok((file, RouteFactsV1::from_metadata(&metadata)))
}

fn audit_and_bind(
    canonical_path: &Path,
    file: &File,
    descriptor_facts: RouteFactsV1,
    profile: &GitAdmissionProfileV1,
) -> Result<WriteCheckV1, CustodyGitError> {
    let effective_uid = effective_uid();
    if matches!(profile, GitAdmissionProfileV1::Production) && effective_uid == 0 {
        return Err(CustodyGitError::RouteRefusal(
            "the production Git runner refuses uid 0".into(),
        ));
    }
    let trusted_uid = match profile {
        GitAdmissionProfileV1::Production => 0,
        #[cfg(test)]
        GitAdmissionProfileV1::TestSystem => 0,
        #[cfg(test)]
        GitAdmissionProfileV1::TestFixture { .. } => effective_uid,
    };
    let anchor: Option<PathBuf> = match profile {
        #[cfg(test)]
        GitAdmissionProfileV1::TestFixture { trust_anchor } => {
            Some(trust_anchor.canonicalize().map_err(|error| {
                CustodyGitError::RouteRefusal(format!(
                    "cannot canonicalize fixture anchor: {error}"
                ))
            })?)
        }
        #[cfg(test)]
        _ => None,
        #[cfg(not(test))]
        GitAdmissionProfileV1::Production => None,
    };
    if let Some(anchor) = anchor.as_deref() {
        if !canonical_path.starts_with(anchor) {
            return Err(CustodyGitError::RouteRefusal(
                "fixture Git route is outside its trust anchor".into(),
            ));
        }
    }
    let mut current = canonical_path.to_path_buf();
    let mut recorded_write_check = None;
    loop {
        let metadata = std::fs::symlink_metadata(&current).map_err(|error| {
            CustodyGitError::RouteRefusal(format!("cannot inspect Git route component: {error}"))
        })?;
        if !route_component_owner_is_trusted(metadata.uid(), trusted_uid) {
            return Err(CustodyGitError::RouteRefusal(format!(
                "Git route component is not owned by trusted uid {trusted_uid}"
            )));
        }
        let write_check = deny_effective_write(&current, &metadata, effective_uid)?;
        recorded_write_check.get_or_insert(write_check);
        if anchor.as_deref() == Some(current.as_path()) || current == Path::new("/") {
            break;
        }
        current = current
            .parent()
            .ok_or_else(|| {
                CustodyGitError::RouteRefusal("Git route has no filesystem root".into())
            })?
            .to_path_buf();
    }
    #[cfg(test)]
    run_route_audit_hook(canonical_path);
    let path_metadata = std::fs::symlink_metadata(canonical_path).map_err(|error| {
        CustodyGitError::RouteRefusal(format!("cannot bind Git route path: {error}"))
    })?;
    let path_facts = RouteFactsV1::from_metadata(&path_metadata);
    let file_facts = RouteFactsV1::from_metadata(&file.metadata().map_err(|error| {
        CustodyGitError::RouteRefusal(format!("cannot re-fstat Git route: {error}"))
    })?);
    if !same_route_facts(&path_facts, &descriptor_facts)
        || !same_route_facts(&file_facts, &descriptor_facts)
    {
        return Err(CustodyGitError::RouteIdentityChanged);
    }
    Ok(recorded_write_check.expect("route has a final component"))
}

fn route_component_owner_is_trusted(owner: u32, trusted_uid: u32) -> bool {
    owner == trusted_uid
}

#[cfg(test)]
pub(crate) fn route_component_owner_is_trusted_for_test(owner: u32, trusted_uid: u32) -> bool {
    route_component_owner_is_trusted(owner, trusted_uid)
}

/// Uses the same root-mode or `faccessat(AT_EACCESS)` boundary as admission. For an ordinary
/// user this reports whether ACL/group/other access now permits a write to the audited component.
#[cfg(test)]
pub(crate) fn effective_write_is_permitted_for_test(path: &Path) -> Result<bool, CustodyGitError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        CustodyGitError::RouteRefusal(format!("cannot inspect test route component: {error}"))
    })?;
    Ok(deny_effective_write(path, &metadata, effective_uid()).is_err())
}

#[cfg(test)]
pub(crate) fn running_as_root_for_test() -> bool {
    effective_uid() == 0
}

fn deny_effective_write(
    path: &Path,
    metadata: &std::fs::Metadata,
    effective_uid: u32,
) -> Result<WriteCheckV1, CustodyGitError> {
    if effective_uid == 0 {
        if metadata.mode() & 0o022 != 0 {
            return Err(CustodyGitError::RouteRefusal(
                "root test profile found group or other writable Git route component".into(),
            ));
        }
        return Ok(WriteCheckV1::ModeBitsAsRoot);
    }
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| CustodyGitError::RouteRefusal("Git route path contains a NUL byte".into()))?;
    // SAFETY: `path` is a live NUL-terminated path and `faccessat` does not retain it. The
    // effective-access flag is required so group membership and ACLs participate in the check.
    let result =
        unsafe { libc::faccessat(libc::AT_FDCWD, path.as_ptr(), libc::W_OK, libc::AT_EACCESS) };
    if result == 0 {
        return Err(CustodyGitError::RouteRefusal(
            "executing user can write a Git route component".into(),
        ));
    }
    classify_write_denial(std::io::Error::last_os_error())
}

/// Classifies a failed `faccessat(W_OK)` probe. `EACCES`, `EROFS`, and `EPERM` each prove the
/// executing user cannot write the component: macOS reports `EROFS` for `/` on the sealed system
/// volume and `EPERM` for SIP-protected paths. Any other errno leaves writability unproven, so the
/// route is refused.
pub(crate) fn classify_write_denial(
    error: std::io::Error,
) -> Result<WriteCheckV1, CustodyGitError> {
    match error.raw_os_error() {
        Some(libc::EACCES | libc::EROFS | libc::EPERM) => Ok(WriteCheckV1::Faccessat),
        _ => Err(CustodyGitError::RouteRefusal(format!(
            "cannot check effective write access: {error}"
        ))),
    }
}

fn effective_uid() -> u32 {
    // SAFETY: `geteuid` has no preconditions and simply reads this process's effective uid.
    unsafe { libc::geteuid() }
}

fn digest_file(mut file: File) -> Result<[u8; 32], CustodyGitError> {
    let mut context = digest::Context::new(&digest::SHA256);
    let mut buffer = [0_u8; 32 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(CustodyGitError::Stream)?;
        if count == 0 {
            return Ok(context.finish().as_ref().try_into().expect("SHA-256 size"));
        }
        context.update(&buffer[..count]);
    }
}

/// Test-only classifier retained here so the 2B2 ext4 lane can reuse the exact admission rule.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ext4AdmissionV1 {
    Admitted,
    NotExt4Superblock,
    MissingMount,
    WrongFilesystemType,
    AmbiguousMount,
    MalformedMountInfo,
}

/// Classify a retained descriptor's Linux ext4 admission from its already-observed superblock
/// magic and device major/minor pair. The caller performs `fstatfs` separately; this pure parser
/// keeps mountinfo fixture coverage deterministic.
#[cfg(test)]
pub(crate) fn classify_ext4_admission_for_test(
    filesystem_magic: libc::c_long,
    device: (u32, u32),
    mountinfo: &str,
) -> Ext4AdmissionV1 {
    if filesystem_magic != 0xEF53 {
        return Ext4AdmissionV1::NotExt4Superblock;
    }
    let mut matched_types = Vec::new();
    for line in mountinfo.lines() {
        // The kernel writes exactly one `" - "` separator per line: every path-shaped field escapes
        // whitespace as `\040`, so a second separator means this text was not produced by
        // `/proc/<pid>/mountinfo` and no part of it is admissible lane evidence.
        let mut halves = line.split(" - ");
        let (Some(left), Some(right), None) = (halves.next(), halves.next(), halves.next()) else {
            return Ext4AdmissionV1::MalformedMountInfo;
        };
        // Left-hand side: mount ID, parent ID, `major:minor`, root, mount point, and mount options
        // are all mandatory. Any number of optional propagation fields may follow them.
        let left_fields: Vec<&str> = left.split_whitespace().collect();
        let [mount_id, parent_id, major_minor, _root, _mount_point, _options, ..] =
            left_fields.as_slice()
        else {
            return Ext4AdmissionV1::MalformedMountInfo;
        };
        let (Ok(_mount_id), Ok(_parent_id)) = (mount_id.parse::<u64>(), parent_id.parse::<u64>())
        else {
            return Ext4AdmissionV1::MalformedMountInfo;
        };
        let Some((major, minor)) = major_minor.split_once(':') else {
            return Ext4AdmissionV1::MalformedMountInfo;
        };
        let (Ok(major), Ok(minor)) = (major.parse::<u32>(), minor.parse::<u32>()) else {
            return Ext4AdmissionV1::MalformedMountInfo;
        };
        // Right-hand side: filesystem type, mount source, and super options, and nothing else.
        let right_fields: Vec<&str> = right.split_whitespace().collect();
        let [filesystem_type, _source, _super_options] = right_fields.as_slice() else {
            return Ext4AdmissionV1::MalformedMountInfo;
        };
        if (major, minor) == device {
            matched_types.push(*filesystem_type);
        }
    }
    match matched_types.as_slice() {
        [] => Ext4AdmissionV1::MissingMount,
        ["ext4"] => Ext4AdmissionV1::Admitted,
        [_] => Ext4AdmissionV1::WrongFilesystemType,
        _ => Ext4AdmissionV1::AmbiguousMount,
    }
}

pub(crate) fn parse_git_version(bytes: &[u8]) -> Result<(u32, u32, u32), CustodyGitError> {
    let text = std::str::from_utf8(bytes).map_err(|_| {
        CustodyGitError::UnsupportedVersion("Git version output was not UTF-8".into())
    })?;
    let token = text
        .trim()
        .strip_prefix("git version ")
        .ok_or_else(|| CustodyGitError::UnsupportedVersion("malformed Git version output".into()))?
        .split_whitespace()
        .next()
        .ok_or_else(|| CustodyGitError::UnsupportedVersion("missing Git version".into()))?;
    let numeric: String = token
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect();
    let mut parts = numeric.split('.');
    let parse_part = |part: Option<&str>| {
        part.filter(|part| !part.is_empty())
            .ok_or_else(|| CustodyGitError::UnsupportedVersion("malformed Git version".into()))?
            .parse::<u32>()
            .map_err(|_| CustodyGitError::UnsupportedVersion("malformed Git version".into()))
    };
    let major = parse_part(parts.next())?;
    let minor = parse_part(parts.next())?;
    let patch = match parts.next() {
        Some(value) => parse_part(Some(value))?,
        None => 0,
    };
    if parts.next().is_some() {
        return Err(CustodyGitError::UnsupportedVersion(
            "malformed Git version".into(),
        ));
    }
    Ok((major, minor, patch))
}

fn validate_component(value: &str, label: &str) -> Result<(), CustodyGitError> {
    if value.is_empty() || value == "." || value == ".." || value.contains(['/', '\\', '\0']) {
        return Err(CustodyGitError::InvalidCommand(format!(
            "{label} is not one relative component"
        )));
    }
    // A component that reaches the fixed argv as a positional operand must not be able to become a
    // Git option instead. `InitBare { dir: "-q" }` would otherwise leave `init` with no directory
    // operand and initialize the rooted cwd. The closed argv table has no `--` terminator, so the
    // refusal is here rather than in each variant.
    if value.starts_with('-') {
        return Err(CustodyGitError::InvalidCommand(format!(
            "{label} must not begin with '-'"
        )));
    }
    Ok(())
}

fn validate_object_store_path(path: &Path) -> Result<(), CustodyGitError> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.is_empty() || !path.is_absolute() {
        return Err(CustodyGitError::InvalidObjectStoreRoute(
            "path must be absolute and non-empty".into(),
        ));
    }
    if bytes
        .iter()
        .any(|byte| matches!(byte, b':' | b'"' | b'\\' | b'\n' | 0))
    {
        return Err(CustodyGitError::InvalidObjectStoreRoute(
            "path contains a prohibited alternate-list byte".into(),
        ));
    }
    Ok(())
}

fn digest_path(domain: &[u8], bytes: &[u8]) -> GitPathDigestV1 {
    let mut context = digest::Context::new(&digest::SHA256);
    context.update(domain);
    context.update(&(bytes.len() as u64).to_be_bytes());
    context.update(bytes);
    GitPathDigestV1(context.finish().as_ref().try_into().expect("SHA-256 size"))
}

fn set_environment(
    command: &mut Command,
    evidence: &mut BTreeMap<String, String>,
    key: &str,
    value: &str,
) {
    command.env(key, value);
    evidence.insert(key.to_owned(), value.to_owned());
}

enum StreamReaderError {
    Limit,
    Io(std::io::Error),
}

fn copy_input(
    source: GitStdinSourceV1,
    mut child_stdin: std::process::ChildStdin,
) -> std::io::Result<u64> {
    match source {
        GitStdinSourceV1::Bytes(bytes) => {
            child_stdin.write_all(&bytes)?;
            Ok(bytes.len() as u64)
        }
        GitStdinSourceV1::File { file, max_bytes } => {
            // `take` is the read bound itself: the writer never reads a byte past the caller's
            // limit, so a descriptor that grew after its `fstat` cannot extend the child's input.
            let mut bounded = file.take(max_bytes);
            let copied = std::io::copy(&mut bounded, &mut child_stdin)?;
            // Re-`fstat` rather than read further. A file that outgrew its bound would have been
            // silently truncated, which corrupts a pack instead of refusing it.
            if bounded.into_inner().metadata()?.len() > max_bytes {
                return Err(std::io::Error::other(format!(
                    "stdin grew past its {max_bytes}-byte bound while the child ran"
                )));
            }
            Ok(copied)
        }
    }
}

/// Name the `fstat` type a refused stdin descriptor actually had, so the typed refusal says what
/// the caller handed the seam.
fn describe_file_type(file_type: &std::fs::FileType) -> &'static str {
    use std::os::unix::fs::FileTypeExt as _;

    if file_type.is_dir() {
        "directory"
    } else if file_type.is_symlink() {
        "symbolic link"
    } else if file_type.is_fifo() {
        "fifo"
    } else if file_type.is_socket() {
        "socket"
    } else if file_type.is_char_device() {
        "character device"
    } else if file_type.is_block_device() {
        "block device"
    } else {
        "a non-regular file"
    }
}

/// Join the stdin writer exactly once, whatever the schedule, and keep its I/O result for the
/// caller-facing error precedence below.
fn join_writer(
    writer: &mut Option<thread::JoinHandle<std::io::Result<u64>>>,
) -> Result<std::io::Result<u64>, CustodyGitError> {
    writer
        .take()
        .expect("the stdin writer is joined once")
        .join()
        .map_err(|_| CustodyGitError::Stdin(thread_panic_error()))
}

fn spawn_reader<R: std::io::Read + Send + 'static>(
    mut stream: R,
    limit: usize,
    target: GitStdoutTargetV1,
    child: Arc<Mutex<Child>>,
    overflow: Arc<AtomicBool>,
) -> thread::JoinHandle<Result<GitStdoutV1, StreamReaderError>> {
    thread::spawn(move || {
        let (mut captured, mut destination) = match target {
            GitStdoutTargetV1::Capture => (Some(Vec::new()), None),
            GitStdoutTargetV1::File(file) => (None, Some(file)),
        };
        let mut evidence = digest::Context::new(&digest::SHA256);
        let mut length = 0_usize;
        let mut buffer = [0_u8; 8192];
        loop {
            let count = match stream.read(&mut buffer) {
                Ok(count) => count,
                Err(error) => return abort_reader(&child, &overflow, StreamReaderError::Io(error)),
            };
            if count == 0 {
                return Ok(match captured {
                    Some(bytes) => GitStdoutV1::Captured(bytes),
                    None => GitStdoutV1::Streamed(GitStreamEvidenceV1 {
                        length,
                        sha256: evidence.finish().as_ref().try_into().expect("SHA-256 size"),
                    }),
                });
            }
            if length.saturating_add(count) > limit {
                return abort_reader(&child, &overflow, StreamReaderError::Limit);
            }
            if let Some(file) = destination.as_mut() {
                if let Err(error) = file.write_all(&buffer[..count]) {
                    return abort_reader(&child, &overflow, StreamReaderError::Io(error));
                }
            } else if let Some(bytes) = captured.as_mut() {
                bytes.extend_from_slice(&buffer[..count]);
            }
            length += count;
            evidence.update(&buffer[..count]);
        }
    })
}

fn abort_reader<T>(
    child: &Arc<Mutex<Child>>,
    overflow: &AtomicBool,
    error: StreamReaderError,
) -> Result<T, StreamReaderError> {
    overflow.store(true, Ordering::SeqCst);
    if let Ok(mut child) = child.lock() {
        let _ = signal_process_group(&mut child, libc::SIGTERM);
    }
    Err(error)
}

fn arm_drain_deadline(
    child: Arc<Mutex<Child>>,
    deadline: Instant,
    complete: Arc<AtomicBool>,
    timed_out: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        while Instant::now() < deadline {
            if complete.load(Ordering::SeqCst) {
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
        if complete.load(Ordering::SeqCst) {
            return;
        }
        timed_out.store(true, Ordering::SeqCst);
        if let Ok(mut child) = child.lock() {
            let _ = terminate_process_group(&mut child);
        }
    });
}

fn wait_for_child(
    child: &Arc<Mutex<Child>>,
    overflow: &AtomicBool,
    deadline: Instant,
) -> Result<(ExitStatus, bool), CustodyGitError> {
    loop {
        let status = child
            .lock()
            .map_err(|_| CustodyGitError::Stream(thread_panic_error()))?
            .try_wait()
            .map_err(CustodyGitError::Stream)?;
        if let Some(status) = status {
            return Ok((status, false));
        }
        if overflow.load(Ordering::SeqCst) || Instant::now() >= deadline {
            let mut child = child
                .lock()
                .map_err(|_| CustodyGitError::Stream(thread_panic_error()))?;
            let status = terminate_process_group(&mut child).map_err(CustodyGitError::Stream)?;
            return Ok((status, !overflow.load(Ordering::SeqCst)));
        }
        thread::sleep(Duration::from_millis(1));
    }
}

fn signal_process_group(child: &mut Child, signal: i32) -> std::io::Result<()> {
    let pgid = i32::try_from(child.id()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "child pid is invalid")
    })?;
    // `SystemProcessAuthorityV1` owns the audited Unix group-signal syscall. The process group
    // was created above with `process_group(0)`, so the direct child pid is this runner's group
    // leader and every inherited pipe holder is in the same group.
    // macOS answers a group signal with `EPERM`, not `ESRCH`, while the group leader is exiting: a
    // process that has begun exit, or an unreaped zombie, cannot be signalled, and `waitpid` may not
    // report the leader for a moment. When the leader has exited, the signal is moot. A transient
    // `EPERM` is retried for a bounded window; a persistent one is reported.
    let retry_until = Instant::now() + Duration::from_millis(100);
    loop {
        let outcome = SystemProcessAuthorityV1.signal_process_group(pgid as u32, signal);
        if outcome.return_code == 0 || outcome.errno == Some(libc::ESRCH) {
            return Ok(());
        }
        if outcome.errno != Some(libc::EPERM) {
            return Err(std::io::Error::from_raw_os_error(
                outcome.errno.unwrap_or(libc::EIO),
            ));
        }
        // `Child` caches a reaped status, so a later `try_wait`/`wait` still returns it.
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        if Instant::now() >= retry_until {
            return Err(std::io::Error::from_raw_os_error(libc::EPERM));
        }
        thread::sleep(Duration::from_millis(1));
    }
}

pub(crate) fn terminate_process_group(child: &mut Child) -> std::io::Result<ExitStatus> {
    signal_process_group(child, libc::SIGTERM)?;
    let grace_deadline = Instant::now() + Duration::from_millis(25);
    let mut exited = None;
    while Instant::now() < grace_deadline {
        if let Some(status) = child.try_wait()? {
            exited = Some(status);
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    // A reaped group leader does not prove that a helper which inherited the pipes has gone away.
    // Kill the full group after the grace window even if `try_wait` already observed the leader.
    signal_process_group(child, libc::SIGKILL)?;
    match exited {
        Some(status) => Ok(status),
        None => child.wait(),
    }
}

fn stream_evidence(bytes: &[u8]) -> GitStreamEvidenceV1 {
    GitStreamEvidenceV1 {
        length: bytes.len(),
        sha256: digest::digest(&digest::SHA256, bytes)
            .as_ref()
            .try_into()
            .expect("SHA-256 size"),
    }
}

fn thread_panic_error() -> std::io::Error {
    std::io::Error::other("custody Git I/O thread panicked")
}
