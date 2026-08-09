//! Descriptor-relative filesystem custody primitives — the single owner of the workspace's
//! raw custody syscalls (R2f1b A4).
//!
//! Two layers live here, deliberately separated:
//!
//! 1. **Mechanism** (the `pub` free functions near the bottom): validated single-component child
//!    names, the no-follow directory/child opens, the no-follow child stat, the atomic
//!    no-replace rename, open-object identity comparison, and the failure-injection countdown.
//!    These return bare [`std::io::Error`] (or a small typed refusal) and carry *no* message
//!    vocabulary, precisely so each caller keeps its own operator-facing error text byte for
//!    byte. The binary's `local_file` module is the second caller: its `PinnedDirectory` keeps
//!    all of its own policy (durable object fingerprints, session-cwd binding, quarantine,
//!    replacement, bounded readers) and reaches through these functions for the syscalls.
//! 2. **Policy** ([`PinnedDirectoryV1`] and the `verify_*`/`VerifiedRemovalV1` boundary): the
//!    custody contract used by R2f1b and by both storage reapers.

use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::ffi::CStr;
#[cfg(unix)]
use std::ffi::CString;
use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

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
    /// The mirror image of [`Self::TargetExists`], produced only by the REPLACE primitive: a
    /// replacement was asked to overwrite a record that is not there. `replace` and `publish` are
    /// separately named operations so this is caller error, not a state to recover from.
    #[error("{0}: target does not exist")]
    TargetMissing(String),
    #[error("{0}: injected sync failure")]
    InjectedSync(String),
}

/// The outcome of a custody publication — shared by the no-replace
/// ([`PinnedDirectoryV1::publish_new_regular_child`]) and replacing
/// ([`PinnedDirectoryV1::replace_regular_child`]) primitives.
///
/// One type for both on purpose. The two operations differ only in their rename flag, and after
/// the rename their outcome lattices are identical; 2b2's writer performs a no-replace publish
/// (`ProtectionPrepared`) and then a series of replaces (`Materializing`, `LiveProtected`, …), so
/// two structurally identical enums would be a footgun rather than a distinction.
///
/// Both need a richer answer than `Result<(), _>`, and the split is the load-bearing part.
/// Stated as exactly what each arm PROVES, and no more:
///
/// * `Err(FsCustodyError)` — the rename **provably did not happen**, established either because
///   the operation refused before reaching the rename, or because post-error verification found
///   the staged source name still present AND still identical to the object the caller handed in.
///   Whatever occupied the target name before is intact and is still authoritative — for a
///   no-replace publication that includes an ordinary `EEXIST`, where another actor published
///   first and this refusal is exactly the expected, correct answer.
/// * `Ok(_)` — the rename **did happen**, established either by the syscall succeeding or by
///   post-error verification finding the target identical to the caller's object — EXCEPT for
///   [`Self::RenameOutcomeUnverified`], which proves nothing in either direction. Nothing after
///   the rename may be reported as an `Err`, because a `?`-using caller would read that as "no
///   effect" and could retry a rename whose source name no longer exists, or keep treating a
///   superseded record as current.
///
/// Within `Ok`, only [`Self::Durable`] is a clean success. Every other arm is **protective**: no
/// caller may treat one as either success or failure — in R2f1b terms they resolve to "unknown",
/// and unknown never licenses deletion (focused boundary §5.7, "Claim renamed, parent sync
/// ambiguous").
///
/// **Why an errno is not evidence.** A failing `renameat` does not prove the rename did not
/// happen. On a network filesystem a retried RPC can perform the rename and then report a
/// failure: the server completed the first request, the reply was lost, and the retry finds the
/// source already gone. Every arm below therefore rests on a descriptor-level identity
/// comparison, never on the syscall's return value alone.
#[must_use = "a custody publication outcome must be classified: the ambiguous arms are protective \
              and must not be discarded as if the publication were durable"]
#[derive(Debug)]
pub enum CustodyPublicationV1 {
    /// Renamed, parent-synced, and the target reopened as the very object the caller published.
    ///
    /// `retried_rename` is `Some(detail)` when the rename syscall reported an error and
    /// post-error verification proved the effect anyway. That does not weaken the durability
    /// claim — it rests on the same identity comparison and the same parent sync as an uneventful
    /// replacement — but it is retained because it is the operator's only signal that the
    /// filesystem is answering error-after-effect.
    Durable { retried_rename: Option<String> },
    /// Renamed, but the parent directory sync did not complete. Whether a crash now leaves the
    /// new record or the old one is unknown.
    ParentSyncAmbiguous(String),
    /// Renamed and parent-synced, but the target could not be reopened, or reopened as a
    /// different object. Some other actor is writing this name; what is durable is unknown.
    TargetIdentityUnverified(String),
    /// The rename syscall reported an error and verification could not decide whether it took
    /// effect: the staged source name is not provably still the caller's object, and the target
    /// is not provably the caller's object either. Neither "the previous record survived" nor
    /// "the replacement landed" is licensed.
    RenameOutcomeUnverified(String),
}

impl CustodyPublicationV1 {
    /// True only for the one arm that attests a durable replacement.
    #[must_use]
    pub fn is_durable(&self) -> bool {
        matches!(self, Self::Durable { .. })
    }

    /// `Some(detail)` for every protective arm; `None` only for [`Self::Durable`]. Callers that
    /// must fail closed can branch on this single predicate without matching every arm, so a
    /// later arm added here is protective by default rather than by remembering to handle it.
    #[must_use]
    pub fn ambiguity(&self) -> Option<&str> {
        match self {
            Self::Durable { .. } => None,
            Self::ParentSyncAmbiguous(detail)
            | Self::TargetIdentityUnverified(detail)
            | Self::RenameOutcomeUnverified(detail) => Some(detail),
        }
    }
}

/// What an injected replace-rename fault does to the filesystem before it reports its error.
///
/// The three shapes exist because the post-error verification step has three genuinely different
/// inputs to distinguish, and no ordinary fault seam can produce them: an errno alone says nothing
/// about what happened, so the seam has to say what happened *and* return the error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicationRenameFaultV1 {
    /// Report an error without touching anything — the ordinary refusal (bad name, full disk).
    BeforeEffect,
    /// Perform the rename, then report an error. Models a network filesystem's retried RPC.
    AfterEffect,
    /// Unlink the staged source and report an error, leaving the target untouched. Models the
    /// undecidable shape: the source name is gone, but the target is not the caller's object.
    UnlinkSourceOnly,
}

impl PublicationRenameFaultV1 {
    fn encode(self) -> u8 {
        match self {
            Self::BeforeEffect => 0,
            Self::AfterEffect => 1,
            Self::UnlinkSourceOnly => 2,
        }
    }

    fn decode(raw: u8) -> Self {
        match raw {
            1 => Self::AfterEffect,
            2 => Self::UnlinkSourceOnly,
            _ => Self::BeforeEffect,
        }
    }
}

/// Whether the rename underlying a replacement took effect — the internal three-way answer
/// `replace_regular_child` needs, and the reason the pre-rename half can no longer return a bare
/// `Result<(), _>`.
#[derive(Debug)]
enum RenameCommitV1 {
    /// The rename took effect. `syscall_error` is `Some` when that was established by post-error
    /// verification rather than by the syscall succeeding.
    Committed { syscall_error: Option<String> },
    /// The rename reported an error and verification could not decide whether it took effect.
    Unverifiable(String),
}

/// A single-use "fail on the Nth call" hook, shared by every custody failure-injection point.
///
/// Armed with `n`, the next `n - 1` calls to [`Self::fire_if_due`] answer `false` and the `n`th
/// answers `true`, after which the countdown is disarmed. The compare-exchange loop makes that
/// true under concurrent callers: exactly one caller ever observes the firing transition.
///
/// This is the *mechanism* only. Which failure it injects, whether it is compiled at all, and
/// what error it produces are the owning layer's decisions — `bridge-core` compiles its hook
/// unconditionally (the operational surface has no `cfg(test)` callers to key off), while the
/// binary's `local_file` keeps its two hooks `#[cfg(test)]`-gated.
#[derive(Debug, Default)]
pub struct FailureCountdownV1 {
    remaining: AtomicUsize,
}

impl FailureCountdownV1 {
    #[must_use]
    pub fn new() -> Self {
        Self {
            remaining: AtomicUsize::new(0),
        }
    }

    /// Arm the countdown to fire on the `call`th subsequent [`Self::fire_if_due`].
    ///
    /// # Panics
    /// Panics when `call` is zero: "fail on the zeroth call" has no meaning, and silently
    /// treating it as disarmed would make a mis-written test pass for the wrong reason.
    pub fn arm(&self, call: usize) {
        assert!(call > 0, "failure injection call must be positive");
        self.remaining.store(call, Ordering::SeqCst);
    }

    /// Consume one call; answer whether this is the one that must fail.
    pub fn fire_if_due(&self) -> bool {
        let mut remaining = self.remaining.load(Ordering::SeqCst);
        while remaining != 0 {
            match self.remaining.compare_exchange(
                remaining,
                remaining - 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return remaining == 1,
                Err(observed) => remaining = observed,
            }
        }
        false
    }
}

