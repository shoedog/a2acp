//! ADR-0041 slice 2B3b, pass A: the isolated Git object bootstrap.
//!
//! [`bootstrap_git_objects_v1`] consumes the crate-private phase-V [`VerifiedCapsuleV1`] and proves
//! the capsule's Git objects in a private bootstrap repository, `.restore-work/git-bootstrap`,
//! beside empty pinned `home` and `xdg` directories. It returns a retained
//! [`VerifiedGitBootstrapV1`] for pass B. It never returns a restored Git plane or a restore record,
//! and it creates NOTHING under `repository/`: pass B copies the verified object store, restores
//! the metadata, and returns phase G's result.
//!
//! **Effects.** Every write is create-new beneath a retained pin, charged to the original phase-V
//! ledger before it happens. Nothing is renamed, overwritten, or deleted, not even on failure.
//! Every Git child is a [`GitRunnerV1`] child rooted at the retained `.restore-work` pin, with the
//! runner's closed environment and one caller-supplied absolute deadline, and never an object-store
//! route: the only object database it can see is the bootstrap's own.
//!
//! **Remeasurement.** The bootstrap is measured through retained pins only. Each directory is
//! listed through its own descriptor, each child is `fstatat`ed without following links, and each
//! regular file is opened no-follow and `fstat`ed. A link, special file, unexpected name, missing
//! required file, device change, or identity change refuses. The exporter's path walker, which
//! reopens names, is deliberately not used.
//!
//! **Accounting.** The census counts every raw entry (never a rendered name) and every logical byte,
//! separately for the bootstrap, HOME, and XDG. The bootstrap owns ONE reservation on the ledger,
//! entries and bytes together ([`OwnedChargeV1`]): the nine `for_init` entries and 4 KiB before
//! `init`, then the three index entries and the proven bound before `index-pack`. HOME and XDG
//! descendants are reserved nothing, so any entry or byte there is an overrun. Every outcome of a
//! child settles exactly once. A complete census that fits releases exactly the unused allowance, and
//! the owned figure drops to the measurement, so nothing is released twice; an overrun, or a census
//! that cannot complete, releases nothing and is refused beside the child's own failure. A
//! nominally read-only child owns a zero-effect allowance: the tree must equal its pre-child
//! baseline on every outcome. That baseline keeps, for every regular file, the complete non-atime
//! facts the existing `fstat` seam reports (length, device, inode, mode, mtime, ctime; reading
//! changes only atime) AND a SHA-256 streamed from a no-follow descriptor opened through the
//! retained pin, because a same-inode, same-length rewrite changes neither the names nor the lengths.
//! Every digest descriptor is itself bound to that census: its own `fstat`, before the first read and
//! after the last, must equal the file's measured non-atime facts, and exactly the measured byte count
//! must be read, so a name swapped to another inode after the census cannot lend a digest its bytes.
//! Every digest, before a child and after it, keeps the ONE absolute deadline: there is no exception
//! after a child, and no success past it. So does everything between a child's result and a verified
//! value: a read-only child's post-child success gate, the staged rewinds, and the closing gate (mount
//! census, exact tree, admitted-file rehash, every retained chain). The runner enforces the deadline
//! only while a child starts and waits, and a digest checks it only before each read, so each of those
//! steps is followed by a guard ([`observe_deadline`]) that reads the clock as the step returns, and
//! each loop by one before every iteration; filesystem and platform calls cannot be preempted, so
//! that is the strongest observation there is. The last guard is the one immediately before the
//! destination root's identity check, which stays the LAST fallible call: only infallible construction
//! follows it. A gate that serves a child that already failed, admission, or a runner callback is not
//! held to the deadline (a failed child's effect and ledger are still reported beside its failure).
//! A create-new that provably created nothing gives its reservation back.
//! Nothing here touches a byte on disk: reconciliation changes only the ledger and the typed refusal.

use super::{
    create_staging_directory, hash_prefix, joined, lossy, recheck_entry, refuse_mounts_within,
    refused_link, staging_refusal, CustodyRestoreErrorV1, StagedPlaintextV1, VerifiedCapsuleV1,
    HASH_BUFFER_BYTES_V1, RESTORE_WORK_DIR_NAME_V1,
};
use crate::custody_capsule::CustodyCapsuleArtifactRoleV1;
use crate::custody_export::{
    bounded, index_pack_logical_bound, stdout_limit_for_objects, strict_object_message_id,
    verify_pack_stdout_limit, CustodyExportErrorV1, GitDirectoryBudgetV1, GitRunRecordV1,
    ScratchLedgerV1, ENTRY_ALLOWANCE_BYTES_V1, FSCK_ALLOWED_NOTICE_V1, GIT_DIR_MAX_DEPTH_V1,
    GIT_DIR_MAX_ENTRIES_V1, GIT_MIN_STDOUT_LIMIT_V1, GIT_STDERR_LIMIT_V1, INDEX_PACK_ENTRIES_V1,
};
use crate::custody_git::{
    CustodyGitError, GitCommandV1, GitObjectFormatV1, GitRootNamesV1, GitRunRequestV1,
    GitRunResultV1, GitRunnerV1,
};
use crate::custody_inventory::CustodyStateClassV1;
use crate::custody_seal::{
    CustodyCoverageClassV1, CustodyGitObjectFormatV1, CustodyGitObjectKindV1, CustodyManifestV1,
};
use crate::fs_custody::{
    pinned_root_unchanged, ChildKindV1, ChildStatV1, FsCustodyError, PinnedDirectoryV1,
};
use ring::digest;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::File;
use std::io::{ErrorKind, Read as _, Seek as _, SeekFrom};
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;
use std::time::Instant;

// ---------------------------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------------------------

const HOME_NAME_V1: &str = "home";
const XDG_NAME_V1: &str = "xdg";
const BOOTSTRAP_NAME_V1: &str = "git-bootstrap";
/// The nested directories `git init --bare --template=` leaves in the bootstrap, parents first.
const BOOTSTRAP_DIRECTORIES_V1: [&str; 6] = [
    "objects",
    "objects/info",
    "objects/pack",
    "refs",
    "refs/heads",
    "refs/tags",
];
const PACK_DIRECTORY_V1: &str = "objects/pack";

// ---------------------------------------------------------------------------------------------
// Typed progress, provenance, and failure
// ---------------------------------------------------------------------------------------------

/// The steps of phase G, in order. Pass A reaches [`Self::BootstrapGate`]; the rest belong to
/// pass B and are enumerated here so one progress type spans both.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum GitPlaneStepV1 {
    Admission,
    BootstrapInit,
    Indexing,
    Inventory,
    Closure,
    Fsck,
    BootstrapGate,
    Objects,
    Metadata,
    Refs,
    Config,
    FinalGate,
}

/// What authenticated the object format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GitFormatProvenanceV1 {
    /// A Captured database: the manifest's objects and the pack agree with the caller's format.
    CapsuleObjectsAndPack,
    /// A valid Empty database: the caller chose the format. The sealed capsule authenticated
    /// nothing about it.
    CallerSelectedEmpty,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum GitBootstrapErrorV1 {
    // Admission.
    #[error("a staged input is not the verified plaintext: {name}")]
    StagedInputChanged { name: String },
    #[error("the object format disagrees with the capsule: {0}")]
    FormatMismatch(String),
    #[error("the captured Git pack shape is refused: {0}")]
    PackShape(String),
    #[error("an Empty object database is contradicted: {0}")]
    EmptyShape(String),
    #[error("a coverage state phase G cannot restore: {class:?}: {detail}")]
    UnsupportedCoverage {
        class: CustodyCoverageClassV1,
        detail: &'static str,
    },
    #[error("the manifest's object inventory is invalid: {0}")]
    ManifestInventory(String),
    // Destination, pins, ledger.
    #[error(transparent)]
    Restore(#[from] CustodyRestoreErrorV1),
    // The bounded descriptor-relative census.
    #[error("a bootstrap entry is not the expected one: {name}")]
    UnexpectedEntry { name: String },
    #[error("a bootstrap entry is a special file: {name}")]
    SpecialEntry { name: String },
    #[error("a required bootstrap entry is missing: {name}")]
    MissingEntry { name: String },
    #[error("the bootstrap census exceeded its bound: {0}")]
    CensusBound(String),
    #[error("an admitted object file is no longer the bytes that were admitted: {relative}")]
    AdmittedObjectChanged { relative: String },
    // Git.
    #[error(transparent)]
    Git(#[from] CustodyGitError),
    #[error("git {command} failed with status {status:?}: {detail}")]
    GitChild {
        command: &'static str,
        status: Option<i32>,
        detail: String,
    },
    #[error("git rejected a strict object check: {0}")]
    StrictObjectCheck(String),
    #[error("git refused the pack: {0}")]
    StrictPack(String),
    #[error("git printed output the bootstrap refuses: {0}")]
    GitOutput(String),
    #[error("the bytes index-pack consumed are not the verified staged pack")]
    VerificationInputMismatch,
    #[error("the bootstrap's objects are not the manifest's inventory: {0}")]
    InventoryMismatch(String),
    #[error("the manifest's objects are not closed in the bootstrap: {0}")]
    ClosureIncomplete(String),
    /// A child failed, and reconciling what it left behind failed too. Neither is dropped: the
    /// reconciliation failure is a budget or custody refusal in its own right.
    #[error("{primary}; reconciling the failed child's effects also failed: {reconcile}")]
    ChildAndReconcile {
        primary: Box<GitBootstrapErrorV1>,
        reconcile: Box<GitBootstrapErrorV1>,
    },
}

/// A refused or failed bootstrap, with how far it got. Pass A never begins active materialization,
/// so that flag is always false; it is carried so one failure shape spans both passes. Every byte
/// created before the failure is preserved.
#[derive(Debug)]
pub(crate) struct GitBootstrapFailureV1 {
    pub(crate) cause: GitBootstrapErrorV1,
    /// The last step that completed whole, if any.
    pub(crate) last_completed: Option<GitPlaneStepV1>,
    pub(crate) active_materialization_began: bool,
    /// The one ledger's charge when the failure was returned.
    pub(crate) ledger_used: u64,
}

/// The vacuous-or-proven closure of the manifest's objects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GitClosureProofV1 {
    /// Empty: no object, no pack, no `rev-list` child.
    Vacuous,
    /// Captured: every inventory object was fed to `rev-list`, and the reached set equals it.
    Reachable { objects: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GitClosureEvidenceV1 {
    pub(crate) inventory_objects: usize,
    pub(crate) closure: GitClosureProofV1,
    /// `fsck --strict` ran and every diagnostic was classified.
    pub(crate) fsck_strict: bool,
}

/// One admitted bootstrap object file: its retained no-follow descriptor, and its length and SHA-256
/// measured from it.
#[derive(Debug)]
pub(crate) struct AdmittedObjectFileV1 {
    /// Relative to the bootstrap, as `objects/pack/pack-<hash>.pack`.
    pub(crate) relative: String,
    pub(crate) file: File,
    pub(crate) length: u64,
    pub(crate) sha256: [u8; 32],
    pub(crate) dev: u64,
    pub(crate) ino: u64,
}

/// The bootstrap's object store.
#[derive(Debug)]
pub(crate) enum BootstrapObjectsV1 {
    /// The one staged pack, strictly indexed; its `.pack`, `.idx`, and `.rev`, in that order.
    Captured {
        pack_hash: String,
        files: Vec<AdmittedObjectFileV1>,
    },
    Empty,
}

/// An active payload phase G restores: the staged frame, by index into the verified capsule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ActiveFrameV1 {
    pub(crate) class: CustodyCoverageClassV1,
    pub(crate) staged: usize,
}

/// Pass A's success value, retained for pass B. It is NOT a restored Git plane: no `repository/`
/// entry exists, and no refs, HEAD, config, or final object store.
#[derive(Debug)]
pub(crate) struct VerifiedGitBootstrapV1 {
    /// The original verified capsule, with its one ledger and every retained staged descriptor.
    pub(crate) verified: VerifiedCapsuleV1,
    pub(crate) home: PinnedDirectoryV1,
    pub(crate) xdg: PinnedDirectoryV1,
    /// Every bootstrap directory by its path relative to `git-bootstrap` (`""` is the bootstrap).
    pub(crate) directories: BTreeMap<String, PinnedDirectoryV1>,
    pub(crate) object_format: GitObjectFormatV1,
    pub(crate) provenance: GitFormatProvenanceV1,
    pub(crate) objects: BootstrapObjectsV1,
    pub(crate) closure: GitClosureEvidenceV1,
    /// `RefsAndHead` and `StashAndReflogs`, where Captured.
    pub(crate) active_frames: Vec<ActiveFrameV1>,
    /// Each Git child's recorded evidence, in order.
    pub(crate) runs: Vec<GitRunRecordV1>,
}

impl VerifiedGitBootstrapV1 {
    pub(crate) fn bootstrap(&self) -> Option<&PinnedDirectoryV1> {
        self.directories.get("")
    }

    pub(crate) fn pack_directory(&self) -> Option<&PinnedDirectoryV1> {
        self.directories.get(PACK_DIRECTORY_V1)
    }
}

// ---------------------------------------------------------------------------------------------
// Test seams
// ---------------------------------------------------------------------------------------------

/// Where a test hook runs.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum GitHookPointV1 {
    /// Entry to phase G, before its first gate (the destination root).
    AdmissionEntry,
    /// HOME and XDG exist; the bootstrap does not (the work directory).
    BeforeBootstrapCreate,
    /// A child's census is about to run, before any pre-spawn check (the work directory).
    BeforeChildCensus,
    /// A child's pre-spawn callback, before its gate (the work directory).
    BeforeChild,
    /// A child's post-exit callback, before its gate (the work directory).
    AfterChild,
    /// One bootstrap directory is listed, before its entries are examined (that directory).
    CensusListed,
    /// A listed entry was `fstatat`ed and is about to be opened or adopted (that entry).
    BeforeCensusOpen,
    /// All proofs are done, before the closing gate (the destination root).
    BeforeFinalGate,
    /// A digest opened a file through its retained directory pin, and the descriptor matched its
    /// census fact; nothing has been read from it yet (that file).
    DigestOpened,
    /// A digest read a file to its end; its byte count and its closing `fstat` check have not run
    /// yet (that file).
    DigestHashed,
    /// A late-validation step has returned and the one deadline is about to be read: the event's
    /// label names the step (no path). A hook that sleeps here is that step having been slow.
    DeadlineObserved,
}

#[cfg(test)]
pub(crate) struct GitHookEventV1<'a> {
    /// The Git child's command label, or `""` outside a child.
    pub(crate) label: &'static str,
    pub(crate) path: &'a Path,
}

