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
use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
#[cfg(unix)]
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// A filesystem object's birth timestamp, normalized as signed Unix-epoch seconds plus
/// nanoseconds. The nanosecond component is always in `0..1_000_000_000`, including for times
/// before the epoch, so the serialized pair has one stable representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct BirthTimeV1 {
    secs: i64,
    nanos: u32,
}

impl<'de> Deserialize<'de> for BirthTimeV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct WireBirthTimeV1 {
            secs: i64,
            nanos: u32,
        }

        let wire = WireBirthTimeV1::deserialize(deserializer)?;
        if wire.nanos >= 1_000_000_000 {
            return Err(serde::de::Error::custom(
                "birth timestamp nanoseconds must be less than 1000000000",
            ));
        }
        Ok(Self {
            secs: wire.secs,
            nanos: wire.nanos,
        })
    }
}

impl BirthTimeV1 {
    /// Construct a birth timestamp whose nanosecond component is canonical.
    #[must_use]
    pub const fn new(secs: i64, nanos: u32) -> Option<Self> {
        if nanos < 1_000_000_000 {
            Some(Self { secs, nanos })
        } else {
            None
        }
    }

    /// Convert a `SystemTime` into the canonical signed-seconds representation. A timestamp
    /// outside the `i64` epoch range is unavailable rather than a custody observation error.
    #[must_use]
    pub fn from_system_time(time: SystemTime) -> Option<Self> {
        match time.duration_since(UNIX_EPOCH) {
            Ok(duration) => Some(Self {
                secs: i64::try_from(duration.as_secs()).ok()?,
                nanos: duration.subsec_nanos(),
            }),
            Err(error) => {
                let duration = error.duration();
                let whole_seconds = i64::try_from(duration.as_secs()).ok()?;
                if duration.subsec_nanos() == 0 {
                    Some(Self {
                        secs: whole_seconds.checked_neg()?,
                        nanos: 0,
                    })
                } else {
                    Some(Self {
                        secs: whole_seconds.checked_neg()?.checked_sub(1)?,
                        nanos: 1_000_000_000 - duration.subsec_nanos(),
                    })
                }
            }
        }
    }