pub struct PinnedDirectoryV1 {
    file: File,
    canonical_path: PathBuf,
    identity: DirectoryIdentityV1,
    sync_failure_countdown: FailureCountdownV1,
    publication_rename_failure_countdown: FailureCountdownV1,
    publication_rename_failure_shape: AtomicU8,
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
            sync_failure_countdown: FailureCountdownV1::new(),
            publication_rename_failure_countdown: FailureCountdownV1::new(),
            publication_rename_failure_shape: AtomicU8::new(0),
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
        if self.sync_failure_countdown.fire_if_due() {
            return Err(FsCustodyError::InjectedSync(label.to_owned()));
        }
        self.file
            .sync_all()
            .map_err(|error| FsCustodyError::Io(label.to_owned(), error))
    }

    pub fn sync_journal_recovery_barrier(&self, label: &str) -> Result<(), FsCustodyError> {
        self.sync(label)
    }

    pub fn fail_sync_on_nth_call_for_test(&self, call: usize) {
        self.sync_failure_countdown.arm(call);
    }

    /// Arm the publication-rename fault. Unlike the sync hook, this seam must state what the
    /// filesystem DID as well as what it reported: an errno carries no information about whether
    /// the rename took effect, which is precisely the condition the post-error verification in
    /// [`Self::publish_new_regular_child`] / [`Self::replace_regular_child`] exists to resolve.
    ///
    /// ONE countdown for BOTH primitives: publishes and replaces on this directory decrement the
    /// same counter, so "fail on the Nth call" counts them together. A caller arming call N in a
    /// publish-then-replace sequence (2b2's writer shape) must count every rename in between.
    pub fn fail_publication_rename_on_nth_call_for_test(
        &self,
        call: usize,
        shape: PublicationRenameFaultV1,
    ) {
        self.publication_rename_failure_shape
            .store(shape.encode(), Ordering::SeqCst);
        self.publication_rename_failure_countdown.arm(call);
    }

    fn armed_publication_rename_fault(&self) -> Option<PublicationRenameFaultV1> {
        self.publication_rename_failure_countdown
            .fire_if_due()
            .then(|| {
                PublicationRenameFaultV1::decode(
                    self.publication_rename_failure_shape.load(Ordering::SeqCst),
                )
            })
    }

    pub fn open_regular_file(&self, name: &OsStr, label: &str) -> Result<File, FsCustodyError> {
        open_regular_child(&self.file, name, label)
    }

    /// Does a directory entry of ANY kind exist at `name` beneath this pinned directory?
    ///
    /// Descriptor-relative and no-follow, so a dangling symlink at `name` answers `true`: the
    /// question is whether the NAME is taken, not whether it resolves. Deliberately says nothing
    /// about the entry's kind or contents — callers that must fail closed on a name's mere
    /// existence (the custody deletion gate) need exactly this and nothing more, and giving them
    /// an answer that depended on decoding would make a corrupt file read as absent.
    pub fn child_entry_exists(&self, name: &OsStr, label: &str) -> Result<bool, FsCustodyError> {
        child_entry_exists_impl(&self.file, name, label)
    }

    /// Atomically publish an already-synced regular child at a name that must be FREE, then sync
    /// this parent directory.
    ///
    /// See [`CustodyPublicationV1`] for what each arm proves. In particular `Err` still means the
    /// publication provably did not happen, and for this no-replace operation that deliberately
    /// includes the ordinary `EEXIST` refusal — another actor publishing into the target name
    /// first is an expected, correct answer, not an anomaly, and the classification must never
    /// convert it into ambiguity or success.
    pub fn publish_new_regular_child(
        &self,
        source: RegularChildRefV1<'_>,
        target_name: &OsStr,
        label: &str,
    ) -> Result<CustodyPublicationV1, FsCustodyError> {
        self.publish_new_regular_child_with_before_rename(source, target_name, label, || Ok(()))
    }

    /// [`Self::publish_new_regular_child`] with a last-chance barrier that runs after every
    /// pre-check and strictly before the rename becomes visible.
    pub fn publish_new_regular_child_with_before_rename<F>(
        &self,
        source: RegularChildRefV1<'_>,
        target_name: &OsStr,
        label: &str,
        before_rename: F,
    ) -> Result<CustodyPublicationV1, FsCustodyError>
    where
        F: FnOnce() -> Result<(), FsCustodyError>,
    {
        let commit =
            publish_new_regular_child_impl(self, &source, target_name, label, before_rename)?;
        self.settle_publication(commit, &source, target_name, label, "published")
    }

    /// Atomically REPLACE an existing regular child with one the caller already built and synced,
    /// then sync this parent directory — the R2f1b custody transition primitive.
    ///
    /// Same custody discipline as [`Self::publish_new_regular_child`]: both names are validated
    /// single components, the rename is `renameat` against this retained descriptor (never a raw
    /// path rename, so a same-name replacement of any ancestor cannot redirect it), and the
    /// source is identity-checked against the descriptor the caller holds before anything moves.
    /// The one deliberate difference is the rename flag: this operation clobbers, which is why it
    /// is separately named rather than a mode of the publication path.
    ///
    /// **Caller obligation.** `source.file`'s own contents must already be `sync_all`'d; this
    /// call syncs the *directory*, not the file — exactly as the publication path does.
    ///
    /// See [`CustodyPublicationV1`] for what each arm proves.
    pub fn replace_regular_child(
        &self,
        source: RegularChildRefV1<'_>,
        target_name: &OsStr,
        label: &str,
    ) -> Result<CustodyPublicationV1, FsCustodyError> {
        let commit = replace_regular_child_impl(self, &source, target_name, label)?;
        self.settle_publication(commit, &source, target_name, label, "replaced")
    }

    /// The post-rename half shared by both publication primitives: parent sync, reopen, identity
    /// re-verification.
    ///
    /// Everything from here on is an OUTCOME, never an `Err`. Returning `Err` after the rename
    /// would tell the caller the publication did not happen, which is false — that inversion is
    /// the whole reason this type exists. `verb` is the only difference between the two callers.
    fn settle_publication(
        &self,
        commit: RenameCommitV1,
        source: &RegularChildRefV1<'_>,
        target_name: &OsStr,
        label: &str,
        verb: &str,
    ) -> Result<CustodyPublicationV1, FsCustodyError> {
        let retried_rename = match commit {
            RenameCommitV1::Committed { syscall_error } => syscall_error,
            RenameCommitV1::Unverifiable(detail) => {
                return Ok(CustodyPublicationV1::RenameOutcomeUnverified(detail))
            }
        };
        let note = |message: String| match &retried_rename {
            Some(detail) => format!("{message} (rename reported an error first: {detail})"),
            None => message,
        };
        if let Err(error) = self.sync(label) {
            return Ok(CustodyPublicationV1::ParentSyncAmbiguous(note(format!(
                "{label}: record {verb} but the parent sync is ambiguous: {error}"
            ))));
        }
        let opened_target = match self.open_regular_file(target_name, label) {
            Ok(file) => file,
            Err(error) => {
                return Ok(CustodyPublicationV1::TargetIdentityUnverified(note(
                    format!(
                        "{label}: record {verb} and parent synced but the target could not be \
                         reopened: {error}"
                    ),
                )))
            }
        };
        match same_regular_file(&opened_target, source.file, label) {
            Ok(true) => Ok(CustodyPublicationV1::Durable { retried_rename }),
            Ok(false) => Ok(CustodyPublicationV1::TargetIdentityUnverified(note(
                format!(
                "{label}: record {verb} and parent synced but the target is now a different object"
            ),
            ))),
            Err(error) => Ok(CustodyPublicationV1::TargetIdentityUnverified(note(
                format!(
                    "{label}: record {verb} and parent synced but its identity could not be read: \
                 {error}"
                ),
            ))),
        }
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

/// The workspace's one path-addressed directory open: read-only, `O_DIRECTORY`, `O_NOFOLLOW`,
/// `O_CLOEXEC`. A final symlink is refused rather than followed, and a non-directory is refused
/// by the kernel (`ENOTDIR`) before any content is read — which also means a FIFO substituted for
/// a directory can never block this call.
///
/// Returns the bare [`std::io::Error`] so each caller keeps its own message vocabulary.
pub fn open_directory_no_follow_raw(path: &Path) -> Result<File, std::io::Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut options = OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
        options.open(path)
    }
    #[cfg(not(unix))]
    {
        File::open(path)
    }
}

fn open_directory_no_follow(path: &Path, label: &str) -> Result<File, FsCustodyError> {
    open_directory_no_follow_raw(path).map_err(|error| FsCustodyError::Io(label.to_owned(), error))
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

/// Why a proposed child name is not usable as a single-component name beneath a retained
/// directory descriptor. Discriminated so each caller can keep its own wording for each case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChildNameRefusalV1 {
    /// Empty, `.`, `..`, or containing a path separator — none of which *name a child*: they
    /// walk the namespace, and handing any of them to a raw `openat`/`renameat*` would escape
    /// the single-component contract the retained descriptor exists to enforce.
    NotOneComponent,
    /// Contains an interior NUL byte, which cannot be expressed as a C string at all.
    ContainsNul,
}

/// Validate one child name and produce the C string the raw syscalls need.
///
/// The order of the two checks is load-bearing for message parity: callers that word the
/// "not one component" and "contains NUL" refusals differently must agree on which one a name
/// violating both (e.g. `a\0/b`) reports. Component-shape is checked first.
#[cfg(unix)]
pub fn validated_child_name(name: &OsStr) -> Result<CString, ChildNameRefusalV1> {
    use std::os::unix::ffi::OsStrExt as _;
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes == b"." || bytes == b".." || bytes.contains(&b'/') {
        return Err(ChildNameRefusalV1::NotOneComponent);
    }
    CString::new(bytes).map_err(|_| ChildNameRefusalV1::ContainsNul)
}

/// What a caller wants layered onto the mandatory read-only, no-follow, close-on-exec child open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChildOpenOptionsV1 {
    /// Add `O_NONBLOCK`, so a FIFO substituted for the expected object refuses instead of
    /// blocking the calling thread until a peer opens the other end.
    pub nonblocking: bool,
    /// Add `O_DIRECTORY`, so a non-directory is refused by the kernel rather than opened.
    pub directory: bool,
}

/// The workspace's one descriptor-relative child `openat`: `O_RDONLY | O_CLOEXEC | O_NOFOLLOW`
/// plus `options`, resolved against `parent` so no path component is re-traversed and a
/// same-name replacement of any ancestor cannot redirect it.
///
/// The opened object's KIND is deliberately not checked here — each layer states its own kind
/// policy (regular-vs-directory, link count) on the returned handle, and their refusals differ.
#[cfg(unix)]
pub fn open_child_no_follow(
    parent: &File,
    name: &CStr,
    options: ChildOpenOptionsV1,
) -> Result<File, std::io::Error> {
    use std::os::fd::{AsRawFd as _, FromRawFd as _};
    let mut flags = libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW;
    if options.nonblocking {
        flags |= libc::O_NONBLOCK;
    }
    if options.directory {
        flags |= libc::O_DIRECTORY;
    }
    // SAFETY: parent is a live directory descriptor and name is a validated single component.
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if fd == -1 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: successful openat returned an owned fd, adopted uniquely by File here.
    Ok(unsafe { File::from_raw_fd(fd) })
}

/// The workspace's one descriptor-relative child `fstatat(AT_SYMLINK_NOFOLLOW)`: reports the
/// directory ENTRY itself, never a symlink's target. `Ok(None)` means the name is genuinely
/// absent (`ENOENT`); `Ok(Some(_))` means an entry of some kind exists.
#[cfg(unix)]
pub fn stat_child_no_follow(
    parent: &File,
    name: &CStr,
) -> Result<Option<libc::stat>, std::io::Error> {
    use std::mem::MaybeUninit;
    use std::os::fd::AsRawFd as _;
    let mut stat = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: the parent descriptor, validated name, and writable stat buffer are all live.
    let result = unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result == -1 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ENOENT) {
            return Ok(None);
        }
        return Err(error);
    }
    // SAFETY: a successful fstatat initialized the complete stat value.
    Ok(Some(unsafe { stat.assume_init() }))
}

/// Why an atomic no-replace rename did not happen.
///
/// The two arms are structurally distinct, and that is the whole point:
/// [`Self::PlatformUnsupported`] is constructed ONLY by the compile arm for a platform that has
/// no no-replace rename syscall at all, never from a syscall result. It cannot be inferred from
/// an errno, because the errnos a real refusal carries (`ENOSYS`, `EOPNOTSUPP`, and on Linux the
/// identical `ENOTSUP`) decode to [`std::io::ErrorKind::Unsupported`] too — a kernel or
/// filesystem that declines `RENAME_NOREPLACE`/`RENAME_EXCL` (pre-3.15 Linux, overlayfs, NFS,
/// SMB/FUSE/exFAT) is a runtime fact an operator can act on, and reporting it as a platform
/// limitation both loses the errno and states something untrue.
#[cfg(unix)]
#[derive(Debug)]
pub enum RenameNoReplaceRefusalV1 {
    /// This build has no no-replace rename at all. Compile-time only.
    PlatformUnsupported,
    /// The syscall ran and the kernel refused. Carries the errno verbatim.
    Io(std::io::Error),
}

/// The workspace's one atomic no-replace rename between two validated child names beneath the
/// SAME retained directory descriptor. Target absence is part of the rename's linearization
/// point, so it holds even against an actor that does not honor the caller's owner lock.
///
/// **A [`RenameNoReplaceRefusalV1::Io`] refusal does NOT prove the rename did not happen.** The
/// syscall is atomic; the *report* of its result is not durable. On a network filesystem a
/// retried RPC can perform the rename and then report a failure — the server completed the first
/// request, the reply was lost, and the retry finds the source already gone. A caller that maps
/// this errno straight to "nothing was published" will treat a published record as absent, which
/// is the FAIL-OPEN direction: a `ProtectionPrepared`-class writer concludes the checkout is
/// unprotected while its record is on disk.
///
/// Callers must establish what happened by comparing descriptor-level identity;
/// [`classify_failed_publication_rename`] is the one implementation of that rule, shared with
/// [`rename_child_replacing`]. The `EEXIST` case is not an exception to any of this — see
/// `publish_new_regular_child_impl`, which keeps it a true refusal.
///
/// [`RenameNoReplaceRefusalV1::PlatformUnsupported`] is different in kind: it is constructed only
/// by a compile arm, never from a syscall result, so nothing ran and nothing needs classifying.
#[cfg(unix)]
pub fn rename_child_no_replace(
    parent: &File,
    source: &CStr,
    target: &CStr,
) -> Result<(), RenameNoReplaceRefusalV1> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::os::fd::AsRawFd as _;
        // SAFETY: both names are validated single components beneath the same live retained
        // directory descriptor.
        #[cfg(target_os = "macos")]
        let result = unsafe {
            libc::renameatx_np(
                parent.as_raw_fd(),
                source.as_ptr(),
                parent.as_raw_fd(),
                target.as_ptr(),
                libc::RENAME_EXCL,
            )
        };
        #[cfg(target_os = "linux")]
        let result = unsafe {
            libc::renameat2(
                parent.as_raw_fd(),
                source.as_ptr(),
                parent.as_raw_fd(),
                target.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result == -1 {
            return Err(RenameNoReplaceRefusalV1::Io(std::io::Error::last_os_error()));
        }
        Ok(())
    }
    // The ONLY construction site of `PlatformUnsupported`: a build with no such syscall.
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (parent, source, target);
        Err(RenameNoReplaceRefusalV1::PlatformUnsupported)
    }
}