#[cfg(test)]
type GitHookV1 = Box<dyn Fn(&GitHookEventV1<'_>)>;

#[cfg(test)]
thread_local! {
    static GIT_HOOKS: RefCell<BTreeMap<GitHookPointV1, GitHookV1>> = RefCell::new(BTreeMap::new());
    /// A device reported for a bootstrap directory or file, by its bootstrap-relative name.
    static CENSUS_DEV: RefCell<BTreeMap<String, u64>> = const { RefCell::new(BTreeMap::new()) };
    /// Which of a census fact's timestamps are reported as never having advanced.
    static CENSUS_CLOCKS: std::cell::Cell<StalledClocksV1> =
        const { std::cell::Cell::new(StalledClocksV1 { mtime: false, ctime: false }) };
}

/// The timestamps a test makes the census see as never having moved: a filesystem whose clock is too
/// coarse to show a write made within one tick. The metadata comparison then cannot see that write,
/// which isolates every other guard (the digest, and each other field) from the stalled ones.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct StalledClocksV1 {
    pub(crate) mtime: bool,
    pub(crate) ctime: bool,
}

/// Uninstalls its one seam when dropped.
#[cfg(test)]
pub(crate) enum GitSeamGuardV1 {
    Hook(GitHookPointV1),
    CensusDev(String),
    CensusClocks,
}

#[cfg(test)]
impl Drop for GitSeamGuardV1 {
    fn drop(&mut self) {
        match self {
            Self::Hook(point) => GIT_HOOKS.with(|hooks| {
                hooks.borrow_mut().remove(point);
            }),
            Self::CensusDev(name) => CENSUS_DEV.with(|devices| {
                devices.borrow_mut().remove(name);
            }),
            Self::CensusClocks => {
                CENSUS_CLOCKS.with(|clocks| clocks.set(StalledClocksV1::default()))
            }
        }
    }
}

#[cfg(test)]
pub(crate) fn install_git_hook_for_test(
    point: GitHookPointV1,
    hook: impl Fn(&GitHookEventV1<'_>) + 'static,
) -> GitSeamGuardV1 {
    GIT_HOOKS.with(|hooks| hooks.borrow_mut().insert(point, Box::new(hook)));
    GitSeamGuardV1::Hook(point)
}

/// Reports `dev` for the bootstrap entry at `relative` (`""` is the bootstrap itself).
#[cfg(test)]
pub(crate) fn override_census_dev_for_test(relative: &str, dev: u64) -> GitSeamGuardV1 {
    CENSUS_DEV.with(|devices| devices.borrow_mut().insert(relative.to_owned(), dev));
    GitSeamGuardV1::CensusDev(relative.to_owned())
}

/// Makes the census report the `stalled` timestamps of every regular file as zero.
#[cfg(test)]
pub(crate) fn stall_census_clocks_for_test(stalled: StalledClocksV1) -> GitSeamGuardV1 {
    CENSUS_CLOCKS.with(|clocks| clocks.set(stalled));
    GitSeamGuardV1::CensusClocks
}

#[cfg(test)]
fn run_git_hook(point: GitHookPointV1, label: &'static str, path: &Path) {
    GIT_HOOKS.with(|hooks| {
        if let Some(hook) = hooks.borrow().get(&point) {
            hook(&GitHookEventV1 { label, path });
        }
    });
}

#[cfg(test)]
fn observed_dev(relative: &str, observed: u64) -> u64 {
    CENSUS_DEV
        .with(|devices| devices.borrow().get(relative).copied())
        .unwrap_or(observed)
}

#[cfg(not(test))]
fn observed_dev(_relative: &str, observed: u64) -> u64 {
    observed
}

#[cfg(test)]
fn observed_stat(mut stat: ChildStatV1) -> ChildStatV1 {
    let stalled = CENSUS_CLOCKS.with(std::cell::Cell::get);
    if stalled.mtime {
        stat.mtime_ns = 0;
    }
    if stalled.ctime {
        stat.ctime_ns = 0;
    }
    stat
}

#[cfg(not(test))]
fn observed_stat(stat: ChildStatV1) -> ChildStatV1 {
    stat
}

// ---------------------------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------------------------

fn manifest_format(format: GitObjectFormatV1) -> CustodyGitObjectFormatV1 {
    match format {
        GitObjectFormatV1::Sha1 => CustodyGitObjectFormatV1::Sha1,
        GitObjectFormatV1::Sha256 => CustodyGitObjectFormatV1::Sha256,
    }
}

const fn hex_width(format: GitObjectFormatV1) -> usize {
    match format {
        GitObjectFormatV1::Sha1 => 40,
        GitObjectFormatV1::Sha256 => 64,
    }
}

/// The raw hash width 2B2's index bound formula takes.
const fn raw_hash_width(format: GitObjectFormatV1) -> u64 {
    match format {
        GitObjectFormatV1::Sha1 => 20,
        GitObjectFormatV1::Sha256 => 32,
    }
}

fn kind_name(kind: CustodyGitObjectKindV1) -> &'static str {
    match kind {
        CustodyGitObjectKindV1::Commit => "commit",
        CustodyGitObjectKindV1::Tree => "tree",
        CustodyGitObjectKindV1::Blob => "blob",
        CustodyGitObjectKindV1::Tag => "tag",
        CustodyGitObjectKindV1::Unknown => "unknown",
    }
}

fn is_object_id(text: &str, format: GitObjectFormatV1) -> bool {
    text.len() == hex_width(format)
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The manifest's objects as an `oid -> kind name` map, in the caller's format. A duplicate object
/// or a different format refuses.
fn manifest_inventory(
    manifest: &CustodyManifestV1,
    format: GitObjectFormatV1,
) -> Result<BTreeMap<String, &'static str>, GitBootstrapErrorV1> {
    let mut inventory = BTreeMap::new();
    for object in manifest.original_objects() {
        if object.format() != manifest_format(format) {
            return Err(GitBootstrapErrorV1::FormatMismatch(format!(
                "{} is a {:?} object, and the caller selected {format:?}",
                object.object_id(),
                object.format()
            )));
        }
        if inventory
            .insert(object.object_id().to_owned(), kind_name(object.kind()))
            .is_some()
        {
            return Err(GitBootstrapErrorV1::ManifestInventory(format!(
                "{} is listed twice",
                object.object_id()
            )));
        }
    }
    Ok(inventory)
}

/// The records of `stdout`, split on a LITERAL line feed: valid UTF-8, every record LF-terminated,
/// so a final partial record is a truncated output and refuses. Nothing else is normalized: a carriage
/// return stays in its record (`str::lines` would silently strip it), and an empty record stays an
/// empty record, for each grammar to refuse.
fn complete_lines<'a>(stdout: &'a [u8], what: &str) -> Result<Vec<&'a str>, GitBootstrapErrorV1> {
    let text = std::str::from_utf8(stdout)
        .map_err(|_| GitBootstrapErrorV1::GitOutput(format!("{what} is not UTF-8")))?;
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let Some(body) = text.strip_suffix('\n') else {
        return Err(GitBootstrapErrorV1::GitOutput(format!(
            "{what} ends in a partial line"
        )));
    };
    Ok(body.split('\n').collect())
}

/// `cat-file --batch-all-objects` rows against the manifest's exact `(oid, kind)` set. A duplicate,
/// absent, extra, wrong-kind, or malformed row is fatal.
fn compare_inventory(
    stdout: &[u8],
    expected: &BTreeMap<String, &'static str>,
    format: GitObjectFormatV1,
) -> Result<(), GitBootstrapErrorV1> {
    let mut observed: BTreeMap<&str, &str> = BTreeMap::new();
    for line in complete_lines(stdout, "cat-file")? {
        let mut fields = line.split(' ');
        let (Some(object_id), Some(kind), None) = (fields.next(), fields.next(), fields.next())
        else {
            return Err(GitBootstrapErrorV1::GitOutput(bounded(line)));
        };
        if !is_object_id(object_id, format) || !["blob", "tree", "commit", "tag"].contains(&kind) {
            return Err(GitBootstrapErrorV1::GitOutput(bounded(line)));
        }
        if observed.insert(object_id, kind).is_some() {
            return Err(GitBootstrapErrorV1::InventoryMismatch(format!(
                "{object_id} is listed twice"
            )));
        }
    }
    for (object_id, kind) in expected {
        match observed.get(object_id.as_str()) {
            None => {
                return Err(GitBootstrapErrorV1::InventoryMismatch(format!(
                    "{object_id} is missing from the bootstrap"
                )))
            }
            Some(found) if found != kind => {
                return Err(GitBootstrapErrorV1::InventoryMismatch(format!(
                    "{object_id} is {found}, not {kind}"
                )))
            }
            Some(_) => {}
        }
    }
    if let Some(object_id) = observed
        .keys()
        .find(|object_id| !expected.contains_key(**object_id))
    {
        return Err(GitBootstrapErrorV1::InventoryMismatch(format!(
            "{object_id} is an extra object"
        )));
    }
    Ok(())
}

/// `rev-list --objects --missing=print` over EVERY inventory object: a `?` marker, a malformed row,
/// a duplicate, an unexpected object, or an unreached inventory object refuses.
fn compare_closure(
    stdout: &[u8],
    expected: &BTreeMap<String, &'static str>,
    format: GitObjectFormatV1,
) -> Result<usize, GitBootstrapErrorV1> {
    let mut reached: BTreeSet<&str> = BTreeSet::new();
    for line in complete_lines(stdout, "rev-list")? {
        if let Some(missing) = line.strip_prefix('?') {
            return Err(GitBootstrapErrorV1::ClosureIncomplete(format!(
                "{} is reachable from the inventory but absent",
                bounded(missing)
            )));
        }
        if !is_object_id(line, format) {
            return Err(GitBootstrapErrorV1::GitOutput(bounded(line)));
        }
        if !reached.insert(line) {
            return Err(GitBootstrapErrorV1::GitOutput(format!(
                "rev-list printed {line} twice"
            )));
        }
        if !expected.contains_key(line) {
            return Err(GitBootstrapErrorV1::ClosureIncomplete(format!(
                "{line} is an unexpected closure object"
            )));
        }
    }
    if let Some(object_id) = expected
        .keys()
        .find(|object_id| !reached.contains(object_id.as_str()))
    {
        return Err(GitBootstrapErrorV1::ClosureIncomplete(format!(
            "{object_id} was not reached"
        )));
    }
    Ok(reached.len())
}