    /// Read `Metadata::created()` when the filesystem exposes it. Missing birthtime support is
    /// intentionally represented as `None` and never turns identity observation into an error.
    #[must_use]
    pub fn from_metadata(metadata: &std::fs::Metadata) -> Option<Self> {
        metadata.created().ok().and_then(Self::from_system_time)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryIdentityV1 {
    pub canonical_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dev: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ino: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub btime: Option<BirthTimeV1>,
}

impl DirectoryIdentityV1 {
    /// Match an observed directory identity using birthtime as a strengthening refinement.
    ///
    /// Path, device, and inode retain their exact pre-birthtime semantics. A birthtime mismatch
    /// refuses only when both identities carry one; if either side lacks birthtime, verification
    /// falls back to the legacy `(canonical_path, dev, ino)` verdict so pre-upgrade records keep
    /// verifying exactly as before.
    #[must_use]
    pub fn matches(&self, observed: &Self) -> bool {
        self.canonical_path == observed.canonical_path
            && self.dev == observed.dev
            && self.ino == observed.ino
            && match (self.btime, observed.btime) {
                (Some(expected), Some(actual)) => expected == actual,
                _ => true,
            }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegularFileIdentityV1 {
    pub dev: Option<u64>,
    pub ino: Option<u64>,
    pub len: u64,
    pub btime: BirthTimeV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[rustfmt::skip]
pub struct RequiredObjectIdentityV2 { pub dev: u64, pub ino: u64, pub birthtime: BirthTimeV1 }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[rustfmt::skip]
pub struct FileContentSnapshotV2 { pub object: RequiredObjectIdentityV2, pub content_len: u64 }
#[rustfmt::skip]
pub fn required_object_identity_v2(dev: u64, ino: u64, birthtime: Option<BirthTimeV1>, label: &str) -> Result<RequiredObjectIdentityV2, FsCustodyError> {
    Ok(RequiredObjectIdentityV2 { dev, ino, birthtime: birthtime.ok_or_else(|| FsCustodyError::Unsupported(label.into()))? })
}
pub fn required_file_content_snapshot_v2(
    file: &File,
    label: &str,
) -> Result<FileContentSnapshotV2, FsCustodyError> {
    let RegularFileIdentityV1 {
        dev: Some(dev),
        ino: Some(ino),
        len,
        btime,
    } = regular_file_identity(file, label)?
    else {
        return Err(FsCustodyError::Unsupported(label.into()));
    };
    Ok(FileContentSnapshotV2 {
        object: RequiredObjectIdentityV2 {
            dev,
            ino,
            birthtime: btime,
        },
        content_len: len,
    })
}
pub const MAX_CHILD_NAME_V2_BYTES: usize = 255;
pub const MAX_RESERVED_SOURCE_V2_BYTES: usize = 243;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildNameV2(OsString);
impl ChildNameV2 {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ChildNameRefusalV1> {
        if bytes.len() > MAX_CHILD_NAME_V2_BYTES
            || bytes.is_empty()
            || bytes == b"."
            || bytes == b".."
            || bytes.contains(&b'/')
            || bytes.contains(&b'\\')
        {
            return Err(ChildNameRefusalV1::NotOneComponent);
        }
        if bytes.contains(&0) {
            return Err(ChildNameRefusalV1::ContainsNul);
        }
        let value = std::str::from_utf8(bytes).map_err(|_| ChildNameRefusalV1::NotOneComponent)?;
        Ok(Self(OsString::from(value)))
    }
    pub fn as_os_str(&self) -> &OsStr {
        &self.0
    }
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) fn is_reserved_target(&self) -> bool {
        self.0.as_encoded_bytes().starts_with(b".a2a-v2-")
    }
    pub fn reserved(
        namespace: ReservedNameNamespaceV2,
        target: &Self,
    ) -> Result<Self, FsCustodyError> {
        let target = target.0.to_str().expect("ChildNameV2 is portable UTF-8");
        Self::from_bytes(&[namespace.prefix(), target.as_bytes()].concat())
            .map_err(|_| FsCustodyError::InvalidChildName("reserved child name".into()))
    }
    pub fn parse_reserved(
        expected: ReservedNameNamespaceV2,
        encoded: &Self,
    ) -> Result<Self, FsCustodyError> {
        encoded
            .0
            .to_str()
            .expect("ChildNameV2 is portable UTF-8")
            .as_bytes()
            .strip_prefix(expected.prefix())
            .and_then(|value| Self::from_bytes(value).ok())
            .ok_or_else(|| FsCustodyError::InvalidChildName("reserved child name".into()))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReservedNameNamespaceV2 {
    TransactionIntent,
    Staging,
    ReplacementCapture,
    RetirementCapture,
}
impl ReservedNameNamespaceV2 {
    #[rustfmt::skip]
    pub const ALL: [Self; 4] = [Self::TransactionIntent, Self::Staging, Self::ReplacementCapture, Self::RetirementCapture];
    #[rustfmt::skip]
    fn prefix(self) -> &'static [u8] { [b".a2a-v2-int-", b".a2a-v2-stg-", b".a2a-v2-rpc-", b".a2a-v2-rtc-"][self as usize] }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CustodyOperationKindV2 {
    Replace,
    Retire,
}
#[derive(Debug)]
#[rustfmt::skip]
pub struct CustodyIntentV2(CustodyOperationKindV2, ChildNameV2, RequiredObjectIdentityV2, FileContentSnapshotV2, [ChildNameV2; 4]);
impl CustodyIntentV2 {
    pub fn new(
        operation: CustodyOperationKindV2,
        target: ChildNameV2,
        expected: RequiredObjectIdentityV2,
        staged: FileContentSnapshotV2,
    ) -> Result<Self, FsCustodyError> {
        let [a, b, c, d] =
            ReservedNameNamespaceV2::ALL.map(|value| ChildNameV2::reserved(value, &target));
        Ok(Self(operation, target, expected, staged, [a?, b?, c?, d?]))
    }
    #[rustfmt::skip]
    pub fn parts(&self) -> (CustodyOperationKindV2, &ChildNameV2, &RequiredObjectIdentityV2, &FileContentSnapshotV2) { (self.0, &self.1, &self.2, &self.3) }
    pub fn reserved_name(&self, namespace: ReservedNameNamespaceV2) -> &ChildNameV2 {
        &self.4[namespace as usize]
    }
    pub fn capture_name(&self) -> &ChildNameV2 {
        &self.4[(match self.0 {
            CustodyOperationKindV2::Replace => ReservedNameNamespaceV2::ReplacementCapture,
            CustodyOperationKindV2::Retire => ReservedNameNamespaceV2::RetirementCapture,
        }) as usize]
    }
}
#[must_use]
#[derive(Debug)]
pub enum CustodyCaptureOutcomeV2 {
    RefusedNoEffect(String),
    ExpectedCaptured(RequiredObjectIdentityV2),
    UnexpectedRestored(RequiredObjectIdentityV2),
    Retained(RequiredObjectIdentityV2, String),
    Unknown(String),
    CompileUnsupported,
    RuntimeUnsupported(String),
}
#[cfg(unix)]
#[rustfmt::skip]
pub(crate) fn required_identity_at_v2(parent: &File, name: &OsStr, label: &str) -> Result<RequiredObjectIdentityV2, FsCustodyError> {
    Ok(required_file_content_snapshot_v2(&open_regular_child(parent, name, label)?, label)?.object)
}
#[cfg(unix)]
pub fn capture_target_no_replace_v2(
    parent: &File,
    intent: &CustodyIntentV2,
    label: &str,
) -> CustodyCaptureOutcomeV2 {
    capture_target_no_replace_v2_with(parent, intent, label, |_| {}, rename_child_no_replace)
}
#[cfg(not(unix))]
pub fn capture_target_no_replace_v2(
    _parent: &File,
    _intent: &CustodyIntentV2,
    _label: &str,
) -> CustodyCaptureOutcomeV2 {
    CustodyCaptureOutcomeV2::CompileUnsupported
}
#[cfg(unix)]
fn capture_target_no_replace_v2_with<H, R>(
    parent: &File,
    intent: &CustodyIntentV2,
    label: &str,
    boundary: H,
    rename: R,
) -> CustodyCaptureOutcomeV2
where
    H: FnMut(bool),
    R: FnMut(&File, &CStr, &CStr) -> Result<(), RenameNoReplaceRefusalV1>,
{
    capture_target_no_replace_v2_with_probe(
        parent,
        intent,
        label,
        boundary,
        rename,
        required_identity_at_v2,
    )
}
#[cfg(unix)]
fn capture_target_no_replace_v2_with_probe<H, R, P>(
    parent: &File,
    intent: &CustodyIntentV2,
    label: &str,
    mut boundary: H,
    mut rename: R,
    mut identity_at: P,
) -> CustodyCaptureOutcomeV2
where
    H: FnMut(bool),
    R: FnMut(&File, &CStr, &CStr) -> Result<(), RenameNoReplaceRefusalV1>,
    P: FnMut(&File, &OsStr, &str) -> Result<RequiredObjectIdentityV2, FsCustodyError>,
{
    use CustodyCaptureOutcomeV2::*;
    let target = intent.1.as_os_str();
    let custody = intent.capture_name().as_os_str();
    let target_c = child_name_cstring(target, label).expect("validated target");
    let custody_c = child_name_cstring(custody, label).expect("validated custody");
    let mut at = |name| identity_at(parent, name, label);
    match at(target) {
        Ok(_) => {}
        Err(FsCustodyError::Unsupported(reason)) => {
            return RuntimeUnsupported(format!("{label}: {reason}"))
        }
        Err(error) => {
            return RefusedNoEffect(format!(
                "{label}: pre-capture identity unavailable: {error}"
            ))
        }
    }
    boundary(false);
    let captured = match rename(parent, &target_c, &custody_c) {
        Err(RenameNoReplaceRefusalV1::PlatformUnsupported) => return CompileUnsupported,
        Err(RenameNoReplaceRefusalV1::Io(error))
            if [libc::ENOSYS, libc::ENOTSUP, libc::EOPNOTSUPP]
                .contains(&error.raw_os_error().unwrap_or_default()) =>
        {
            return RuntimeUnsupported(format!("{label}: {error}"))
        }
        Err(RenameNoReplaceRefusalV1::Io(error)) => {
            return Unknown(format!("{label}: capture outcome is unknown: {error}"))
        }
        Ok(()) => match at(custody) {
            Ok(captured) => captured,
            Err(error) => {
                return Unknown(format!("{label}: captured identity is unknown: {error}"))
            }
        },
    };
    if captured == intent.2 {
        return ExpectedCaptured(captured);
    }
    boundary(true);
    Unknown(format!("{label}: captured unexpected target identity"))
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
    #[error("{label}: enumeration exceeds the {limit}-child bound")]
    EnumerationLimitExceeded { label: String, limit: usize },
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

    // Consumed only by the unix custody arms; non-unix keeps the shape without callers.
    #[cfg_attr(not(unix), allow(dead_code))]
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
#[cfg_attr(not(unix), allow(dead_code))]
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
        if !identity.matches(&before) || !identity.matches(&after) {
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

    #[cfg_attr(not(unix), allow(dead_code))]
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

    /// Unix-only: the lease is `flock`-backed, and `crate::liveness` is itself `cfg(unix)`.
    /// Its only caller (the preparation flight's terminal replacement) is unix-gated too. This
    /// follows the established 3b1/3c1/3c2 guard pattern for the non-unix lane.
    #[cfg(unix)]
    pub fn with_existing_regular_child_lease<T, F>(
        &self,
        name: &OsStr,
        label: &str,
        operation: F,
    ) -> Result<T, FsCustodyError>
    where
        F: FnOnce(&File) -> T,
    {
        let opened = self.open_regular_file(name, label)?;
        let _lease = crate::liveness::acquire_persistent_lock_file(
            opened
                .try_clone()
                .map_err(|error| FsCustodyError::Io(label.to_owned(), error))?,
            self.canonical_path.join(Path::new(name)),
        )
        .map_err(|error| FsCustodyError::Io(label.to_owned(), error))?;
        let current = self.open_regular_file(name, label)?;
        if !same_regular_file(&opened, &current, label)? {
            return Err(FsCustodyError::IdentityChanged(label.to_owned()));
        }
        Ok(operation(&opened))
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

pub type ObjectIdentityV2 = RequiredObjectIdentityV2;

#[derive(Clone, Debug)]
#[cfg_attr(not(unix), allow(dead_code))]
pub struct JournalRootBindingV2 {
    anchor: ObjectIdentityV2,
    parent_name: ChildNameV2,
    parent: ObjectIdentityV2,
    root_name: ChildNameV2,
    root: ObjectIdentityV2,
    operation_lock_name: ChildNameV2,
    operation_lock: ObjectIdentityV2,
}

impl JournalRootBindingV2 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        anchor: ObjectIdentityV2,
        parent_name: ChildNameV2,
        parent: ObjectIdentityV2,
        root_name: ChildNameV2,
        root: ObjectIdentityV2,
        operation_lock_name: ChildNameV2,
        operation_lock: ObjectIdentityV2,
    ) -> Result<Self, FsCustodyError> {
        if root_name == operation_lock_name {
            return Err(FsCustodyError::InvalidChildName(
                "journal root and operation lock must be siblings".into(),
            ));
        }
        Ok(Self {
            anchor,
            parent_name,
            parent,
            root_name,
            root,
            operation_lock_name,
            operation_lock,
        })
    }
}

#[cfg_attr(not(unix), allow(dead_code))]
pub struct JournalRootCustodyV2 {
    anchor: PinnedDirectoryV1,
    parent: PinnedDirectoryV1,
    root: PinnedDirectoryV1,
    binding: JournalRootBindingV2,
    operation_mutex: std::sync::Mutex<()>,
    protective_debt: AtomicU8,
    append_limit: AtomicUsize,
    file_sync_failure: FailureCountdownV1,
}

#[cfg_attr(not(unix), allow(dead_code))]
pub struct JournalRootOperationV2<'a> {
    _mutex: std::sync::MutexGuard<'a, ()>,
    lock: File,
    label: String,
    custody: &'a JournalRootCustodyV2,
}

#[cfg(unix)]
impl Drop for JournalRootOperationV2<'_> {
    fn drop(&mut self) {
        crate::liveness::flock_unlock(&self.lock, &self.label);
    }
}

#[cfg(unix)]
fn verify_directory_identity_v2(
    file: &File,
    expected: ObjectIdentityV2,
    label: &str,
) -> Result<(), FsCustodyError> {
    use std::os::unix::fs::MetadataExt as _;
    let metadata = file
        .metadata()
        .map_err(|error| FsCustodyError::Io(label.to_owned(), error))?;
    if !metadata.is_dir() {
        return Err(FsCustodyError::Unsupported(format!(
            "{label}: route object is not a directory"
        )));
    }
    let observed = required_object_identity_v2(
        metadata.dev(),
        metadata.ino(),
        BirthTimeV1::from_metadata(&metadata),
        label,
    )?;
    if observed != expected {
        return Err(FsCustodyError::IdentityChanged(label.to_owned()));
    }
    Ok(())
}

#[cfg(unix)]
fn verify_regular_identity_v2(
    file: &File,
    expected: ObjectIdentityV2,
    label: &str,
) -> Result<(), FsCustodyError> {
    if required_file_content_snapshot_v2(file, label)?.object != expected {
        return Err(FsCustodyError::IdentityChanged(label.to_owned()));
    }
    Ok(())
}

#[cfg(unix)]
impl JournalRootCustodyV2 {
    pub fn open(
        anchor_path: &Path,
        binding: &JournalRootBindingV2,
        label: &str,
    ) -> Result<Self, FsCustodyError> {
        let anchor = PinnedDirectoryV1::open(anchor_path, label)?;
        verify_directory_identity_v2(&anchor.file, binding.anchor, label)?;
        let parent = open_directory_child(&anchor, binding.parent_name.as_os_str(), label)?;
        verify_directory_identity_v2(&parent.file, binding.parent, label)?;
        let root = open_directory_child(&parent, binding.root_name.as_os_str(), label)?;
        verify_directory_identity_v2(&root.file, binding.root, label)?;
        let lock =
            open_regular_child(&parent.file, binding.operation_lock_name.as_os_str(), label)?;
        verify_regular_identity_v2(&lock, binding.operation_lock, label)?;
        let custody = Self {
            anchor,
            parent,
            root,
            binding: binding.clone(),
            operation_mutex: std::sync::Mutex::new(()),
            protective_debt: AtomicU8::new(0),
            append_limit: AtomicUsize::new(usize::MAX),
            file_sync_failure: FailureCountdownV1::new(),
        };
        custody.prove_route(label)?;
        Ok(custody)
    }

    pub fn begin_operation(
        &self,
        label: &str,
    ) -> Result<JournalRootOperationV2<'_>, FsCustodyError> {
        self.begin_operation_with(
            label,
            || {},
            |file| crate::liveness::flock_nb(file, true),
            || {},
        )
    }

    /// Open and nonblocking-flock one existing regular child without entering the journal
    /// operation lock. The returned snapshot lets the caller validate content without receiving
    /// the file or any route path; pre/post route and name checks bind the held inode to `name`.
    pub(crate) fn acquire_existing_regular_child_lease(
        &self,
        name: &ChildNameV2,
        label: &str,
    ) -> Result<(crate::liveness::PersistentLockGuard, FileContentSnapshotV2), FsCustodyError> {
        self.prove_route(label)?;
        let file = open_regular_child(&self.root.file, name.as_os_str(), label)?;
        let opened = required_file_content_snapshot_v2(&file, label)?;
        let lease = crate::liveness::acquire_persistent_lock_file(file, PathBuf::from(label))
            .map_err(|error| {
                if [libc::ENOSYS, libc::ENOTSUP, libc::ENOLCK]
                    .contains(&error.raw_os_error().unwrap_or_default())
                {
                    FsCustodyError::Unsupported(label.to_owned())
                } else {
                    FsCustodyError::Io(label.to_owned(), error)
                }
            })?;
        self.prove_route(label)?;
        let current = required_file_content_snapshot_v2(
            &open_regular_child(&self.root.file, name.as_os_str(), label)?,
            label,
        )?;
        if current.object != opened.object {
            return Err(FsCustodyError::IdentityChanged(label.to_owned()));
        }
        Ok((lease, current))
    }

    fn begin_operation_with<E, F, H>(
        &self,
        label: &str,
        entered: E,
        try_flock: F,
        after_lock: H,
    ) -> Result<JournalRootOperationV2<'_>, FsCustodyError>
    where
        E: FnOnce(),
        F: FnOnce(&File) -> std::io::Result<bool>,
        H: FnOnce(),
    {
        entered();
        let mutex = self.operation_mutex.lock().map_err(|_| {
            FsCustodyError::Unsupported(format!("{label}: operation mutex is poisoned"))
        })?;
        let lock = open_regular_child(
            &self.parent.file,
            self.binding.operation_lock_name.as_os_str(),
            label,
        )?;
        verify_regular_identity_v2(&lock, self.binding.operation_lock, label)?;
        let acquired = try_flock(&lock).map_err(|error| {
            let raw = error.raw_os_error();
            if raw == Some(libc::ENOSYS) || raw == Some(libc::ENOTSUP) || raw == Some(libc::ENOLCK)
            {
                FsCustodyError::Unsupported(label.to_owned())
            } else {
                FsCustodyError::Io(label.to_owned(), error)
            }
        })?;
        if !acquired {
            return Err(FsCustodyError::Io(
                label.to_owned(),
                std::io::ErrorKind::WouldBlock.into(),
            ));
        }
        let operation = JournalRootOperationV2 {
            _mutex: mutex,
            lock,
            label: label.to_owned(),
            custody: self,
        };
        after_lock();
        self.prove_route(label)?;
        Ok(operation)
    }

    fn prove_route(&self, label: &str) -> Result<(), FsCustodyError> {
        verify_directory_identity_v2(&self.anchor.file, self.binding.anchor, label)?;
        verify_directory_identity_v2(&self.parent.file, self.binding.parent, label)?;
        verify_directory_identity_v2(&self.root.file, self.binding.root, label)?;
        let anchor = open_directory_no_follow(&self.anchor.canonical_path, label)?;
        verify_directory_identity_v2(&anchor, self.binding.anchor, label)?;
        let parent =
            open_directory_child_file(&anchor, self.binding.parent_name.as_os_str(), label)?;
        verify_directory_identity_v2(&parent, self.binding.parent, label)?;
        let root = open_directory_child_file(&parent, self.binding.root_name.as_os_str(), label)?;
        verify_directory_identity_v2(&root, self.binding.root, label)
    }
}

#[cfg(unix)]
impl JournalRootOperationV2<'_> {
    pub(crate) fn root_file(&self) -> &File {
        &self.custody.root.file
    }
    pub(crate) fn prove_route_v2(&self, label: &str) -> Result<(), FsCustodyError> {
        self.custody.prove_route(label)
    }
}

#[cfg(not(unix))]
impl JournalRootCustodyV2 {
    pub fn open(
        _anchor_path: &Path,
        _binding: &JournalRootBindingV2,
        label: &str,
    ) -> Result<Self, FsCustodyError> {
        Err(FsCustodyError::Unsupported(label.to_owned()))
    }

    pub fn begin_operation(
        &self,
        label: &str,
    ) -> Result<JournalRootOperationV2<'_>, FsCustodyError> {
        Err(FsCustodyError::Unsupported(label.to_owned()))
    }
}
#[must_use]
#[derive(Debug, PartialEq, Eq)]
pub enum JournalMutationOutcomeV2 {
    Complete,
    Refused(String),
    Retained(String),
    ProtectiveDebt(String),
    Unsupported(String),
}
#[cfg(unix)]
struct OwnedJournalWriteV2<'a, 'b>(&'a JournalRootOperationV2<'b>, File);
#[cfg(unix)]
impl JournalRootOperationV2<'_> {
    fn failed(&self, error: FsCustodyError, changed: bool) -> JournalMutationOutcomeV2 {
        match error {
            error if changed => self.retained(error.to_string()),
            FsCustodyError::Unsupported(reason) => JournalMutationOutcomeV2::Unsupported(reason),
            error => JournalMutationOutcomeV2::Refused(error.to_string()),
        }
    }
    pub(crate) fn record_protective_debt<T>(&self, outcome: T) -> T {
        self.custody.protective_debt.store(1, Ordering::SeqCst);
        outcome
    }
    pub(crate) fn clear_protective_debt(&self) {
        self.custody.protective_debt.store(0, Ordering::SeqCst);
    }
    fn retained(&self, reason: String) -> JournalMutationOutcomeV2 {
        self.record_protective_debt(JournalMutationOutcomeV2::Retained(reason))
    }
    fn protective(&self, reason: impl Into<String>) -> JournalMutationOutcomeV2 {
        self.record_protective_debt(JournalMutationOutcomeV2::ProtectiveDebt(reason.into()))
    }
    fn refuse_reserved_target(&self, target: &ChildNameV2) -> Result<(), JournalMutationOutcomeV2> {
        if target.is_reserved_target() {
            Err(JournalMutationOutcomeV2::Refused("reserved target".into()))
        } else {
            Ok(())
        }
    }
    fn refuse_debt(&self, label: &str) -> Result<(), JournalMutationOutcomeV2> {
        if self.debt() {
            Err(JournalMutationOutcomeV2::ProtectiveDebt(format!(
                "{label}: protective debt"
            )))
        } else {
            Ok(())
        }
    }
    pub(crate) fn guard(
        &self,
        allowed: Option<&ChildNameV2>,
        footprint: usize,
        label: &str,
    ) -> Result<(), JournalMutationOutcomeV2> {
        self.refuse_debt(label)?;
        self.prove_route_v2(label)
            .map_err(|error| self.failed(error, false))?;
        let names = match enumerate_directory_names(self.root_file(), 4096, label) {
            Ok(names) => names,
            Err(FsCustodyError::EnumerationLimitExceeded { .. }) => {
                return Err(self.protective(format!("{label}: census exceeds capacity")))
            }
            Err(error) => return Err(self.failed(error, false)),
        };
        let residue = names.iter().any(|name| {
            name.as_bytes().starts_with(b".a2a-v2-")
                && allowed.is_none_or(|value| name != value.as_os_str())
        });
        self.prove_route_v2(label)
            .map_err(|error| self.failed(error, false))?;
        if self.debt() || residue || names.len().saturating_add(footprint) > 4096 {
            return Err(self.protective(format!("{label}: protective debt")));
        }
        Ok(())
    }
    #[allow(dead_code)]
    pub(crate) fn recovery_debt(&self, label: &str) -> Result<bool, FsCustodyError> {
        self.prove_route_v2(label)?;
        let residue = enumerate_directory_names(self.root_file(), 4096, label)?
            .into_iter()
            .any(|name| name.as_bytes().starts_with(b".a2a-v2-"));
        self.prove_route_v2(label)?;
        Ok(self.debt() || residue)
    }
    pub(crate) fn debt(&self) -> bool {
        self.custody.protective_debt.load(Ordering::SeqCst) != 0
    }
    fn settle(
        &self,
        session: &OwnedJournalWriteV2<'_, '_>,
        label: &str,
    ) -> Result<FileContentSnapshotV2, JournalMutationOutcomeV2> {
        if self.custody.file_sync_failure.fire_if_due() {
            return Err(self.failed(FsCustodyError::InjectedSync(label.into()), true));
        }
        session
            .1
            .sync_all()
            .map_err(|error| self.failed(FsCustodyError::Io(label.into(), error), true))?;
        let snapshot = required_file_content_snapshot_v2(&session.1, label)
            .map_err(|error| self.failed(error, true))?;
        session
            .0
            .prove_route_v2(label)
            .map_err(|error| self.failed(error, true))?;
        self.custody
            .root
            .sync(label)
            .map_err(|error| self.failed(error, true))?;
        session
            .0
            .prove_route_v2(label)
            .map_err(|error| self.failed(error, true))?;
        Ok(snapshot)
    }
    fn rollback_append(
        &self,
        session: &OwnedJournalWriteV2<'_, '_>,
        expected: FileContentSnapshotV2,
        label: &str,
    ) -> bool {
        session.1.set_len(expected.content_len).is_ok()
            && session.1.sync_all().is_ok()
            && matches!(required_file_content_snapshot_v2(&session.1, label), Ok(value) if value == expected)
            && self.custody.root.sync(label).is_ok()
            && session.0.prove_route_v2(label).is_ok()
    }
    fn retain_append_debt(
        &self,
        target: &ChildNameV2,
        label: &str,
        reason: String,
    ) -> JournalMutationOutcomeV2 {
        let _file_synced = ChildNameV2::reserved(ReservedNameNamespaceV2::Staging, target)
            .ok()
            .and_then(|name| {
                create_new_regular_child_at(self.root_file(), name.as_os_str(), label).ok()
            })
            .is_some_and(|file| file.sync_all().is_ok());
        let _root_synced = self.custody.root.sync(label).is_ok();
        self.retained(reason)
    }
    pub fn stage(
        &self,
        target: &ChildNameV2,
        bytes: &[u8],
        label: &str,
    ) -> Result<FileContentSnapshotV2, JournalMutationOutcomeV2> {
        self.refuse_debt(label)?;
        self.guard(None, 1, label)?;
        self.refuse_reserved_target(target)?;
        let name = ChildNameV2::reserved(ReservedNameNamespaceV2::Staging, target)
            .map_err(|error| self.failed(error, false))?;
        let file = create_new_regular_child_at(self.root_file(), name.as_os_str(), label)
            .map_err(|error| self.failed(error, false))?;
        let mut session = OwnedJournalWriteV2(self, file);
        session
            .1
            .write_all(bytes)
            .map_err(|error| self.failed(FsCustodyError::Io(label.into(), error), true))?;
        self.settle(&session, label)
    }
    pub fn publish(
        &self,
        target: &ChildNameV2,
        staged: FileContentSnapshotV2,
        label: &str,
    ) -> Result<FileContentSnapshotV2, JournalMutationOutcomeV2> {
        self.publish_with(target, staged, label, || {})
    }
    fn publish_with<F: FnOnce()>(
        &self,
        target: &ChildNameV2,
        staged: FileContentSnapshotV2,
        label: &str,
        before_publish: F,
    ) -> Result<FileContentSnapshotV2, JournalMutationOutcomeV2> {
        self.refuse_debt(label)?;
        // The census runs before every other fallible step: a failed staging
        // name derivation or a reserved target must not preempt residue
        // classification, and a staging name derived from a reserved target
        // must not be whitelisted.
        let derived = ChildNameV2::reserved(ReservedNameNamespaceV2::Staging, target);
        let allowed = match &derived {
            Ok(name) if !target.is_reserved_target() => Some(name),
            _ => None,
        };
        self.guard(allowed, 0, label)?;
        self.refuse_reserved_target(target)?;
        let name = derived.map_err(|error| self.failed(error, false))?;
        let file = open_regular_child(self.root_file(), name.as_os_str(), label)
            .map_err(|error| self.failed(error, true))?;
        let observed = required_file_content_snapshot_v2(&file, label)
            .map_err(|error| self.failed(error, true))?;
        if observed != staged {
            return Err(self.retained(format!("{label}: staged object changed")));
        }
        match self
            .custody
            .root
            .publish_new_regular_child_with_before_rename(
                RegularChildRefV1::new(name.as_os_str(), &file),
                target.as_os_str(),
                label,
                || {
                    before_publish();
                    self.prove_route_v2(label)
                },
            ) {
            Ok(CustodyPublicationV1::Durable { .. }) => {
                self.prove_route_v2(label)
                    .map_err(|error| self.failed(error, true))?;
                Ok(staged)
            }
            Ok(value) => Err(self.retained(format!("{value:?}"))),
            Err(error) => Err(self.failed(error, true)),
        }
    }
    pub fn append(
        &self,
        name: &ChildNameV2,
        expected: FileContentSnapshotV2,
        position: u64,
        bytes: &[u8],
        label: &str,
    ) -> Result<FileContentSnapshotV2, JournalMutationOutcomeV2> {
        self.refuse_debt(label)?;
        self.guard(None, 1, label)?;
        self.refuse_reserved_target(name)?;
        let file = open_regular_child_for_update(self.root_file(), name.as_os_str(), true, label)
            .map_err(|error| self.failed(error, false))?;
        let observed = required_file_content_snapshot_v2(&file, label)
            .map_err(|error| self.failed(error, false))?;
        if observed != expected || position != expected.content_len {
            return Err(JournalMutationOutcomeV2::Refused(label.into()));
        }
        let mut session = OwnedJournalWriteV2(self, file);
        self.prove_route_v2(label)
            .map_err(|error| self.failed(error, true))?;
        let limit = self.custody.append_limit.swap(usize::MAX, Ordering::SeqCst);
        let write = session.1.write(&bytes[..bytes.len().min(limit)]);
        let result = match write {
            Ok(count) if count == bytes.len() => self.settle(&session, label),
            Ok(_) => Err(self.retained(format!("{label}: partial append"))),
            Err(error) => Err(self.failed(FsCustodyError::Io(label.into(), error), true)),
        };
        match result {
            Err(_) if self.rollback_append(&session, expected, label) => {
                self.clear_protective_debt();
                Err(JournalMutationOutcomeV2::Refused(format!(
                    "{label}: append rolled back"
                )))
            }
            Err(error) => Err(self.retain_append_debt(name, label, format!("{error:?}"))),
            value => value,
        }
    }
    pub fn read(
        &self,
        name: &ChildNameV2,
        expected: FileContentSnapshotV2,
        limit: usize,
        label: &str,
    ) -> Result<Vec<u8>, FsCustodyError> {
        self.prove_route_v2(label)?;
        let mut file = open_regular_child(self.root_file(), name.as_os_str(), label)?;
        if expected.content_len > limit as u64
            || required_file_content_snapshot_v2(&file, label)? != expected
        {
            return Err(FsCustodyError::IdentityChanged(label.into()));
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take(limit.saturating_add(1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| FsCustodyError::Io(label.into(), error))?;
        if bytes.len() as u64 != expected.content_len
            || required_file_content_snapshot_v2(&file, label)? != expected
        {
            return Err(FsCustodyError::IdentityChanged(label.into()));
        }
        self.prove_route_v2(label)?;
        Ok(bytes)
    }
    pub fn enumerate(&self, limit: usize, label: &str) -> Result<Vec<OsString>, FsCustodyError> {
        self.prove_route_v2(label)?;
        let names = enumerate_directory_names(self.root_file(), limit, label)?;
        self.prove_route_v2(label)?;
        Ok(names)
    }
    pub fn sync(&self, label: &str) -> JournalMutationOutcomeV2 {
        if let Err(value) = self.guard(None, 0, label) {
            return value;
        }
        match self
            .custody
            .root
            .sync(label)
            .and_then(|_| self.prove_route_v2(label))
        {
            Ok(()) => JournalMutationOutcomeV2::Complete,
            Err(error) => self.failed(error, true),
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
        btime: BirthTimeV1::from_metadata(&metadata),
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
        btime: None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathIdentityComparisonV1 {
    Same,
    Different,
    CannotProve,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PathObjectIdentityV1 {
    #[cfg(unix)]
    Unix { dev: u64, ino: u64 },
}
fn path_object_identity(metadata: &std::fs::Metadata) -> Option<PathObjectIdentityV1> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        Some(PathObjectIdentityV1::Unix {
            dev: metadata.dev(),
            ino: metadata.ino(),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
}
struct DeepestExistingPathV1 {
    canonical: PathBuf,
    identity: PathObjectIdentityV1,
    missing_tail: Vec<OsString>,
}
fn deepest_existing_path(path: &Path) -> Option<DeepestExistingPathV1> {
    if !path.is_absolute() {
        return None;
    }
    let mut current = path.to_path_buf();
    let mut missing_tail = Vec::new();
    loop {
        match std::fs::metadata(&current) {
            Ok(metadata) => {
                let identity = path_object_identity(&metadata)?;
                let canonical = std::fs::canonicalize(&current).ok()?;
                if path_object_identity(&std::fs::metadata(&canonical).ok()?)? != identity {
                    return None;
                }
                missing_tail.reverse();
                return Some(DeepestExistingPathV1 {
                    canonical,
                    identity,
                    missing_tail,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match std::fs::symlink_metadata(&current) {
                    Ok(_) => return None,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => return None,
                }
                missing_tail.push(current.file_name()?.to_os_string());
                current = current.parent()?.to_path_buf();
            }
            Err(_) => return None,
        }
    }
}
fn alternate_ascii_case(name: &std::ffi::OsStr) -> Option<OsString> {
    let mut bytes = name.to_str()?.as_bytes().to_vec();
    let byte = bytes.iter_mut().find(|byte| byte.is_ascii_alphabetic())?;
    *byte = if byte.is_ascii_lowercase() {
        byte.to_ascii_uppercase()
    } else {
        byte.to_ascii_lowercase()
    };
    Some(String::from_utf8(bytes).ok()?.into())
}
fn probe_case_sensitivity(
    parent: &Path,
    name: &std::ffi::OsStr,
    expected: PathObjectIdentityV1,
) -> Option<bool> {
    let alternate = alternate_ascii_case(name)?;
    match std::fs::symlink_metadata(parent.join(alternate)) {
        Ok(metadata) => Some(path_object_identity(&metadata)? != expected),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(true),
        Err(_) => None,
    }
}
fn case_sensitive_at(ancestor: &Path) -> Option<bool> {
    if let (Some(parent), Some(name)) = (ancestor.parent(), ancestor.file_name()) {
        let expected = path_object_identity(&std::fs::symlink_metadata(ancestor).ok()?)?;
        if let Some(answer) = probe_case_sensitivity(parent, name, expected) {
            return Some(answer);
        }
    }
    let entries = std::fs::read_dir(ancestor).ok()?;
    for entry in entries.take(64) {
        let entry = entry.ok()?;
        let expected = path_object_identity(&std::fs::symlink_metadata(entry.path()).ok()?)?;
        if let Some(answer) = probe_case_sensitivity(ancestor, &entry.file_name(), expected) {
            return Some(answer);
        }
    }
    None
}
fn compare_missing_tail(
    left: &[OsString],
    right: &[OsString],
    case_sensitive: bool,
) -> PathIdentityComparisonV1 {
    if left.len() != right.len() {
        return PathIdentityComparisonV1::Different;
    }
    // Scan EVERY differing component. A single proven-different component settles the whole
    // path, so returning `CannotProve` at the first ambiguous one would refuse pairs like
    // ["wt", "child-a"] vs ["WT", "child-b"], whose second component is plainly different.
    let mut ambiguous = false;
    for (left, right) in left.iter().zip(right) {
        if left == right {
            continue;
        }
        if case_sensitive {
            return PathIdentityComparisonV1::Different;
        }
        let (Some(left), Some(right)) = (left.to_str(), right.to_str()) else {
            ambiguous = true;
            continue;
        };
        if left.eq_ignore_ascii_case(right) || ascii_skeletons_could_normalize_alike(left, right) {
            ambiguous = true;
            continue;
        }
        return PathIdentityComparisonV1::Different;
    }
    if ambiguous {
        PathIdentityComparisonV1::CannotProve
    } else {
        PathIdentityComparisonV1::Different
    }
}

/// Could these two names be canonical-equivalent spellings of one entry, without consulting a
/// Unicode table?
///
/// Canonical decomposition only ever ADDS ASCII base letters — NFC `é` decomposes to ASCII `e`
/// plus a combining acute — and never removes one. So if two names are canonical equivalents,
/// one's ASCII-letter skeleton must be a subsequence of the other's. That is a NECESSARY
/// condition, so failing it PROVES the names differ.
///
/// Refusing on any non-ASCII byte instead (the previous rule) was sound but far too blunt: it
/// classified `équipe` vs `other` as `CannotProve`, which would leave the exact-absence proof
/// unable to authorize in any repository holding a single non-ASCII name.
fn ascii_skeletons_could_normalize_alike(left: &str, right: &str) -> bool {
    fn skeleton(value: &str) -> Vec<u8> {
        value
            .bytes()
            .filter(u8::is_ascii)
            .map(|byte| byte.to_ascii_lowercase())
            .collect()
    }
    fn is_subsequence(needle: &[u8], haystack: &[u8]) -> bool {
        let mut haystack = haystack.iter();
        needle
            .iter()
            .all(|wanted| haystack.any(|candidate| candidate == wanted))
    }
    // Pure-ASCII names have NO canonical-equivalent spellings, so normalization cannot relate
    // them and the skeleton test must not be consulted: `w` and `wt` are plainly different names,
    // yet `w`'s skeleton IS a subsequence of `wt`'s. Only reach for the skeleton once a non-ASCII
    // byte is actually in play.
    if left.is_ascii() && right.is_ascii() {
        return false;
    }
    let (left, right) = (skeleton(left), skeleton(right));
    is_subsequence(&left, &right) || is_subsequence(&right, &left)
}
pub fn compare_path_identities(
    left: impl AsRef<Path>,
    right: impl AsRef<Path>,
) -> PathIdentityComparisonV1 {
    let (Some(left), Some(right)) = (
        deepest_existing_path(left.as_ref()),
        deepest_existing_path(right.as_ref()),
    ) else {
        return PathIdentityComparisonV1::CannotProve;
    };
    if left.identity != right.identity {
        return PathIdentityComparisonV1::Different;
    }
    if left.missing_tail == right.missing_tail {
        return PathIdentityComparisonV1::Same;
    }
    let Some(case_sensitive) = case_sensitive_at(&left.canonical) else {
        return PathIdentityComparisonV1::CannotProve;
    };
    compare_missing_tail(&left.missing_tail, &right.missing_tail, case_sensitive)
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
        btime: BirthTimeV1::from_metadata(&metadata),
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
        btime: None,
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
fn open_directory_child_file(
    parent: &File,
    name: &OsStr,
    label: &str,
) -> Result<File, FsCustodyError> {
    let name = child_name_cstring(name, label)?;
    open_child_no_follow(
        parent,
        &name,
        ChildOpenOptionsV1 {
            nonblocking: false,
            directory: true,
        },
    )
    .map_err(|error| FsCustodyError::Io(label.to_owned(), error))
}
#[cfg(unix)]
fn open_directory_child(
    parent: &PinnedDirectoryV1,
    name: &OsStr,
    label: &str,
) -> Result<PinnedDirectoryV1, FsCustodyError> {
    let file = open_directory_child_file(&parent.file, name, label)?;
    let canonical_path = parent.canonical_path.join(name);
    let identity = directory_identity(&canonical_path, &file, label)?;
    Ok(PinnedDirectoryV1 {
        file,
        canonical_path,
        identity,
        sync_failure_countdown: FailureCountdownV1::new(),
        publication_rename_failure_countdown: FailureCountdownV1::new(),
        publication_rename_failure_shape: AtomicU8::new(0),
    })
}
pub fn regular_file_identity(
    file: &File,
    label: &str,
) -> Result<RegularFileIdentityV1, FsCustodyError> {
    let metadata = file
        .metadata()
        .map_err(|error| FsCustodyError::Io(label.to_owned(), error))?;
    if !metadata.is_file() {
        return Err(FsCustodyError::Unsupported(format!(
            "{label}: child is not a regular file"
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        Ok(RegularFileIdentityV1 {
            dev: Some(metadata.dev()),
            ino: Some(metadata.ino()),
            len: metadata.len(),
            btime: BirthTimeV1::from_metadata(&metadata)
                .ok_or_else(|| FsCustodyError::Unsupported(label.to_owned()))?,
        })
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Err(FsCustodyError::Unsupported(label.to_owned()))
    }
}
#[cfg(unix)]
pub(crate) fn create_new_regular_child_at(
    parent: &File,
    name: &OsStr,
    label: &str,
) -> Result<File, FsCustodyError> {
    use std::os::fd::{AsRawFd as _, FromRawFd as _};
    let name = child_name_cstring(name, label)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_CREAT | libc::O_EXCL | libc::O_RDWR | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o600,
        )
    };
    if fd == -1 {
        let error = std::io::Error::last_os_error();
        return if error.raw_os_error() == Some(libc::EEXIST) {
            Err(FsCustodyError::TargetExists(label.to_owned()))
        } else {
            Err(FsCustodyError::Io(label.to_owned(), error))
        };
    }
    let file = unsafe { File::from_raw_fd(fd) };
    regular_file_identity(&file, label)?;
    Ok(file)
}
#[cfg(unix)]
fn open_regular_child_for_update(
    parent: &File,
    name: &OsStr,
    append: bool,
    label: &str,
) -> Result<File, FsCustodyError> {
    use std::os::fd::{AsRawFd as _, FromRawFd as _};
    let name = child_name_cstring(name, label)?;
    let mut flags = libc::O_RDWR | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK;
    if append {
        flags |= libc::O_APPEND;
    }
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if fd == -1 {
        return Err(FsCustodyError::Io(
            label.to_owned(),
            std::io::Error::last_os_error(),
        ));
    }
    let file = unsafe { File::from_raw_fd(fd) };
    regular_file_identity(&file, label)?;
    Ok(file)
}
#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
struct DirectoryStreamV1(*mut libc::DIR);
#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
impl Drop for DirectoryStreamV1 {
    fn drop(&mut self) {
        unsafe { libc::closedir(self.0) };
    }
}
#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
fn errno_location() -> *mut libc::c_int {
    #[cfg(target_os = "linux")]
    unsafe {
        libc::__errno_location()
    }
    #[cfg(target_os = "macos")]
    unsafe {
        libc::__error()
    }
}
#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
pub(crate) fn enumerate_directory_names(
    directory: &File,
    limit: usize,
    label: &str,
) -> Result<Vec<OsString>, FsCustodyError> {
    use std::os::fd::AsRawFd as _;
    use std::os::unix::ffi::OsStringExt as _;
    let fd = unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if fd == -1 {
        return Err(FsCustodyError::Io(
            label.to_owned(),
            std::io::Error::last_os_error(),
        ));
    }
    let stream = unsafe { libc::fdopendir(fd) };
    if stream.is_null() {
        unsafe { libc::close(fd) };
        return Err(FsCustodyError::Io(
            label.to_owned(),
            std::io::Error::last_os_error(),
        ));
    }
    let stream = DirectoryStreamV1(stream);
    unsafe { libc::rewinddir(stream.0) };
    let mut names = Vec::new();
    loop {
        unsafe { *errno_location() = 0 };
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            let errno = unsafe { *errno_location() };
            if errno == 0 {
                break;
            }
            return Err(FsCustodyError::Io(
                label.to_owned(),
                std::io::Error::from_raw_os_error(errno),
            ));
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        if names.len() == limit {
            return Err(FsCustodyError::EnumerationLimitExceeded {
                label: label.to_owned(),
                limit,
            });
        }
        let name = OsString::from_vec(name.to_vec());
        validated_child_name(&name)
            .map_err(|_| FsCustodyError::InvalidChildName(label.to_owned()))?;
        names.push(name);
    }
    Ok(names)
}
#[cfg(not(all(unix, any(target_os = "linux", target_os = "macos"))))]
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) fn enumerate_directory_names(
    _directory: &File,
    _limit: usize,
    label: &str,
) -> Result<Vec<OsString>, FsCustodyError> {
    Err(FsCustodyError::Unsupported(label.to_owned()))
}
#[cfg(unix)]
pub(crate) fn child_name_cstring(name: &OsStr, label: &str) -> Result<CString, FsCustodyError> {
    validated_child_name(name).map_err(|_| FsCustodyError::InvalidChildName(label.to_owned()))
}

#[cfg(unix)]
pub(crate) fn open_regular_child(
    parent: &File,
    name: &OsStr,
    label: &str,
) -> Result<File, FsCustodyError> {
    let name = child_name_cstring(name, label)?;
    let file = open_child_no_follow(
        parent,
        &name,
        ChildOpenOptionsV1 {
            nonblocking: true,
            ..ChildOpenOptionsV1::default()
        },
    )
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
pub(crate) fn open_regular_child(
    _parent: &File,
    _name: &OsStr,
    label: &str,
) -> Result<File, FsCustodyError> {
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
pub(crate) fn child_entry_exists_impl(
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
pub(crate) fn child_entry_exists_impl(
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

/// The identity of a real directory at `path`, captured from one no-follow metadata probe. A
/// symlink or a non-directory is an error, never a silently-followed success. Birthtime comes
/// from the same [`std::fs::Metadata`] as device and inode so the three fields always describe
/// one observation.
#[cfg(unix)]
fn observed_directory_identity(path: &Path) -> Result<DirectoryIdentityV1, String> {
    use std::os::unix::fs::MetadataExt as _;
    let metadata =
        std::fs::symlink_metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(format!("{} is a symlink", path.display()));
    }
    if !metadata.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    Ok(DirectoryIdentityV1 {
        canonical_path: path.to_string_lossy().into_owned(),
        dev: Some(metadata.dev()),
        ino: Some(metadata.ino()),
        btime: BirthTimeV1::from_metadata(&metadata),
    })
}

#[cfg(not(unix))]
fn observed_directory_identity(path: &Path) -> Result<DirectoryIdentityV1, String> {
    Err(format!(
        "{}: filesystem identity (dev/ino) is unavailable on this platform, so a directory swap \
         cannot be detected",
        path.display()
    ))
}

/// `(dev, ino)` of a real directory at `path`. A symlink or a non-directory is an error, never a
/// silently-followed success.
pub fn directory_dev_ino(path: &Path) -> Result<(u64, u64), String> {
    let identity = observed_directory_identity(path)?;
    match (identity.dev, identity.ino) {
        (Some(dev), Some(ino)) => Ok((dev, ino)),
        _ => Err(format!(
            "{}: filesystem identity (dev/ino) is unavailable on this platform, so a directory \
             swap cannot be detected",
            path.display()
        )),
    }
}

/// Re-verify that a pinned root's PATH still resolves to the descriptor that was pinned. This is
/// the swap check: an actor controlling the parent can rename the root away and put another
/// directory in its place, after which every path-based operation lands in the replacement.
pub fn pinned_root_unchanged(pin: &PinnedDirectoryV1) -> Result<(), String> {
    let observed = directory_path_identity(pin.canonical_path(), "pinned scan root")
        .map_err(|error| error.to_string())?;
    let want = pin.identity();
    if !want.matches(&observed) {
        return Err(format!(
            "pinned scan root {} now resolves to a different directory (observed {:?}, pinned \
             {:?})",
            pin.canonical_path().display(),
            observed,
            want,
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
/// not a symlink, still canonically resolving to itself. Device, inode, and optional birthtime
/// come from one metadata probe so a later verify-then-act boundary can prove nothing was
/// exchanged in between without mixing observations.
pub fn verify_payload_directory_identity(
    path: &Path,
) -> Result<DirectoryIdentityV1, PayloadIdentityRefusalV1> {
    let identity = match observed_directory_identity(path) {
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
/// is the device, inode, and optional birthtime those gates authorized, and this refuses rather
/// than acting if the name now points at anything else. Birthtime is strengthening-only: two
/// present values must match, while either missing value preserves the legacy `(dev, ino)`
/// verdict. `act` returns its own failure text unchanged; whether the payload is actually gone is
/// then read from the filesystem, never inferred from that result.
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
    expected_identity: &DirectoryIdentityV1,
    act: F,
) -> VerifiedRemovalV1
where
    F: FnOnce() -> Result<(), String>,
{
    verify_then_remove_with_identity_probe(
        pin,
        payload,
        expected_identity,
        observed_directory_identity,
        act,
    )
}

fn verify_then_remove_with_identity_probe<F, P>(
    pin: &PinnedDirectoryV1,
    payload: &Path,
    expected_identity: &DirectoryIdentityV1,
    identity_probe: P,
    act: F,
) -> VerifiedRemovalV1
where
    F: FnOnce() -> Result<(), String>,
    P: FnOnce(&Path) -> Result<DirectoryIdentityV1, String>,
{
    if let Err(detail) = pinned_root_unchanged(pin) {
        return VerifiedRemovalV1::Refused(RemovalBoundaryRefusalV1::RootIdentityChanged {
            detail,
        });
    }
    match identity_probe(payload) {
        Ok(now) if expected_identity.matches(&now) => {}
        Ok(now) => {
            let detail = match (expected_identity.btime, now.btime) {
                (Some(expected_btime), Some(now_btime)) if expected_btime != now_btime => {
                    match (
                        expected_identity.dev,
                        expected_identity.ino,
                        now.dev,
                        now.ino,
                    ) {
                        (Some(expected_dev), Some(expected_ino), Some(now_dev), Some(now_ino)) => {
                            format!(
                                "{} changed identity between the gates and the removal (dev/ino \
                                 {}/{} to {}/{}; btime {:?} to {:?})",
                                payload.display(),
                                expected_dev,
                                expected_ino,
                                now_dev,
                                now_ino,
                                expected_btime,
                                now_btime,
                            )
                        }
                        _ => format!(
                            "{} changed identity between the gates and the removal (dev/ino \
                             {:?}/{:?} to {:?}/{:?}; btime {:?} to {:?})",
                            payload.display(),
                            expected_identity.dev,
                            expected_identity.ino,
                            now.dev,
                            now.ino,
                            expected_btime,
                            now_btime,
                        ),
                    }
                }
                _ => match (
                    expected_identity.dev,
                    expected_identity.ino,
                    now.dev,
                    now.ino,
                ) {
                    (Some(expected_dev), Some(expected_ino), Some(now_dev), Some(now_ino)) => {
                        format!(
                            "{} changed identity between the gates and the removal (dev/ino \
                             {}/{} to {}/{})",
                            payload.display(),
                            expected_dev,
                            expected_ino,
                            now_dev,
                            now_ino,
                        )
                    }
                    _ => format!(
                        "{} changed identity between the gates and the removal (dev/ino \
                         {:?}/{:?} to {:?}/{:?})",
                        payload.display(),
                        expected_identity.dev,
                        expected_identity.ino,
                        now.dev,
                        now.ino,
                    ),
                },
            };
            return VerifiedRemovalV1::Refused(RemovalBoundaryRefusalV1::PayloadIdentityChanged {
                detail,
            });
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

    /// The comparator must be wrong in NEITHER direction. Over-refusal is not "safe": it would
    /// leave the exact-absence proof unable to authorize whenever a repository holds any other
    /// registration, which is how T3a's earlier attempt failed.
    #[test]
    fn missing_tail_comparison_proves_difference_without_over_refusing() {
        fn tail(parts: &[&str]) -> Vec<OsString> {
            parts.iter().map(OsString::from).collect()
        }
        let insensitive = false;
        let sensitive = true;

        // Proven different — these must NOT refuse.
        for (left, right, why) in [
            (tail(&["wt"]), tail(&["other"]), "plain ASCII siblings"),
            (
                tail(&["w"]),
                tail(&["wt"]),
                "ASCII prefix vs longer ASCII: no normalization relates them",
            ),
            (
                tail(&["\u{e9}quipe"]),
                tail(&["other"]),
                "non-ASCII vs unrelated ASCII",
            ),
            (
                tail(&["\u{e9}a"]),
                tail(&["\u{e9}b"]),
                "shared non-ASCII prefix, different ASCII",
            ),
            (
                tail(&["wt", "child-a"]),
                tail(&["WT", "child-b"]),
                "ambiguous first component, provably different second",
            ),
            (
                tail(&["a"]),
                tail(&["a", "b"]),
                "different component counts",
            ),
        ] {
            assert_eq!(
                compare_missing_tail(&left, &right, insensitive),
                PathIdentityComparisonV1::Different,
                "{why}: must be provably different on a case-insensitive ancestor"
            );
        }

        // Genuinely ambiguous — these MUST refuse.
        for (left, right, why) in [
            (tail(&["WT"]), tail(&["wt"]), "case-only difference"),
            (
                tail(&["caf\u{e9}"]),
                tail(&["cafe\u{301}"]),
                "NFC vs NFD spelling of one name",
            ),
            (
                tail(&["r\u{e9}sum\u{e9}"]),
                tail(&["resume"]),
                "decomposable vs plain: skeleton is a subsequence",
            ),
        ] {
            assert_eq!(
                compare_missing_tail(&left, &right, insensitive),
                PathIdentityComparisonV1::CannotProve,
                "{why}: must refuse on a case-insensitive ancestor"
            );
        }

        // A case-sensitive ancestor lets bytes decide — NFC and NFD are different filenames there.
        assert_eq!(
            compare_missing_tail(&tail(&["caf\u{e9}"]), &tail(&["cafe\u{301}"]), sensitive),
            PathIdentityComparisonV1::Different,
            "case-sensitive ancestor: distinct byte sequences are distinct names"
        );
        assert_eq!(
            compare_missing_tail(&tail(&["WT"]), &tail(&["wt"]), sensitive),
            PathIdentityComparisonV1::Different,
            "case-sensitive ancestor: case is significant"
        );
    }

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

    /// Birthtime is a refinement, not a migration barrier: two present values must agree,
    /// while either missing value preserves the legacy path/device/inode verdict.
    #[test]
    fn directory_identity_birthtime_is_a_strengthening_refinement() {
        let legacy = DirectoryIdentityV1 {
            canonical_path: "/root/worktree".to_string(),
            dev: Some(7),
            ino: Some(11),
            btime: None,
        };
        let first = DirectoryIdentityV1 {
            btime: Some(BirthTimeV1::new(1_700_000_000, 123).unwrap()),
            ..legacy.clone()
        };
        let second = DirectoryIdentityV1 {
            btime: Some(BirthTimeV1::new(1_700_000_000, 124).unwrap()),
            ..legacy.clone()
        };

        assert!(
            legacy.matches(&legacy),
            "None/None keeps the legacy verdict"
        );
        assert!(
            legacy.matches(&first),
            "None/Some falls back to the legacy verdict"
        );
        assert!(
            first.matches(&legacy),
            "Some/None falls back to the legacy verdict"
        );
        assert!(first.matches(&first), "equal Some birthtimes match");
        assert!(
            !first.matches(&second),
            "differing present birthtimes must refuse even when path/device/inode match"
        );

        let different_inode = DirectoryIdentityV1 {
            ino: Some(12),
            btime: None,
            ..legacy.clone()
        };
        assert!(
            !legacy.matches(&different_inode),
            "None/None must retain a legacy inode mismatch"
        );
        assert!(
            !first.matches(&different_inode),
            "Some/None must fall through to the legacy inode mismatch"
        );
        let different_inode_with_btime = DirectoryIdentityV1 {
            btime: first.btime,
            ..different_inode
        };
        assert!(
            !legacy.matches(&different_inode_with_btime),
            "None/Some must fall through to the legacy inode mismatch"
        );
    }

    /// Locks the normalized representation for timestamps immediately before the Unix epoch.
    #[test]
    fn birthtime_before_epoch_has_one_canonical_seconds_nanos_pair() {
        let time = UNIX_EPOCH
            .checked_sub(Duration::from_nanos(1))
            .expect("one nanosecond before the epoch is representable");
        assert_eq!(
            BirthTimeV1::from_system_time(time),
            BirthTimeV1::new(-1, 999_999_999)
        );
    }

    #[test]
    fn path_identity_compares_existing_and_absent_paths_without_spelling_assumptions() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first");
        let second = root.path().join("second");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        assert_eq!(
            compare_path_identities(&first, &second),
            PathIdentityComparisonV1::Different
        );
        assert_eq!(
            compare_path_identities(first.join("wt"), first.join("other")),
            PathIdentityComparisonV1::Different,
            "clearly distinct absent siblings must not be over-refused"
        );
        assert_eq!(
            compare_path_identities(first.join("wt"), second.join("wt")),
            PathIdentityComparisonV1::Different
        );
    }
    #[test]
    fn path_identity_treats_missing_case_and_unicode_aliases_conservatively() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            compare_path_identities(root.path().join("wt"), root.path().join("other")),
            PathIdentityComparisonV1::Different,
            "distinct ASCII names must not be over-refused"
        );
        assert_eq!(
            compare_missing_tail(
                &[OsString::from("résumé")],
                &[OsString::from("re\u{301}sume\u{301}")],
                true,
            ),
            PathIdentityComparisonV1::Different,
            "a case-sensitive ancestor lets byte-different names decide"
        );
        assert_eq!(
            compare_missing_tail(&[OsString::from("wt")], &[OsString::from("WT")], false),
            PathIdentityComparisonV1::CannotProve,
            "the case-insensitive branch must refuse on every test platform"
        );
        assert_eq!(
            compare_missing_tail(
                &[OsString::from("résumé")],
                &[OsString::from("re\u{301}sume\u{301}")],
                false,
            ),
            PathIdentityComparisonV1::CannotProve,
            "a non-ASCII difference may be normalization-equivalent"
        );
        // Superseded by the counted review: blanket-refusing on any non-ASCII byte was sound but
        // functionally inert, since one non-ASCII name anywhere in the repository would stop the
        // exact-absence proof authorizing at all. Canonical decomposition only ADDS ASCII base
        // letters, so `équipe` (skeleton `quipe`) cannot be a spelling of `other`.
        assert_eq!(
            compare_missing_tail(
                &[OsString::from("équipe")],
                &[OsString::from("other")],
                false,
            ),
            PathIdentityComparisonV1::Different,
            "a non-ASCII name is still provably different from an unrelated one"
        );
        assert_eq!(
            compare_missing_tail(&[OsString::from("wt")], &[OsString::from("other")], false),
            PathIdentityComparisonV1::Different,
            "ASCII names that are not equal under ASCII case folding are distinct"
        );
    }
    #[cfg(unix)]
    #[test]
    fn path_identity_refuses_an_unreadable_ancestor() {
        const ROOT_ENV: &str = "BRIDGE_CORE_UNREADABLE_PATH_ROOT";
        if let Some(root) = std::env::var_os(ROOT_ENV) {
            let ancestor = PathBuf::from(root).join("unreadable");
            assert_eq!(
                compare_path_identities(ancestor.join("wt"), ancestor.join("other")),
                PathIdentityComparisonV1::CannotProve
            );
            return;
        }
        use std::os::unix::{fs::PermissionsExt as _, process::CommandExt as _};
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let ancestor = root.path().join("unreadable");
        fs::create_dir(&ancestor).unwrap();
        fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o000)).unwrap();
        let uid = unsafe { libc::geteuid() };
        let test = format!(
            "{}::path_identity_refuses_an_unreadable_ancestor",
            module_path!().strip_prefix("bridge_core::").unwrap()
        );
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg(test)
            .env(ROOT_ENV, root.path())
            .uid(if uid == 0 { 65_534 } else { uid })
            .status()
            .unwrap();
        assert!(status.success());
    }
    #[cfg(unix)]
    #[test]
    fn path_identity_follows_a_symlinked_parent_for_existing_paths() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("real");
        let alias = root.path().join("alias");
        let child = real.join("child");
        fs::create_dir(&real).unwrap();
        fs::create_dir(&child).unwrap();
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        assert_eq!(
            compare_path_identities(child, alias.join("child")),
            PathIdentityComparisonV1::Same
        );
        let dangling = root.path().join("dangling");
        std::os::unix::fs::symlink(root.path().join("missing"), &dangling).unwrap();
        assert_eq!(
            compare_path_identities(dangling.join("child"), root.path().join("other")),
            PathIdentityComparisonV1::CannotProve
        );
        #[cfg(target_os = "macos")]
        assert_eq!(
            compare_path_identities("/var", "/private/var"),
            PathIdentityComparisonV1::Same
        );
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
            pinned.identity().btime,
            BirthTimeV1::from_metadata(&expected)
        );
        assert_eq!(
            pinned.canonical_path(),
            fs::canonicalize(dir.path()).unwrap()
        );
    }

    /// Discriminates a regression that drops the `refinement-aware before and after matcher`
    /// recheck in `PinnedDirectoryV1::open` *entirely*: a background thread continuously
    /// replaces the target directory (via an atomic directory-to-directory rename, so the path
    /// is never momentarily absent) while the foreground repeatedly calls `open` until it
    /// observes the guard firing, bounded by a wall-clock budget.
    ///
    /// HONEST LIMIT: `FsCustodyError::IdentityChanged` carries only the caller's `label`, with no
    /// field distinguishing which side of the check tripped — the pre-open `before` stat or the
    /// post-open `after` stat. Because the swapper races continuously through the whole guarded
    /// window (often swapping more than once per `open` attempt), a *one-sided* weakening — e.g.
    /// keeping only the before matcher and dropping the after matcher, or vice versa — often
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
        // Exact per-platform tripwire. Linux: every measured Linux (the
        // hermetic-verify container AND GitHub's ubuntu runner, 2026-08-10)
        // returns ENOTDIR — the O_DIRECTORY check fires before O_NOFOLLOW's
        // ELOOP on these kernels. The original ELOOP pin was never observed
        // true on any Linux this repo gates on; corrected at the operator
        // boundary on that two-environment CI evidence (fs_custody ledger
        // item resolved — the assertion stays EXACT, not a tolerance).
        #[cfg(target_os = "macos")]
        const EXPECTED_ERRNO: i32 = libc::ENOTDIR;
        #[cfg(target_os = "linux")]
        const EXPECTED_ERRNO: i32 = libc::ENOTDIR;

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
        let dir = tempfile::tempdir().unwrap();
        let pinned = PinnedDirectoryV1::open(dir.path(), "stale source").unwrap();
        let source_name = OsStr::new("source.tmp");
        let target_name = OsStr::new("target.final");
        fs::write(dir.path().join(source_name), b"original").unwrap();
        let stale_handle = fs::File::open(dir.path().join(source_name)).unwrap();

        // Pre-create the replacement while the original is still live, so the two objects cannot
        // receive the same inode even on an aggressively recycling filesystem. The replacing
        // rename then puts that already-distinct object under `source_name`.
        let replacement_path = dir.path().join("replacement.tmp");
        fs::write(&replacement_path, b"swapped").unwrap();
        let replacement_handle = fs::File::open(&replacement_path).unwrap();
        assert!(
            !same_regular_file(&stale_handle, &replacement_handle, "stale source setup").unwrap(),
            "test setup must produce a genuinely different file identity"
        );
        fs::rename(&replacement_path, dir.path().join(source_name)).unwrap();
        let replacement_at_name = fs::File::open(dir.path().join(source_name)).unwrap();
        assert!(
            !same_regular_file(&stale_handle, &replacement_at_name, "stale source setup").unwrap(),
            "precondition: the same-name replacement must not match the retained source"
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

        // Pre-create the substitute while the caller's source still exists, prove the matcher
        // sees a different live object, then atomically replace the name with that object.
        let substitute_path = dir.path().join("substitute.tmp");
        fs::write(&substitute_path, b"substituted").unwrap();
        let substitute_file = fs::File::open(&substitute_path).unwrap();
        assert!(
            !same_regular_file(&source_file, &substitute_file, "replace substituted setup")
                .unwrap(),
            "test setup must produce a genuinely different file identity"
        );
        fs::rename(&substitute_path, dir.path().join(source_name)).unwrap();
        let substitute_at_name = fs::File::open(dir.path().join(source_name)).unwrap();
        assert!(
            !same_regular_file(
                &source_file,
                &substitute_at_name,
                "replace substituted setup"
            )
            .unwrap(),
            "precondition: the same-name substitute must not match the retained source"
        );

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

        // A same-name substitution at the source is NOT proof that nothing moved. Pre-create
        // the impostor while our source is still live, prove the matcher sees distinct objects,
        // then replace the source name so inode recycling cannot collapse the fixture.
        let impostor_path = dir.path().join("impostor.tmp");
        fs::write(&impostor_path, b"impostor").unwrap();
        let impostor = fs::File::open(&impostor_path).unwrap();
        assert!(
            !same_regular_file(&ours, &impostor, "rename effect setup").unwrap(),
            "test setup must produce a genuinely different file identity"
        );
        fs::rename(&impostor_path, dir.path().join(source_name)).unwrap();
        let impostor_at_name = fs::File::open(dir.path().join(source_name)).unwrap();
        assert!(
            !same_regular_file(&ours, &impostor_at_name, "rename effect setup").unwrap(),
            "precondition: the same-name impostor must not match the retained source"
        );
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
        let identity = verify_payload_directory_identity(&real).unwrap();
        assert_eq!(identity.canonical_path, real.to_string_lossy().into_owned());
        assert_eq!(identity.dev, Some(expected.dev()));
        assert_eq!(identity.ino, Some(expected.ino()));
        assert_eq!(identity.btime, BirthTimeV1::from_metadata(&expected));

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

        // Pre-create the replacement while the authorized directory is still live. Two
        // simultaneous objects cannot share an inode, so the replacing rename makes the
        // exchange deterministic even on an aggressively recycling filesystem.
        let replacement = base.join("replacement");
        fs::create_dir(&replacement).unwrap();
        fs::rename(&replacement, &payload).unwrap();
        let observed = verify_payload_directory_identity(&payload).unwrap();
        assert!(
            !authorized.matches(&observed),
            "the strengthening matcher must see the exchanged payload before the boundary runs"
        );

        let mut acts = 0_u32;
        let verified = verify_then_remove(&pinned, &payload, &authorized, || {
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

    /// Pins the recycled-inode bypass directly: device and inode alone still match, but two
    /// present birthtimes differ. The typed payload-identity refusal must win before the act.
    #[cfg(unix)]
    #[test]
    fn verify_then_remove_refuses_same_dev_ino_with_different_btime_without_acting() {
        let root = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(root.path()).unwrap();
        let pinned = PinnedDirectoryV1::open(&base, "recycled payload identity").unwrap();
        let payload = base.join("payload");
        fs::create_dir(&payload).unwrap();

        let legacy = verify_payload_directory_identity(&payload).unwrap();
        let expected = DirectoryIdentityV1 {
            btime: Some(BirthTimeV1::new(1_700_000_000, 123).unwrap()),
            ..legacy.clone()
        };
        let observed = DirectoryIdentityV1 {
            btime: Some(BirthTimeV1::new(1_700_000_000, 124).unwrap()),
            ..legacy
        };
        assert_eq!(
            (expected.dev, expected.ino),
            (observed.dev, observed.ino),
            "the regression must isolate btime from the legacy identity"
        );
        assert!(
            !expected.matches(&observed),
            "the strengthening matcher must reject the btime divergence"
        );

        let mut acts = 0_u32;
        let verified = verify_then_remove_with_identity_probe(
            &pinned,
            &payload,
            &expected,
            move |_| Ok(observed),
            || {
                acts += 1;
                Ok(())
            },
        );

        assert_eq!(acts, 0, "the act must not run after a btime refusal");
        match verified {
            VerifiedRemovalV1::Refused(RemovalBoundaryRefusalV1::PayloadIdentityChanged {
                detail,
            }) => assert!(
                detail.contains("btime")
                    && detail.contains("changed identity between the gates and the removal"),
                "the typed refusal must name the btime divergence, got {detail}"
            ),
            other => panic!("expected PayloadIdentityChanged, got {other:?}"),
        }
    }

    /// Birthtime is strengthening-only at the destructive boundary itself: either missing side
    /// must preserve the legacy matching `(dev, ino)` verdict and let the act run exactly once.
    #[cfg(unix)]
    #[test]
    fn verify_then_remove_falls_back_to_dev_ino_when_either_btime_is_missing() {
        let root = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(root.path()).unwrap();
        let pinned = PinnedDirectoryV1::open(&base, "legacy payload identity").unwrap();
        let payload = base.join("payload");
        fs::create_dir(&payload).unwrap();

        let legacy = DirectoryIdentityV1 {
            btime: None,
            ..verify_payload_directory_identity(&payload).unwrap()
        };
        let refined = DirectoryIdentityV1 {
            btime: Some(BirthTimeV1::new(1_700_000_000, 123).unwrap()),
            ..legacy.clone()
        };

        for (expected, observed, missing_side) in [
            (legacy.clone(), refined.clone(), "expected"),
            (refined, legacy, "observed"),
        ] {
            let mut acts = 0_u32;
            let verified = verify_then_remove_with_identity_probe(
                &pinned,
                &payload,
                &expected,
                move |_| Ok(observed),
                || {
                    acts += 1;
                    Ok(())
                },
            );
            assert_eq!(
                acts, 1,
                "{missing_side}-btime fallback must preserve the legacy act"
            );
            assert!(matches!(
                verified,
                VerifiedRemovalV1::Acted {
                    observation: RemovalObservationV1::ReportedSuccessButPresent { .. },
                    root_changed_during: None,
                }
            ));
        }
    }

    /// The strengthening must not perturb the legacy no-birthtime refusal text: downstream
    /// reports retain these bytes when `(dev, ino)` changes and neither side carries btime.
    #[cfg(unix)]
    #[test]
    fn verify_then_remove_preserves_legacy_mismatch_detail_without_btime() {
        let root = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(root.path()).unwrap();
        let pinned = PinnedDirectoryV1::open(&base, "legacy mismatch bytes").unwrap();
        let payload = base.join("payload");
        fs::create_dir(&payload).unwrap();

        let expected = DirectoryIdentityV1 {
            btime: None,
            ..verify_payload_directory_identity(&payload).unwrap()
        };
        let mut observed = expected.clone();
        observed.ino = expected.ino.map(|ino| ino.wrapping_add(1));
        assert!(!expected.matches(&observed));

        let mut acts = 0_u32;
        let verified = verify_then_remove_with_identity_probe(
            &pinned,
            &payload,
            &expected,
            move |_| Ok(observed),
            || {
                acts += 1;
                Ok(())
            },
        );

        assert_eq!(acts, 0, "the act must not run after the legacy mismatch");
        assert_eq!(
            verified,
            VerifiedRemovalV1::Refused(RemovalBoundaryRefusalV1::PayloadIdentityChanged {
                detail: format!(
                    "{} changed identity between the gates and the removal (dev/ino {}/{} to {}/{})",
                    payload.display(),
                    expected.dev.unwrap(),
                    expected.ino.unwrap(),
                    expected.dev.unwrap(),
                    expected.ino.unwrap().wrapping_add(1),
                ),
            })
        );
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
        let verified = verify_then_remove(&pinned, &payload, &authorized, || {
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
            let verified = verify_then_remove(&pinned, &payload, &authorized, || {
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
            let verified = verify_then_remove(&pinned, &payload, &authorized, || {
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

        let verified = verify_then_remove(&pinned, &payload, &authorized, || {
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

    #[cfg(unix)]
    mod journal_route_custody_v2 {
        use super::*;
        use std::cell::Cell;
        use std::os::unix::fs::MetadataExt as _;

        struct RouteCase {
            _dir: tempfile::TempDir,
            anchor: PathBuf,
            parent: PathBuf,
            root: PathBuf,
            lock: PathBuf,
            binding: JournalRootBindingV2,
        }

        fn object(path: &Path) -> ObjectIdentityV2 {
            let metadata = fs::metadata(path).unwrap();
            required_object_identity_v2(
                metadata.dev(),
                metadata.ino(),
                BirthTimeV1::from_metadata(&metadata),
                "fixture identity",
            )
            .unwrap()
        }

        fn binding(case: &RouteCase) -> JournalRootBindingV2 {
            JournalRootBindingV2::new(
                object(&case.anchor),
                ChildNameV2::from_bytes(b"parent").unwrap(),
                object(&case.parent),
                ChildNameV2::from_bytes(b"journal").unwrap(),
                object(&case.root),
                ChildNameV2::from_bytes(b"operation.lock").unwrap(),
                object(&case.lock),
            )
            .unwrap()
        }

        fn route_case() -> RouteCase {
            let dir = tempfile::tempdir().unwrap();
            let anchor = dir.path().join("anchor");
            let parent = anchor.join("parent");
            let root = parent.join("journal");
            let lock = parent.join("operation.lock");
            fs::create_dir_all(&root).unwrap();
            fs::write(&lock, b"").unwrap();
            let binding = JournalRootBindingV2::new(
                object(&anchor),
                ChildNameV2::from_bytes(b"parent").unwrap(),
                object(&parent),
                ChildNameV2::from_bytes(b"journal").unwrap(),
                object(&root),
                ChildNameV2::from_bytes(b"operation.lock").unwrap(),
                object(&lock),
            )
            .unwrap();
            RouteCase {
                _dir: dir,
                anchor,
                parent,
                root,
                lock,
                binding,
            }
        }

        fn fill_root_to(case: &RouteCase, count: usize) {
            for index in fs::read_dir(&case.root).unwrap().count()..count {
                fs::hard_link(&case.lock, case.root.join(format!("ordinary-{index}"))).unwrap();
            }
        }

        fn flock(file: &File) -> std::io::Result<bool> {
            crate::liveness::flock_nb(file, true)
        }

        fn unlock(file: &File) {
            crate::liveness::flock_unlock(file, "test peer");
        }

        #[derive(Clone, Copy)]
        enum Replaced {
            Anchor,
            Parent,
            Root,
        }

        fn replace(case: &RouteCase, replaced: Replaced) {
            match replaced {
                Replaced::Anchor => {
                    fs::rename(&case.anchor, case._dir.path().join("old-anchor")).unwrap();
                    fs::create_dir_all(&case.root).unwrap();
                    fs::write(&case.lock, b"replacement").unwrap();
                }
                Replaced::Parent => {
                    fs::rename(&case.parent, case.anchor.join("old-parent")).unwrap();
                    fs::create_dir_all(&case.root).unwrap();
                    fs::write(&case.lock, b"replacement").unwrap();
                }
                Replaced::Root => {
                    fs::rename(&case.root, case.parent.join("old-root")).unwrap();
                    fs::create_dir(&case.root).unwrap();
                }
            }
        }

        fn assert_identity_refusal<T>(result: Result<T, FsCustodyError>) {
            assert!(matches!(result, Err(FsCustodyError::IdentityChanged(_))));
        }

        #[test]
        fn journal_route_custody_v2_anchor_parent_and_root_replacement_refuse_every_schedule() {
            for replaced in [Replaced::Anchor, Replaced::Parent, Replaced::Root] {
                let case = route_case();
                let custody = JournalRootCustodyV2::open(
                    &case.anchor,
                    &case.binding,
                    "before-lock replacement",
                )
                .unwrap();
                replace(&case, replaced);
                assert_identity_refusal(custody.begin_operation("before-lock replacement"));

                let case = route_case();
                let custody = JournalRootCustodyV2::open(
                    &case.anchor,
                    &case.binding,
                    "contended replacement",
                )
                .unwrap();
                let peer = File::open(&case.lock).unwrap();
                assert!(flock(&peer).unwrap());
                replace(&case, replaced);
                assert!(matches!(
                    custody.begin_operation("contended replacement"),
                    Err(FsCustodyError::Io(_, ref error))
                        if error.kind() == std::io::ErrorKind::WouldBlock
                ));
                unlock(&peer);
                assert_identity_refusal(custody.begin_operation("post-contention replacement"));

                let case = route_case();
                let custody = JournalRootCustodyV2::open(
                    &case.anchor,
                    &case.binding,
                    "after-lock replacement",
                )
                .unwrap();
                assert_identity_refusal(custody.begin_operation_with(
                    "after-lock replacement",
                    || {},
                    flock,
                    || replace(&case, replaced),
                ));
            }
        }

        #[test]
        fn journal_route_custody_v2_wrong_lock_and_missing_root_refuse_without_creation() {
            let case = route_case();
            let custody =
                JournalRootCustodyV2::open(&case.anchor, &case.binding, "bound route").unwrap();
            let expected_lock = object(&case.lock);
            fs::rename(&case.lock, case.parent.join("old-operation.lock")).unwrap();
            fs::write(&case.lock, b"planted").unwrap();
            assert_ne!(object(&case.lock), expected_lock);
            assert_identity_refusal(JournalRootCustodyV2::open(
                &case.anchor,
                &case.binding,
                "planted lock",
            ));
            assert_identity_refusal(custody.begin_operation("planted lock"));
            fs::remove_file(&case.lock).unwrap();
            fs::create_dir(&case.lock).unwrap();
            assert!(matches!(
                JournalRootCustodyV2::open(&case.anchor, &case.binding, "wrong lock type"),
                Err(FsCustodyError::Unsupported(_))
            ));

            let case = route_case();
            let custody =
                JournalRootCustodyV2::open(&case.anchor, &case.binding, "removable root").unwrap();
            fs::remove_dir(&case.root).unwrap();
            assert!(
                JournalRootCustodyV2::open(&case.anchor, &case.binding, "removed root").is_err()
            );
            assert!(custody.begin_operation("removed root").is_err());
            assert!(!case.root.exists(), "custody must never recreate the root");
        }

        #[test]
        fn journal_route_custody_v2_stale_and_bound_lock_cells_cannot_both_authorize() {
            let case = route_case();
            let custody =
                JournalRootCustodyV2::open(&case.anchor, &case.binding, "stale route").unwrap();
            let held_new = std::cell::RefCell::new(None);
            assert_identity_refusal(custody.begin_operation_with(
                "stale route",
                || {},
                flock,
                || {
                    replace(&case, Replaced::Root);
                    fs::rename(&case.lock, case.parent.join("old-operation.lock")).unwrap();
                    fs::write(&case.lock, b"new cell").unwrap();
                    let held = File::open(&case.lock).unwrap();
                    assert!(flock(&held).unwrap());
                    assert!(!flock(&File::open(&case.lock).unwrap()).unwrap());
                    *held_new.borrow_mut() = Some(held);
                },
            ));
            let new_binding = binding(&case);
            let current =
                JournalRootCustodyV2::open(&case.anchor, &new_binding, "current route").unwrap();
            assert!(matches!(
                current.begin_operation("bound cell contention"),
                Err(FsCustodyError::Io(_, ref error))
                    if error.kind() == std::io::ErrorKind::WouldBlock
            ));
            unlock(held_new.borrow().as_ref().unwrap());
            assert!(current.begin_operation("current route").is_ok());
        }

        #[test]
        fn journal_route_custody_v2_guard_contends_on_an_independent_fd() {
            let case = route_case();
            let custody =
                JournalRootCustodyV2::open(&case.anchor, &case.binding, "independent contention")
                    .unwrap();
            let operation = custody.begin_operation("held operation").unwrap();
            let peer = File::open(&case.lock).unwrap();
            assert!(!flock(&peer).unwrap());
            drop(operation);
            assert!(flock(&peer).unwrap());
            unlock(&peer);
        }

        #[test]
        fn journal_route_custody_v2_existing_child_lease_refuses_wrong_route_and_type() {
            let name = ChildNameV2::from_bytes(b"attempt.lock").unwrap();

            let moved = route_case();
            fs::write(moved.root.join(name.as_os_str()), b"").unwrap();
            let custody = JournalRootCustodyV2::open(
                &moved.anchor,
                &moved.binding,
                "existing child moved route",
            )
            .unwrap();
            replace(&moved, Replaced::Root);
            assert_identity_refusal(
                custody.acquire_existing_regular_child_lease(&name, "existing child moved route"),
            );
            assert!(
                !moved.root.join(name.as_os_str()).exists(),
                "the accessor must not create a child in the replacement route"
            );

            let wrong_type = route_case();
            fs::create_dir(wrong_type.root.join(name.as_os_str())).unwrap();
            let custody = JournalRootCustodyV2::open(
                &wrong_type.anchor,
                &wrong_type.binding,
                "existing child wrong type",
            )
            .unwrap();
            assert!(matches!(
                custody.acquire_existing_regular_child_lease(&name, "existing child wrong type"),
                Err(FsCustodyError::Unsupported(_))
            ));
            assert!(wrong_type.root.join(name.as_os_str()).is_dir());
        }

        #[test]
        fn journal_route_custody_v2_existing_child_lease_contends_and_releases() {
            let case = route_case();
            let name = ChildNameV2::from_bytes(b"attempt.lock").unwrap();
            fs::write(case.root.join(name.as_os_str()), b"").unwrap();
            let first_custody =
                JournalRootCustodyV2::open(&case.anchor, &case.binding, "first lease").unwrap();
            let second_custody =
                JournalRootCustodyV2::open(&case.anchor, &case.binding, "second lease").unwrap();
            let (first, snapshot) = first_custody
                .acquire_existing_regular_child_lease(&name, "first lease")
                .unwrap();
            assert_eq!(snapshot.content_len, 0);
            assert!(matches!(
                second_custody.acquire_existing_regular_child_lease(&name, "contended lease"),
                Err(FsCustodyError::Io(_, ref error))
                    if error.kind() == std::io::ErrorKind::WouldBlock
            ));
            drop(first);
            drop(
                second_custody
                    .acquire_existing_regular_child_lease(&name, "released lease")
                    .unwrap(),
            );
        }

        #[test]
        fn journal_route_custody_v2_same_cell_waits_on_the_process_mutex() {
            use std::sync::atomic::{AtomicBool, Ordering};
            use std::sync::{mpsc, Arc};
            use std::time::Duration;
            let case = route_case();
            let custody = Arc::new(
                JournalRootCustodyV2::open(&case.anchor, &case.binding, "same cell").unwrap(),
            );
            let held = custody
                .begin_operation("first same-cell operation")
                .unwrap();
            // Ordering tokens: `main_holding` stays true until immediately before the
            // first guard drops; the peer's injected flock records whether its attempt
            // ran while the guard was still held. With the process mutex present the
            // peer cannot reach flock before the release; if the mutex were removed it
            // would reach flock while `main_holding` is true (or refuse WouldBlock on
            // the held lease), failing the final assertions either way.
            let main_holding = Arc::new(AtomicBool::new(true));
            let flock_saw_holder = Arc::new(AtomicBool::new(false));
            let (entered_tx, entered_rx) = mpsc::channel();
            let (done_tx, done_rx) = mpsc::channel();
            let peer = Arc::clone(&custody);
            let peer_holding = Arc::clone(&main_holding);
            let peer_saw = Arc::clone(&flock_saw_holder);
            let thread = std::thread::spawn(move || {
                done_tx
                    .send(
                        peer.begin_operation_with(
                            "queued same-cell operation",
                            || entered_tx.send(()).unwrap(),
                            move |file| {
                                if peer_holding.load(Ordering::SeqCst) {
                                    peer_saw.store(true, Ordering::SeqCst);
                                }
                                flock(file)
                            },
                            || {},
                        )
                        .is_ok(),
                    )
                    .unwrap();
            });
            entered_rx.recv().unwrap();
            assert!(done_rx.recv_timeout(Duration::from_millis(100)).is_err());
            main_holding.store(false, Ordering::SeqCst);
            drop(held);
            assert!(done_rx.recv_timeout(Duration::from_secs(2)).unwrap());
            thread.join().unwrap();
            assert!(
                !flock_saw_holder.load(Ordering::SeqCst),
                "peer reached flock while the first operation was still held: the \
                 process mutex did not serialize same-cell acquisition"
            );
        }

        #[test]
        fn journal_route_custody_v2_constructor_rejects_shared_root_and_lock_name() {
            let case = route_case();
            let same = ChildNameV2::from_bytes(b"same").unwrap();
            assert!(matches!(
                JournalRootBindingV2::new(
                    object(&case.anchor),
                    ChildNameV2::from_bytes(b"parent").unwrap(),
                    object(&case.parent),
                    same.clone(),
                    object(&case.root),
                    same,
                    object(&case.lock),
                ),
                Err(FsCustodyError::InvalidChildName(_))
            ));
        }

        #[test]
        fn journal_route_custody_v2_external_binding_and_unsupported_primitive_fail_closed() {
            let case = route_case();
            fs::write(case.root.join("binding.json"), b"untrusted").unwrap();
            let custody =
                JournalRootCustodyV2::open(&case.anchor, &case.binding, "external binding")
                    .unwrap();
            let flock_calls = Cell::new(0);
            let after_lock = Cell::new(0);
            let result = custody.begin_operation_with(
                "unsupported flock",
                || {},
                |_| {
                    flock_calls.set(flock_calls.get() + 1);
                    Err(std::io::Error::from_raw_os_error(libc::ENOTSUP))
                },
                || after_lock.set(after_lock.get() + 1),
            );
            assert!(matches!(result, Err(FsCustodyError::Unsupported(_))));
            assert_eq!((flock_calls.get(), after_lock.get()), (1, 0));
            assert!(matches!(
                required_object_identity_v2(1, 2, None, "missing birthtime"),
                Err(FsCustodyError::Unsupported(_))
            ));
        }

        #[test]
        fn journal_owned_surface_v2_stage_publish_append_read_enumerate_and_sync() {
            let case = route_case();
            let custody =
                JournalRootCustodyV2::open(&case.anchor, &case.binding, "owned surface").unwrap();
            let operation = custody.begin_operation("owned surface").unwrap();
            let name = ChildNameV2::from_bytes(b"record").unwrap();
            let staged = operation.stage(&name, b"A", "stage").unwrap();
            operation.publish(&name, staged, "publish").unwrap();
            let expected = required_file_content_snapshot_v2(
                &File::open(case.root.join("record")).unwrap(),
                "expected",
            )
            .unwrap();
            let wrong =
                required_file_content_snapshot_v2(&File::open(&case.lock).unwrap(), "wrong")
                    .unwrap();
            assert!(matches!(
                operation.append(&name, wrong, 1, b"X", "wrong object"),
                Err(JournalMutationOutcomeV2::Refused(_))
            ));
            assert!(matches!(
                operation.append(&name, expected, 0, b"X", "wrong position"),
                Err(JournalMutationOutcomeV2::Refused(_))
            ));
            assert_eq!(operation.read(&name, expected, 2, "read").unwrap(), b"A");
            assert_eq!(
                operation.enumerate(2, "enumerate").unwrap(),
                vec![name.as_os_str().to_os_string()]
            );
            operation
                .append(&name, expected, 1, b"B", "append")
                .unwrap();
            assert!(matches!(
                operation.sync("sync"),
                JournalMutationOutcomeV2::Complete
            ));
            assert_eq!(fs::read(case.root.join("record")).unwrap(), b"AB");
        }

        #[test]
        fn journal_owned_surface_v2_protects_publication_and_append_failures() {
            let case = route_case();
            let custody =
                JournalRootCustodyV2::open(&case.anchor, &case.binding, "publish").unwrap();
            let operation = custody.begin_operation("publish").unwrap();
            let name = ChildNameV2::from_bytes(b"record").unwrap();
            let staged = operation.stage(&name, b"A", "stage").unwrap();
            let outcome = operation.publish_with(&name, staged, "publish", || {
                fs::write(case.root.join("record"), b"intruder").unwrap();
            });
            assert!(matches!(
                outcome,
                Err(JournalMutationOutcomeV2::Retained(_))
            ));
            assert_eq!(fs::read(case.root.join("record")).unwrap(), b"intruder");

            for fault in 0..3 {
                let case = route_case();
                fs::write(case.root.join("record"), b"A").unwrap();
                let custody =
                    JournalRootCustodyV2::open(&case.anchor, &case.binding, "append").unwrap();
                let operation = custody.begin_operation("append").unwrap();
                let expected = required_file_content_snapshot_v2(
                    &File::open(case.root.join("record")).unwrap(),
                    "append",
                )
                .unwrap();
                match fault {
                    0 => custody.append_limit.store(1, Ordering::SeqCst),
                    1 => custody.file_sync_failure.arm(1),
                    _ => custody.root.fail_sync_on_nth_call_for_test(1),
                }
                let outcome = operation.append(&name, expected, 1, b"BC", "append failure");
                assert!(matches!(outcome, Err(JournalMutationOutcomeV2::Refused(_))));
                assert_eq!(fs::read(case.root.join("record")).unwrap(), b"A");
            }
            assert!(matches!(
                operation.stage(&ChildNameV2::from_bytes(b"next").unwrap(), b"x", "blocked"),
                Err(JournalMutationOutcomeV2::ProtectiveDebt(_))
            ));
        }

        #[test]
        fn journal_owned_surface_v2_reserves_stage_capacity_and_overcap_is_protective() {
            for count in [4095, 4096, 4097] {
                let case = route_case();
                fill_root_to(&case, count);
                let custody =
                    JournalRootCustodyV2::open(&case.anchor, &case.binding, "stage capacity")
                        .unwrap();
                let operation = custody.begin_operation("stage capacity").unwrap();
                let outcome = operation.stage(
                    &ChildNameV2::from_bytes(b"record").unwrap(),
                    b"A",
                    "stage capacity",
                );
                let admitted = count == 4095;
                assert_eq!(outcome.is_ok(), admitted);
                if !admitted {
                    assert!(matches!(
                        outcome,
                        Err(JournalMutationOutcomeV2::ProtectiveDebt(_))
                    ));
                }
                assert_eq!(
                    fs::read_dir(&case.root).unwrap().count(),
                    count + usize::from(admitted)
                );
            }
        }

        #[test]
        fn journal_owned_surface_v2_reserved_targets_refuse_before_effect() {
            for target in [
                ".a2a-v2-int-record",
                ".a2a-v2-stg-record",
                ".a2a-v2-rpc-record",
                ".a2a-v2-rtc-record",
                ".a2a-v2-x",
            ] {
                // A reserved-named object present in the root is residue: the
                // admission census refuses protectively before the name-level
                // refusal can apply, and the root is untouched.
                let case = route_case();
                fs::write(case.root.join(target), b"A").unwrap();
                let custody =
                    JournalRootCustodyV2::open(&case.anchor, &case.binding, "reserved target")
                        .unwrap();
                let operation = custody.begin_operation("reserved target").unwrap();
                let name = ChildNameV2::from_bytes(target.as_bytes()).unwrap();
                let expected = required_file_content_snapshot_v2(
                    &File::open(case.root.join(name.as_os_str())).unwrap(),
                    "reserved target",
                )
                .unwrap();
                for outcome in [
                    operation.stage(&name, b"B", "reserved target"),
                    operation.publish(&name, expected, "reserved target"),
                    operation.append(&name, expected, 1, b"B", "reserved target"),
                ] {
                    assert!(
                        matches!(outcome, Err(JournalMutationOutcomeV2::ProtectiveDebt(_))),
                        "{outcome:?}"
                    );
                    assert_eq!(fs::read_dir(&case.root).unwrap().count(), 1);
                    assert_eq!(fs::read(case.root.join(name.as_os_str())).unwrap(), b"A");
                }

                // On a clean root the pure name refusal applies with no effect.
                let clean = route_case();
                let custody =
                    JournalRootCustodyV2::open(&clean.anchor, &clean.binding, "reserved name")
                        .unwrap();
                let operation = custody.begin_operation("reserved name").unwrap();
                for outcome in [
                    operation.stage(&name, b"B", "reserved name"),
                    operation.publish(&name, expected, "reserved name"),
                    operation.append(&name, expected, 1, b"B", "reserved name"),
                ] {
                    assert!(
                        matches!(
                            outcome,
                            Err(JournalMutationOutcomeV2::Refused(ref reason))
                                if reason.contains("reserved target")
                        ),
                        "{outcome:?}"
                    );
                    assert_eq!(fs::read_dir(&clean.root).unwrap().count(), 0);
                    assert!(!operation.debt());
                }
            }

            // A reserved target whose derived staging object is the only entry
            // must still classify protectively: publish may not whitelist the
            // staging name it derived from a reserved target.
            let case = route_case();
            let target = ChildNameV2::from_bytes(b".a2a-v2-x").unwrap();
            let staging = ChildNameV2::reserved(ReservedNameNamespaceV2::Staging, &target).unwrap();
            fs::write(case.root.join(staging.as_os_str()), b"S").unwrap();
            let custody =
                JournalRootCustodyV2::open(&case.anchor, &case.binding, "derived staging").unwrap();
            let operation = custody.begin_operation("derived staging").unwrap();
            let staged = required_file_content_snapshot_v2(
                &File::open(case.root.join(staging.as_os_str())).unwrap(),
                "derived staging",
            )
            .unwrap();
            assert!(matches!(
                operation.publish(&target, staged, "derived staging"),
                Err(JournalMutationOutcomeV2::ProtectiveDebt(_))
            ));
            assert_eq!(fs::read_dir(&case.root).unwrap().count(), 1);
        }

        #[test]
        fn journal_long_reserved_publish_targets_cannot_bypass_the_census() {
            let mut long = b".a2a-v2-".to_vec();
            long.resize(244, b'a');
            let long = ChildNameV2::from_bytes(&long).unwrap();

            // Residue-bearing root: the census classifies protectively even
            // though staging-name derivation for this target fails.
            let case = route_case();
            fs::write(case.root.join(".a2a-v2-x"), b"A").unwrap();
            let custody =
                JournalRootCustodyV2::open(&case.anchor, &case.binding, "long reserved").unwrap();
            let operation = custody.begin_operation("long reserved").unwrap();
            let sample = required_file_content_snapshot_v2(
                &File::open(case.root.join(".a2a-v2-x")).unwrap(),
                "long reserved",
            )
            .unwrap();
            assert!(matches!(
                operation.publish(&long, sample, "long reserved"),
                Err(JournalMutationOutcomeV2::ProtectiveDebt(_))
            ));
            assert!(operation.debt());
            assert_eq!(fs::read_dir(&case.root).unwrap().count(), 1);
            assert_eq!(fs::read(case.root.join(".a2a-v2-x")).unwrap(), b"A");

            // Clean root: the reserved-name refusal names the actual reason.
            let clean = route_case();
            let custody =
                JournalRootCustodyV2::open(&clean.anchor, &clean.binding, "long clean").unwrap();
            let operation = custody.begin_operation("long clean").unwrap();
            assert!(matches!(
                operation.publish(&long, sample, "long clean"),
                Err(JournalMutationOutcomeV2::Refused(ref reason))
                    if reason.contains("reserved target")
            ));
            assert_eq!(fs::read_dir(&clean.root).unwrap().count(), 0);
            assert!(!operation.debt());
        }

        #[test]
        fn journal_each_mutator_classifies_residue_on_a_fresh_handle() {
            for mutator in 0..3 {
                let case = route_case();
                fs::write(case.root.join("record"), b"A").unwrap();
                fs::write(case.root.join(".a2a-v2-x"), b"R").unwrap();
                let custody =
                    JournalRootCustodyV2::open(&case.anchor, &case.binding, "fresh residue")
                        .unwrap();
                let operation = custody.begin_operation("fresh residue").unwrap();
                assert!(!operation.debt());
                let reserved = ChildNameV2::from_bytes(b".a2a-v2-x").unwrap();
                let expected = required_file_content_snapshot_v2(
                    &File::open(case.root.join("record")).unwrap(),
                    "fresh residue",
                )
                .unwrap();
                let outcome = match mutator {
                    0 => operation.stage(&reserved, b"B", "fresh residue"),
                    1 => operation.publish(&reserved, expected, "fresh residue"),
                    _ => operation.append(&reserved, expected, 1, b"B", "fresh residue"),
                };
                assert!(
                    matches!(outcome, Err(JournalMutationOutcomeV2::ProtectiveDebt(_))),
                    "mutator {mutator}: {outcome:?}"
                );
                assert_eq!(fs::read_dir(&case.root).unwrap().count(), 2);
            }
        }

        #[test]
        fn journal_recorded_debt_dominates_reserved_and_route_refusals() {
            let case = route_case();
            fill_root_to(&case, 4096);
            let custody =
                JournalRootCustodyV2::open(&case.anchor, &case.binding, "debt dominates").unwrap();
            let operation = custody.begin_operation("debt dominates").unwrap();
            let record = ChildNameV2::from_bytes(b"record").unwrap();
            assert!(matches!(
                operation.stage(&record, b"A", "capacity debt"),
                Err(JournalMutationOutcomeV2::ProtectiveDebt(_))
            ));
            assert!(operation.debt());
            let sample = required_file_content_snapshot_v2(
                &File::open(case.root.join("ordinary-0")).unwrap(),
                "sample",
            )
            .unwrap();
            let reserved = ChildNameV2::from_bytes(b".a2a-v2-x").unwrap();
            for outcome in [
                operation.stage(&reserved, b"B", "reserved on debt"),
                operation.publish(&reserved, sample, "reserved on debt"),
                operation.append(
                    &reserved,
                    sample,
                    sample.content_len,
                    b"B",
                    "reserved on debt",
                ),
            ] {
                assert!(
                    matches!(outcome, Err(JournalMutationOutcomeV2::ProtectiveDebt(_))),
                    "{outcome:?}"
                );
            }
            fs::rename(&case.root, case.root.with_file_name("journal-moved")).unwrap();
            assert!(matches!(
                operation.stage(&record, b"A", "route loss on debt"),
                Err(JournalMutationOutcomeV2::ProtectiveDebt(_))
            ));
        }

        #[test]
        fn journal_append_reserves_capacity_headroom() {
            for count in [4095, 4096] {
                let case = route_case();
                fs::write(case.root.join("record"), b"A").unwrap();
                fill_root_to(&case, count);
                let custody =
                    JournalRootCustodyV2::open(&case.anchor, &case.binding, "append capacity")
                        .unwrap();
                let operation = custody.begin_operation("append capacity").unwrap();
                let record = ChildNameV2::from_bytes(b"record").unwrap();
                let expected = required_file_content_snapshot_v2(
                    &File::open(case.root.join("record")).unwrap(),
                    "append capacity",
                )
                .unwrap();
                let outcome = operation.append(&record, expected, 1, b"B", "append capacity");
                if count == 4095 {
                    assert!(outcome.is_ok(), "{outcome:?}");
                    assert_eq!(fs::read(case.root.join("record")).unwrap(), b"AB");
                } else {
                    assert!(matches!(
                        outcome,
                        Err(JournalMutationOutcomeV2::ProtectiveDebt(_))
                    ));
                    assert_eq!(fs::read(case.root.join("record")).unwrap(), b"A");
                }
            }
        }
    }
    #[cfg(not(unix))]
    mod journal_route_custody_v2_non_unix {
        use super::*;

        #[test]
        fn journal_route_custody_v2_refuses_before_opening_on_non_unix() {
            let identity =
                required_object_identity_v2(1, 2, BirthTimeV1::new(3, 4), "synthetic identity")
                    .unwrap();
            let binding = JournalRootBindingV2::new(
                identity,
                ChildNameV2::from_bytes(b"parent").unwrap(),
                identity,
                ChildNameV2::from_bytes(b"journal").unwrap(),
                identity,
                ChildNameV2::from_bytes(b"operation.lock").unwrap(),
                identity,
            )
            .unwrap();
            assert!(matches!(
                JournalRootCustodyV2::open(Path::new("never-opened"), &binding, "non-unix"),
                Err(FsCustodyError::Unsupported(_))
            ));
        }
    }
    #[cfg(unix)]
    mod custody_v2 {
        use super::*;
        #[rustfmt::skip]
        fn custody_v2_case(operation: CustodyOperationKindV2, predecessor: &[u8]) -> (tempfile::TempDir, File, CustodyIntentV2, PathBuf) {
            let dir = tempfile::tempdir().unwrap();
            fs::write(dir.path().join("target"), predecessor).unwrap();
            fs::write(dir.path().join("staged"), b"successor").unwrap();
            let snapshot = |name| required_file_content_snapshot_v2(&File::open(dir.path().join(name)).unwrap(), name).unwrap();
            let intent = CustodyIntentV2::new(operation, ChildNameV2::from_bytes(b"target").unwrap(), snapshot("target").object, snapshot("staged")).unwrap();
            let custody = dir.path().join(intent.capture_name().as_os_str());
            let parent = open_directory_no_follow_raw(dir.path()).unwrap();
            (dir, parent, intent, custody)
        }

        #[test]
        fn custody_v2_file_snapshot_tracks_length_outside_identity_and_refuses_non_regular() {
            use std::io::Write as _;
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("file");
            let mut file = File::create(&path).unwrap();
            let before = required_file_content_snapshot_v2(&file, "before growth").unwrap();
            file.write_all(b"changed").unwrap();
            let after = required_file_content_snapshot_v2(&file, "after growth").unwrap();
            assert_eq!(before.object, after.object);
            assert_ne!(before.content_len, after.content_len);
            let non_regular =
                required_file_content_snapshot_v2(&File::open(dir.path()).unwrap(), "dir");
            assert!(matches!(non_regular, Err(FsCustodyError::Unsupported(_))));
            let no_birthtime = required_object_identity_v2(1, 2, None, "missing birthtime");
            assert!(matches!(no_birthtime, Err(FsCustodyError::Unsupported(_))));
        }

        #[test]
        fn custody_v2_incomplete_pre_capture_identity_never_reaches_rename() {
            use std::cell::Cell;

            fn assert_stops_before_rename(parent: &File, intent: &CustodyIntentV2) {
                let boundaries = Cell::new(0);
                let renames = Cell::new(0);
                let outcome = capture_target_no_replace_v2_with(
                    parent,
                    intent,
                    "incomplete identity",
                    |_| boundaries.set(boundaries.get() + 1),
                    |_, _, _| {
                        renames.set(renames.get() + 1);
                        Ok(())
                    },
                );
                assert!(matches!(
                    outcome,
                    CustodyCaptureOutcomeV2::RefusedNoEffect(_)
                        | CustodyCaptureOutcomeV2::RuntimeUnsupported(_)
                ));
                assert_eq!((boundaries.get(), renames.get()), (0, 0));
            }

            let (dir, parent, intent, _) =
                custody_v2_case(CustodyOperationKindV2::Replace, b"predecessor");
            fs::remove_file(dir.path().join("target")).unwrap();
            assert_stops_before_rename(&parent, &intent);

            fs::create_dir(dir.path().join("target")).unwrap();
            assert_stops_before_rename(&parent, &intent);
            assert_stops_before_rename(&File::open(dir.path().join("staged")).unwrap(), &intent);

            let renames = Cell::new(0);
            let outcome = capture_target_no_replace_v2_with_probe(
                &parent,
                &intent,
                "missing birthtime",
                |_| panic!("identity refusal must precede the boundary"),
                |_, _, _| {
                    renames.set(renames.get() + 1);
                    Ok(())
                },
                |_, _, _| Err(FsCustodyError::Unsupported("missing birthtime".into())),
            );
            assert!(matches!(
                outcome,
                CustodyCaptureOutcomeV2::RuntimeUnsupported(_)
            ));
            assert_eq!(renames.get(), 0);
        }

        #[test]
        fn custody_v2_names_and_both_intents_are_distinct_bounded_and_reversible() {
            use std::os::unix::ffi::OsStrExt as _;
            for invalid in [
                b"".as_slice(),
                b".",
                b"..",
                b"a/b",
                b"a\\b",
                b"a\0b",
                b"\xff",
            ] {
                assert!(
                    ChildNameV2::from_bytes(invalid).is_err(),
                    "accepted {invalid:?}"
                );
            }
            let target = ChildNameV2::from_bytes(b"authority").unwrap();
            let mut encoded = Vec::new();
            for namespace in ReservedNameNamespaceV2::ALL {
                let name = ChildNameV2::reserved(namespace, &target).unwrap();
                assert_eq!(
                    ChildNameV2::parse_reserved(namespace, &name).unwrap(),
                    target
                );
                for other in ReservedNameNamespaceV2::ALL {
                    if other != namespace {
                        assert!(ChildNameV2::parse_reserved(other, &name).is_err());
                    }
                }
                encoded.push(name.as_os_str().as_bytes().to_vec());
            }
            encoded.sort();
            encoded.dedup();
            assert_eq!(encoded.len(), 4);

            let exact_bound =
                ChildNameV2::from_bytes(&vec![b'x'; MAX_RESERVED_SOURCE_V2_BYTES]).unwrap();
            for namespace in ReservedNameNamespaceV2::ALL {
                let encoded = ChildNameV2::reserved(namespace, &exact_bound).unwrap();
                assert_eq!(
                    encoded.as_os_str().as_bytes().len(),
                    MAX_CHILD_NAME_V2_BYTES
                );
                assert_eq!(
                    ChildNameV2::parse_reserved(namespace, &encoded).unwrap(),
                    exact_bound
                );
            }

            let expected =
                required_object_identity_v2(1, 2, BirthTimeV1::new(3, 4), "expected").unwrap();
            let staged = FileContentSnapshotV2 {
                object: required_object_identity_v2(5, 6, BirthTimeV1::new(7, 8), "staged")
                    .unwrap(),
                content_len: 9,
            };
            assert_ne!(expected, staged.object);
            let overflow =
                ChildNameV2::from_bytes(&vec![b'x'; MAX_RESERVED_SOURCE_V2_BYTES + 1]).unwrap();
            for operation in [
                CustodyOperationKindV2::Replace,
                CustodyOperationKindV2::Retire,
            ] {
                let intent =
                    CustodyIntentV2::new(operation, target.clone(), expected, staged).unwrap();
                assert_eq!(intent.parts(), (operation, &target, &expected, &staged));
                let namespace = match operation {
                    CustodyOperationKindV2::Replace => ReservedNameNamespaceV2::ReplacementCapture,
                    CustodyOperationKindV2::Retire => ReservedNameNamespaceV2::RetirementCapture,
                };
                assert_eq!(
                    ChildNameV2::parse_reserved(namespace, intent.capture_name()).unwrap(),
                    target
                );
                let overflowed =
                    CustodyIntentV2::new(operation, overflow.clone(), expected, staged);
                assert!(overflowed.is_err());
            }
        }

        #[test]
        fn custody_v2_expected_capture_uses_distinct_replace_and_retire_names() {
            for operation in [
                CustodyOperationKindV2::Replace,
                CustodyOperationKindV2::Retire,
            ] {
                let (dir, parent, intent, custody) = custody_v2_case(operation, b"predecessor");
                let outcome = capture_target_no_replace_v2(&parent, &intent, "expected capture");
                assert!(matches!(
                    outcome,
                    CustodyCaptureOutcomeV2::ExpectedCaptured(_)
                ));
                assert_eq!(fs::read(custody).unwrap(), b"predecessor");
                assert!(!dir.path().join("target").exists());
            }
        }

        #[test]
        fn custody_v2_occupied_custody_is_unknown_without_clobbering_either_object() {
            let (dir, parent, intent, custody) =
                custody_v2_case(CustodyOperationKindV2::Replace, b"predecessor");
            let outcome = capture_target_no_replace_v2_with(
                &parent,
                &intent,
                "occupied custody",
                |restoring| {
                    if !restoring {
                        fs::write(&custody, b"occupied").unwrap();
                    }
                },
                rename_child_no_replace,
            );
            assert!(matches!(outcome, CustodyCaptureOutcomeV2::Unknown(_)));
            assert_eq!(fs::read(dir.path().join("target")).unwrap(), b"predecessor");
            assert_eq!(fs::read(custody).unwrap(), b"occupied");
        }

        #[test]
        fn custody_v2_failed_capture_never_claims_or_restores_foreign_custody() {
            use std::cell::Cell;

            let (dir, parent, intent, custody) =
                custody_v2_case(CustodyOperationKindV2::Replace, b"predecessor");
            let calls = Cell::new(0);
            let outcome = capture_target_no_replace_v2_with(
                &parent,
                &intent,
                "foreign custody",
                |restoring| {
                    assert!(!restoring, "capture error must not enter restoration");
                    fs::write(&custody, b"foreign").unwrap();
                    fs::rename(dir.path().join("target"), dir.path().join("lost")).unwrap();
                },
                |_, _, _| {
                    calls.set(calls.get() + 1);
                    Err(RenameNoReplaceRefusalV1::Io(
                        std::io::ErrorKind::Other.into(),
                    ))
                },
            );
            assert!(matches!(outcome, CustodyCaptureOutcomeV2::Unknown(_)));
            assert_eq!(calls.get(), 1);
            assert_eq!(fs::read(custody).unwrap(), b"foreign");
            assert_eq!(fs::read(dir.path().join("lost")).unwrap(), b"predecessor");
            assert!(!dir.path().join("target").exists());
        }

        #[test]
        fn custody_v2_error_after_effect_and_hard_link_back_never_probes_or_claims_no_effect() {
            use std::cell::Cell;

            let (dir, parent, intent, custody) =
                custody_v2_case(CustodyOperationKindV2::Replace, b"predecessor");
            let calls = Cell::new(0);
            let probes = Cell::new(0);
            let outcome = capture_target_no_replace_v2_with_probe(
                &parent,
                &intent,
                "error after effect",
                |_| {},
                |parent, source, destination| {
                    calls.set(calls.get() + 1);
                    rename_child_no_replace(parent, source, destination).unwrap();
                    fs::hard_link(&custody, dir.path().join("target")).unwrap();
                    Err(RenameNoReplaceRefusalV1::Io(
                        std::io::ErrorKind::Other.into(),
                    ))
                },
                |parent, name, label| {
                    probes.set(probes.get() + 1);
                    required_identity_at_v2(parent, name, label)
                },
            );
            assert!(matches!(outcome, CustodyCaptureOutcomeV2::Unknown(_)));
            assert_eq!(calls.get(), 1);
            assert_eq!(probes.get(), 1, "only the pre-capture probe is allowed");
            assert_eq!(fs::read(dir.path().join("target")).unwrap(), b"predecessor");
            assert_eq!(fs::read(custody).unwrap(), b"predecessor");
        }

        #[test]
        fn custody_v2_unexpected_capture_stays_in_custody_without_restoration() {
            use std::cell::Cell;

            let (dir, parent, intent, custody) =
                custody_v2_case(CustodyOperationKindV2::Replace, b"A");
            let calls = Cell::new(0);
            let outcome = capture_target_no_replace_v2_with(
                &parent,
                &intent,
                "A/B substitution",
                |restoring| {
                    if !restoring {
                        fs::rename(dir.path().join("target"), dir.path().join("A")).unwrap();
                        fs::write(dir.path().join("target"), b"B").unwrap();
                    }
                },
                |parent, source, destination| {
                    calls.set(calls.get() + 1);
                    rename_child_no_replace(parent, source, destination)
                },
            );
            assert!(matches!(outcome, CustodyCaptureOutcomeV2::Unknown(_)));
            assert_eq!(calls.get(), 1);
            assert_eq!(fs::read(dir.path().join("A")).unwrap(), b"A");
            assert_eq!(fs::read(custody).unwrap(), b"B");
            assert!(!dir.path().join("target").exists());
        }

        #[test]
        fn custody_v2_old_restoration_boundary_substitution_never_moves_c_into_target() {
            use std::cell::Cell;

            let (dir, parent, intent, custody) =
                custody_v2_case(CustodyOperationKindV2::Replace, b"A");
            let calls = Cell::new(0);
            let outcome = capture_target_no_replace_v2_with(
                &parent,
                &intent,
                "custody substitution",
                |restoring| {
                    if restoring {
                        fs::rename(&custody, dir.path().join("B")).unwrap();
                        fs::write(&custody, b"C").unwrap();
                    } else {
                        fs::rename(dir.path().join("target"), dir.path().join("A")).unwrap();
                        fs::write(dir.path().join("target"), b"B").unwrap();
                    }
                },
                |parent, source, destination| {
                    calls.set(calls.get() + 1);
                    rename_child_no_replace(parent, source, destination)
                },
            );
            assert!(matches!(outcome, CustodyCaptureOutcomeV2::Unknown(_)));
            assert_eq!(calls.get(), 1);
            assert_eq!(fs::read(dir.path().join("A")).unwrap(), b"A");
            assert_eq!(fs::read(dir.path().join("B")).unwrap(), b"B");
            assert_eq!(fs::read(custody).unwrap(), b"C");
            assert!(!dir.path().join("target").exists());
        }

        #[test]
        fn custody_v2_target_takeover_leaves_captured_object_as_unknown_debt() {
            use std::cell::Cell;

            let (dir, parent, intent, custody) =
                custody_v2_case(CustodyOperationKindV2::Retire, b"A");
            let calls = Cell::new(0);
            let outcome = capture_target_no_replace_v2_with(
                &parent,
                &intent,
                "target takeover",
                |restoring| {
                    if restoring {
                        fs::write(dir.path().join("target"), b"takeover").unwrap();
                    } else {
                        fs::rename(dir.path().join("target"), dir.path().join("A")).unwrap();
                        fs::write(dir.path().join("target"), b"B").unwrap();
                    }
                },
                |parent, source, destination| {
                    calls.set(calls.get() + 1);
                    rename_child_no_replace(parent, source, destination)
                },
            );
            assert!(matches!(outcome, CustodyCaptureOutcomeV2::Unknown(_)));
            assert_eq!(calls.get(), 1);
            assert_eq!(fs::read(dir.path().join("target")).unwrap(), b"takeover");
            assert_eq!(fs::read(custody).unwrap(), b"B");
        }

        #[test]
        fn custody_v2_compile_unsupported_and_io_are_typed_without_fallback() {
            use std::cell::Cell;
            for (case, refusal) in [
                (0, RenameNoReplaceRefusalV1::PlatformUnsupported),
                (
                    1,
                    RenameNoReplaceRefusalV1::Io(std::io::Error::from_raw_os_error(libc::ENOTSUP)),
                ),
                (
                    2,
                    RenameNoReplaceRefusalV1::Io(std::io::Error::from_raw_os_error(libc::EIO)),
                ),
            ] {
                let (dir, parent, intent, custody) =
                    custody_v2_case(CustodyOperationKindV2::Replace, b"predecessor");
                let mut refusal = Some(refusal);
                let calls = Cell::new(0);
                let outcome = capture_target_no_replace_v2_with(
                    &parent,
                    &intent,
                    "unsupported",
                    |_| {},
                    |_, _, _| {
                        calls.set(calls.get() + 1);
                        if case == 2 {
                            fs::rename(dir.path().join("target"), dir.path().join("lost")).unwrap();
                        }
                        Err(refusal.take().unwrap())
                    },
                );
                assert!(match case {
                    0 => matches!(outcome, CustodyCaptureOutcomeV2::CompileUnsupported),
                    1 => matches!(outcome, CustodyCaptureOutcomeV2::RuntimeUnsupported(_)),
                    _ => matches!(outcome, CustodyCaptureOutcomeV2::Unknown(_)),
                });
                assert_eq!(calls.get(), 1, "one no-replace attempt and zero fallbacks");
                let preserved = if case == 2 { "lost" } else { "target" };
                assert_eq!(
                    fs::read(dir.path().join(preserved)).unwrap(),
                    b"predecessor"
                );
                assert!(
                    !custody.exists(),
                    "no outcome may fall back to replacing rename"
                );
            }
        }
    }

    #[cfg(not(unix))]
    mod custody_v2_non_unix {
        use super::*;

        #[test]
        fn portable_surface_constructs_and_capture_is_compile_unsupported() {
            assert!(ChildNameV2::from_bytes(b"\xff").is_err());
            let target = ChildNameV2::from_bytes(b"target").unwrap();
            let expected =
                required_object_identity_v2(1, 2, BirthTimeV1::new(3, 4), "expected").unwrap();
            let staged = FileContentSnapshotV2 {
                object: required_object_identity_v2(5, 6, BirthTimeV1::new(7, 8), "staged")
                    .unwrap(),
                content_len: 9,
            };
            let intent =
                CustodyIntentV2::new(CustodyOperationKindV2::Replace, target, expected, staged)
                    .unwrap();
            let ignored_parent = tempfile::tempfile().unwrap();
            assert!(matches!(
                capture_target_no_replace_v2(&ignored_parent, &intent, "unsupported"),
                CustodyCaptureOutcomeV2::CompileUnsupported
            ));
        }
    }
}