/// The workspace's one descriptor-relative atomic REPLACING rename between two validated child
/// names beneath the SAME retained directory descriptor.
///
/// Deliberately separate from [`rename_child_no_replace`], and deliberately plain POSIX
/// `renameat`: replacement is atomic on every unix, so this needs no `RENAME_*` flag and no
/// per-OS arm — which also means it has no "only if the target exists" mode. That absence is why
/// the presence pre-check in the calling layer is documented as advisory.
///
/// **An `Err` from this function does NOT prove the rename did not happen.** The syscall is
/// atomic; the *report* of its result is not durable. On a network filesystem a retried RPC can
/// perform the rename and then report a failure — the server completed the first request, the
/// reply was lost, and the retry finds the source already gone (typically `ENOENT`). A caller
/// that maps this errno straight to "nothing moved" will treat a superseded record as
/// authoritative. Callers must establish what happened by comparing descriptor-level identity;
/// [`classify_failed_publication_rename`] is the one implementation of that rule.
///
/// [`rename_child_no_replace`] carries the identical hazard and is classified the same way, by
/// the same [`classify_failed_publication_rename`]; the two must be kept in step.
#[cfg(unix)]
pub fn rename_child_replacing(
    parent: &File,
    source: &CStr,
    target: &CStr,
) -> Result<(), std::io::Error> {
    use std::os::fd::AsRawFd as _;
    // SAFETY: both names are validated single components beneath the same live retained
    // directory descriptor.
    let result = unsafe {
        libc::renameat(
            parent.as_raw_fd(),
            source.as_ptr(),
            parent.as_raw_fd(),
            target.as_ptr(),
        )
    };
    if result == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Whether two already-fetched metadata readings describe the same filesystem object.
#[cfg(unix)]
#[must_use]
pub fn same_open_object(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(unix)]
fn child_name_cstring(name: &OsStr, label: &str) -> Result<CString, FsCustodyError> {
    validated_child_name(name).map_err(|_| FsCustodyError::InvalidChildName(label.to_owned()))
}

#[cfg(unix)]
fn open_regular_child(parent: &File, name: &OsStr, label: &str) -> Result<File, FsCustodyError> {
    let name = child_name_cstring(name, label)?;
    let file = open_child_no_follow(parent, &name, ChildOpenOptionsV1::default())
        .map_err(|error| FsCustodyError::Io(label.to_owned(), error))?;
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
    let left = left
        .metadata()
        .map_err(|error| FsCustodyError::Io(label.to_owned(), error))?;
    let right = right
        .metadata()
        .map_err(|error| FsCustodyError::Io(label.to_owned(), error))?;
    Ok(same_open_object(&left, &right))
}

#[cfg(not(unix))]
fn same_regular_file(_left: &File, _right: &File, _label: &str) -> Result<bool, FsCustodyError> {
    Ok(false)
}

/// The pre-rename half of [`PinnedDirectoryV1::publish_new_regular_child_with_before_rename`],
/// plus the post-error verification that decides what a failing no-replace rename actually did.
///
/// **`Err` here means the publication provably did not happen.** Established either by refusing
/// before the rename, or by [`classify_failed_publication_rename`] finding the staged source name
/// still present and still identical to the caller's object.
///
/// For this operation that guarantee has to survive a case the replacing primitive does not have:
/// an ordinary `EEXIST`. Another actor publishing into the target name inside the `before_rename`
/// window is exactly what `RENAME_NOREPLACE`/`RENAME_EXCL` exists to refuse, and the classifier
/// keeps it a true `Err` because the rename left our staged source untouched — the intact-source
/// rule fires before the target is ever consulted, and the target rule demands a positive identity
/// match that a foreign file cannot satisfy.
#[cfg(unix)]
fn publish_new_regular_child_impl<F>(
    pinned: &PinnedDirectoryV1,
    source: &RegularChildRefV1<'_>,
    target_name: &OsStr,
    label: &str,
    before_rename: F,
) -> Result<RenameCommitV1, FsCustodyError>
where
    F: FnOnce() -> Result<(), FsCustodyError>,
{
    let parent = &pinned.file;
    let source_cname = child_name_cstring(source.name, label)?;
    let target_cname = child_name_cstring(target_name, label)?;
    if source_cname.as_bytes() == target_cname.as_bytes() {
        return Err(FsCustodyError::InvalidChildName(label.to_owned()));
    }
    let opened_source = open_regular_child(parent, source.name, label)?;
    if !same_regular_file(&opened_source, source.file, label)? {
        return Err(FsCustodyError::IdentityChanged(label.to_owned()));
    }

    match stat_child_no_follow(parent, &target_cname) {
        Ok(Some(_)) => return Err(FsCustodyError::TargetExists(label.to_owned())),
        Ok(None) => {}
        Err(error) => return Err(FsCustodyError::Io(label.to_owned(), error)),
    }

    before_rename()?;

    let renamed = match pinned.armed_publication_rename_fault() {
        None => rename_child_no_replace(parent, &source_cname, &target_cname),
        Some(shape) => inject_publication_rename_fault(
            parent,
            &source_cname,
            &target_cname,
            PublicationRenameKindV1::NoReplace,
            shape,
        )
        .map_err(RenameNoReplaceRefusalV1::Io),
    };
    match renamed {
        Ok(()) => Ok(RenameCommitV1::Committed {
            syscall_error: None,
        }),
        // A platform with no no-replace rename at all never performed anything, so it stays a
        // true `Err` without consulting the filesystem: there is no effect to classify.
        Err(RenameNoReplaceRefusalV1::PlatformUnsupported) => {
            Err(FsCustodyError::Unsupported(label.to_owned()))
        }
        Err(RenameNoReplaceRefusalV1::Io(error)) => {
            classify_failed_publication_rename(parent, source, target_name, label, error)
        }
    }
}

#[cfg(unix)]
fn child_entry_exists_impl(
    parent: &File,
    name: &OsStr,
    label: &str,
) -> Result<bool, FsCustodyError> {
    let name = child_name_cstring(name, label)?;
    stat_child_no_follow(parent, &name)
        .map(|entry| entry.is_some())
        .map_err(|error| FsCustodyError::Io(label.to_owned(), error))
}

#[cfg(not(unix))]
fn child_entry_exists_impl(
    _parent: &File,
    _name: &OsStr,
    label: &str,
) -> Result<bool, FsCustodyError> {
    Err(FsCustodyError::Unsupported(label.to_owned()))
}

/// The pre-rename half of [`PinnedDirectoryV1::replace_regular_child`], plus the post-error
/// verification that decides what a failing rename actually did.
///
/// **`Err` here means the rename provably did not happen**, and that claim is now earned rather
/// than assumed. Two ways it is established: the function refused before reaching the rename at
/// all; or the rename reported an error and [`classify_failed_publication_rename`] found the staged
/// source name still present and still identical to the caller's object. An errno on its own is
/// never sufficient — see [`rename_child_replacing`].
#[cfg(unix)]
fn replace_regular_child_impl(
    pinned: &PinnedDirectoryV1,
    source: &RegularChildRefV1<'_>,
    target_name: &OsStr,
    label: &str,
) -> Result<RenameCommitV1, FsCustodyError> {
    let parent = &pinned.file;
    let source_cname = child_name_cstring(source.name, label)?;
    let target_cname = child_name_cstring(target_name, label)?;
    if source_cname.as_bytes() == target_cname.as_bytes() {
        return Err(FsCustodyError::InvalidChildName(label.to_owned()));
    }
    let opened_source = open_regular_child(parent, source.name, label)?;
    if !same_regular_file(&opened_source, source.file, label)? {
        return Err(FsCustodyError::IdentityChanged(label.to_owned()));
    }

    // ADVISORY, not a linearization point. `rename_child_no_replace` gets its target-absence
    // guarantee from the rename syscall's own flag; a replacing `renameat` has no "only if
    // present" counterpart, so a target unlinked between this stat and the rename below is still
    // created. The check exists to catch a caller reaching for `replace` where `publish` was
    // meant — an operation ordering error — not to defeat a concurrent actor.
    match stat_child_no_follow(parent, &target_cname) {
        Ok(Some(_)) => {}
        Ok(None) => return Err(FsCustodyError::TargetMissing(label.to_owned())),
        Err(error) => return Err(FsCustodyError::Io(label.to_owned(), error)),
    }

    let renamed = match pinned.armed_publication_rename_fault() {
        None => rename_child_replacing(parent, &source_cname, &target_cname),
        Some(shape) => inject_publication_rename_fault(
            parent,
            &source_cname,
            &target_cname,
            PublicationRenameKindV1::Replacing,
            shape,
        ),
    };
    match renamed {
        Ok(()) => Ok(RenameCommitV1::Committed {
            syscall_error: None,
        }),
        Err(error) => classify_failed_publication_rename(parent, source, target_name, label, error),
    }
}

/// What a failing publication rename actually did to the filesystem, as a bare verdict with no
/// message vocabulary — the MECHANISM half of the classification, so both this module's policy
/// layer and the binary's `local_file` (which keeps its own operator-facing text byte for byte)
/// share one implementation of the rule instead of two copies of a fail-open-critical boundary.
/// The rule itself is documented on [`classify_publication_rename_effect`].
#[cfg(unix)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicationRenameEffectV1 {
    /// The staged source name is still present AND still the caller's object: nothing moved.
    NotRenamed,
    /// The target is the caller's object: the rename took effect despite the error.
    Renamed,
    /// Neither is provable. Callers must treat this as protective — never as either decisive
    /// answer.
    Unverified,
}

/// Decide what a FAILING publication rename did — no-replace or replacing alike — using only
/// descriptor-level identity evidence.
///
/// Three answers, in the order the evidence supports them:
///
/// 1. the staged source name is still present AND still the caller's object ⇒ nothing moved, so
///    the syscall error is a true `Err`;
/// 2. otherwise the target is the caller's object ⇒ the rename took effect despite the error;
/// 3. otherwise nothing is proven, and the caller gets a protective, undecidable outcome.
///
/// Both demands are POSITIVE identity matches rather than mere existence: a same-name
/// substitution at the source must not read as "nothing happened", and a foreign object at the
/// target must not read as "our publication landed". Anything short of a match falls through to
/// rule 3, the fail-closed direction.
///
/// The ordinary no-replace `EEXIST` — another actor published first — stays a true refusal by
/// RULE 1, not rule 2: a refused rename leaves the staged source untouched, so rule 1 fires and
/// the target is never consulted. Rule 2's identity demand is load-bearing exactly when rule 1
/// does NOT fire (the staged source is gone or substituted).
///
/// The EVIDENCE ORDER is itself load-bearing, not stylistic: when BOTH names are the caller's
/// object (a hard link created inside the failure window), the rename was refused and nothing
/// was published — the intact-source answer must win. A target-first classifier would report
/// `Renamed`, and the caller would then attest `Durable` for a publication that never happened.
/// Pinned by `the_shared_rename_effect_rule_demands_positive_identity_on_both_sides`.
#[cfg(unix)]
#[must_use]
pub fn classify_publication_rename_effect(
    parent: &File,
    source_name: &OsStr,
    source: &File,
    target_name: &OsStr,
) -> PublicationRenameEffectV1 {
    let is_ours = |name: &OsStr| {
        open_regular_child(parent, name, "publication rename effect")
            .ok()
            .and_then(|opened| same_regular_file(&opened, source, "publication rename effect").ok())
            .unwrap_or(false)
    };
    if is_ours(source_name) {
        PublicationRenameEffectV1::NotRenamed
    } else if is_ours(target_name) {
        PublicationRenameEffectV1::Renamed
    } else {
        PublicationRenameEffectV1::Unverified
    }
}