/// `fsck --strict` writes NOTHING to stdout for a clean database: any byte, even whitespace or a lone
/// newline, refuses. Stderr is empty, or holds exactly one LF-terminated record, the documented
/// refless-database notice; a strict-object rejection carries a message id and is its own refusal.
/// The WHOLE stream is decoded and classified, with no trimming, before either is chosen: invalid
/// UTF-8, a partial record, a blank or whitespace-padded or CR-terminated record, an unclassified
/// record anywhere, and a repeated notice all refuse, even beside a recognized strict row. This runs
/// BEFORE the generic exit check, so a fully classified strict rejection keeps its type.
fn classify_fsck(result: &GitRunResultV1) -> Result<(), GitBootstrapErrorV1> {
    let stdout = result
        .captured_stdout()
        .ok_or_else(|| GitBootstrapErrorV1::GitOutput("fsck stdout was streamed".into()))?;
    if !stdout.is_empty() {
        return Err(GitBootstrapErrorV1::GitOutput(bounded(
            &String::from_utf8_lossy(stdout),
        )));
    }
    let records = complete_lines(&result.stderr, "fsck stderr")?;
    let mut notices = 0_usize;
    let mut strict: Option<String> = None;
    let mut unclassified: Option<String> = None;
    for record in records {
        if record == FSCK_ALLOWED_NOTICE_V1 {
            notices += 1;
        } else if let Some(message_id) = strict_object_message_id(record) {
            strict.get_or_insert(message_id);
        } else {
            unclassified.get_or_insert_with(|| bounded(record));
        }
    }
    // The whole stream is judged first. A recognized strict row never excuses an unclassified row or a
    // repeated notice beside it, before it or after it: the stream is not classified, so it is output
    // the bootstrap refuses, not a strict-object rejection.
    if let Some(record) = unclassified {
        return Err(GitBootstrapErrorV1::GitOutput(record));
    }
    if notices > 1 {
        return Err(GitBootstrapErrorV1::GitOutput(
            "fsck printed its notice more than once".into(),
        ));
    }
    if let Some(message_id) = strict {
        return Err(GitBootstrapErrorV1::StrictObjectCheck(message_id));
    }
    Ok(())
}

/// `index-pack --strict`'s own classifier, before the generic exit check. Its stderr is failure
/// typing only: on status zero it is bounded diagnostic data and cannot make a child succeed. On a
/// nonzero status the WHOLE bounded diagnostic is scanned, line by line, for a strict-object id (the
/// exporter's helper reads only a bounded count of lines per call); whatever it finds, the child is
/// refused.
fn classify_index_pack(result: &GitRunResultV1) -> Result<(), GitBootstrapErrorV1> {
    if result.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&result.stderr);
    if let Some(message_id) = detail.lines().find_map(strict_object_message_id) {
        return Err(GitBootstrapErrorV1::StrictObjectCheck(message_id));
    }
    Err(GitBootstrapErrorV1::StrictPack(bounded(detail.trim())))
}

/// `index-pack --strict --stdin` prints exactly one record, `pack\t<lower-hex>\n`, at the format's
/// hex width: no other line, no surrounding whitespace, no missing final newline, no lossy decoding.
fn parse_index_pack_record(
    stdout: &[u8],
    format: GitObjectFormatV1,
) -> Result<String, GitBootstrapErrorV1> {
    let hash = stdout
        .strip_prefix(b"pack\t")
        .and_then(|rest| rest.strip_suffix(b"\n"))
        .filter(|hash| {
            hash.len() == hex_width(format)
                && hash
                    .iter()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
        })
        .ok_or_else(|| GitBootstrapErrorV1::GitOutput(bounded(&String::from_utf8_lossy(stdout))))?;
    // Every byte was checked to be ASCII hex, so this cannot fail.
    String::from_utf8(hash.to_vec())
        .map_err(|_| GitBootstrapErrorV1::GitOutput("index-pack printed a non-ASCII hash".into()))
}

/// `verify-pack -v` proves the generated pack with exactly ONE terminal record, `expected`
/// (`<bootstrap>/objects/pack/pack-<oid>.pack: ok`), and three rules say so independently: it is the
/// LAST record, it appears ONCE, and no OTHER row looks like a proof. It is valid UTF-8 and
/// LF-terminated. The verbose rows before it are bounded diagnostics, never evidence (the inventory
/// and the closure are proven by their own children), but each must be a complete nonempty row
/// with no carriage return.
fn check_verify_pack_proof(stdout: &[u8], expected: &str) -> Result<(), GitBootstrapErrorV1> {
    let not_ok = || {
        GitBootstrapErrorV1::GitOutput("verify-pack did not report the generated pack as ok".into())
    };
    let records = complete_lines(stdout, "verify-pack")?;
    let Some((last, earlier)) = records.split_last() else {
        return Err(not_ok());
    };
    // Last: a proof followed by anything is not the terminal record.
    if *last != expected {
        return Err(not_ok());
    }
    // Once: a repeated proof is not exactly one proof.
    if records.iter().filter(|row| **row == expected).count() != 1 {
        return Err(GitBootstrapErrorV1::GitOutput(
            "verify-pack reported the proof more than once".into(),
        ));
    }
    for row in earlier {
        let refusal = if row.ends_with(": ok") && *row != expected {
            // A lookalike for another pack, before the real proof.
            "verify-pack printed a proof-shaped row before its proof"
        } else if row.trim().is_empty() {
            "verify-pack printed a blank row"
        } else if row.contains('\r') {
            "verify-pack printed a carriage return"
        } else {
            continue;
        };
        return Err(GitBootstrapErrorV1::GitOutput(refusal.into()));
    }
    Ok(())
}

fn require_success(
    result: &GitRunResultV1,
    command: &'static str,
) -> Result<(), GitBootstrapErrorV1> {
    if result.status.success() {
        return Ok(());
    }
    Err(GitBootstrapErrorV1::GitChild {
        command,
        status: result.evidence.exit_status,
        detail: bounded(String::from_utf8_lossy(&result.stderr).trim()),
    })
}

fn ledger_error(error: CustodyExportErrorV1) -> GitBootstrapErrorV1 {
    GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Budget(error.to_string()))
}

fn fs_error(error: FsCustodyError) -> GitBootstrapErrorV1 {
    if refused_link(&error) {
        return GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::SymlinkRefused {
            name: error.to_string(),
        });
    }
    match error {
        FsCustodyError::IdentityChanged(detail) => {
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::IdentityChanged(detail))
        }
        other => GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Io(other.to_string())),
    }
}

/// The ONE absolute deadline, read: once it has passed, whatever is being validated is a Git timeout
/// and never a success. The comparison is the runner's own (`now >= deadline`): the deadline instant
/// itself is already spent.
fn before_deadline(deadline: Instant) -> Result<(), GitBootstrapErrorV1> {
    if Instant::now() >= deadline {
        return Err(GitBootstrapErrorV1::Git(CustodyGitError::Timeout));
    }
    Ok(())
}

/// [`before_deadline`], called right after the late-validation step named `step` returned and before
/// anything else is done toward a verified result. Every guard of the success path is this call; the
/// test hook inside it is how a test makes exactly that step slow. Filesystem and platform calls cannot
/// be preempted, so the deadline can only be observed when each returns.
fn observe_deadline(deadline: Instant, step: &'static str) -> Result<(), GitBootstrapErrorV1> {
    #[cfg(test)]
    run_git_hook(GitHookPointV1::DeadlineObserved, step, Path::new(""));
    #[cfg(not(test))]
    let _ = step;
    before_deadline(deadline)
}

/// What a sweep owes the clock before each of its steps: a guard that observes the one deadline, or
/// [`unpaced`]. Only success-only code is paced: a gate or census that runs for a child that already
/// failed, around admission, or inside a runner callback must still complete and report, so it is the
/// caller's explicit, visible choice to leave it unpaced.
type PaceV1<'a> = &'a dyn Fn(&'static str) -> Result<(), GitBootstrapErrorV1>;

/// The pace of a sweep that is not held to the deadline (see [`PaceV1`]).
fn unpaced(_step: &'static str) -> Result<(), GitBootstrapErrorV1> {
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Admission
// ---------------------------------------------------------------------------------------------

/// The coverage-dependent object shape.
enum ObjectShapeV1 {
    /// Exactly one staged pack, by index.
    Captured {
        pack: usize,
    },
    Empty,
}

fn coverage_state(
    manifest: &CustodyManifestV1,
    class: CustodyCoverageClassV1,
) -> Option<CustodyStateClassV1> {
    manifest
        .coverage()
        .iter()
        .find(|row| row.class() == class)
        .map(|row| row.state())
}

/// Identify the object shape and the active payloads from the manifest's coverage rows and the
/// staged roles, refusing every impossible or unsupported state. Nothing here is downgraded to
/// Empty. Pure: no filesystem effect.
fn classify_shape(
    verified: &VerifiedCapsuleV1,
) -> Result<(ObjectShapeV1, GitFormatProvenanceV1, Vec<ActiveFrameV1>), GitBootstrapErrorV1> {
    let manifest = &verified.control.manifest;
    let packs: Vec<usize> = verified
        .staged
        .iter()
        .enumerate()
        .filter(|(_, one)| one.role == CustodyCapsuleArtifactRoleV1::GitObjectPack)
        .map(|(index, _)| index)
        .collect();
    let objects = manifest.original_objects();
    let (shape, provenance) = match coverage_state(manifest, CustodyCoverageClassV1::ObjectDatabase)
    {
        Some(CustodyStateClassV1::Captured) => {
            if objects.is_empty() {
                return Err(GitBootstrapErrorV1::PackShape(
                    "a Captured object database lists no objects".into(),
                ));
            }
            let [pack] = packs.as_slice() else {
                return Err(GitBootstrapErrorV1::PackShape(format!(
                    "a Captured object database needs exactly one pack, found {}",
                    packs.len()
                )));
            };
            (
                ObjectShapeV1::Captured { pack: *pack },
                GitFormatProvenanceV1::CapsuleObjectsAndPack,
            )
        }
        Some(CustodyStateClassV1::Empty) => {
            if !packs.is_empty() {
                return Err(GitBootstrapErrorV1::EmptyShape(
                    "an Empty object database has a staged pack".into(),
                ));
            }
            if !objects.is_empty() {
                return Err(GitBootstrapErrorV1::EmptyShape(
                    "an Empty object database lists objects".into(),
                ));
            }
            (
                ObjectShapeV1::Empty,
                GitFormatProvenanceV1::CallerSelectedEmpty,
            )
        }
        _ => {
            return Err(GitBootstrapErrorV1::UnsupportedCoverage {
                class: CustodyCoverageClassV1::ObjectDatabase,
                detail: "the object database is neither Captured nor Empty",
            })
        }
    };

    let mut active = Vec::new();
    for class in [
        CustodyCoverageClassV1::RefsAndHead,
        CustodyCoverageClassV1::StashAndReflogs,
    ] {
        let payloads: Vec<usize> = verified
            .staged
            .iter()
            .enumerate()
            .filter(|(_, one)| one.role == CustodyCapsuleArtifactRoleV1::CoveragePayload(class))
            .map(|(index, _)| index)
            .collect();
        match (coverage_state(manifest, class), payloads.as_slice()) {
            (Some(CustodyStateClassV1::Captured), [staged]) => active.push(ActiveFrameV1 {
                class,
                staged: *staged,
            }),
            (Some(CustodyStateClassV1::Captured), _) => {
                return Err(GitBootstrapErrorV1::UnsupportedCoverage {
                    class,
                    detail: "a Captured active class needs exactly one staged payload",
                })
            }
            (Some(CustodyStateClassV1::Empty), []) => {}
            (Some(CustodyStateClassV1::Empty), _) => {
                return Err(GitBootstrapErrorV1::UnsupportedCoverage {
                    class,
                    detail: "an Empty active class has a staged payload",
                })
            }
            _ => {
                return Err(GitBootstrapErrorV1::UnsupportedCoverage {
                    class,
                    detail: "an active class is neither Captured nor Empty",
                })
            }
        }
    }
    Ok((shape, provenance, active))
}

/// Re-measure one staged plaintext from its retained descriptor against its recorded length and
/// SHA-256, through a bounded streaming read, and rewind it.
fn recheck_staged(one: &mut StagedPlaintextV1) -> Result<(), GitBootstrapErrorV1> {
    let changed = || GitBootstrapErrorV1::StagedInputChanged {
        name: lossy(one.name.as_bytes()),
    };
    let (length, sha256) = hash_prefix(&mut one.file, one.length).map_err(|error| {
        GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Io(format!(
            "{}: {error}",
            lossy(one.name.as_bytes())
        )))
    })?;
    if length != one.length || sha256 != one.sha256 {
        return Err(changed());
    }
    one.file.seek(SeekFrom::Start(0)).map_err(|error| {
        GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Io(format!(
            "{}: {error}",
            lossy(one.name.as_bytes())
        )))
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// The bootstrap work state
// ---------------------------------------------------------------------------------------------

/// The phase gate over whatever is retained so far: the mount census first, then
/// `.restore-work/`, `plain/`, every staging directory, `home`, `xdg`, and every bootstrap
/// directory, each of which must still be the entry its parent holds, and the destination root's
/// identity LAST. A change after the last check is outside what any gate can see; consumers act
/// through the retained pins, not by path. This gate is not held to the deadline: it runs around
/// admission, and inside a runner callback, where a refusal's shape is pinned (see [`PaceV1`]).
fn gate_parts(
    dest: &super::RestoreDestinationV1,
    home: Option<&PinnedDirectoryV1>,
    xdg: Option<&PinnedDirectoryV1>,
    dirs: &BTreeMap<String, PinnedDirectoryV1>,
) -> Result<(), GitBootstrapErrorV1> {
    refuse_mounts_within(&dest.root)?;
    sweep_chains(dest, home, xdg, dirs, &unpaced)
}

/// The gate's retained-chain sweep: every chain, then the root's identity LAST. The closing gate runs
/// its own census, then the exact tree, and only then this. `pace` runs before EVERY step (so a sweep
/// that owes the deadline observes it between steps, and right after whatever slow step came before
/// the sweep) and once more immediately before the root's identity: that identity is the last fallible
/// call, and the guard before it is the last statement ahead of it.
fn sweep_chains(
    dest: &super::RestoreDestinationV1,
    home: Option<&PinnedDirectoryV1>,
    xdg: Option<&PinnedDirectoryV1>,
    dirs: &BTreeMap<String, PinnedDirectoryV1>,
    pace: PaceV1<'_>,
) -> Result<(), GitBootstrapErrorV1> {
    pace("chain")?;
    dest.recheck_staging_chain(b"")?;
    for key in dest.staging.borrow().keys() {
        pace("chain")?;
        dest.recheck_staging_chain(key)?;
    }
    let work_name = RESTORE_WORK_DIR_NAME_V1.as_bytes();
    for (name, pin) in [(HOME_NAME_V1, home), (XDG_NAME_V1, xdg)] {
        if let Some(pin) = pin {
            pace("chain")?;
            recheck_entry(
                &dest.work,
                name.as_bytes(),
                pin,
                &joined(work_name, name.as_bytes()),
            )?;
        }
    }
    let bootstrap_name = joined(work_name, BOOTSTRAP_NAME_V1.as_bytes());
    // Parents sort before their children, so each parent's own entry is rechecked first.
    for (relative, pin) in dirs {
        pace("chain")?;
        if relative.is_empty() {
            recheck_entry(
                &dest.work,
                BOOTSTRAP_NAME_V1.as_bytes(),
                pin,
                &bootstrap_name,
            )?;
            continue;
        }
        let (parent_relative, component) =
            relative.rsplit_once('/').unwrap_or(("", relative.as_str()));
        let parent = dirs.get(parent_relative).ok_or_else(|| {
            CustodyRestoreErrorV1::IdentityChanged(format!(
                "{relative}: its bootstrap parent is not retained"
            ))
        })?;
        recheck_entry(
            parent,
            component.as_bytes(),
            pin,
            &joined(&bootstrap_name, relative.as_bytes()),
        )?;
    }
    pace("root")?;
    pinned_root_unchanged(&dest.root)
        .map_err(|detail| CustodyRestoreErrorV1::IdentityChanged(detail).into())
}

fn listing_error(error: FsCustodyError) -> GitBootstrapErrorV1 {
    match error {
        FsCustodyError::EnumerationLimitExceeded { .. } => GitBootstrapErrorV1::CensusBound(
            format!("the bootstrap holds more than {GIT_DIR_MAX_ENTRIES_V1} entries"),
        ),
        other => fs_error(other),
    }
}

fn vanished(name: &str) -> GitBootstrapErrorV1 {
    GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::IdentityChanged(format!(
        "{name}: the entry vanished during the census"
    )))
}

/// Open the regular file `component` beneath `directory` no-follow, `fstat` the descriptor, and
/// require it to be the entry `stat` listed, on the root's device. A link met here is the no-follow
/// open's `ELOOP`, refused and never followed.
fn open_measured_file(
    directory: &PinnedDirectoryV1,
    component: &OsStr,
    child: &str,
    stat: &ChildStatV1,
    dev: u64,
) -> Result<ChildStatV1, GitBootstrapErrorV1> {
    let file = directory
        .open_regular_file(component, "custody restore bootstrap file")
        .map_err(|error| {
            if refused_link(&error) {
                GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::SymlinkRefused {
                    name: child.to_owned(),
                })
            } else {
                fs_error(error)
            }
        })?;
    let opened = file.metadata().map_err(|error| {
        GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Io(format!("{child}: {error}")))
    })?;
    let fact = observed_stat(ChildStatV1::from_metadata(&opened));
    if fact.kind != ChildKindV1::Regular || fact.dev != stat.dev || fact.ino != stat.ino {
        return Err(GitBootstrapErrorV1::Restore(
            CustodyRestoreErrorV1::IdentityChanged(format!(
                "{child}: the opened file is not the listed entry"
            )),
        ));
    }
    if observed_dev(child, fact.dev) != dev {
        return Err(GitBootstrapErrorV1::Restore(
            CustodyRestoreErrorV1::DeviceCrossing {
                name: format!("{BOOTSTRAP_NAME_V1}/{child}"),
            },
        ));
    }
    Ok(fact)
}

/// What a directory root holds beneath it: every RAW entry, counted by enumeration and never keyed
/// by a rendered name (two distinct non-UTF-8 names render alike and are still two entries), and the
/// logical bytes of its regular files.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct SubtreeV1 {
    entries: u64,
    bytes: u64,
}

impl SubtreeV1 {
    fn count_entry(&mut self) -> Result<(), GitBootstrapErrorV1> {
        self.entries = self.entries.checked_add(1).ok_or_else(|| {
            GitBootstrapErrorV1::CensusBound("measured entries overflowed".into())
        })?;
        Ok(())
    }

    fn count_bytes(&mut self, bytes: u64) -> Result<(), GitBootstrapErrorV1> {
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or_else(|| GitBootstrapErrorV1::CensusBound("measured bytes overflowed".into()))?;
        Ok(())
    }
}

/// Everything beneath `directory`, measured through its pin without following a link: every entry is
/// counted, every name is recorded for refusals, and every regular file's bytes are added. A link or
/// special file refuses; depth, the shared entry bound, and the root's device are bounded exactly as
/// the bootstrap's own walk.
fn measure_subtree(
    directory: &PinnedDirectoryV1,
    relative: &str,
    depth: usize,
    remaining: &mut u64,
    dev: u64,
    names: &mut Vec<String>,
    subtree: &mut SubtreeV1,
) -> Result<(), GitBootstrapErrorV1> {
    if depth > GIT_DIR_MAX_DEPTH_V1 {
        return Err(GitBootstrapErrorV1::CensusBound(format!(
            "{relative} nests deeper than {GIT_DIR_MAX_DEPTH_V1}"
        )));
    }
    let listed = directory
        .list_child_names(remaining, "custody restore subtree listing")
        .map_err(listing_error)?;
    for name in &listed {
        let child = bootstrap_relative(relative, name.as_bytes());
        let component = OsStr::from_bytes(name.as_bytes());
        let stat = directory
            .child_metadata_no_follow(component, "custody restore subtree entry")
            .map_err(fs_error)?
            .ok_or_else(|| vanished(&child))?;
        subtree.count_entry()?;
        names.push(child.clone());
        match stat.kind {
            ChildKindV1::Symlink => {
                return Err(GitBootstrapErrorV1::Restore(
                    CustodyRestoreErrorV1::SymlinkRefused { name: child },
                ))
            }
            ChildKindV1::Special(_) => {
                return Err(GitBootstrapErrorV1::SpecialEntry { name: child })
            }
            ChildKindV1::Regular => {
                let fact = open_measured_file(directory, component, &child, &stat, dev)?;
                subtree.count_bytes(fact.size)?;
            }
            ChildKindV1::Directory => {
                let opened = directory
                    .open_existing_child_directory(component, "custody restore subtree directory")
                    .map_err(fs_error)?;
                if Some(stat.dev) != opened.identity().dev
                    || Some(stat.ino) != opened.identity().ino
                {
                    return Err(GitBootstrapErrorV1::Restore(
                        CustodyRestoreErrorV1::IdentityChanged(format!(
                            "{child}: the entry changed while it was measured"
                        )),
                    ));
                }
                let observed = opened
                    .current_metadata("custody restore subtree directory")
                    .map_err(fs_error)?
                    .dev;
                if observed_dev(&child, observed) != dev {
                    return Err(GitBootstrapErrorV1::Restore(
                        CustodyRestoreErrorV1::DeviceCrossing {
                            name: format!("{BOOTSTRAP_NAME_V1}/{child}"),
                        },
                    ));
                }
                measure_subtree(&opened, &child, depth + 1, remaining, dev, names, subtree)?;
            }
        }
    }
    Ok(())
}

/// One regular file as the census saw it through its own no-follow descriptor: every fact the
/// existing `fstat` seam supplies except `atime`, which any read changes. Two observations are equal
/// exactly when it is the same inode with the same length, mode, mtime, and ctime.
#[derive(Debug, PartialEq, Eq)]
struct FileFactV1 {
    length: u64,
    dev: u64,
    ino: u64,
    mode: u32,
    mtime_ns: i128,
    ctime_ns: i128,
}

impl FileFactV1 {
    fn of(stat: &ChildStatV1) -> Self {
        Self {
            length: stat.size,
            dev: stat.dev,
            ino: stat.ino,
            mode: stat.mode,
            mtime_ns: stat.mtime_ns,
            ctime_ns: stat.ctime_ns,
        }
    }
}

/// The SHA-256 of `file` from its start, one bounded buffer at a time, and how many bytes it read. It
/// reads at most `expected + 1` (a file that grew is never read whole), so the count says whether the
/// file held exactly `expected` bytes. The ONE absolute `deadline` is checked before every read: there
/// is no digest without it.
fn stream_digest(
    file: &mut File,
    expected: u64,
    deadline: Instant,
    relative: &str,
) -> Result<(u64, [u8; 32]), GitBootstrapErrorV1> {
    let mut limited = file.by_ref().take(expected.saturating_add(1));
    let mut context = digest::Context::new(&digest::SHA256);
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES_V1];
    let mut count = 0_u64;
    loop {
        before_deadline(deadline)?;
        match limited.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                count = count.saturating_add(read as u64);
                context.update(&buffer[..read]);
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => {
                return Err(GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Io(
                    format!("{relative}: {error}"),
                )))
            }
        }
    }
    let mut sha256 = [0_u8; 32];
    sha256.copy_from_slice(context.finish().as_ref());
    Ok((count, sha256))
}

/// A digest's custody refusal: the file it read is not the one the census measured.
fn digest_identity(relative: &str, detail: &str) -> GitBootstrapErrorV1 {
    GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::IdentityChanged(format!(
        "{relative}: {detail}"
    )))
}

/// The digest descriptor's own `fstat`, observed exactly as the census observes it, must be the regular
/// file the census measured: the same device, inode, length, mode, mtime, and ctime (everything but
/// `atime`, which any read moves). A name reopened after the census could be another inode holding the
/// very same bytes, and equal digests would never show it.
fn require_measured_file(
    file: &File,
    measured: &FileFactV1,
    relative: &str,
    when: &str,
) -> Result<(), GitBootstrapErrorV1> {
    let opened = file.metadata().map_err(|error| {
        GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Io(format!("{relative}: {error}")))
    })?;
    let stat = observed_stat(ChildStatV1::from_metadata(&opened));
    if stat.kind != ChildKindV1::Regular || FileFactV1::of(&stat) != *measured {
        return Err(digest_identity(
            relative,
            &format!("the digest descriptor is not the file the census measured {when}"),
        ));
    }
    Ok(())
}

/// One complete census of the tree, equal to another exactly when nothing in it changed.
#[derive(Debug, Default, PartialEq, Eq)]
struct TreeMeasurementV1 {
    files: BTreeMap<String, FileFactV1>,
    /// Every entry and byte beneath the bootstrap root (the root itself is not counted here):
    /// known directories and files, unexpected directories, and everything beneath them.
    bootstrap: SubtreeV1,
    /// Everything beneath HOME and XDG, whose descendants have no reservation at all.
    home: SubtreeV1,
    xdg: SubtreeV1,
    /// Names the census does not understand: unexpected entries, everything beneath an unexpected
    /// directory, and anything in HOME or XDG. Each subtree is still measured, no-follow. These name
    /// refusals; they are never a counting key.
    unexpected: Vec<String>,
}