#[cfg(unix)]
fn classify_failed_publication_rename(
    parent: &File,
    source: &RegularChildRefV1<'_>,
    target_name: &OsStr,
    label: &str,
    error: std::io::Error,
) -> Result<RenameCommitV1, FsCustodyError> {
    match classify_publication_rename_effect(parent, source.name, source.file, target_name) {
        PublicationRenameEffectV1::NotRenamed => Err(FsCustodyError::Io(label.to_owned(), error)),
        PublicationRenameEffectV1::Renamed => Ok(RenameCommitV1::Committed {
            syscall_error: Some(error.to_string()),
        }),
        PublicationRenameEffectV1::Unverified => Ok(RenameCommitV1::Unverifiable(format!(
            "{label}: the publication rename reported an error and its effect could not be \
             verified — the staged source is not provably intact and the target is not provably \
             the published object ({error})"
        ))),
    }
}

/// Which rename a publication performs. The fault seam needs it so an injected `AfterEffect`
/// reproduces the operation actually under test rather than a different one.
#[cfg(unix)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PublicationRenameKindV1 {
    NoReplace,
    Replacing,
}

/// Perform an injected publication-rename fault: do what `shape` says, then report an error.
///
/// Compiled unconditionally, like the sync hook and for the same reason (`FailureCountdownV1`'s
/// module doc): the operational surface has no `cfg(test)` callers to key off, and a hook that
/// only exists under `cfg(test)` cannot be armed from an integration test.
#[cfg(unix)]
fn inject_publication_rename_fault(
    parent: &File,
    source: &CStr,
    target: &CStr,
    kind: PublicationRenameKindV1,
    shape: PublicationRenameFaultV1,
) -> Result<(), std::io::Error> {
    use std::os::fd::AsRawFd as _;
    match shape {
        PublicationRenameFaultV1::BeforeEffect => {}
        PublicationRenameFaultV1::AfterEffect => match kind {
            PublicationRenameKindV1::Replacing => rename_child_replacing(parent, source, target)?,
            PublicationRenameKindV1::NoReplace => rename_child_no_replace(parent, source, target)
                .map_err(|refusal| match refusal {
                RenameNoReplaceRefusalV1::Io(error) => error,
                RenameNoReplaceRefusalV1::PlatformUnsupported => {
                    std::io::Error::from(std::io::ErrorKind::Unsupported)
                }
            })?,
        },
        PublicationRenameFaultV1::UnlinkSourceOnly => {
            // SAFETY: a live directory descriptor and a validated single-component name.
            let removed = unsafe { libc::unlinkat(parent.as_raw_fd(), source.as_ptr(), 0) };
            if removed == -1 {
                return Err(std::io::Error::last_os_error());
            }
        }
    }
    Err(std::io::Error::other(
        "injected publication rename failure for test",
    ))
}

#[cfg(not(unix))]
fn replace_regular_child_impl(
    _pinned: &PinnedDirectoryV1,
    _source: &RegularChildRefV1<'_>,
    _target_name: &OsStr,
    label: &str,
) -> Result<RenameCommitV1, FsCustodyError> {
    Err(FsCustodyError::Unsupported(label.to_owned()))
}

#[cfg(not(unix))]
fn publish_new_regular_child_impl<F>(
    _pinned: &PinnedDirectoryV1,
    _source: &RegularChildRefV1<'_>,
    _target_name: &OsStr,
    label: &str,
    _before_rename: F,
) -> Result<RenameCommitV1, FsCustodyError>
where
    F: FnOnce() -> Result<(), FsCustodyError>,
{
    Err(FsCustodyError::Unsupported(label.to_owned()))
}

// ---------------------------------------------------------------------------------------------
// The verify-then-act boundary shared by every destructive custody caller.
//
// Both storage reapers carried structurally identical copies of this: pinned-root recheck,
// payload `(dev, ino)`, canonicalize-equals-self, the pre-removal identity recheck, the
// presence probe that decides what the removal actually did, and the post-removal root recheck
// that decides whether the outcome can be attested at all. Two copies of a destructive boundary
// is two places for it to rot; the refusal DETAIL text lives here with the check that produces
// it, while the operator-facing gate vocabulary stays with each command's report.
// ---------------------------------------------------------------------------------------------

/// `(dev, ino)` of a real directory at `path`. A symlink or a non-directory is an error, never a
/// silently-followed success.
#[cfg(unix)]
pub fn directory_dev_ino(path: &Path) -> Result<(u64, u64), String> {
    use std::os::unix::fs::MetadataExt as _;
    let metadata =
        std::fs::symlink_metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(format!("{} is a symlink", path.display()));
    }
    if !metadata.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
pub fn directory_dev_ino(path: &Path) -> Result<(u64, u64), String> {
    Err(format!(
        "{}: filesystem identity (dev/ino) is unavailable on this platform, so a directory swap \
         cannot be detected",
        path.display()
    ))
}

/// Re-verify that a pinned root's PATH still resolves to the descriptor that was pinned. This is
/// the swap check: an actor controlling the parent can rename the root away and put another
/// directory in its place, after which every path-based operation lands in the replacement.
pub fn pinned_root_unchanged(pin: &PinnedDirectoryV1) -> Result<(), String> {
    let (dev, ino) = directory_dev_ino(pin.canonical_path())?;
    let want = pin.identity();
    if want.dev != Some(dev) || want.ino != Some(ino) {
        return Err(format!(
            "pinned scan root {} now resolves to a different directory (dev/ino {dev}/{ino}, pinned \
             {:?}/{:?})",
            pin.canonical_path().display(),
            want.dev,
            want.ino
        ));
    }
    Ok(())
}

/// The pinned root's identity rendered for an operator-facing gate line or evidence record.
#[must_use]
pub fn root_identity_label(pin: &PinnedDirectoryV1) -> String {
    let identity = pin.identity();
    match (identity.dev, identity.ino) {
        (Some(dev), Some(ino)) => format!("dev {dev} / ino {ino}"),
        _ => "unavailable".to_string(),
    }
}

/// Why a payload directory failed its own identity gate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PayloadIdentityRefusalV1 {
    /// The path names a symlink. Following it would act on something else entirely.
    IsSymlink,
    /// The path is not a real directory (or could not be stat'ed at all).
    NotADirectory { detail: String },
    /// The path no longer canonically resolves to itself, so its name and its object disagree.
    IdentityChanged { detail: String },
}

/// A payload directory's identity, re-derived from the filesystem right now: a real directory,
/// not a symlink, still canonically resolving to itself. Returns its `(dev, ino)` so a later
/// verify-then-act boundary can prove nothing was exchanged in between.
pub fn verify_payload_directory_identity(
    path: &Path,
) -> Result<(u64, u64), PayloadIdentityRefusalV1> {
    let identity = match directory_dev_ino(path) {
        Ok(identity) => identity,
        Err(detail) => {
            if std::fs::symlink_metadata(path)
                .map(|metadata| metadata.file_type().is_symlink())
                .unwrap_or(false)
            {
                return Err(PayloadIdentityRefusalV1::IsSymlink);
            }
            return Err(PayloadIdentityRefusalV1::NotADirectory { detail });
        }
    };
    match std::fs::canonicalize(path) {
        Ok(canonical) if canonical == path => {}
        Ok(canonical) => {
            return Err(PayloadIdentityRefusalV1::IdentityChanged {
                detail: format!("{} now resolves to {}", path.display(), canonical.display()),
            })
        }
        Err(error) => {
            return Err(PayloadIdentityRefusalV1::IdentityChanged {
                detail: format!("{} has no canonical path: {error}", path.display()),
            })
        }
    }
    Ok(identity)
}

/// Why the LAST boundary recheck — the one immediately before a destructive act — refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemovalBoundaryRefusalV1 {
    /// The pinned root changed between the gates and this instant.
    RootIdentityChanged { detail: String },
    /// The payload is no longer the object the gates measured and authorized.
    PayloadIdentityChanged { detail: String },
    /// The payload is no longer a real directory at all.
    PayloadNotADirectory { detail: String },
}

/// What a removal actually did, read from the filesystem rather than from its exit status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemovalObservationV1 {
    /// Reported success, and the payload is gone.
    Removed,
    /// Reported success, but the payload is still there. That is not success.
    ReportedSuccessButPresent { detail: String },
    /// Reported failure, and the payload is still there: a genuine partial removal.
    Failed { detail: String },
    /// Reported failure, but the payload is gone: WHAT was removed cannot be attested.
    ReportedFailureButGone { detail: String },
}

/// The result of [`verify_then_remove`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerifiedRemovalV1 {
    /// A boundary recheck refused before the act ran. Nothing was touched.
    Refused(RemovalBoundaryRefusalV1),
    /// The act ran. `observation` says what it did; `root_changed_during` is `Some(detail)` when
    /// the pinned root moved while the act was in flight — in which case the observation cannot
    /// be attributed to the intended object at all, however clean it looks.
    Acted {
        observation: RemovalObservationV1,
        root_changed_during: Option<String>,
    },
}