/// How a bootstrap tree is held to what it may contain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReconcileV1 {
    /// Exactly the budget's file set, every required file present, every admitted file unchanged.
    Exact,
    /// After a child that failed: names are not constrained, but nothing may escape its bound.
    Tolerant,
}

/// Whether a child may write. A read-only child owns a zero-effect allowance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChildRoleV1 {
    Writing,
    ReadOnly,
}

/// What the bootstrap OWNS of the one ledger: the entry allowances (the bootstrap root's own among
/// them) and logical bytes it has charged and not released. The only way down is
/// [`Self::settle_to`], which releases exactly the difference from a complete measurement that fits,
/// and leaves the owned figure equal to it, so the same allowance can never be released twice and
/// nothing is ever released from a charge this bootstrap does not own. The ledger's own saturating
/// `release` is never what makes that true.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct OwnedChargeV1 {
    entries: u64,
    bytes: u64,
}

impl OwnedChargeV1 {
    fn settle_to(
        &mut self,
        ledger: &RefCell<ScratchLedgerV1>,
        entries: u64,
        bytes: u64,
    ) -> Result<(), GitBootstrapErrorV1> {
        let overflow = || {
            ledger_error(CustodyExportErrorV1::ScratchLedger(
                "a release exceeded the charge this bootstrap owns".into(),
            ))
        };
        let unused_entries = self.entries.checked_sub(entries).ok_or_else(overflow)?;
        let unused_bytes = self.bytes.checked_sub(bytes).ok_or_else(overflow)?;
        let released = unused_entries
            .checked_mul(ENTRY_ALLOWANCE_BYTES_V1)
            .and_then(|allowance| allowance.checked_add(unused_bytes))
            .ok_or_else(overflow)?;
        ledger.borrow_mut().release(released);
        self.entries = entries;
        self.bytes = bytes;
        Ok(())
    }
}

/// An entry/byte pair just reserved on the ledger. Rolling it back consumes it, so a pair can be
/// given back at most once, and only by the code that reserved it.
#[derive(Debug)]
struct ReservedPairV1 {
    entries: u64,
    bytes: u64,
    /// What was charged: `entries` allowances plus `bytes`, computed before anything was reserved.
    charged: u64,
}

impl ReservedPairV1 {
    /// Reserve the entries, then the bytes. If the second reservation is refused, the first is
    /// released at once, exactly as charged: nothing stays charged for what was never made.
    fn reserve(
        ledger: &RefCell<ScratchLedgerV1>,
        entries: u64,
        bytes: u64,
    ) -> Result<Self, GitBootstrapErrorV1> {
        let overflow = || {
            ledger_error(CustodyExportErrorV1::ScratchLedger(
                "a reservation overflowed".into(),
            ))
        };
        let allowance = entries
            .checked_mul(ENTRY_ALLOWANCE_BYTES_V1)
            .ok_or_else(overflow)?;
        let charged = allowance.checked_add(bytes).ok_or_else(overflow)?;
        let mut ledger = ledger.borrow_mut();
        ledger.reserve_entries(entries).map_err(ledger_error)?;
        if let Err(error) = ledger.reserve(bytes) {
            ledger.release(allowance);
            return Err(ledger_error(error));
        }
        Ok(Self {
            entries,
            bytes,
            charged,
        })
    }

    /// The pair's charge now belongs to the bootstrap's [`OwnedChargeV1`].
    fn into_owned(self) -> OwnedChargeV1 {
        OwnedChargeV1 {
            entries: self.entries,
            bytes: self.bytes,
        }
    }

    /// Give the whole pair back: the create-new it was reserved for provably created nothing.
    fn roll_back(self, ledger: &RefCell<ScratchLedgerV1>) {
        ledger.borrow_mut().release(self.charged);
    }
}

struct WorkV1 {
    verified: VerifiedCapsuleV1,
    home: PinnedDirectoryV1,
    xdg: PinnedDirectoryV1,
    dirs: BTreeMap<String, PinnedDirectoryV1>,
    /// The names the bootstrap may hold. Its entry and byte figures are not read: `owned` is the
    /// single account of what is reserved.
    budget: GitDirectoryBudgetV1,
    /// Files the bootstrap must hold now.
    required: BTreeSet<String>,
    /// The bootstrap tree's reservation: the entries (the root's own included) and logical bytes
    /// this invocation has charged to the ledger and not released, including a writing child's
    /// before it runs.
    owned: OwnedChargeV1,
    /// The one absolute deadline of every child and of every digest, before a child and after it, and
    /// of every late-validation guard (the success gate, the rewinds, the closing gate).
    deadline: Instant,
    /// The pack `index-pack` named, once admitted.
    pack_hash: Option<String>,
    admitted: Vec<AdmittedObjectFileV1>,
    runs: Vec<GitRunRecordV1>,
    progress: Option<GitPlaneStepV1>,
}

impl WorkV1 {
    fn dest(&self) -> &super::RestoreDestinationV1 {
        &self.verified.destination
    }

    /// The phase gate: the mount census, then every retained chain, then the destination root's
    /// identity LAST. See [`gate_parts`]. Not held to the deadline.
    fn gate(&self) -> Result<(), GitBootstrapErrorV1> {
        gate_parts(self.dest(), Some(&self.home), Some(&self.xdg), &self.dirs)
    }

    /// [`Self::gate`] for a result that is about to become a success (a read-only child's post-child
    /// success arm), held to the ONE absolute deadline: observed before the census, right after it
    /// (the sweep's first step), before every chain, and immediately before the root's identity, which
    /// is still the LAST fallible call.
    fn deadline_gate(&self) -> Result<(), GitBootstrapErrorV1> {
        let deadline = self.deadline;
        observe_deadline(deadline, "gate")?;
        refuse_mounts_within(&self.dest().root)?;
        sweep_chains(
            self.dest(),
            Some(&self.home),
            Some(&self.xdg),
            &self.dirs,
            &|step| observe_deadline(deadline, step),
        )
    }

    fn work_path(&self) -> &Path {
        self.dest().work.canonical_path()
    }

    // -----------------------------------------------------------------------------------------
    // The bounded, descriptor-relative census
    // -----------------------------------------------------------------------------------------

    /// Measure the bootstrap through retained pins only. With `adopt`, a directory the census meets
    /// that is not yet retained (the ones `git init` created) is opened no-follow through its
    /// retained parent and retained; otherwise every directory must already be a retained pin.
    /// A directory the census does not expect is never adopted, but its whole subtree is measured
    /// no-follow (as is everything in HOME and XDG), so what a child left there is accounted.
    /// `pace` runs before every bootstrap directory and every bootstrap entry (see [`PaceV1`]).
    fn measure(
        &mut self,
        adopt: bool,
        pace: PaceV1<'_>,
    ) -> Result<TreeMeasurementV1, GitBootstrapErrorV1> {
        let dev = self.dest().dev;
        let mut measurement = TreeMeasurementV1::default();
        let mut remaining = GIT_DIR_MAX_ENTRIES_V1 as u64;
        measure_subtree(
            &self.home,
            HOME_NAME_V1,
            0,
            &mut remaining,
            dev,
            &mut measurement.unexpected,
            &mut measurement.home,
        )?;
        measure_subtree(
            &self.xdg,
            XDG_NAME_V1,
            0,
            &mut remaining,
            dev,
            &mut measurement.unexpected,
            &mut measurement.xdg,
        )?;

        let mut stack = vec![(String::new(), 0_usize)];
        while let Some((relative, depth)) = stack.pop() {
            pace("census")?;
            if depth > GIT_DIR_MAX_DEPTH_V1 {
                return Err(GitBootstrapErrorV1::CensusBound(format!(
                    "{relative} nests deeper than {GIT_DIR_MAX_DEPTH_V1}"
                )));
            }
            let directory = self.dirs.get(&relative).ok_or_else(|| {
                GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::IdentityChanged(format!(
                    "{relative}: a bootstrap directory is not retained"
                )))
            })?;
            let observed = directory
                .current_metadata("custody restore bootstrap directory")
                .map_err(fs_error)?
                .dev;
            if observed_dev(&relative, observed) != dev {
                return Err(GitBootstrapErrorV1::Restore(
                    CustodyRestoreErrorV1::DeviceCrossing {
                        name: format!("{BOOTSTRAP_NAME_V1}/{relative}"),
                    },
                ));
            }
            let names = directory
                .list_child_names(&mut remaining, "custody restore bootstrap listing")
                .map_err(listing_error)?;
            #[cfg(test)]
            run_git_hook(GitHookPointV1::CensusListed, "", directory.canonical_path());

            let mut adopted = Vec::new();
            for name in &names {
                pace("census")?;
                let child = bootstrap_relative(&relative, name.as_bytes());
                let component = OsStr::from_bytes(name.as_bytes());
                let stat = directory
                    .child_metadata_no_follow(component, "custody restore bootstrap entry")
                    .map_err(fs_error)?
                    .ok_or_else(|| vanished(&child))?;
                measurement.bootstrap.count_entry()?;
                #[cfg(test)]
                run_git_hook(
                    GitHookPointV1::BeforeCensusOpen,
                    "",
                    &directory.canonical_path().join(component),
                );
                match stat.kind {
                    ChildKindV1::Symlink => {
                        return Err(GitBootstrapErrorV1::Restore(
                            CustodyRestoreErrorV1::SymlinkRefused { name: child },
                        ))
                    }
                    ChildKindV1::Special(_) => {
                        return Err(GitBootstrapErrorV1::SpecialEntry { name: child })
                    }
                    ChildKindV1::Directory => {
                        let matches = |pin: &PinnedDirectoryV1| {
                            Some(stat.dev) == pin.identity().dev
                                && Some(stat.ino) == pin.identity().ino
                        };
                        if !BOOTSTRAP_DIRECTORIES_V1.contains(&child.as_str()) {
                            measurement.unexpected.push(child.clone());
                            let opened = directory
                                .open_existing_child_directory(
                                    component,
                                    "custody restore bootstrap directory",
                                )
                                .map_err(fs_error)?;
                            if !matches(&opened) {
                                return Err(GitBootstrapErrorV1::Restore(
                                    CustodyRestoreErrorV1::IdentityChanged(format!(
                                        "{child}: the entry changed while it was measured"
                                    )),
                                ));
                            }
                            measure_subtree(
                                &opened,
                                &child,
                                depth + 1,
                                &mut remaining,
                                dev,
                                &mut measurement.unexpected,
                                &mut measurement.bootstrap,
                            )?;
                            continue;
                        }
                        match self.dirs.get(&child) {
                            Some(retained) if matches(retained) => {}
                            Some(_) => {
                                return Err(GitBootstrapErrorV1::Restore(
                                    CustodyRestoreErrorV1::IdentityChanged(format!(
                                        "{child}: the entry is no longer the retained directory"
                                    )),
                                ))
                            }
                            None if adopt => {
                                let opened = directory
                                    .open_existing_child_directory(
                                        component,
                                        "custody restore bootstrap directory",
                                    )
                                    .map_err(fs_error)?;
                                if !matches(&opened) {
                                    return Err(GitBootstrapErrorV1::Restore(
                                        CustodyRestoreErrorV1::IdentityChanged(format!(
                                            "{child}: the entry changed while it was adopted"
                                        )),
                                    ));
                                }
                                adopted.push((child.clone(), opened));
                            }
                            None => {
                                return Err(GitBootstrapErrorV1::Restore(
                                    CustodyRestoreErrorV1::IdentityChanged(format!(
                                        "{child}: a bootstrap directory is not retained"
                                    )),
                                ))
                            }
                        }
                        stack.push((child, depth + 1));
                    }
                    ChildKindV1::Regular => {
                        let fact = open_measured_file(directory, component, &child, &stat, dev)?;
                        measurement.bootstrap.count_bytes(fact.size)?;
                        measurement.files.insert(child, FileFactV1::of(&fact));
                    }
                }
            }
            self.dirs.extend(adopted);
        }
        Ok(measurement)
    }

    /// Measure the bootstrap, hold it to `mode`, and (with `release`) settle the ledger's reservation
    /// for it, entries and logical bytes together, down to what was measured.
    ///
    /// Nothing reserved HOME's or XDG's descendants, so any entry or byte there is an overrun. The
    /// bootstrap holds its root and everything beneath it against the reservation it owns. An overrun
    /// in any domain, or a census that cannot complete (a link, a special file, the depth or entry
    /// bound, an identity or device change: [`Self::measure`] returns its refusal), releases NOTHING
    /// and returns the refusal: absence is never inferred. Only a complete census that fits releases,
    /// and it releases exactly the unused allowance, once.
    fn reconcile(
        &mut self,
        mode: ReconcileV1,
        adopt: bool,
        release: bool,
    ) -> Result<TreeMeasurementV1, GitBootstrapErrorV1> {
        self.reconcile_paced(mode, adopt, release, &unpaced)
    }

    /// [`Self::reconcile`] with its census paced by `pace` (see [`Self::measure`]). A census the pace
    /// cuts short cannot complete, so, like every census that cannot, it releases NOTHING.
    fn reconcile_paced(
        &mut self,
        mode: ReconcileV1,
        adopt: bool,
        release: bool,
        pace: PaceV1<'_>,
    ) -> Result<TreeMeasurementV1, GitBootstrapErrorV1> {
        let measured = self.measure(adopt, pace)?;
        let named = |name: &String| self.budget.files.contains(name);
        if mode == ReconcileV1::Exact {
            if let Some(name) = measured.unexpected.first() {
                return Err(GitBootstrapErrorV1::UnexpectedEntry { name: name.clone() });
            }
            if let Some(name) = measured.files.keys().find(|name| !named(name)) {
                return Err(GitBootstrapErrorV1::UnexpectedEntry { name: name.clone() });
            }
            if let Some(name) = self
                .required
                .iter()
                .find(|name| !measured.files.contains_key(*name))
            {
                return Err(GitBootstrapErrorV1::MissingEntry { name: name.clone() });
            }
            if !self.required.is_empty() {
                if let Some(name) = BOOTSTRAP_DIRECTORIES_V1
                    .iter()
                    .find(|name| !self.dirs.contains_key(**name))
                {
                    return Err(GitBootstrapErrorV1::MissingEntry {
                        name: (*name).to_owned(),
                    });
                }
            }
            for admitted in &self.admitted {
                let unchanged = measured.files.get(&admitted.relative).is_some_and(|fact| {
                    fact.dev == admitted.dev
                        && fact.ino == admitted.ino
                        && fact.length == admitted.length
                });
                if !unchanged {
                    return Err(GitBootstrapErrorV1::Restore(
                        CustodyRestoreErrorV1::IdentityChanged(format!(
                            "{}: no longer the admitted object file",
                            admitted.relative
                        )),
                    ));
                }
            }
        }
        let overflow = || GitBootstrapErrorV1::CensusBound("measured totals overflowed".into());
        let outside_entries = measured
            .home
            .entries
            .checked_add(measured.xdg.entries)
            .ok_or_else(overflow)?;
        let outside_bytes = measured
            .home
            .bytes
            .checked_add(measured.xdg.bytes)
            .ok_or_else(overflow)?;
        if outside_entries > 0 || outside_bytes > 0 {
            return Err(GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Budget(
                format!(
                    "home and xdg hold {outside_bytes} bytes and {outside_entries} entries above their 0 byte and 0 entry reservation"
                ),
            )));
        }
        // The bootstrap root is one of the entries it owns.
        let entries = measured
            .bootstrap
            .entries
            .checked_add(1)
            .ok_or_else(overflow)?;
        if entries > self.owned.entries {
            return Err(GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Budget(
                format!(
                    "{BOOTSTRAP_NAME_V1} holds {entries} entries above its {} entry reservation",
                    self.owned.entries
                ),
            )));
        }
        if measured.bootstrap.bytes > self.owned.bytes {
            return Err(GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Budget(
                format!(
                    "{BOOTSTRAP_NAME_V1} holds {} bytes above its {} byte reservation",
                    measured.bootstrap.bytes, self.owned.bytes
                ),
            )));
        }
        if release {
            self.owned.settle_to(
                &self.verified.destination.ledger,
                entries,
                measured.bootstrap.bytes,
            )?;
        }
        Ok(measured)
    }

    /// The SHA-256 of every regular file `measured` holds, each streamed from a fresh no-follow
    /// descriptor opened through the RETAINED pin of its directory (never by path, never through the
    /// exporter's walker). The facts see names, identities, lengths, modes, and timestamps; only this
    /// sees a same-length rewrite in place. HOME and XDG hold no file a baseline can admit (any entry
    /// there refuses first), so every file here is a bootstrap file.
    ///
    /// Each descriptor is BOUND to the census: its own `fstat`, before the first read and again after
    /// the last, must equal the complete non-atime facts that file was measured with, and exactly the
    /// measured byte count must be read, so a name swapped to another inode after the census, a file
    /// that changed under the read, and a file that was cut short or grown are all refused, and every
    /// digest returned is of the very inode the census measured. Every read keeps the ONE absolute
    /// `deadline`, baseline and post-child alike.
    fn digest_files(
        &self,
        measured: &TreeMeasurementV1,
        deadline: Instant,
    ) -> Result<BTreeMap<String, [u8; 32]>, GitBootstrapErrorV1> {
        let mut digests = BTreeMap::new();
        for (relative, fact) in &measured.files {
            let (parent, leaf) = relative.rsplit_once('/').unwrap_or(("", relative.as_str()));
            let directory = self.dirs.get(parent).ok_or_else(|| {
                GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::IdentityChanged(format!(
                    "{relative}: its bootstrap parent is not retained"
                )))
            })?;
            let mut file = directory
                .open_regular_file(OsStr::new(leaf), "custody restore bootstrap file digest")
                .map_err(|error| {
                    if refused_link(&error) {
                        GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::SymlinkRefused {
                            name: relative.clone(),
                        })
                    } else {
                        fs_error(error)
                    }
                })?;
            require_measured_file(&file, fact, relative, "before it was read")?;
            #[cfg(test)]
            run_git_hook(
                GitHookPointV1::DigestOpened,
                "",
                &directory.canonical_path().join(leaf),
            );
            let (read, sha256) = stream_digest(&mut file, fact.length, deadline, relative)?;
            #[cfg(test)]
            run_git_hook(
                GitHookPointV1::DigestHashed,
                "",
                &directory.canonical_path().join(leaf),
            );
            if read != fact.length {
                return Err(digest_identity(
                    relative,
                    &format!(
                        "{read} bytes were read, and the census measured {}",
                        fact.length
                    ),
                ));
            }
            require_measured_file(&file, fact, relative, "after it was read")?;
            digests.insert(relative.clone(), sha256);
        }
        Ok(digests)
    }

    // -----------------------------------------------------------------------------------------
    // One Git child
    // -----------------------------------------------------------------------------------------

    /// Reserve `entries` allowances and `bytes` more, as one pair, before a writing child runs. The
    /// pair is rolled back at once if its second half is refused.
    fn reserve_more(&mut self, entries: u64, bytes: u64) -> Result<(), GitBootstrapErrorV1> {
        let overflow = || {
            ledger_error(CustodyExportErrorV1::ScratchLedger(
                "the bootstrap's reservation overflowed".into(),
            ))
        };
        let next = OwnedChargeV1 {
            entries: self
                .owned
                .entries
                .checked_add(entries)
                .ok_or_else(overflow)?,
            bytes: self.owned.bytes.checked_add(bytes).ok_or_else(overflow)?,
        };
        ReservedPairV1::reserve(&self.dest().ledger, entries, bytes)?;
        self.owned = next;
        Ok(())
    }

    /// Run one child rooted at the retained work pin, and settle what it left, EXACTLY ONCE, whatever
    /// its outcome. The tree is censused first (a replaced entry stops the next child, and a census
    /// that refuses releases nothing), both runner callbacks run the phase gate, and `classify`
    /// judges a returned result (status, then output).
    ///
    /// A writing child's failure, from the runner at any point or from `classify`, is reconciled
    /// once against the reservation it owns; after it succeeds, its caller reconciles once the
    /// outputs it admits are known. A read-only child owns a zero-effect allowance: on EVERY outcome
    /// the tree must equal its pre-child baseline, and any effect is a refusal beside its failure.
    fn run_child<T>(
        &mut self,
        runner: &GitRunnerV1,
        names: &GitRootNamesV1,
        label: &'static str,
        request: GitRunRequestV1,
        role: ChildRoleV1,
        classify: impl FnOnce(&GitRunResultV1) -> Result<T, GitBootstrapErrorV1>,
    ) -> Result<T, GitBootstrapErrorV1> {
        #[cfg(test)]
        run_git_hook(GitHookPointV1::BeforeChildCensus, label, self.work_path());
        let baseline = self.reconcile(ReconcileV1::Exact, false, false)?;
        // A read-only child is judged against these facts AND every file's digest. The baseline
        // digests keep to the one deadline: nothing starts once it has passed.
        let digests = match role {
            ChildRoleV1::ReadOnly => self.digest_files(&baseline, self.deadline)?,
            ChildRoleV1::Writing => BTreeMap::new(),
        };
        let callback_error: RefCell<Option<GitBootstrapErrorV1>> = RefCell::new(None);
        let outcome = {
            let this = &*self;
            let check = |point_label: &'static str, after: bool| {
                #[cfg(test)]
                run_git_hook(
                    if after {
                        GitHookPointV1::AfterChild
                    } else {
                        GitHookPointV1::BeforeChild
                    },
                    point_label,
                    this.work_path(),
                );
                #[cfg(not(test))]
                let _ = (point_label, after);
                match this.gate() {
                    Ok(()) => Ok(()),
                    Err(error) => {
                        *callback_error.borrow_mut() = Some(error);
                        Err(CustodyGitError::RouteRefusal(
                            "the bootstrap gate refused".into(),
                        ))
                    }
                }
            };
            runner.run(
                &this.dest().work,
                names,
                request,
                || check(label, false),
                || check(label, true),
            )
        };
        let callback_error = callback_error.into_inner();
        // Every runner error is one outcome of a child that may have run: a callback refusal, a
        // timeout, a cap, binary drift, a failed stream, and a refusal before the spawn alike.
        let verdict = match outcome {
            Ok(result) => {
                self.runs.push(GitRunRecordV1 {
                    label,
                    version: result.evidence.version,
                    argv: result.evidence.argv.clone(),
                    environment: result.evidence.environment.clone(),
                    exit_status: result.evidence.exit_status,
                    stdin: result.evidence.stdin.clone(),
                    stdout: result.evidence.stdout.clone(),
                    stderr: result.evidence.stderr.clone(),
                });
                match callback_error {
                    None => classify(&result),
                    Some(error) => Err(error),
                }
            }
            Err(error) => Err(callback_error.unwrap_or(error.into())),
        };
        match (role, verdict) {
            (ChildRoleV1::Writing, Ok(value)) => Ok(value),
            (ChildRoleV1::Writing, Err(primary)) => Err(self.with_reconcile(primary)),
            (ChildRoleV1::ReadOnly, verdict) => {
                self.settle_read_only(&baseline, &digests, label, verdict)
            }
        }
    }

    /// Reconcile what a failed writing child left behind, once, and return `primary` with any
    /// reconciliation failure kept beside it: a budget overrun or a custody refusal found here is
    /// never discarded in favor of the child's own error.
    fn with_reconcile(&mut self, primary: GitBootstrapErrorV1) -> GitBootstrapErrorV1 {
        match self.reconcile(ReconcileV1::Tolerant, true, true) {
            Ok(_) => primary,
            Err(reconcile) => GitBootstrapErrorV1::ChildAndReconcile {
                primary: Box::new(primary),
                reconcile: Box::new(reconcile),
            },
        }
    }

    /// Settle a read-only child, once, on any outcome: the tree must be EXACTLY what the pre-child
    /// census found (and, as always, exactly the bootstrap's file set), so an added, removed,
    /// replaced, or resized entry anywhere, in the bootstrap, HOME, or XDG, is a reconciliation
    /// failure; so is a file whose mode or timestamps moved, and so is a file whose bytes changed in
    /// place, which only its digest can show. A returned result, a runner error, a nonzero status,
    /// and a parser refusal all keep their own failure beside it. Nothing is ever released: the
    /// allowance is zero. A result that is about to become a success passes the success gate held to
    /// the ONE absolute deadline ([`Self::deadline_gate`]). A failed child's census and effect
    /// comparison are never cut short by the deadline, so its effect is still named beside its
    /// failure; only its digest pass keeps the deadline, as it always did.
    fn settle_read_only<T>(
        &mut self,
        baseline: &TreeMeasurementV1,
        digests: &BTreeMap<String, [u8; 32]>,
        label: &'static str,
        verdict: Result<T, GitBootstrapErrorV1>,
    ) -> Result<T, GitBootstrapErrorV1> {
        let effect = self
            .reconcile(ReconcileV1::Exact, false, false)
            .and_then(|measured| {
                let refused = |changed: String| {
                    Err(GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Budget(
                        format!("{label}: a read-only child changed the bootstrap tree: {changed}"),
                    )))
                };
                if &measured != baseline {
                    // A file whose identity, length, mode, or timestamps differ names itself;
                    // otherwise the totals differ.
                    return refused(
                        match measured
                            .files
                            .iter()
                            .find(|(name, fact)| baseline.files.get(*name) != Some(*fact))
                        {
                            Some((name, _)) => format!(
                                "{name} was replaced, resized, or had its mode or timestamps changed"
                            ),
                            None => format!(
                                "{} entries and {} bytes before it, {} entries and {} bytes after it",
                                baseline.bootstrap.entries,
                                baseline.bootstrap.bytes,
                                measured.bootstrap.entries,
                                measured.bootstrap.bytes
                            ),
                        },
                    );
                }
                // Every fact is equal, so this reads the very files, at the very lengths, that the
                // baseline pass read, under the SAME absolute deadline: there is no exception after a
                // child. Once it has passed the pass refuses with a timeout (beside the child's own
                // timeout, if that is what spent it), and never succeeds past it.
                let after = self.digest_files(&measured, self.deadline)?;
                match after
                    .iter()
                    .find(|(name, sha256)| digests.get(*name) != Some(*sha256))
                {
                    Some((name, _)) => refused(format!("{name} was rewritten in place")),
                    None => Ok(()),
                }
            });
        match (verdict, effect) {
            (Ok(value), Ok(())) => {
                // The census first, then the sweep with the root's identity last, all held to the ONE
                // absolute deadline: the digest pass above checks it only before each read, so a clock
                // that passed during the last read, the descriptor's closing `fstat`, or this gate's own
                // slow census must not let the child's result through.
                self.deadline_gate()?;
                Ok(value)
            }
            (Ok(_), Err(reconcile)) => Err(reconcile),
            (Err(primary), Ok(())) => Err(primary),
            (Err(primary), Err(reconcile)) => Err(GitBootstrapErrorV1::ChildAndReconcile {
                primary: Box::new(primary),
                reconcile: Box::new(reconcile),
            }),
        }
    }

    /// Re-measure every admitted object file from its retained descriptor against the length and
    /// SHA-256 measured at admission, and rewind it. The census sees names, identities, and
    /// lengths; only this sees bytes changed in place. Every file is streamed under the ONE absolute
    /// `deadline` (checked before each read, and before each file), never through an unbounded read.
    fn remeasure_admitted(&mut self) -> Result<(), GitBootstrapErrorV1> {
        let deadline = self.deadline;
        for admitted in &mut self.admitted {
            observe_deadline(deadline, "rehash")?;
            let io = |error: std::io::Error| {
                GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Io(format!(
                    "{}: {error}",
                    admitted.relative
                )))
            };
            admitted.file.seek(SeekFrom::Start(0)).map_err(io)?;
            let (length, sha256) = stream_digest(
                &mut admitted.file,
                admitted.length,
                deadline,
                &admitted.relative,
            )?;
            if length != admitted.length || sha256 != admitted.sha256 {
                return Err(GitBootstrapErrorV1::AdmittedObjectChanged {
                    relative: admitted.relative.clone(),
                });
            }
            admitted.file.seek(SeekFrom::Start(0)).map_err(io)?;
        }
        Ok(())
    }

    /// The runner consumed a duplicate of the staged pack's descriptor to EOF, and a duplicate
    /// shares its file offset. Every staged descriptor goes back to offset 0, as phase V returned it.
    /// Each rewind is preceded by the ONE absolute deadline's guard.
    fn rewind_staged(&mut self) -> Result<(), GitBootstrapErrorV1> {
        let deadline = self.deadline;
        for one in &mut self.verified.staged {
            observe_deadline(deadline, "rewind")?;
            one.file.seek(SeekFrom::Start(0)).map_err(|error| {
                GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Io(format!(
                    "{}: staged rewind: {error}",
                    lossy(one.name.as_bytes())
                )))
            })?;
        }
        Ok(())
    }

    /// The closing gate, in this order: the mount census; the exact tree and every admitted file's
    /// bytes; every retained chain; the destination root's identity LAST. Nothing is returned that
    /// a mutation before this gate, or inside its census, could have staled. Every step is held to the
    /// ONE absolute deadline: it is observed on entry, right after the mount census, inside the tree
    /// census (before each directory and entry) and right after it, before each admitted file, and
    /// before each chain; the sweep's guard immediately before the root's identity is the last
    /// statement ahead of that identity, which stays the LAST fallible call.
    fn closing_gate(&mut self) -> Result<(), GitBootstrapErrorV1> {
        let deadline = self.deadline;
        let pace = |step: &'static str| observe_deadline(deadline, step);
        pace("closing")?;
        refuse_mounts_within(&self.dest().root)?;
        pace("closing-census")?;
        self.reconcile_paced(ReconcileV1::Exact, false, true, &pace)?;
        pace("closing-tree")?;
        self.remeasure_admitted()?;
        sweep_chains(
            self.dest(),
            Some(&self.home),
            Some(&self.xdg),
            &self.dirs,
            &pace,
        )
    }

    // -----------------------------------------------------------------------------------------
    // The steps
    // -----------------------------------------------------------------------------------------

    fn names() -> Result<GitRootNamesV1, GitBootstrapErrorV1> {
        Ok(GitRootNamesV1::new(
            HOME_NAME_V1,
            XDG_NAME_V1,
            BOOTSTRAP_NAME_V1,
        )?)
    }

    fn init_bare(
        &mut self,
        runner: &GitRunnerV1,
        object_format: GitObjectFormatV1,
        deadline: Instant,
    ) -> Result<(), GitBootstrapErrorV1> {
        let request = GitRunRequestV1::new(
            GitCommandV1::InitBare {
                dir: BOOTSTRAP_NAME_V1.to_owned(),
                object_format,
            },
            Vec::new(),
            GIT_MIN_STDOUT_LIMIT_V1,
            GIT_STDERR_LIMIT_V1,
            deadline,
        );
        let names = match Self::names() {
            Ok(names) => names,
            Err(error) => return Err(self.with_reconcile(error)),
        };
        self.run_child(
            runner,
            &names,
            "init --bare",
            request,
            ChildRoleV1::Writing,
            |result| require_success(result, "init --bare"),
        )?;
        self.required = self.budget.files.clone();
        // The first successful measure adopts the directories `git init` created, and holds the
        // tree to exactly `HEAD` and `config`. The step is complete only after it.
        self.reconcile(ReconcileV1::Exact, true, true)?;
        self.gate()?;
        self.progress = Some(GitPlaneStepV1::BootstrapInit);
        Ok(())
    }

    /// Captured only: strictly index the staged pack into the bootstrap, prove the generated
    /// pack/index pair with `verify-pack`, and retain the three admitted files.
    fn index_pack(
        &mut self,
        runner: &GitRunnerV1,
        object_format: GitObjectFormatV1,
        pack: usize,
        objects: usize,
        deadline: Instant,
    ) -> Result<(), GitBootstrapErrorV1> {
        let (length, sha256) = {
            let staged = &self.verified.staged[pack];
            (staged.length, staged.sha256)
        };
        let bound = index_pack_logical_bound(length, objects as u64, raw_hash_width(object_format))
            .map_err(ledger_error)?;
        // The three entries and the bound, as one pair: refused whole or reserved whole.
        self.reserve_more(INDEX_PACK_ENTRIES_V1, bound)?;
        // Anything that refuses before the child runs (the staged descriptor, the request, the names)
        // still owns that reservation, and settles it: nothing of it was used.
        let prepared = (|| {
            let mut stdin = self.verified.staged[pack]
                .file
                .try_clone()
                .map_err(|error| {
                    CustodyRestoreErrorV1::Io(format!("staged pack descriptor: {error}"))
                })?;
            stdin.seek(SeekFrom::Start(0)).map_err(|error| {
                CustodyRestoreErrorV1::Io(format!("staged pack rewind: {error}"))
            })?;
            let request = GitRunRequestV1::from_file(
                GitCommandV1::IndexPackStrictStdin,
                stdin,
                length,
                GIT_MIN_STDOUT_LIMIT_V1,
                GIT_STDERR_LIMIT_V1,
                deadline,
            )?;
            Ok::<_, GitBootstrapErrorV1>((request, Self::names()?))
        })();
        let (request, names) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => return Err(self.with_reconcile(error)),
        };
        let pack_hash = self.run_child(
            runner,
            &names,
            "index-pack --strict --stdin",
            request,
            ChildRoleV1::Writing,
            |result| {
                classify_index_pack(result)?;
                // The bytes verified are the bytes staged: the runner attests what the pipe accepted.
                if result.evidence.stdin.length as u64 != length
                    || result.evidence.stdin.sha256 != sha256
                {
                    return Err(GitBootstrapErrorV1::VerificationInputMismatch);
                }
                let stdout = result.captured_stdout().ok_or_else(|| {
                    GitBootstrapErrorV1::GitOutput("index-pack stdout was streamed".into())
                })?;
                parse_index_pack_record(stdout, object_format)
            },
        )?;
        self.budget
            .admit_index_pack(&pack_hash, bound)
            .map_err(ledger_error)?;
        self.required = self.budget.files.clone();
        self.reconcile(ReconcileV1::Exact, false, true)?;
        self.admit_pack_files(&pack_hash, length, sha256)?;
        self.pack_hash = Some(pack_hash.clone());
        self.gate()?;

        // `verify-pack` proves the generated pack/index pair.
        let limit = verify_pack_stdout_limit(objects, object_format).map_err(ledger_error)?;
        let request = GitRunRequestV1::new(
            GitCommandV1::VerifyPack {
                git_dir: BOOTSTRAP_NAME_V1.to_owned(),
                pack_hash: pack_hash.clone(),
                object_format,
            },
            Vec::new(),
            limit,
            GIT_STDERR_LIMIT_V1,
            deadline,
        );
        let expected = format!("{BOOTSTRAP_NAME_V1}/objects/pack/pack-{pack_hash}.pack: ok");
        self.run_child(
            runner,
            &names,
            "verify-pack",
            request,
            ChildRoleV1::ReadOnly,
            |result| {
                require_success(result, "verify-pack")?;
                let stdout = result.captured_stdout().ok_or_else(|| {
                    GitBootstrapErrorV1::GitOutput("verify-pack was streamed".into())
                })?;
                check_verify_pack_proof(stdout, &expected)
            },
        )
    }

    /// Open the three files `index-pack` wrote no-follow through the retained pack directory,
    /// measure each from its descriptor, and require the pack to be exactly the staged bytes.
    fn admit_pack_files(
        &mut self,
        pack_hash: &str,
        staged_length: u64,
        staged_sha256: [u8; 32],
    ) -> Result<(), GitBootstrapErrorV1> {
        let mut admitted = Vec::with_capacity(3);
        {
            let pack_dir = self.dirs.get(PACK_DIRECTORY_V1).ok_or_else(|| {
                GitBootstrapErrorV1::MissingEntry {
                    name: PACK_DIRECTORY_V1.to_owned(),
                }
            })?;
            for extension in ["pack", "idx", "rev"] {
                let leaf = format!("pack-{pack_hash}.{extension}");
                let relative = format!("{PACK_DIRECTORY_V1}/{leaf}");
                let mut file = pack_dir
                    .open_regular_file(OsStr::new(&leaf), "custody restore admitted object file")
                    .map_err(fs_error)?;
                let metadata = file
                    .metadata()
                    .map_err(|error| CustodyRestoreErrorV1::Io(format!("{relative}: {error}")))?;
                let stat = ChildStatV1::from_metadata(&metadata);
                let (length, sha256) = hash_prefix(&mut file, stat.size)
                    .map_err(|error| CustodyRestoreErrorV1::Io(format!("{relative}: {error}")))?;
                if length != stat.size {
                    return Err(GitBootstrapErrorV1::Restore(
                        CustodyRestoreErrorV1::IdentityChanged(format!(
                            "{relative}: it changed while it was admitted"
                        )),
                    ));
                }
                file.seek(SeekFrom::Start(0))
                    .map_err(|error| CustodyRestoreErrorV1::Io(format!("{relative}: {error}")))?;
                if extension == "pack" && (length != staged_length || sha256 != staged_sha256) {
                    return Err(GitBootstrapErrorV1::VerificationInputMismatch);
                }
                admitted.push(AdmittedObjectFileV1 {
                    relative,
                    file,
                    length,
                    sha256,
                    dev: stat.dev,
                    ino: stat.ino,
                });
            }
        }
        self.admitted = admitted;
        Ok(())
    }

    fn prove_inventory(
        &mut self,
        runner: &GitRunnerV1,
        object_format: GitObjectFormatV1,
        expected: &BTreeMap<String, &'static str>,
        deadline: Instant,
    ) -> Result<(), GitBootstrapErrorV1> {
        let request = GitRunRequestV1::new(
            GitCommandV1::CatFileAllObjects,
            Vec::new(),
            stdout_limit_for_objects(expected.len()),
            GIT_STDERR_LIMIT_V1,
            deadline,
        );
        let names = Self::names()?;
        self.run_child(
            runner,
            &names,
            "cat-file --batch-all-objects",
            request,
            ChildRoleV1::ReadOnly,
            |result| {
                require_success(result, "cat-file --batch-all-objects")?;
                let stdout = result.captured_stdout().ok_or_else(|| {
                    GitBootstrapErrorV1::GitOutput("cat-file stdout was streamed".into())
                })?;
                compare_inventory(stdout, expected, object_format)
            },
        )
    }

    fn prove_closure(
        &mut self,
        runner: &GitRunnerV1,
        object_format: GitObjectFormatV1,
        expected: &BTreeMap<String, &'static str>,
        deadline: Instant,
    ) -> Result<usize, GitBootstrapErrorV1> {
        // EVERY inventory object is an input: blobs, trees, annotated tags, and unreachable ones.
        let mut stdin = Vec::new();
        for object_id in expected.keys() {
            stdin.extend_from_slice(object_id.as_bytes());
            stdin.push(b'\n');
        }
        let request = GitRunRequestV1::new(
            GitCommandV1::RevListMissingPrint,
            stdin,
            stdout_limit_for_objects(expected.len().saturating_mul(2)),
            GIT_STDERR_LIMIT_V1,
            deadline,
        );
        let names = Self::names()?;
        self.run_child(
            runner,
            &names,
            "rev-list --missing=print",
            request,
            ChildRoleV1::ReadOnly,
            |result| {
                require_success(result, "rev-list --missing=print")?;
                let stdout = result.captured_stdout().ok_or_else(|| {
                    GitBootstrapErrorV1::GitOutput("rev-list stdout was streamed".into())
                })?;
                compare_closure(stdout, expected, object_format)
            },
        )
    }

    fn prove_fsck(
        &mut self,
        runner: &GitRunnerV1,
        objects: usize,
        deadline: Instant,
    ) -> Result<(), GitBootstrapErrorV1> {
        let request = GitRunRequestV1::new(
            GitCommandV1::FsckStrict,
            Vec::new(),
            stdout_limit_for_objects(objects),
            GIT_STDERR_LIMIT_V1,
            deadline,
        );
        let names = Self::names()?;
        self.run_child(
            runner,
            &names,
            "fsck --strict",
            request,
            ChildRoleV1::ReadOnly,
            |result| {
                // The classifier runs BEFORE the exit status: a strict rejection exits nonzero.
                classify_fsck(result)?;
                require_success(result, "fsck --strict")
            },
        )
    }

    fn ledger_used(&self) -> u64 {
        self.dest().ledger.borrow().used()
    }
}