/// Verify-then-act: the LAST identity checks immediately before a destructive act, the act
/// itself, and the post-act re-verification that decides whether its outcome can be attested.
///
/// The gates a caller ran earlier took time, and time is the whole hazard: `expected_identity`
/// is the `(dev, ino)` those gates authorized, and this refuses rather than acting if the name
/// now points at anything else. `act` returns its own failure text unchanged; whether the payload
/// is actually gone is then read from the filesystem, never inferred from that result.
///
/// PARKED (S3 dual-review deferral, carried into the R2f1b custody plan §9): the boundary this
/// function closes is *around* the act, not *inside* it. Both reapers still pass a path-addressed
/// `remove_dir_all` as `act`, which re-traverses every path component itself — so a hostile actor
/// who swaps a component AFTER this recheck and BEFORE the traversal reaches it is outside what
/// the pre/post identity checks can see. Closing it needs a descriptor-relative recursive removal
/// (an `openat`/`O_NOFOLLOW` component walk plus `unlinkat`) built here and wired into the
/// production `ReapEnv::remove_tree`; the `act` seam is deliberately shaped so that swap is a
/// substitution behind this same signature, with no change to either reaper's gate logic.
pub fn verify_then_remove<F>(
    pin: &PinnedDirectoryV1,
    payload: &Path,
    expected_identity: (u64, u64),
    act: F,
) -> VerifiedRemovalV1
where
    F: FnOnce() -> Result<(), String>,
{
    if let Err(detail) = pinned_root_unchanged(pin) {
        return VerifiedRemovalV1::Refused(RemovalBoundaryRefusalV1::RootIdentityChanged {
            detail,
        });
    }
    match directory_dev_ino(payload) {
        Ok(now) if now == expected_identity => {}
        Ok(now) => {
            return VerifiedRemovalV1::Refused(RemovalBoundaryRefusalV1::PayloadIdentityChanged {
                detail: format!(
                    "{} changed identity between the gates and the removal (dev/ino {}/{} to \
                     {}/{})",
                    payload.display(),
                    expected_identity.0,
                    expected_identity.1,
                    now.0,
                    now.1
                ),
            })
        }
        Err(detail) => {
            return VerifiedRemovalV1::Refused(RemovalBoundaryRefusalV1::PayloadNotADirectory {
                detail,
            })
        }
    }

    let reported = act();
    let gone = std::fs::symlink_metadata(payload).is_err();
    let observation = match (reported, gone) {
        (Ok(()), true) => RemovalObservationV1::Removed,
        (Ok(()), false) => RemovalObservationV1::ReportedSuccessButPresent {
            detail: format!(
                "the removal reported success but {} is still present",
                payload.display()
            ),
        },
        (Err(detail), false) => RemovalObservationV1::Failed { detail },
        (Err(detail), true) => RemovalObservationV1::ReportedFailureButGone {
            detail: format!("removal reported an error ({detail}) but the path is gone"),
        },
    };
    VerifiedRemovalV1::Acted {
        observation,
        root_changed_during: pinned_root_unchanged(pin).err(),
    }
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

        let outcome = pinned
            .publish_new_regular_child(
                RegularChildRefV1::new(source_name, &source_file),
                target_name,
                "barrier then publish",
            )
            .unwrap();

        assert!(outcome.is_durable(), "unexpected outcome: {outcome:?}");
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

        let outcome = pinned
            .publish_new_regular_child(
                RegularChildRefV1::new(source_name, &source_file),
                target_name,
                "publish happy path",
            )
            .unwrap();

        assert!(outcome.is_durable(), "unexpected outcome: {outcome:?}");
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

    // ---------------------------------------------------------------------------------------
    // PinnedDirectoryV1::replace_regular_child — the R2f1b custody REPLACE primitive.
    //
    // Every custody transition after `ProtectionPrepared` overwrites an existing record, so this
    // is the one operation in the module that is *allowed* to clobber a target. Its contract is
    // stated as an outcome, not a bool: `Err` means the rename provably did not happen, and every
    // `Ok` arm means it did.
    // ---------------------------------------------------------------------------------------

    /// Discriminates a replace primitive that does not actually replace — e.g. one that inherits
    /// the no-replace `fstatat` pre-check or the `RENAME_EXCL`/`RENAME_NOREPLACE` flag and so
    /// refuses an existing target, which would make every custody transition after
    /// `ProtectionPrepared` impossible.
    #[cfg(unix)]
    #[test]
    fn replace_regular_child_overwrites_an_existing_record_and_reports_durable() {
        let dir = tempfile::tempdir().unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "custody replace").unwrap();
        let source_name = OsStr::new("record.tmp");
        let target_name = OsStr::new("record.custody.v1.json");
        fs::write(dir.path().join(target_name), b"prepared").unwrap();
        fs::write(dir.path().join(source_name), b"live-protected").unwrap();
        let source_file = fs::File::open(dir.path().join(source_name)).unwrap();

        let outcome = pinned
            .replace_regular_child(
                RegularChildRefV1::new(source_name, &source_file),
                target_name,
                "custody replace",
            )
            .unwrap();

        assert!(outcome.is_durable(), "unexpected outcome: {outcome:?}");
        assert_eq!(outcome.ambiguity(), None);
        assert_eq!(
            fs::read(dir.path().join(target_name)).unwrap(),
            b"live-protected"
        );
        assert!(
            !dir.path().join(source_name).exists(),
            "the replacing rename must consume the source name"
        );
    }

    /// The ambiguous post-rename-sync fault, and the reason the outcome is a type rather than a
    /// `Result<(), _>`: the record on disk IS the replacement (so this is not a clean failure)
    /// but its durability across a crash is unknown (so it is not a clean success either).
    ///
    /// Discriminates two distinct regressions: (a) mapping the parent-sync failure to `Err`,
    /// which a `?`-using caller would read as "the replacement did not happen" and might retry
    /// or treat the previous state as authoritative; and (b) swallowing the sync failure and
    /// reporting `Durable`, which would license a later deletion on evidence that does not exist.
    #[cfg(unix)]
    #[test]
    fn replace_regular_child_reports_a_protective_ambiguity_when_the_parent_sync_fails() {
        let dir = tempfile::tempdir().unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "ambiguous replace").unwrap();
        let source_name = OsStr::new("record.tmp");
        let target_name = OsStr::new("record.custody.v1.json");
        fs::write(dir.path().join(target_name), b"prepared").unwrap();
        fs::write(dir.path().join(source_name), b"preserved").unwrap();
        let source_file = fs::File::open(dir.path().join(source_name)).unwrap();
        pinned.fail_sync_on_nth_call_for_test(1);

        let outcome = pinned
            .replace_regular_child(
                RegularChildRefV1::new(source_name, &source_file),
                target_name,
                "ambiguous replace",
            )
            .unwrap();

        assert!(!outcome.is_durable(), "unexpected outcome: {outcome:?}");
        let detail = outcome
            .ambiguity()
            .expect("a failed parent sync must be reported as an ambiguity");
        assert!(
            detail.contains("ambiguous"),
            "ambiguity detail must say so: {detail}"
        );
        // Not a clean failure: the rename already happened.
        assert_eq!(
            fs::read(dir.path().join(target_name)).unwrap(),
            b"preserved"
        );
        assert!(!dir.path().join(source_name).exists());
    }

    /// The error-after-effect hazard, and the reason `Err` can no longer be produced by simply
    /// mapping the rename syscall's errno. A `renameat` that fails is NOT proof that nothing
    /// moved: on a network filesystem a retried RPC can perform the rename and then report a
    /// failure (the server completed the first request, the reply was lost, and the retry finds
    /// the source already gone). Mapping that straight to `Err` tells the caller the previous
    /// record is still authoritative when it has in fact been superseded — the exact inversion
    /// the `Err`-means-nothing-happened contract exists to prevent.
    ///
    /// Discriminates a primitive that trusts the errno instead of verifying by identity.
    #[cfg(unix)]
    #[test]
    fn a_rename_that_took_effect_despite_a_syscall_error_is_never_a_plain_error() {
        let dir = tempfile::tempdir().unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "error after effect").unwrap();
        let source_name = OsStr::new("record.tmp");
        let target_name = OsStr::new("record.custody.v1.json");
        fs::write(dir.path().join(target_name), b"prepared").unwrap();
        fs::write(dir.path().join(source_name), b"live-protected").unwrap();
        let source_file = fs::File::open(dir.path().join(source_name)).unwrap();
        pinned
            .fail_publication_rename_on_nth_call_for_test(1, PublicationRenameFaultV1::AfterEffect);

        let outcome = pinned
            .replace_regular_child(
                RegularChildRefV1::new(source_name, &source_file),
                target_name,
                "error after effect",
            )
            .expect("a rename that took effect must never be reported as a plain error");

        // The effect is real, and post-error verification proved it, so the replacement is
        // attested exactly as an uneventful one would be — but the syscall error is retained so an
        // operator can see the filesystem is answering error-after-effect.
        assert!(outcome.is_durable(), "unexpected outcome: {outcome:?}");
        let retried = match &outcome {
            CustodyPublicationV1::Durable { retried_rename } => retried_rename.as_deref(),
            other => panic!("unexpected outcome: {other:?}"),
        };
        assert!(
            retried.is_some_and(|detail| detail.contains("injected")),
            "the superseded syscall error must be retained: {retried:?}"
        );
        assert_eq!(
            fs::read(dir.path().join(target_name)).unwrap(),
            b"live-protected"
        );
        assert!(!dir.path().join(source_name).exists());
    }

    /// The other direction, and the reason the repair is verification rather than a blanket
    /// weakening of the contract: when the rename genuinely did not happen, `Err` must still mean
    /// exactly that, with the staged source untouched and the previous record authoritative.
    /// Discriminates a "repair" that gives up and reports every rename error as ambiguous, which
    /// would make an ordinary refusal (a wrong name, a full disk) indistinguishable from a lost
    /// effect and leave every caller permanently unable to conclude anything.
    #[cfg(unix)]
    #[test]
    fn a_rename_error_with_no_effect_is_still_a_provably_not_renamed_error() {
        let dir = tempfile::tempdir().unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "error before effect").unwrap();
        let source_name = OsStr::new("record.tmp");
        let target_name = OsStr::new("record.custody.v1.json");
        fs::write(dir.path().join(target_name), b"prepared").unwrap();
        fs::write(dir.path().join(source_name), b"live-protected").unwrap();
        let source_file = fs::File::open(dir.path().join(source_name)).unwrap();
        pinned.fail_publication_rename_on_nth_call_for_test(
            1,
            PublicationRenameFaultV1::BeforeEffect,
        );

        let error = pinned
            .replace_regular_child(
                RegularChildRefV1::new(source_name, &source_file),
                target_name,
                "error before effect",
            )
            .unwrap_err();

        assert!(matches!(error, FsCustodyError::Io(_, _)));
        assert_eq!(fs::read(dir.path().join(target_name)).unwrap(), b"prepared");
        assert_eq!(
            fs::read(dir.path().join(source_name)).unwrap(),
            b"live-protected"
        );
    }

    /// When verification itself cannot decide, the outcome is PROTECTIVE — never a plain `Err`
    /// (which would claim the previous record survived) and never a durable success. The injected
    /// shape is "the staged source name is gone but the target is not our object", which is what a
    /// caller sees when a rename error coincides with any other actor touching either name.
    ///
    /// Discriminates a verification step that falls back to one of the two decisive answers when
    /// the evidence supports neither.
    #[cfg(unix)]
    #[test]
    fn a_rename_error_whose_effect_cannot_be_verified_is_protective() {
        let dir = tempfile::tempdir().unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "unverifiable rename").unwrap();
        let source_name = OsStr::new("record.tmp");
        let target_name = OsStr::new("record.custody.v1.json");
        fs::write(dir.path().join(target_name), b"prepared").unwrap();
        fs::write(dir.path().join(source_name), b"live-protected").unwrap();
        let source_file = fs::File::open(dir.path().join(source_name)).unwrap();
        pinned.fail_publication_rename_on_nth_call_for_test(
            1,
            PublicationRenameFaultV1::UnlinkSourceOnly,
        );

        let outcome = pinned
            .replace_regular_child(
                RegularChildRefV1::new(source_name, &source_file),
                target_name,
                "unverifiable rename",
            )
            .expect("an undecidable rename outcome must not be reported as a plain error");

        assert!(!outcome.is_durable(), "unexpected outcome: {outcome:?}");
        let detail = outcome
            .ambiguity()
            .expect("an undecidable rename outcome must be protective");
        assert!(
            detail.contains("could not be verified"),
            "unexpected detail: {detail}"
        );
        // Neither conclusion is licensed: the previous record is still what is on disk here, but
        // the caller is told nothing that lets it act on that.
        assert_eq!(fs::read(dir.path().join(target_name)).unwrap(), b"prepared");
    }

    /// Discriminates a replace primitive that silently *creates* an absent target. `replace` and
    /// `publish` are separately named operations precisely so a caller cannot reach for the wrong
    /// one; a create-if-absent replace would let a transition write a record for a checkout whose
    /// `ProtectionPrepared` publication never landed.
    ///
    /// HONEST LIMIT: the pre-check is advisory, not a linearization point. Unlike
    /// `rename_child_no_replace` — whose target-absence guarantee lives in the rename syscall's
    /// own flag — a replacing `renameat` has no "only if present" flag, so a target unlinked
    /// between this `fstatat` and the rename is still created. The check catches caller error,
    /// not a hostile racer.
    #[cfg(unix)]
    #[test]
    fn replace_regular_child_refuses_an_absent_target_and_leaves_the_source_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "replace absent").unwrap();
        let source_name = OsStr::new("record.tmp");
        let target_name = OsStr::new("record.custody.v1.json");
        fs::write(dir.path().join(source_name), b"live-protected").unwrap();
        let source_file = fs::File::open(dir.path().join(source_name)).unwrap();

        let error = pinned
            .replace_regular_child(
                RegularChildRefV1::new(source_name, &source_file),
                target_name,
                "replace absent",
            )
            .unwrap_err();

        assert!(matches!(error, FsCustodyError::TargetMissing(_)));
        assert!(!dir.path().join(target_name).exists());
        assert_eq!(
            fs::read(dir.path().join(source_name)).unwrap(),
            b"live-protected"
        );
    }

    /// Discriminates dropping the source-identity pre-check, which would let a replacement
    /// publish bytes the caller never wrote: the caller holds a descriptor on the object it
    /// built, and a same-name substitution between build and publish must refuse rather than
    /// clobber the durable record with an attacker's file.
    #[cfg(unix)]
    #[test]
    fn replace_regular_child_refuses_a_source_name_substituted_since_the_caller_wrote_it() {
        let dir = tempfile::tempdir().unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "replace substituted").unwrap();
        let source_name = OsStr::new("record.tmp");
        let target_name = OsStr::new("record.custody.v1.json");
        fs::write(dir.path().join(target_name), b"prepared").unwrap();
        fs::write(dir.path().join(source_name), b"mine").unwrap();
        let source_file = fs::File::open(dir.path().join(source_name)).unwrap();
        // Same name, different inode.
        fs::remove_file(dir.path().join(source_name)).unwrap();
        fs::write(dir.path().join(source_name), b"substituted").unwrap();

        let error = pinned
            .replace_regular_child(
                RegularChildRefV1::new(source_name, &source_file),
                target_name,
                "replace substituted",
            )
            .unwrap_err();

        assert!(matches!(error, FsCustodyError::IdentityChanged(_)));
        assert_eq!(fs::read(dir.path().join(target_name)).unwrap(), b"prepared");
    }

    /// Discriminates handing an unvalidated name to the raw replacing `renameat`. A replacing
    /// rename is strictly more dangerous than the no-replace one — a traversing name would let a
    /// caller clobber an arbitrary path — so the single-component contract must hold here too.
    #[cfg(unix)]
    #[test]
    fn replace_regular_child_refuses_a_traversing_target_name() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let inner = outside.join("inner");
        fs::create_dir(&inner).unwrap();
        let victim = outside.join("victim");
        fs::write(&victim, b"not yours").unwrap();
        let pinned = PinnedDirectoryV1::open(&inner, "replace traversal").unwrap();
        let source_name = OsStr::new("record.tmp");
        fs::write(inner.join(source_name), b"mine").unwrap();
        let source_file = fs::File::open(inner.join(source_name)).unwrap();

        let error = pinned
            .replace_regular_child(
                RegularChildRefV1::new(source_name, &source_file),
                OsStr::new("../victim"),
                "replace traversal",
            )
            .unwrap_err();

        assert!(matches!(error, FsCustodyError::InvalidChildName(_)));
        assert_eq!(fs::read(&victim).unwrap(), b"not yours");
    }

    /// The custody deletion gate refuses on a record's mere existence, so this probe must answer
    /// "taken" for entries a decoder would reject. Discriminates an implementation built on
    /// `open_regular_file` (or on a decode attempt), which would report a directory, a dangling
    /// symlink, or an unreadable file as ABSENT — and absent is the one answer that licenses
    /// deletion.
    #[cfg(unix)]
    #[test]
    fn child_entry_exists_reports_any_taken_name_not_just_readable_regular_files() {
        let dir = tempfile::tempdir().unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "entry probe").unwrap();
        fs::write(dir.path().join("regular"), b"x").unwrap();
        fs::create_dir(dir.path().join("as-directory")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("nowhere"), dir.path().join("dangling"))
            .unwrap();

        for name in ["regular", "as-directory", "dangling"] {
            assert!(
                pinned
                    .child_entry_exists(OsStr::new(name), "entry probe")
                    .unwrap(),
                "{name} must read as taken"
            );
        }
        assert!(!pinned
            .child_entry_exists(OsStr::new("absent"), "entry probe")
            .unwrap());
    }

    /// Discriminates handing an unvalidated name to the raw `fstatat`: a traversing name would
    /// let the probe answer about an object outside the pinned directory entirely.
    #[cfg(unix)]
    #[test]
    fn child_entry_exists_refuses_a_name_that_is_not_one_component() {
        let dir = tempfile::tempdir().unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "entry probe").unwrap();

        let error = pinned
            .child_entry_exists(OsStr::new("../elsewhere"), "entry probe")
            .unwrap_err();

        assert!(matches!(error, FsCustodyError::InvalidChildName(_)));
    }

    // ---------------------------------------------------------------------------------------
    // No-replace publication: the error-after-effect classification.
    //
    // Same hazard and same repair shape as the replace path, with one semantic difference that
    // drives the extra test below: for a NO-REPLACE publication, "the target exists and is not
    // ours" is a legitimate, expected refusal (`EEXIST` — someone else published first), not an
    // anomaly. The classifier must never convert that into ambiguity or into success.
    // ---------------------------------------------------------------------------------------

    /// The shared rule itself, exercised directly on all three verdicts so both callers
    /// (`fs_custody`'s policy layer and the binary's `local_file`, which keeps its own message
    /// vocabulary) rest on one tested implementation rather than two copies.
    ///
    /// Discriminates the two identity shortcuts that would each be fail-open in a different
    /// direction: "the source name exists ⇒ nothing moved" (a substituted source reads as safe)
    /// and "the target name exists ⇒ we published" (a foreign file reads as our record).
    #[cfg(unix)]
    #[test]
    fn the_shared_rename_effect_rule_demands_positive_identity_on_both_sides() {
        let dir = tempfile::tempdir().unwrap();
        let parent = fs::File::open(dir.path()).unwrap();
        let source_name = OsStr::new("staged.tmp");
        let target_name = OsStr::new("record.json");
        fs::write(dir.path().join(source_name), b"ours").unwrap();
        let ours = fs::File::open(dir.path().join(source_name)).unwrap();

        // Staged source intact and ours: nothing moved.
        assert_eq!(
            classify_publication_rename_effect(&parent, source_name, &ours, target_name),
            PublicationRenameEffectV1::NotRenamed
        );

        // EVIDENCE ORDER, not just identity: hard-link the staged source to the target name so
        // BOTH names are the caller's object. The real no-replace rename would have refused
        // (target exists), so nothing was published and the intact-source answer must win. A
        // target-first classifier answers Renamed here, and the caller would attest Durable for
        // a publication that never happened.
        fs::hard_link(dir.path().join(source_name), dir.path().join(target_name)).unwrap();
        assert_eq!(
            classify_publication_rename_effect(&parent, source_name, &ours, target_name),
            PublicationRenameEffectV1::NotRenamed
        );
        fs::remove_file(dir.path().join(target_name)).unwrap();

        // A same-name substitution at the source is NOT proof that nothing moved.
        fs::remove_file(dir.path().join(source_name)).unwrap();
        fs::write(dir.path().join(source_name), b"impostor").unwrap();
        assert_eq!(
            classify_publication_rename_effect(&parent, source_name, &ours, target_name),
            PublicationRenameEffectV1::Unverified
        );

        // A foreign object at the target is NOT proof that we published — this is the ordinary
        // no-replace EEXIST shape, and it must never read as our own effect.
        fs::write(dir.path().join(target_name), b"theirs").unwrap();
        assert_eq!(
            classify_publication_rename_effect(&parent, source_name, &ours, target_name),
            PublicationRenameEffectV1::Unverified
        );

        // Our object at the target: the rename took effect.
        fs::remove_file(dir.path().join(source_name)).unwrap();
        fs::remove_file(dir.path().join(target_name)).unwrap();
        let ours_path = dir.path().join("ours-again");
        fs::write(&ours_path, b"ours").unwrap();
        let ours_again = fs::File::open(&ours_path).unwrap();
        fs::rename(&ours_path, dir.path().join(target_name)).unwrap();
        assert_eq!(
            classify_publication_rename_effect(&parent, source_name, &ours_again, target_name),
            PublicationRenameEffectV1::Renamed
        );
    }

    /// The fail-OPEN direction, and the reason this round exists. A publication that took effect
    /// but reported an error tells a `ProtectionPrepared`-class writer that the checkout is
    /// unprotected while the record is on disk — and that writer's control flow (abandon, retry,
    /// quarantine) is then driven by the wrong answer.
    ///
    /// Discriminates a publication path that trusts the rename errno instead of verifying by
    /// identity.
    #[cfg(unix)]
    #[test]
    fn a_publication_that_took_effect_despite_a_syscall_error_is_never_a_plain_error() {
        let dir = tempfile::tempdir().unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "publish after effect").unwrap();
        let source_name = OsStr::new("record.tmp");
        let target_name = OsStr::new("record.custody.v1.json");
        fs::write(dir.path().join(source_name), b"prepared").unwrap();
        let source_file = fs::File::open(dir.path().join(source_name)).unwrap();
        pinned
            .fail_publication_rename_on_nth_call_for_test(1, PublicationRenameFaultV1::AfterEffect);

        let outcome = pinned
            .publish_new_regular_child(
                RegularChildRefV1::new(source_name, &source_file),
                target_name,
                "publish after effect",
            )
            .expect("a publication that took effect must never be reported as a plain error");

        assert!(outcome.is_durable(), "unexpected outcome: {outcome:?}");
        let retried = match &outcome {
            CustodyPublicationV1::Durable { retried_rename } => retried_rename.as_deref(),
            other => panic!("unexpected outcome: {other:?}"),
        };
        assert!(
            retried.is_some_and(|detail| detail.contains("injected")),
            "the superseded syscall error must be retained: {retried:?}"
        );
        assert_eq!(fs::read(dir.path().join(target_name)).unwrap(), b"prepared");
        assert!(!dir.path().join(source_name).exists());
    }

    /// The other direction: a rename that genuinely did not happen must still be a true `Err`,
    /// with the staged source intact and the target name still free. Discriminates a "repair"
    /// that reports every rename error as ambiguous, which would make an ordinary refusal
    /// indistinguishable from a lost effect.
    #[cfg(unix)]
    #[test]
    fn a_publication_rename_error_with_no_effect_is_still_a_provably_not_published_error() {
        let dir = tempfile::tempdir().unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "publish before effect").unwrap();
        let source_name = OsStr::new("record.tmp");
        let target_name = OsStr::new("record.custody.v1.json");
        fs::write(dir.path().join(source_name), b"prepared").unwrap();
        let source_file = fs::File::open(dir.path().join(source_name)).unwrap();
        pinned.fail_publication_rename_on_nth_call_for_test(
            1,
            PublicationRenameFaultV1::BeforeEffect,
        );

        let error = pinned
            .publish_new_regular_child(
                RegularChildRefV1::new(source_name, &source_file),
                target_name,
                "publish before effect",
            )
            .unwrap_err();

        assert!(matches!(error, FsCustodyError::Io(_, _)));
        assert!(!dir.path().join(target_name).exists());
        assert_eq!(fs::read(dir.path().join(source_name)).unwrap(), b"prepared");
    }

    /// THE NO-REPLACE-SPECIFIC CASE. A real `EEXIST`: the target is absent at the pre-check, a
    /// foreign actor publishes into that name inside the `before_rename` window, and the real
    /// `RENAME_NOREPLACE`/`RENAME_EXCL` then refuses. This is the whole point of a no-replace
    /// rename working correctly, and it must stay a TRUE refusal.
    ///
    /// Here the target exists and the rename provably did not happen. The refusal survives by
    /// RULE 1: a refused rename leaves the staged source untouched, so the intact-source answer
    /// fires and the foreign target is never consulted. This test therefore pins the END-TO-END
    /// genuine-`EEXIST` path through the real classifier — kernel-produced refusal in, true
    /// refusal out — not the target-side identity discipline, which rule 1 shadows here. The
    /// target-side discipline is pinned where rule 1 cannot fire: the source-substitution and
    /// foreign-target assertions of
    /// `the_shared_rename_effect_rule_demands_positive_identity_on_both_sides`, and the
    /// adversarial source-gone case of
    /// `a_publication_rename_error_whose_effect_cannot_be_verified_is_protective`.
    ///
    /// No fault injection: the refusal here is the genuine article, produced by the kernel.
    #[cfg(unix)]
    #[test]
    fn a_target_published_by_a_foreign_actor_stays_a_true_refusal_not_an_ambiguity() {
        let dir = tempfile::tempdir().unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "foreign publisher").unwrap();
        let source_name = OsStr::new("record.tmp");
        let target_name = OsStr::new("record.custody.v1.json");
        fs::write(dir.path().join(source_name), b"ours").unwrap();
        let source_file = fs::File::open(dir.path().join(source_name)).unwrap();
        let target_path = dir.path().join(target_name);

        let error = pinned
            .publish_new_regular_child_with_before_rename(
                RegularChildRefV1::new(source_name, &source_file),
                target_name,
                "foreign publisher",
                || {
                    fs::write(&target_path, b"theirs").unwrap();
                    Ok(())
                },
            )
            .unwrap_err();

        assert!(matches!(error, FsCustodyError::Io(_, _)));
        // Neither object moved: theirs is untouched and ours is still staged.
        assert_eq!(fs::read(&target_path).unwrap(), b"theirs");
        assert_eq!(fs::read(dir.path().join(source_name)).unwrap(), b"ours");
    }

    /// When the evidence supports neither conclusion the outcome is PROTECTIVE. The injected shape
    /// is the adversarial combination of the two rules: the staged source is gone (so "nothing
    /// moved" is not provable) AND a foreign object occupies the target (so "our publication
    /// landed" is not provable either).
    ///
    /// Discriminates a classifier that resolves an undecidable case toward either decisive answer
    /// — and in particular one that treats a merely-present target as proof of our own effect.
    #[cfg(unix)]
    #[test]
    fn a_publication_rename_error_whose_effect_cannot_be_verified_is_protective() {
        let dir = tempfile::tempdir().unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "unverifiable publish").unwrap();
        let source_name = OsStr::new("record.tmp");
        let target_name = OsStr::new("record.custody.v1.json");
        fs::write(dir.path().join(source_name), b"ours").unwrap();
        let source_file = fs::File::open(dir.path().join(source_name)).unwrap();
        let target_path = dir.path().join(target_name);
        pinned.fail_publication_rename_on_nth_call_for_test(
            1,
            PublicationRenameFaultV1::UnlinkSourceOnly,
        );

        let outcome = pinned
            .publish_new_regular_child_with_before_rename(
                RegularChildRefV1::new(source_name, &source_file),
                target_name,
                "unverifiable publish",
                || {
                    fs::write(&target_path, b"theirs").unwrap();
                    Ok(())
                },
            )
            .expect("an undecidable publication outcome must not be reported as a plain error");

        assert!(!outcome.is_durable(), "unexpected outcome: {outcome:?}");
        let detail = outcome
            .ambiguity()
            .expect("an undecidable publication outcome must be protective");
        assert!(
            detail.contains("could not be verified"),
            "unexpected detail: {detail}"
        );
        assert_eq!(fs::read(&target_path).unwrap(), b"theirs");
    }

    /// The parent sync after a SUCCESSFUL publication rename is the second fail-open route out of
    /// this function, and it predates the errno one: the record is published, only its durability
    /// is unknown. Discriminates the pre-round behaviour, which reported it as `Err` with an
    /// "ambiguous" message — a channel a caller that branches on `is_err()` cannot read.
    #[cfg(unix)]
    #[test]
    fn a_publication_whose_parent_sync_fails_is_ambiguous_not_a_failure() {
        let dir = tempfile::tempdir().unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "publish sync ambiguous").unwrap();
        let source_name = OsStr::new("record.tmp");
        let target_name = OsStr::new("record.custody.v1.json");
        fs::write(dir.path().join(source_name), b"prepared").unwrap();
        let source_file = fs::File::open(dir.path().join(source_name)).unwrap();
        pinned.fail_sync_on_nth_call_for_test(1);

        let outcome = pinned
            .publish_new_regular_child(
                RegularChildRefV1::new(source_name, &source_file),
                target_name,
                "publish sync ambiguous",
            )
            .expect("a published record with an ambiguous sync is not a failed publication");

        assert!(!outcome.is_durable(), "unexpected outcome: {outcome:?}");
        assert!(outcome.ambiguity().is_some_and(|d| d.contains("ambiguous")));
        assert_eq!(fs::read(dir.path().join(target_name)).unwrap(), b"prepared");
    }

    /// Positive control for the whole slice: adding a REPLACE primitive must not weaken the
    /// no-replace publication paths that every `ProtectionPrepared` write still depends on. This
    /// duplicates `publish_refuses_to_replace_an_existing_target`'s assertion deliberately — it
    /// exists to fail loudly if a later refactor "unifies" the two renames.
    #[cfg(unix)]
    #[test]
    fn adding_the_replace_primitive_leaves_publication_no_replace() {
        let dir = tempfile::tempdir().unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "no-replace control").unwrap();
        let source_name = OsStr::new("record.tmp");
        let target_name = OsStr::new("record.custody.v1.json");
        fs::write(dir.path().join(source_name), b"new").unwrap();
        fs::write(dir.path().join(target_name), b"old").unwrap();
        let source_file = fs::File::open(dir.path().join(source_name)).unwrap();

        let error = pinned
            .publish_new_regular_child(
                RegularChildRefV1::new(source_name, &source_file),
                target_name,
                "no-replace control",
            )
            .unwrap_err();

        assert!(matches!(error, FsCustodyError::TargetExists(_)));
        assert_eq!(fs::read(dir.path().join(target_name)).unwrap(), b"old");
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
        let outcome = pinned
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

        assert!(outcome.is_durable(), "unexpected outcome: {outcome:?}");
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

            // CONTRACT CHANGE (no-replace errno classification round): the rename ALREADY
            // happened when the swap is detected, so reporting `Err` here was the same
            // fail-open inversion this round exists to remove — the caller would read "not
            // published" for a publication that landed and was then clobbered. It is now a
            // protective `Ok` arm that attests nothing.
            if matches!(
                result,
                Ok(CustodyPublicationV1::TargetIdentityUnverified(_))
            ) {
                caught = true;
            }
            // `_guard` drops here at the end of each iteration (and on any panic unwinding
            // through this scope), stopping and joining the swapper before the next
            // tempdir/thread pair is created.
        }

        assert!(
            caught,
            "expected publish_new_regular_child_with_before_rename to report at least one \
             post-rename target swap as an unverified target within the time budget"
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

    // ---------------------------------------------------------------------------------------
    // A4 mechanism surface: the shared primitives extracted so `local_file` and both storage
    // reapers stop carrying their own copies. Every test below covers a contract that did not
    // exist before A4 — the pre-existing behaviour is covered by the tests above and by the
    // binary's own suites, which A4 left untouched.
    // ---------------------------------------------------------------------------------------

    /// Discriminates a `validated_child_name` that collapses its two refusals into one. The
    /// binary's `local_file` wraps each with a DIFFERENT operator-facing message, so a validator
    /// that reported `ContainsNul` for `a/b` (or vice versa) would silently change text that
    /// `compatibility_*` tests assert verbatim. Also pins the ORDER: a name violating BOTH rules
    /// must report `NotOneComponent`, because that is the one the pre-A4 `local_file` validator
    /// reported for it.
    #[cfg(unix)]
    #[test]
    fn validated_child_name_discriminates_its_two_refusals_and_fixes_their_order() {
        use std::os::unix::ffi::OsStrExt as _;

        for name in [
            OsStr::new(""),
            OsStr::new("."),
            OsStr::new(".."),
            OsStr::new("a/b"),
        ] {
            assert_eq!(
                validated_child_name(name).unwrap_err(),
                ChildNameRefusalV1::NotOneComponent,
                "{name:?} must be refused as a non-component"
            );
        }
        assert_eq!(
            validated_child_name(OsStr::from_bytes(b"a\0b")).unwrap_err(),
            ChildNameRefusalV1::ContainsNul
        );
        // Violates both: component shape is checked first, exactly as `local_file` always did.
        assert_eq!(
            validated_child_name(OsStr::from_bytes(b"a\0/b")).unwrap_err(),
            ChildNameRefusalV1::NotOneComponent
        );
        assert_eq!(
            validated_child_name(OsStr::new("ordinary.txt"))
                .unwrap()
                .as_bytes(),
            b"ordinary.txt"
        );
    }

    /// Discriminates `ChildOpenOptionsV1::nonblocking` being dropped on the floor. A read-only
    /// open of a FIFO with no writer BLOCKS until one appears; without `O_NONBLOCK` this call
    /// would hang the calling thread forever rather than returning, which is precisely the hazard
    /// `local_file` sets the flag for. The test is bounded by running the open on a worker thread
    /// and failing if it has not returned quickly — a regression hangs the worker, not the suite.
    #[cfg(unix)]
    #[test]
    fn child_open_options_nonblocking_refuses_a_writerless_fifo_instead_of_blocking() {
        use std::os::unix::ffi::OsStrExt as _;

        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("pipe");
        let fifo_c = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: a valid NUL-terminated path in a directory this test owns.
        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
        let parent = open_directory_no_follow_raw(dir.path()).unwrap();
        let name = validated_child_name(OsStr::new("pipe")).unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let opened = open_child_no_follow(
                &parent,
                &name,
                ChildOpenOptionsV1 {
                    nonblocking: true,
                    directory: false,
                },
            );
            let _ = tx.send(opened.is_ok());
        });
        let opened = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("O_NONBLOCK open of a writerless FIFO must return promptly, not block");
        assert!(opened, "the non-blocking FIFO open should still succeed");
    }

    /// Discriminates `ChildOpenOptionsV1::directory` being dropped: without `O_DIRECTORY` a
    /// regular file is a perfectly valid read-only open, so the caller would receive a handle to
    /// a file where it expected a directory. With it the kernel refuses with `ENOTDIR`.
    #[cfg(unix)]
    #[test]
    fn child_open_options_directory_refuses_a_regular_child() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("regular.txt"), b"not a directory").unwrap();
        let parent = open_directory_no_follow_raw(dir.path()).unwrap();
        let name = validated_child_name(OsStr::new("regular.txt")).unwrap();

        let error = open_child_no_follow(
            &parent,
            &name,
            ChildOpenOptionsV1 {
                nonblocking: false,
                directory: true,
            },
        )
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::ENOTDIR));
    }

    /// Discriminates a `stat_child_no_follow` that reports a symlink's TARGET rather than the
    /// link itself (i.e. one that lost `AT_SYMLINK_NOFOLLOW`), and one that reports a genuinely
    /// absent name as an error rather than `Ok(None)` — the absence answer both the publication
    /// pre-check and `local_file`'s quarantine resolution depend on.
    #[cfg(unix)]
    #[test]
    fn stat_child_no_follow_reports_the_entry_itself_and_distinguishes_absence() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("real.txt"), b"real").unwrap();
        std::os::unix::fs::symlink(dir.path().join("real.txt"), dir.path().join("link.txt"))
            .unwrap();
        let parent = open_directory_no_follow_raw(dir.path()).unwrap();

        let link = stat_child_no_follow(
            &parent,
            &validated_child_name(OsStr::new("link.txt")).unwrap(),
        )
        .unwrap()
        .expect("the link entry exists");
        assert_eq!(
            link.st_mode & libc::S_IFMT,
            libc::S_IFLNK,
            "the ENTRY must be reported, not the regular file it points at"
        );

        assert!(stat_child_no_follow(
            &parent,
            &validated_child_name(OsStr::new("absent")).unwrap()
        )
        .unwrap()
        .is_none());
    }

    /// Discriminates a `rename_child_no_replace` that has lost its no-replace flag and would
    /// silently clobber an existing target — the property the whole atomic publication protocol
    /// rests on. Also proves the refusal leaves BOTH names as they were.
    #[cfg(unix)]
    #[test]
    fn rename_child_no_replace_refuses_an_occupied_target_and_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("source"), b"source").unwrap();
        fs::write(dir.path().join("target"), b"target").unwrap();
        let parent = open_directory_no_follow_raw(dir.path()).unwrap();

        let error = rename_child_no_replace(
            &parent,
            &validated_child_name(OsStr::new("source")).unwrap(),
            &validated_child_name(OsStr::new("target")).unwrap(),
        )
        .unwrap_err();
        // A kernel refusal must arrive as `Io` carrying its errno, never as the compile-time
        // `PlatformUnsupported` claim — the R1 distinction, asserted at the primitive itself.
        match error {
            RenameNoReplaceRefusalV1::Io(error) => {
                assert_eq!(error.raw_os_error(), Some(libc::EEXIST))
            }
            other => panic!("expected an Io refusal carrying EEXIST, got {other:?}"),
        }
        assert_eq!(fs::read(dir.path().join("source")).unwrap(), b"source");
        assert_eq!(fs::read(dir.path().join("target")).unwrap(), b"target");

        rename_child_no_replace(
            &parent,
            &validated_child_name(OsStr::new("source")).unwrap(),
            &validated_child_name(OsStr::new("fresh")).unwrap(),
        )
        .unwrap();
        assert!(!dir.path().join("source").exists());
        assert_eq!(fs::read(dir.path().join("fresh")).unwrap(), b"source");
    }

    /// Discriminates a `FailureCountdownV1` that fires on the wrong call, fires repeatedly once
    /// armed, or fires when it was never armed. Two independent countdowns must also not share
    /// state — `local_file` keeps a sync hook and a journal-publication hook side by side on one
    /// directory, and several of its tests arm both.
    #[test]
    fn failure_countdown_fires_exactly_once_on_exactly_the_armed_call() {
        let countdown = FailureCountdownV1::new();
        assert!(!countdown.fire_if_due(), "an unarmed countdown never fires");

        countdown.arm(3);
        assert!(!countdown.fire_if_due());
        assert!(!countdown.fire_if_due());
        assert!(countdown.fire_if_due(), "the third call must fire");
        assert!(!countdown.fire_if_due(), "and it must disarm afterwards");

        let other = FailureCountdownV1::new();
        countdown.arm(1);
        assert!(!other.fire_if_due(), "countdowns must not share state");
        assert!(countdown.fire_if_due());
    }

    /// Discriminates a `verify_payload_directory_identity` that follows a symlink (would authorize
    /// a destructive act against whatever it points at), accepts a non-directory, or drops the
    /// canonicalize-equals-self check that catches a name whose object has been moved out from
    /// under it.
    #[cfg(unix)]
    #[test]
    fn verify_payload_directory_identity_refuses_symlinks_files_and_moved_names() {
        use std::os::unix::fs::MetadataExt as _;

        let dir = tempfile::tempdir().unwrap();
        // The check compares a path against its own canonical form, so the fixture's ROOT must
        // already be canonical: on macOS a tempdir lives under a symlinked `/var` and every path
        // built from it would otherwise be refused for the fixture's sake rather than the test's.
        let base = fs::canonicalize(dir.path()).unwrap();
        let real = base.join("real");
        fs::create_dir(&real).unwrap();
        let expected = fs::symlink_metadata(&real).unwrap();
        assert_eq!(
            verify_payload_directory_identity(&real).unwrap(),
            (expected.dev(), expected.ino())
        );

        let link = base.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(
            verify_payload_directory_identity(&link).unwrap_err(),
            PayloadIdentityRefusalV1::IsSymlink,
            "a symlink must be refused, never resolved to its target"
        );

        let regular = base.join("regular.txt");
        fs::write(&regular, b"payload").unwrap();
        assert!(matches!(
            verify_payload_directory_identity(&regular).unwrap_err(),
            PayloadIdentityRefusalV1::NotADirectory { .. }
        ));

        let absent = base.join("absent");
        assert!(matches!(
            verify_payload_directory_identity(&absent).unwrap_err(),
            PayloadIdentityRefusalV1::NotADirectory { .. }
        ));
    }

    /// Discriminates a `verify_payload_directory_identity` that keeps the `(dev, ino)` check but
    /// drops the canonicalize-equals-self comparison. A path reached through a symlinked ANCESTOR
    /// stats as a real directory with a real identity, so only the canonical comparison catches
    /// that the caller's name and the object it names disagree.
    #[cfg(unix)]
    #[test]
    fn verify_payload_directory_identity_refuses_a_name_reached_through_a_symlinked_ancestor() {
        let dir = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(dir.path()).unwrap();
        let real_parent = base.join("real-parent");
        fs::create_dir(&real_parent).unwrap();
        fs::create_dir(real_parent.join("payload")).unwrap();
        // Control: the payload reached through its REAL parent is accepted, so the refusal below
        // is attributable to the aliased ancestor and not to the fixture's own layout.
        verify_payload_directory_identity(&real_parent.join("payload")).unwrap();
        std::os::unix::fs::symlink(&real_parent, base.join("aliased-parent")).unwrap();

        let through_alias = base.join("aliased-parent").join("payload");
        match verify_payload_directory_identity(&through_alias).unwrap_err() {
            PayloadIdentityRefusalV1::IdentityChanged { detail } => {
                assert!(
                    detail.contains("now resolves to"),
                    "the refusal must name where it actually resolves, got {detail}"
                );
            }
            other => panic!("expected IdentityChanged, got {other:?}"),
        }
    }

    /// Discriminates a `verify_then_remove` that runs its act without re-checking the payload
    /// identity the gates authorized — the whole point of the boundary, since arbitrary time
    /// passes between the gates and the removal. The act must NOT run.
    #[cfg(unix)]
    #[test]
    fn verify_then_remove_refuses_and_never_acts_when_the_payload_was_exchanged() {
        let root = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(root.path()).unwrap();
        let pinned = PinnedDirectoryV1::open(&base, "exchanged payload").unwrap();
        let payload = base.join("payload");
        fs::create_dir(&payload).unwrap();
        let authorized = verify_payload_directory_identity(&payload).unwrap();

        // The gates authorized THAT directory; a different one is now under the same name.
        fs::remove_dir(&payload).unwrap();
        fs::create_dir(&payload).unwrap();

        let mut acts = 0_u32;
        let verified = verify_then_remove(&pinned, &payload, authorized, || {
            acts += 1;
            Ok(())
        });
        assert_eq!(acts, 0, "the act must not run after a refused recheck");
        match verified {
            VerifiedRemovalV1::Refused(RemovalBoundaryRefusalV1::PayloadIdentityChanged {
                detail,
            }) => assert!(
                detail.contains("changed identity between the gates and the removal"),
                "got {detail}"
            ),
            other => panic!("expected PayloadIdentityChanged, got {other:?}"),
        }
        assert!(payload.exists());
    }

    /// Discriminates a `verify_then_remove` that skips the pre-act pinned-root recheck, or that
    /// reports a vanished payload as `NotADirectory` when the ROOT is the thing that moved: the
    /// root refusal must win, because nothing beneath an unattestable root can be attested.
    #[cfg(unix)]
    #[test]
    fn verify_then_remove_refuses_when_the_pinned_root_moved_before_the_act() {
        let outer = tempfile::tempdir().unwrap();
        let outer = fs::canonicalize(outer.path()).unwrap();
        let root = outer.join("root");
        fs::create_dir(&root).unwrap();
        let payload = root.join("payload");
        fs::create_dir(&payload).unwrap();
        let pinned = PinnedDirectoryV1::open(&root, "moved root").unwrap();
        let authorized = verify_payload_directory_identity(&payload).unwrap();

        // Swap the ROOT for a different directory at the same path. The payload travels with the
        // real root, so `payload` now names nothing — and the assertion below is that the ROOT
        // refusal wins anyway rather than the incidental `PayloadNotADirectory`.
        let replacement = outer.join("replacement");
        fs::create_dir(&replacement).unwrap();
        fs::rename(&root, outer.join("root-moved-away")).unwrap();
        fs::rename(&replacement, &root).unwrap();
        assert!(!payload.exists());

        let mut acts = 0_u32;
        let verified = verify_then_remove(&pinned, &payload, authorized, || {
            acts += 1;
            Ok(())
        });
        assert_eq!(acts, 0, "the act must not run after a refused root recheck");
        assert!(matches!(
            verified,
            VerifiedRemovalV1::Refused(RemovalBoundaryRefusalV1::RootIdentityChanged { .. })
        ));
    }

    /// R2f1b A4 R2. Discriminates a `verify_then_remove` whose pre-act recheck only compares
    /// `(dev, ino)` against the authorized pair and never re-establishes that the payload is
    /// still a DIRECTORY. Between admission and the boundary an actor can replace the payload
    /// with a regular file or with a symlink pointing anywhere; a recursive removal handed either
    /// one acts on the wrong object entirely — the symlink case being the dangerous one, since
    /// following it reaches outside the authorized subtree. Both must produce the typed
    /// `PayloadNotADirectory` refusal, and in both the act must never run.
    ///
    /// Note the symlink lands on `PayloadNotADirectory` rather than a symlink-specific variant:
    /// this is the LAST boundary, whose only question is "is this still exactly what was
    /// authorized"; the symlink/not-a-directory distinction belongs to admission
    /// (`verify_payload_directory_identity`), which is separately covered above.
    #[cfg(unix)]
    #[test]
    fn verify_then_remove_refuses_a_payload_replaced_by_a_file_or_a_symlink() {
        for replacement in ["regular file", "symlink"] {
            let root = tempfile::tempdir().unwrap();
            let base = fs::canonicalize(root.path()).unwrap();
            let pinned = PinnedDirectoryV1::open(&base, "replaced payload").unwrap();
            let payload = base.join("payload");
            fs::create_dir(&payload).unwrap();
            let authorized = verify_payload_directory_identity(&payload).unwrap();

            fs::remove_dir(&payload).unwrap();
            if replacement == "regular file" {
                fs::write(&payload, b"not a directory any more").unwrap();
            } else {
                let elsewhere = base.join("elsewhere");
                fs::create_dir(&elsewhere).unwrap();
                std::os::unix::fs::symlink(&elsewhere, &payload).unwrap();
            }

            let mut acts = 0_u32;
            let verified = verify_then_remove(&pinned, &payload, authorized, || {
                acts += 1;
                Ok(())
            });
            assert_eq!(acts, 0, "[{replacement}] the act must never run");
            match verified {
                VerifiedRemovalV1::Refused(RemovalBoundaryRefusalV1::PayloadNotADirectory {
                    detail,
                }) => assert!(
                    detail.contains("payload"),
                    "[{replacement}] the refusal must name the payload, got {detail}"
                ),
                other => panic!("[{replacement}] expected PayloadNotADirectory, got {other:?}"),
            }
            // And the replacement is untouched — including, for the symlink case, its target.
            assert!(fs::symlink_metadata(&payload).is_ok());
            if replacement == "symlink" {
                assert!(
                    base.join("elsewhere").is_dir(),
                    "the link target must survive"
                );
            }
        }
    }

    /// Discriminates a `verify_then_remove` that trusts the act's RETURN VALUE instead of reading
    /// the filesystem: all four combinations of (reported result, actual presence) must be
    /// distinguishable, because "reported success but still present" and "reported failure but
    /// gone" are the two an exit-status-only reading would silently get wrong.
    #[cfg(unix)]
    #[test]
    fn verify_then_remove_classifies_the_act_by_what_the_filesystem_shows() {
        let root = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(root.path()).unwrap();
        let pinned = PinnedDirectoryV1::open(&base, "classification").unwrap();

        let case = |reported: Result<(), String>, actually_remove: bool| {
            let payload = base.join("payload");
            let _ = fs::remove_dir_all(&payload);
            fs::create_dir(&payload).unwrap();
            let authorized = verify_payload_directory_identity(&payload).unwrap();
            let mut acts = 0_u32;
            let verified = verify_then_remove(&pinned, &payload, authorized, || {
                acts += 1;
                if actually_remove {
                    fs::remove_dir_all(&payload).unwrap();
                }
                reported
            });
            // A destructive act must run EXACTLY once: a retry loop hidden in the boundary would
            // unlink twice, and a boundary that swallowed the act would report a phantom outcome.
            assert_eq!(acts, 1, "the act must run exactly once");
            match verified {
                VerifiedRemovalV1::Acted {
                    observation,
                    root_changed_during,
                } => {
                    assert_eq!(root_changed_during, None);
                    observation
                }
                other => panic!("expected Acted, got {other:?}"),
            }
        };

        assert_eq!(case(Ok(()), true), RemovalObservationV1::Removed);
        assert!(matches!(
            case(Ok(()), false),
            RemovalObservationV1::ReportedSuccessButPresent { .. }
        ));
        match case(Err("disk on fire".into()), false) {
            RemovalObservationV1::Failed { detail } => assert_eq!(detail, "disk on fire"),
            other => panic!("expected Failed, got {other:?}"),
        }
        match case(Err("disk on fire".into()), true) {
            RemovalObservationV1::ReportedFailureButGone { detail } => assert!(
                detail.contains("disk on fire") && detail.contains("the path is gone"),
                "got {detail}"
            ),
            other => panic!("expected ReportedFailureButGone, got {other:?}"),
        }
    }

    /// Discriminates a `verify_then_remove` that drops the POST-act root re-verification. A root
    /// swapped while the removal was in flight means the removal may have landed in the
    /// replacement, so even a textbook-clean `Removed` observation must be reported alongside the
    /// root change rather than presented as an attested deletion.
    #[cfg(unix)]
    #[test]
    fn verify_then_remove_reports_a_root_that_moved_while_the_act_was_in_flight() {
        let outer = tempfile::tempdir().unwrap();
        let outer = fs::canonicalize(outer.path()).unwrap();
        let root = outer.join("root");
        fs::create_dir(&root).unwrap();
        let payload = root.join("payload");
        fs::create_dir(&payload).unwrap();
        let pinned = PinnedDirectoryV1::open(&root, "root moved mid-act").unwrap();
        let authorized = verify_payload_directory_identity(&payload).unwrap();

        let verified = verify_then_remove(&pinned, &payload, authorized, || {
            fs::remove_dir_all(&payload).unwrap();
            let replacement = outer.join("replacement");
            fs::create_dir(&replacement).unwrap();
            fs::rename(&replacement, &root).unwrap();
            Ok(())
        });
        match verified {
            VerifiedRemovalV1::Acted {
                observation,
                root_changed_during,
            } => {
                assert_eq!(observation, RemovalObservationV1::Removed);
                let detail = root_changed_during
                    .expect("a root swapped during the act must be reported, not swallowed");
                assert!(
                    detail.contains("now resolves to a different directory"),
                    "got {detail}"
                );
            }
            other => panic!("expected Acted, got {other:?}"),
        }
    }
}