/// `directory/name` relative to the bootstrap, lossily rendered.
fn bootstrap_relative(directory: &str, name: &[u8]) -> String {
    if directory.is_empty() {
        lossy(name)
    } else {
        format!("{directory}/{}", lossy(name))
    }
}

// ---------------------------------------------------------------------------------------------
// The entry point
// ---------------------------------------------------------------------------------------------

/// What admission identified, before any write.
struct AdmissionV1 {
    shape: ObjectShapeV1,
    provenance: GitFormatProvenanceV1,
    active: Vec<ActiveFrameV1>,
    expected: BTreeMap<String, &'static str>,
}

/// Admit phase G: classify the shape, re-measure every staged input from its retained descriptor,
/// and run the gate. Nothing is written.
fn admit(
    verified: &mut VerifiedCapsuleV1,
    object_format: GitObjectFormatV1,
) -> Result<AdmissionV1, GitBootstrapErrorV1> {
    let (shape, provenance, active) = classify_shape(verified)?;
    // Exact manifest/format agreement: every manifest object is in the caller's format, and the
    // inventory is a set. For Empty there is nothing to agree with, so the caller's format stands.
    let expected = manifest_inventory(&verified.control.manifest, object_format)?;
    #[cfg(test)]
    run_git_hook(
        GitHookPointV1::AdmissionEntry,
        "",
        verified.destination.root.canonical_path(),
    );
    for one in &mut verified.staged {
        recheck_staged(one)?;
    }
    // The gate follows the (possibly long) re-measurement, so a drift during it is seen here.
    gate_parts(&verified.destination, None, None, &BTreeMap::new())?;
    Ok(AdmissionV1 {
        shape,
        provenance,
        active,
        expected,
    })
}

/// The directories this step creates.
struct PinsV1 {
    home: PinnedDirectoryV1,
    xdg: PinnedDirectoryV1,
    bootstrap: PinnedDirectoryV1,
    /// The bootstrap's reservation, whole: the nine `for_init` entries and its 4 KiB of bytes.
    owned: OwnedChargeV1,
}

/// Create `home` and `xdg`, then reserve the whole `for_init` budget and create-new and pin
/// `git-bootstrap`, all BEFORE any Git child. The bootstrap's own entry is one of `for_init`'s
/// nine, so it is not charged twice. A planted entry in any of the three slots is a collision.
fn create_pins(
    dest: &super::RestoreDestinationV1,
    budget: &GitDirectoryBudgetV1,
) -> Result<PinsV1, GitBootstrapErrorV1> {
    let work_name = RESTORE_WORK_DIR_NAME_V1.as_bytes();
    let home = create_staging_directory(
        &dest.work,
        &dest.ledger,
        dest.dev,
        HOME_NAME_V1.as_bytes(),
        &joined(work_name, HOME_NAME_V1.as_bytes()),
    )?;
    let xdg = create_staging_directory(
        &dest.work,
        &dest.ledger,
        dest.dev,
        XDG_NAME_V1.as_bytes(),
        &joined(work_name, XDG_NAME_V1.as_bytes()),
    )?;
    #[cfg(test)]
    run_git_hook(
        GitHookPointV1::BeforeBootstrapCreate,
        "",
        dest.work.canonical_path(),
    );
    gate_parts(dest, Some(&home), Some(&xdg), &BTreeMap::new())?;
    // The pair is refused whole or reserved whole; if the create-new then provably created nothing,
    // it is given back whole.
    let reserved = ReservedPairV1::reserve(&dest.ledger, budget.entries, budget.logical_bytes)?;
    let name = joined(work_name, BOOTSTRAP_NAME_V1.as_bytes());
    let bootstrap = match dest.work.create_new_child_directory(
        OsStr::new(BOOTSTRAP_NAME_V1),
        "custody restore git bootstrap",
    ) {
        Ok(bootstrap) => bootstrap,
        Err(error) => {
            // `TargetExists` is returned only where `mkdirat` itself answered `EEXIST`, before any
            // entry existed, so the whole pair goes back. Every other error may follow a
            // successful `mkdirat`, and keeps the charge, as does a refusal after this point.
            if matches!(error, FsCustodyError::TargetExists(_)) {
                reserved.roll_back(&dest.ledger);
            }
            return Err(staging_refusal(error, &name).into());
        }
    };
    let observed = bootstrap
        .current_metadata("custody restore git bootstrap")
        .map_err(|error| CustodyRestoreErrorV1::Io(error.to_string()))?
        .dev;
    #[cfg(test)]
    let observed = super::CREATED_DEV
        .with(|devices| devices.borrow().get(name.as_slice()).copied())
        .unwrap_or(observed);
    if observed != dest.dev {
        return Err(CustodyRestoreErrorV1::DeviceCrossing { name: lossy(&name) }.into());
    }
    Ok(PinsV1 {
        home,
        xdg,
        bootstrap,
        owned: reserved.into_owned(),
    })
}

/// Prove `verified`'s Git objects in an isolated bootstrap repository and retain the proof.
///
/// Admission rechecks every phase-V chain and the destination root, re-measures every staged
/// input from its retained descriptor, and identifies the object shape and the active payloads:
/// Captured needs exactly one pack and exact manifest/pack format agreement; a valid Empty database
/// needs no pack and no objects, takes the caller's format, and reports
/// [`GitFormatProvenanceV1::CallerSelectedEmpty`]; every other combination refuses. Only then are
/// `home`, `xdg`, and `git-bootstrap` created, and the children run.
pub(crate) fn bootstrap_git_objects_v1(
    mut verified: VerifiedCapsuleV1,
    runner: &GitRunnerV1,
    object_format: GitObjectFormatV1,
    deadline: Instant,
) -> Result<VerifiedGitBootstrapV1, GitBootstrapFailureV1> {
    let failure = |cause, last_completed, verified: &VerifiedCapsuleV1| GitBootstrapFailureV1 {
        cause,
        last_completed,
        active_materialization_began: false,
        ledger_used: verified.destination.ledger.borrow().used(),
    };
    let admission = match admit(&mut verified, object_format) {
        Ok(admission) => admission,
        Err(cause) => return Err(failure(cause, None, &verified)),
    };
    let budget = GitDirectoryBudgetV1::for_init(BOOTSTRAP_NAME_V1);
    let pins = match create_pins(&verified.destination, &budget) {
        Ok(pins) => pins,
        Err(cause) => return Err(failure(cause, Some(GitPlaneStepV1::Admission), &verified)),
    };
    let mut work = WorkV1 {
        verified,
        home: pins.home,
        xdg: pins.xdg,
        dirs: BTreeMap::from([(String::new(), pins.bootstrap)]),
        budget,
        required: BTreeSet::new(),
        owned: pins.owned,
        deadline,
        pack_hash: None,
        admitted: Vec::new(),
        runs: Vec::new(),
        progress: Some(GitPlaneStepV1::Admission),
    };
    match run_steps(&mut work, runner, object_format, deadline, &admission) {
        Ok(closure) => Ok(work.into_result(admission, closure, object_format)),
        Err(cause) => Err(failure(cause, work.progress, &work.verified)),
    }
}

impl WorkV1 {
    /// Infallible: the closing gate has passed, and this only moves retained values.
    fn into_result(
        self,
        admission: AdmissionV1,
        closure: GitClosureEvidenceV1,
        object_format: GitObjectFormatV1,
    ) -> VerifiedGitBootstrapV1 {
        let objects = match self.pack_hash {
            Some(pack_hash) => BootstrapObjectsV1::Captured {
                pack_hash,
                files: self.admitted,
            },
            None => BootstrapObjectsV1::Empty,
        };
        VerifiedGitBootstrapV1 {
            verified: self.verified,
            home: self.home,
            xdg: self.xdg,
            directories: self.dirs,
            object_format,
            provenance: admission.provenance,
            objects,
            closure,
            active_frames: admission.active,
            runs: self.runs,
        }
    }
}

fn run_steps(
    work: &mut WorkV1,
    runner: &GitRunnerV1,
    object_format: GitObjectFormatV1,
    deadline: Instant,
    admission: &AdmissionV1,
) -> Result<GitClosureEvidenceV1, GitBootstrapErrorV1> {
    let expected = &admission.expected;
    work.init_bare(runner, object_format, deadline)?;

    let captured = match admission.shape {
        ObjectShapeV1::Captured { pack } => {
            work.index_pack(runner, object_format, pack, expected.len(), deadline)?;
            work.progress = Some(GitPlaneStepV1::Indexing);
            true
        }
        ObjectShapeV1::Empty => false,
    };

    work.prove_inventory(runner, object_format, expected, deadline)?;
    work.progress = Some(GitPlaneStepV1::Inventory);

    let closure = if captured {
        GitClosureProofV1::Reachable {
            objects: work.prove_closure(runner, object_format, expected, deadline)?,
        }
    } else {
        // Empty: no empty rev-list request. The closure of nothing is vacuously whole.
        GitClosureProofV1::Vacuous
    };
    work.progress = Some(GitPlaneStepV1::Closure);

    work.prove_fsck(runner, expected.len(), deadline)?;
    work.progress = Some(GitPlaneStepV1::Fsck);

    // The staged descriptors the runner advanced go back to offset 0 before the closing gate.
    work.rewind_staged()?;

    #[cfg(test)]
    run_git_hook(
        GitHookPointV1::BeforeFinalGate,
        "",
        work.dest().root.canonical_path(),
    );
    // The closing gate: the census, the exact tree and every admitted file's bytes, every chain,
    // and the root LAST. Nothing fallible follows it.
    work.closing_gate()?;
    work.progress = Some(GitPlaneStepV1::BootstrapGate);
    Ok(GitClosureEvidenceV1 {
        inventory_objects: expected.len(),
        closure,
        fsck_strict: true,
    })
}
