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
//!
//! **Pass B1a.** After the bootstrap, [`prevalidate_active_frames_v1`] admits the retained active
//! frames (`RefsAndHead`, `StashAndReflogs`) for the later restore, and writes nothing. It identifies
//! the active roles from the authenticated coverage, refuses an Empty `RefsAndHead` class with a typed
//! refusal before any entry is created (complete phase G needs a captured `HEAD`; none is synthesized),
//! rehashes each retained descriptor under the one deadline, and decodes every active frame completely,
//! through its verified trailer, into a null sink. Each decoded entry feeds one aggregate path table
//! that enforces the exact active allowlist in its actual classes, entry types, parent/child
//! agreement, and the portable-spelling rule. Only a whole, trailer-verified frame is judged final.
//! It returns bounded metadata, [`ActiveFramePlanV1`], for a later second decode; the caller keeps the
//! one capsule, ledger, pins, and deadline. The pass A invariant above is function-scoped: direct
//! [`bootstrap_git_objects_v1`] never creates anything outside `.restore-work/`.
//!
//! **Pass B1b.** [`RefsBuilderV1`] is the pure refs foundation: it parses the exact HEAD, loose refs,
//! packed refs (with peeled rows), `ORIG_HEAD`, `FETCH_HEAD`, and reflogs from readers, under explicit
//! byte, line, entry, and retained-memory limits and the one deadline, never invoking Git and never
//! reading archived configuration; then [`RefsBuilderV1::finish`] resolves symbolic chains with bounded
//! work, applies loose-over-packed precedence, and agrees the result with the manifest's original refs
//! and the verified inventory. [`agree_active_refs_v1`] feeds it from the retained, B1a-prevalidated
//! frames (a second decode bound to [`ActiveFramePlanV1`]); a later pass feeds the same builder from
//! the retained final descriptors. [`encode_inert_config_v1`] is the deterministic active config. It
//! restores nothing: no repository entry, no Git child, no ledger access.
//!
//! **Pass B2a.** The helper whose result is [`FinalObjectStoreV1`] is the first that creates anything
//! under `repository/`. Only after the retained, prevalidated active frames ([`ActiveFramePlanV1`], which must be
//! this capsule's) and the proven bootstrap agree does it create `repository/.git/objects/{pack,info}`
//! through retained pins, create-new, charging every entry before it exists, and copy ONLY the bootstrap's
//! three admitted regular files (`.pack`, `.idx`, `.rev`) from their retained descriptors: each chunk is
//! charged before it is written, the bytes are compared with the admitted length and SHA-256 as they stream
//! and again when read back from the new file, and the file and its directory are synced. The second copy
//! is charged like the first. Every chain, device, and mount is rechecked around the writes under the one
//! deadline, which is read again after every recheck, source read, and file sync, before the next create,
//! charge, write, or sync. Nothing is renamed, overwritten, deleted, or cleaned up, the bootstrap's own HEAD, config, and
//! hooks are never copied, and no Git child runs. A refusal keeps every created byte and reports it as typed
//! partial effects. The result is a helper value for the later passes, not phase G's result, and the pass A
//! invariant above stays function-scoped: direct [`bootstrap_git_objects_v1`] still creates nothing there.
//!
//! **Pass B2b.** [`materialize_active_metadata_v1`] restores the active metadata beneath the retained
//! `repository/.git` pin, after the object copy and before any final Git command. It reverifies each retained
//! active frame from its descriptor immediately before a second complete decode, applies the pass B1a admission
//! rules to every decoded entry again, and only then creates the entry: directories and regular files, each
//! create-new beneath a retained pin this invocation made, charged to the one ledger (the entry, then every
//! chunk) before it exists, the bytes exactly as the frame carries them. Each decoder is drained through its
//! trailer; a later malformed byte is fatal and every partial byte stays. Before any config exists, every
//! restored file is read back from its retained descriptor and agreed with the manifest's original refs and
//! the verified inventory through the pass B1b parsers, and only then is the inert config created, once. The
//! repository keeps one child, `.git`; no hook, include, filter, remote, or archived config is ever restored.
//! Typed progress names the last step completed whole, and every pin and file is retained for the final
//! passes. It runs no Git child and never reopens a staged name.
//!
//! **Pass B2c.** [`restore_git_plane_v1`] is the one wrapper: it consumes the ORIGINAL [`VerifiedCapsuleV1`] and
//! carries it, its pins, its one ledger, and the caller's ONE absolute deadline through every earlier pass in
//! the documented order (the bootstrap, B1a's prevalidation, B1b's agreement, B2a's object copy, B2b's
//! metadata) and then through the final proof, without a second restore attempt, a second ledger, a reset
//! budget, or a new instant. Before any write beneath the repository it has proven the frames and their refs,
//! and the bootstrap's closure; before any final command it has restored the refs, HEAD, and config. The final
//! proof repeats the inventory, the reachability of every inventory object (none for an Empty database), and
//! the strict fsck in the FINAL database, with the same read-only commands and strict parsers, rooted at the
//! retained repository (HOME, XDG, and the Git directory all alias its retained Git directory) in the
//! runner's closed environment, never an object-store route. Each child owns a zero-effect allowance: the
//! tree is the restore's exactly (names, identities, lengths, every restored file's mode, timestamps, and
//! SHA-256, and every directory's mode and timestamps) before and after it, on every outcome, and an effect is
//! refused beside the child's own failure: a failure that is already known (a runner or callback failure, a
//! status or parser refusal) or becomes known inside the paced settlement (the typed timeout, or a custody
//! finding) is COMPLETED once, at one site, without the clock, through one helper that makes every
//! observation still owed, each independently of the others and each unit once: the mount census; the census of
//! the exact tree and its comparison with the baseline; the digest of every restored file by the descriptor the
//! restore RETAINED (positioned reads, bounded to its length plus one byte, an interruption refused and never
//! retried); and every retained chain edge, with the destination root's identity last. Progress is kept CALL by
//! call: a call that returned is attempted whether a refusal or a guard that found the deadline spent followed it,
//! its fact is carried (a census call's `fstat`, listing, or descriptor; the bytes a paced digest read), and it is
//! never made again, while every call that did not run is made once. A refusal of one unit settles no other: a
//! census continues past a replaced or unexpected entry without descending it, a refused NAME says nothing about
//! the retained descriptor's digest, and every retained edge (the work and plain directories, each retained
//! staging directory once, and each admitted file's entry and descriptor) is its own unit. The findings are kept
//! beside the primary in a fixed order (a callback's refusal first), a drifted binary staying the primary:
//! beside the failure, and never as a success. The closing gate is the mount census, the
//! exact tree and digest, every retained chain, and the destination root's identity LAST, each step held to
//! the one deadline with a guard read after every filesystem call returns and before the next is made; only
//! infallible construction of [`RestoredGitPlaneV1`] follows it. A failure is a [`GitPlaneRestoreFailureV1`]: the last step
//! completed whole, whether materialization began, and every retained value, with every created byte left in
//! place. It writes no restore record, cleans nothing up, and has no public or operator surface.

use super::{
    create_staging_directory, hash_prefix, joined, lossy, plain_relative, recheck_entry,
    refuse_mounts_within, refused_link, staging_refusal, CustodyRestoreErrorV1, StagedPlaintextV1,
    VerifiedCapsuleV1, HASH_BUFFER_BYTES_V1, RESTORE_PLAIN_DIR_NAME_V1, RESTORE_WORK_DIR_NAME_V1,
};
use crate::custody_capsule::CustodyCapsuleArtifactRoleV1;
use crate::custody_coverage::route_git_dir_entry;
use crate::custody_export::{
    bounded, index_pack_logical_bound, stdout_limit_for_objects, strict_object_message_id,
    verify_pack_stdout_limit, CustodyExportErrorV1, GitDirectoryBudgetV1, GitRunRecordV1,
    ScratchLedgerV1, ENTRY_ALLOWANCE_BYTES_V1, FSCK_ALLOWED_NOTICE_V1, GIT_DIR_MAX_DEPTH_V1,
    GIT_DIR_MAX_ENTRIES_V1, GIT_MIN_STDOUT_LIMIT_V1, GIT_STDERR_LIMIT_V1, INDEX_PACK_ENTRIES_V1,
};
use crate::custody_frame::{
    CustodyFrameBudgetV1, CustodyFrameContentReaderV1, CustodyFrameDecoderV1, CustodyFrameEntryV1,
    CustodyFrameHeaderV1,
};
use crate::custody_git::{
    CustodyGitError, GitCommandV1, GitObjectFormatV1, GitRootNamesV1, GitRunRequestV1,
    GitRunResultV1, GitRunnerV1,
};
use crate::custody_inventory::CustodyStateClassV1;
use crate::custody_seal::{
    CustodyCoverageClassV1, CustodyGitObjectFormatV1, CustodyGitObjectKindV1, CustodyManifestV1,
    CustodyRefTargetV1,
};
use crate::fs_custody::{
    pinned_root_unchanged, ChildKindV1, ChildStatV1, FsCustodyError, PinnedDirectoryV1,
};
use ring::digest;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, BufReader, ErrorKind, Read as _, Seek as _, SeekFrom};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::FileExt as _;
use std::path::Path;
use std::time::Instant;

// ---------------------------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------------------------

pub(super) const HOME_NAME_V1: &str = "home";
pub(super) const XDG_NAME_V1: &str = "xdg";
pub(super) const BOOTSTRAP_NAME_V1: &str = "git-bootstrap";
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
    #[error("git printed output that is refused: {0}")]
    GitOutput(String),
    #[error("the bytes index-pack consumed are not the verified staged pack")]
    VerificationInputMismatch,
    #[error("the database's objects are not the manifest's inventory: {0}")]
    InventoryMismatch(String),
    #[error("the manifest's objects are not closed in the database: {0}")]
    ClosureIncomplete(String),
    // Active-frame prevalidation (pass B1a).
    #[error(
        "complete phase G refuses an Empty RefsAndHead class: it has no HEAD, and none is synthesized"
    )]
    EmptyRefsAndHead,
    #[error("the {class:?} frame is refused: {fault}")]
    ActiveFrame {
        class: CustodyCoverageClassV1,
        fault: ActiveFrameFaultV1,
    },
    // Refs agreement (pass B1b).
    #[error("the refs agreement refuses: {0}")]
    Refs(RefsFaultV1),
    #[error("the active-frame plan is not this capsule's: {0}")]
    ActivePlanMismatch(String),
    // The final object store (pass B2a).
    #[error("the verified bootstrap is not a proven object store: {0}")]
    ObjectStoreInput(String),
    #[error("the copy of {relative} is not the admitted bytes: {detail}")]
    ObjectCopy { relative: String, detail: String },
    // The active metadata and the final ref agreement (pass B2b).
    #[error("the inputs of the metadata step are not one restore's: {0}")]
    MetadataInput(String),
    #[error("the metadata file {relative} is not the bytes that were written: {detail}")]
    MetadataCopy { relative: String, detail: String },
    // The final repository and its proof (pass B2c).
    #[error("the final proof's inputs are not one restore's: {0}")]
    FinalInput(String),
    #[error("the final repository is not what the restore made: {name}: {fault:?}")]
    FinalTree {
        name: String,
        fault: FinalTreeFaultV1,
    },
    /// A child failed, and reconciling what it left behind failed too. Neither is dropped: the
    /// reconciliation failure is a budget or custody refusal in its own right. When several classes of what a
    /// final child left (the mount census, the exact tree, the retained chains) fail, the first failing class
    /// is the `primary` of a nested value that holds the later ones as its `reconcile`.
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
    /// A decoded active-frame entry is about to be judged: the path is the entry's frame path (pass
    /// B1a).
    FrameEntry,
    /// An object-store entry is about to be created, after its pre-create recheck: the label is
    /// `directory` or `file`, and the path is the entry (pass B2a).
    BeforeObjectCreate,
    /// One chunk of an object file is about to be read, charged, and written: the path is the new file
    /// (pass B2a).
    BeforeObjectChunk,
    /// A created file or its directory was just synced: the label is `file` or `directory`, and the path
    /// is that file or that directory (pass B2a).
    ObjectSynced,
    /// All object-store entries are made, before the final gate: the path is the destination root (pass
    /// B2a).
    BeforeObjectsFinalGate,
    /// One blocking call of the object-store copy has just returned, before its result is examined: the
    /// label is `read` (a chunk read), `write`, or `probe` (the length probe), and the path is the new
    /// file (pass B2a). A hook that sleeps here is that call having been slow.
    ObjectIo,
    /// All active metadata and the config exist, before the final gate: the path is the destination root
    /// (pass B2b).
    BeforeMetadataFinalGate,
    /// One read of a restored file by the refs agreement has just returned, before its result is examined: the
    /// path is that file (pass B2b). A hook that changes the file here changes what the parsers read next.
    AfterRefsRead,
    /// A final child's census is about to run, before any pre-spawn check: the label is the child's, and the
    /// path is the retained repository (pass B2c).
    BeforeFinalChildCensus,
    /// A final child's pre-spawn callback, before its gate (the repository).
    BeforeFinalChild,
    /// A final child's post-exit callback, before its gate (the repository).
    AfterFinalChild,
    /// A directory of the final tree was just `fstat`ed, before anything else is done with it (that directory;
    /// pass B2c).
    FinalCensusStatted,
    /// One directory of the final tree has just been listed, before its entries are examined (that directory).
    FinalCensusListed,
    /// A listed entry of the final tree was `fstatat`ed and is about to be opened (that entry).
    BeforeFinalCensusOpen,
    /// The census opened a final file no-follow, before its descriptor is `fstat`ed (that file).
    FinalFileOpened,
    /// A digest has just opened a final file through its retained directory pin, before its descriptor is
    /// `fstat`ed (that file). The failure-only digest opens nothing: it fires this as it takes up the file's
    /// retained descriptor, before that descriptor is `fstat`ed.
    FinalDigestFileOpened,
    /// A digest opened a final file through its retained directory pin and the descriptor matched its census
    /// fact; nothing has been read from it yet (that file). The failure-only digest fires it when the retained
    /// descriptor is still the file the restore made, before its first read.
    FinalDigestOpened,
    /// A digest read a final file to its end; its closing `fstat` check has not run yet (that file).
    FinalDigestHashed,
    /// The final gates `fstat`ed an admitted bootstrap file by its name; its retained descriptor has not been
    /// `fstat`ed yet (that file).
    FinalSourceStatted,
    /// The final gates rechecked one entry of a staging chain; the next entry of the chain has not been
    /// (the parent directory of the entry that was checked).
    FinalStagingChecked,
    /// Every final child is proven and the staged descriptors are rewound, before the closing gate: the path
    /// is the destination root.
    BeforePlaneFinalGate,
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
    /// A smaller chunk for the object-store copy, so small files span several chunks.
    static COPY_CHUNK: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    /// A fault the object-store copy raises in place of one filesystem effect.
    static OBJECT_FAULT: std::cell::Cell<Option<ObjectFaultV1>> =
        const { std::cell::Cell::new(None) };
    /// How many more calls of each kind (read, write, probe) return as interrupted, having done nothing.
    static OBJECT_INTERRUPTS: std::cell::Cell<[u32; 3]> = const { std::cell::Cell::new([0; 3]) };
    /// The reads of the failure-only digest of a final child: every read call made, by file, and one file whose
    /// first reads return as interrupted, having done nothing.
    static RECONCILE_READS: RefCell<Option<ReconcileReadsV1>> = const { RefCell::new(None) };
}

/// What a test observes of, and injects into, the reads of the failure-only digest (pass B2c).
#[cfg(test)]
#[derive(Debug, Default)]
struct ReconcileReadsV1 {
    /// The file (relative to `.git`) whose next `count` reads return as interrupted.
    interrupt: Option<(String, u32)>,
    /// Every read call made, by file.
    calls: BTreeMap<String, u32>,
}

/// A blocking call of the object-store copy on a descriptor: the three that can return after the deadline
/// and be followed by a retry or a continuation (pass B2a).
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObjectCallV1 {
    /// A chunk read of the retained source.
    Read,
    /// A write of the rest of a chunk to the new file.
    Write,
    /// The one-byte read that proves the source holds no more than its admitted length.
    Probe,
}

/// A fault a test makes the object-store copy raise in place of one filesystem effect, so a failure whose
/// real trigger is a full disk, an I/O error, or a permission refusal is exercised (pass B2a).
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObjectFaultV1 {
    /// Every file's writes stop after this many bytes of that file: the write that would pass the limit
    /// writes only up to it, and the next write fails.
    WriteAfter(u64),
    /// The same limit, but the next write reports that it wrote nothing.
    WriteZeroAfter(u64),
    /// The file's `fsync` fails.
    SyncFile,
    /// The pack directory's `fsync` fails.
    SyncDirectory,
    /// Creating the file fails with an error that is not a collision.
    CreateFile,
    /// Every write accepts at most this many bytes of what it is given, as signals, quotas, and some
    /// filesystems make it: the rest of the chunk is left to the next call.
    ShortWrites(usize),
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
    CopyChunk,
    ObjectFault,
    ObjectInterrupts(ObjectCallV1),
    ReconcileReads,
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
            Self::CopyChunk => COPY_CHUNK.with(|chunk| chunk.set(None)),
            Self::ObjectFault => OBJECT_FAULT.with(|fault| fault.set(None)),
            Self::ObjectInterrupts(call) => OBJECT_INTERRUPTS.with(|left| {
                let mut counts = left.get();
                counts[*call as usize] = 0;
                left.set(counts);
            }),
            Self::ReconcileReads => RECONCILE_READS.with(|reads| *reads.borrow_mut() = None),
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

/// Makes the object-store copy raise `fault` in place of one filesystem effect.
#[cfg(test)]
pub(crate) fn override_object_fault_for_test(fault: ObjectFaultV1) -> GitSeamGuardV1 {
    OBJECT_FAULT.with(|slot| slot.set(Some(fault)));
    GitSeamGuardV1::ObjectFault
}

/// Makes the first `count` calls of `call` return as interrupted (`EINTR`), having done nothing; later calls
/// are real. A regular file does not produce an interruption on demand, and a retried one is exactly the
/// call that must still observe the one deadline.
#[cfg(test)]
pub(crate) fn interrupt_object_calls_for_test(call: ObjectCallV1, count: u32) -> GitSeamGuardV1 {
    OBJECT_INTERRUPTS.with(|left| {
        let mut counts = left.get();
        counts[call as usize] = count;
        left.set(counts);
    });
    GitSeamGuardV1::ObjectInterrupts(call)
}

/// The interruption the next `call` is to return, if a test asked for one: consumed once.
#[cfg(test)]
fn injected_interrupt(call: ObjectCallV1) -> Option<io::Error> {
    OBJECT_INTERRUPTS.with(|left| {
        let mut counts = left.get();
        let slot = &mut counts[call as usize];
        if *slot == 0 {
            return None;
        }
        *slot -= 1;
        left.set(counts);
        Some(io::Error::from(ErrorKind::Interrupted))
    })
}

/// Starts counting every read call of the failure-only digest of a final child, by file (relative to `.git`),
/// and, when `interrupt` names a file and a count, makes that file's first `count` reads return as interrupted
/// (`EINTR`), having done nothing; later reads are real. A regular file does not produce an interruption on
/// demand, and a retried one is exactly the call a failure that is already known must never wait for.
#[cfg(test)]
pub(crate) fn watch_reconcile_reads_for_test(interrupt: Option<(&str, u32)>) -> GitSeamGuardV1 {
    RECONCILE_READS.with(|reads| {
        *reads.borrow_mut() = Some(ReconcileReadsV1 {
            interrupt: interrupt.map(|(file, count)| (file.to_owned(), count)),
            calls: BTreeMap::new(),
        });
    });
    GitSeamGuardV1::ReconcileReads
}

/// Every read call the failure-only digest has made since [`watch_reconcile_reads_for_test`], by file.
#[cfg(test)]
pub(crate) fn reconcile_read_calls_for_test() -> BTreeMap<String, u32> {
    RECONCILE_READS.with(|reads| {
        reads
            .borrow()
            .as_ref()
            .map(|watched| watched.calls.clone())
            .unwrap_or_default()
    })
}

/// One read call of the failure-only digest of `relative` is about to be made: it is counted, and a test may
/// have it return as interrupted instead (consumed once).
#[cfg(test)]
fn reconcile_read_call(relative: &str) -> Option<io::Error> {
    RECONCILE_READS.with(|reads| {
        let mut reads = reads.borrow_mut();
        let watched = reads.as_mut()?;
        *watched.calls.entry(relative.to_owned()).or_default() += 1;
        match &mut watched.interrupt {
            Some((file, left)) if file == relative && *left > 0 => {
                *left -= 1;
                Some(io::Error::from(ErrorKind::Interrupted))
            }
            _ => None,
        }
    })
}

/// Makes the object-store copy read, charge, and write `bytes` at a time instead of one hash buffer.
#[cfg(test)]
pub(crate) fn override_object_copy_chunk_for_test(bytes: usize) -> GitSeamGuardV1 {
    COPY_CHUNK.with(|chunk| chunk.set(Some(bytes.max(1))));
    GitSeamGuardV1::CopyChunk
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
/// absent, extra, wrong-kind, or malformed row is fatal. `database` names what was read in a refusal.
fn compare_inventory(
    stdout: &[u8],
    expected: &BTreeMap<String, &'static str>,
    format: GitObjectFormatV1,
    database: &str,
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
                    "{object_id} is missing from {database}"
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
pub(super) fn before_deadline(deadline: Instant) -> Result<(), GitBootstrapErrorV1> {
    if Instant::now() >= deadline {
        return Err(GitBootstrapErrorV1::Git(CustodyGitError::Timeout));
    }
    Ok(())
}

/// [`before_deadline`], called right after the late-validation step named `step` returned and before
/// anything else is done toward a verified result. Every guard of the success path is this call; the
/// test hook inside it is how a test makes exactly that step slow. Filesystem and platform calls cannot
/// be preempted, so the deadline can only be observed when each returns.
pub(super) fn observe_deadline(
    deadline: Instant,
    step: &'static str,
) -> Result<(), GitBootstrapErrorV1> {
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
pub(super) type PaceV1<'a> = &'a dyn Fn(&'static str) -> Result<(), GitBootstrapErrorV1>;

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

pub(super) fn coverage_state(
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

    let active = classify_active_frames(verified)?;
    Ok((shape, provenance, active))
}

/// The active payloads, from the manifest's coverage rows and the staged roles: `RefsAndHead` and
/// `StashAndReflogs`, each Captured (exactly one staged payload) or Empty (none). Every other state
/// refuses and is never downgraded to Empty. Pure: no filesystem effect. Pass A and pass B1a share it.
fn classify_active_frames(
    verified: &VerifiedCapsuleV1,
) -> Result<Vec<ActiveFrameV1>, GitBootstrapErrorV1> {
    let manifest = &verified.control.manifest;
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
    Ok(active)
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
                compare_inventory(stdout, expected, object_format, "the bootstrap")
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

// ---------------------------------------------------------------------------------------------
// Pass B1a: retained active-frame prevalidation
// ---------------------------------------------------------------------------------------------

/// The most bytes the aggregate path table may retain: two copies of each admitted path (its lowercased
/// key and its spelling) and a fixed allowance for the map's own bookkeeping per entry. The frame
/// codec bounds one frame's entries and bytes; this bounds what prevalidation keeps of all of them.
const ACTIVE_PATH_TABLE_BYTES_V1: u64 = 64 * 1024 * 1024;
const ACTIVE_ENTRY_OVERHEAD_BYTES_V1: u64 = 64;

/// What a decoded frame entry is, as far as the active subset is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActiveEntryKindV1 {
    Directory,
    Regular,
    /// Never admitted: the active subset is directories and regular files only.
    Symlink,
}

/// Why a path component is not portable: the Git-only rule is printable ASCII `0x21..=0x7e`, no
/// backslash, and no trailing dot. Nothing is normalized; every other spelling is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum PortableFaultV1 {
    #[error("a non-ASCII byte")]
    NonAscii,
    #[error("a backslash")]
    Backslash,
    #[error("a space or control byte")]
    Unprintable,
    #[error("a trailing dot")]
    TrailingDot,
}

/// Why one active-frame entry is refused. Paths are rendered lossily, for the message only.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ActiveFrameFaultV1 {
    #[error("{path}: a symlink is not restored by this slice")]
    Symlink { path: String },
    #[error("{path}: not a portable spelling ({reason})")]
    NonPortable {
        path: String,
        reason: PortableFaultV1,
    },
    #[error("{path}: a name that {owner:?} owns, not this class")]
    ForeignClass {
        path: String,
        owner: CustodyCoverageClassV1,
    },
    #[error("{path}: an unsupported or unknown Git-directory name")]
    Unsupported { path: String },
    #[error("{path}: not on the active allowlist")]
    NotAllowlisted { path: String },
    #[error("{path}: expected a {expected:?}")]
    WrongType {
        path: String,
        expected: ActiveEntryKindV1,
    },
    #[error("{path}: listed twice")]
    Duplicate { path: String },
    #[error("{path}: collides with {with} on a portable filesystem")]
    Collision { path: String, with: String },
    #[error("{path}: its parent is not an admitted directory of that exact spelling")]
    Conflict { path: String },
    #[error("{class:?} is not an active Git class")]
    InactiveClass { class: CustodyCoverageClassV1 },
    #[error("a Captured RefsAndHead class holds no HEAD regular file")]
    MissingHead,
    #[error("the active path table exceeds its {bound}-byte bound")]
    PathTableBound { bound: u64 },
}

fn portable_component(component: &[u8]) -> Result<(), PortableFaultV1> {
    for byte in component {
        if !byte.is_ascii() {
            return Err(PortableFaultV1::NonAscii);
        }
        if *byte == b'\\' {
            return Err(PortableFaultV1::Backslash);
        }
        if !(0x21..=0x7e).contains(byte) {
            return Err(PortableFaultV1::Unprintable);
        }
    }
    if component.last() == Some(&b'.') {
        return Err(PortableFaultV1::TrailingDot);
    }
    Ok(())
}

fn portable_path(path: &[u8]) -> Result<(), PortableFaultV1> {
    path.split(|byte| *byte == b'/')
        .try_for_each(portable_component)
}

/// The entry type an allowlisted top-level name must have in `class`, or why the name is refused: the
/// exact allowlist is `HEAD`, `ORIG_HEAD`, `FETCH_HEAD`, `packed-refs`, and `refs` (with its
/// descendants) for `RefsAndHead`, and `logs` (with its descendants) for `StashAndReflogs`.
fn top_level_kind(
    class: CustodyCoverageClassV1,
    top: &[u8],
    path: &[u8],
) -> Result<ActiveEntryKindV1, ActiveFrameFaultV1> {
    match (class, top) {
        (
            CustodyCoverageClassV1::RefsAndHead,
            b"HEAD" | b"ORIG_HEAD" | b"FETCH_HEAD" | b"packed-refs",
        ) => Ok(ActiveEntryKindV1::Regular),
        (CustodyCoverageClassV1::RefsAndHead, b"refs")
        | (CustodyCoverageClassV1::StashAndReflogs, b"logs") => Ok(ActiveEntryKindV1::Directory),
        _ => {
            let route = route_git_dir_entry(top);
            let path = lossy(path);
            Err(if top == b"shallow" || route.unresolved.is_some() {
                ActiveFrameFaultV1::Unsupported { path }
            } else if route.class != class {
                ActiveFrameFaultV1::ForeignClass {
                    path,
                    owner: route.class,
                }
            } else {
                ActiveFrameFaultV1::NotAllowlisted { path }
            })
        }
    }
}

#[derive(Debug)]
struct TableEntryV1 {
    spelling: Vec<u8>,
    kind: ActiveEntryKindV1,
}

/// The one aggregate table every active frame of a restore feeds, in order. Each entry is judged
/// against the exact allowlist and its class, its type, the portable-spelling rule, every earlier entry
/// of every frame (the same path twice, or a different spelling with the same ASCII-lowercased key),
/// and its parent (an admitted directory, spelled exactly as the entry says). An entry that is refused is
/// not admitted. Its retained bytes are bounded.
#[derive(Debug)]
pub(crate) struct ActivePathTableV1 {
    entries: BTreeMap<Vec<u8>, TableEntryV1>,
    retained: u64,
    bound: u64,
    head: bool,
}

impl ActivePathTableV1 {
    pub(crate) fn new(bound: u64) -> Self {
        Self {
            entries: BTreeMap::new(),
            retained: 0,
            bound,
            head: false,
        }
    }

    pub(crate) fn entries(&self) -> u64 {
        self.entries.len() as u64
    }

    /// Judge one entry of `class`'s frame, and admit it if it passes every rule.
    pub(crate) fn admit(
        &mut self,
        class: CustodyCoverageClassV1,
        path: &[u8],
        kind: ActiveEntryKindV1,
    ) -> Result<(), ActiveFrameFaultV1> {
        if !matches!(
            class,
            CustodyCoverageClassV1::RefsAndHead | CustodyCoverageClassV1::StashAndReflogs
        ) {
            return Err(ActiveFrameFaultV1::InactiveClass { class });
        }
        if kind == ActiveEntryKindV1::Symlink {
            return Err(ActiveFrameFaultV1::Symlink { path: lossy(path) });
        }
        portable_path(path).map_err(|reason| ActiveFrameFaultV1::NonPortable {
            path: lossy(path),
            reason,
        })?;
        let top = path.split(|byte| *byte == b'/').next().unwrap_or(path);
        let expected = top_level_kind(class, top, path)?;
        let parent = path
            .iter()
            .rposition(|byte| *byte == b'/')
            .map(|at| &path[..at]);
        if parent.is_none() && kind != expected {
            return Err(ActiveFrameFaultV1::WrongType {
                path: lossy(path),
                expected,
            });
        }
        let key = path.to_ascii_lowercase();
        if let Some(found) = self.entries.get(&key) {
            return Err(if found.spelling == path {
                ActiveFrameFaultV1::Duplicate { path: lossy(path) }
            } else {
                ActiveFrameFaultV1::Collision {
                    path: lossy(path),
                    with: lossy(&found.spelling),
                }
            });
        }
        if let Some(parent) = parent {
            match self.entries.get(&parent.to_ascii_lowercase()) {
                Some(found)
                    if found.kind == ActiveEntryKindV1::Directory && found.spelling == parent => {}
                _ => return Err(ActiveFrameFaultV1::Conflict { path: lossy(path) }),
            }
        }
        let retained = self
            .retained
            .saturating_add(2 * path.len() as u64)
            .saturating_add(ACTIVE_ENTRY_OVERHEAD_BYTES_V1);
        if retained > self.bound {
            return Err(ActiveFrameFaultV1::PathTableBound { bound: self.bound });
        }
        self.retained = retained;
        if class == CustodyCoverageClassV1::RefsAndHead && path == b"HEAD" {
            // The top-level rule made this a regular file.
            self.head = true;
        }
        self.entries.insert(
            key,
            TableEntryV1 {
                spelling: path.to_vec(),
                kind,
            },
        );
        Ok(())
    }

    /// A Captured refs class holds exactly one `HEAD` regular file: the table's paths are unique, so
    /// "one" is whether the regular `HEAD` entry was admitted.
    pub(crate) fn finish(&self) -> Result<(), ActiveFrameFaultV1> {
        if self.head {
            Ok(())
        } else {
            Err(ActiveFrameFaultV1::MissingHead)
        }
    }
}

fn class_tag(class: CustodyCoverageClassV1) -> u8 {
    match class {
        CustodyCoverageClassV1::RefsAndHead => 1,
        CustodyCoverageClassV1::StashAndReflogs => 2,
        _ => 0,
    }
}

fn kind_tag(kind: ActiveEntryKindV1) -> u8 {
    match kind {
        ActiveEntryKindV1::Directory => 1,
        ActiveEntryKindV1::Regular => 2,
        ActiveEntryKindV1::Symlink => 3,
    }
}

/// An order-sensitive SHA-256 over a frame's admitted entries: class, then for each entry its kind, its
/// path (length-prefixed), and its content length. A second decode that reproduces it has met the same
/// paths, types, and sizes in the same order; the content itself is bound by the staged digest.
pub(crate) struct ActiveEntryDigestV1 {
    context: digest::Context,
}

impl ActiveEntryDigestV1 {
    pub(crate) fn new(class: CustodyCoverageClassV1) -> Self {
        let mut context = digest::Context::new(&digest::SHA256);
        context.update(b"a2a-active-frame-entries-v1\0");
        context.update(&[class_tag(class)]);
        Self { context }
    }

    pub(crate) fn record(&mut self, kind: ActiveEntryKindV1, path: &[u8], length: u64) {
        self.context.update(&[kind_tag(kind)]);
        self.context.update(&(path.len() as u64).to_le_bytes());
        self.context.update(path);
        self.context.update(&length.to_le_bytes());
    }

    pub(crate) fn finish(self) -> [u8; 32] {
        let mut sha256 = [0_u8; 32];
        sha256.copy_from_slice(self.context.finish().as_ref());
        sha256
    }
}

/// What prevalidation retains of one whole, trailer-verified active frame: which staged payload it is,
/// the staged length and SHA-256 it was verified against, and its entry counts and entry digest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActiveFrameSummaryV1 {
    pub(crate) class: CustodyCoverageClassV1,
    /// Index into the verified capsule's staged plaintexts.
    pub(crate) staged: usize,
    pub(crate) length: u64,
    pub(crate) sha256: [u8; 32],
    pub(crate) entries: u64,
    pub(crate) directories: u64,
    pub(crate) regular_files: u64,
    pub(crate) content_bytes: u64,
    pub(crate) entries_sha256: [u8; 32],
}

/// Bounded private metadata for the later second decode: one summary per active frame, in class order.
/// It holds no path and no descriptor; the caller still owns the one verified capsule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActiveFramePlanV1 {
    pub(crate) frames: Vec<ActiveFrameSummaryV1>,
}

fn frame_invalid(class: CustodyCoverageClassV1) -> GitBootstrapErrorV1 {
    GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::FrameInvalid { class })
}

/// Re-measure one retained staged plaintext from its descriptor against its recorded length and
/// SHA-256, and leave it at offset 0. The descriptor is rewound explicitly before the read and after it;
/// the read is bounded to one byte past the recorded length and keeps the ONE absolute deadline before
/// every read; and a guard follows the rehash and precedes the closing rewind.
pub(crate) fn reverify_staged_v1(
    one: &mut StagedPlaintextV1,
    deadline: Instant,
) -> Result<(), GitBootstrapErrorV1> {
    observe_deadline(deadline, "frame-rehash")?;
    let name = lossy(one.name.as_bytes());
    let io_refusal = |error: io::Error| {
        GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Io(format!("{name}: {error}")))
    };
    one.file.seek(SeekFrom::Start(0)).map_err(io_refusal)?;
    let (length, sha256) = stream_digest(&mut one.file, one.length, deadline, &name)?;
    if length != one.length || sha256 != one.sha256 {
        return Err(GitBootstrapErrorV1::StagedInputChanged { name: name.clone() });
    }
    observe_deadline(deadline, "frame-rewind")?;
    one.file.seek(SeekFrom::Start(0)).map_err(io_refusal)?;
    Ok(())
}

/// The decoder's source: at most `recorded length + 1` bytes of the retained descriptor, one bounded
/// read at a time, each preceded by the ONE absolute deadline, and every byte read is hashed and counted
/// so the decoded bytes can be bound to the verified plaintext.
pub(super) struct PacedFrameReaderV1<'a> {
    pub(super) inner: io::Take<&'a File>,
    pub(super) deadline: Instant,
    pub(super) expired: &'a Cell<bool>,
    pub(super) context: digest::Context,
    pub(super) count: u64,
}

impl io::Read for PacedFrameReaderV1<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if before_deadline(self.deadline).is_err() {
            self.expired.set(true);
            return Err(io::Error::new(
                ErrorKind::TimedOut,
                "the phase-G deadline passed",
            ));
        }
        let read = self.inner.read(buffer)?;
        self.context.update(&buffer[..read]);
        self.count = self.count.saturating_add(read as u64);
        Ok(read)
    }
}

#[derive(Default)]
struct FrameCountsV1 {
    entries: u64,
    directories: u64,
    regular_files: u64,
    content_bytes: u64,
}

/// Prevalidate one retained active frame: rehash it, then decode it completely, through its verified
/// trailer, into a null sink, feeding every entry to the aggregate `table`. The first entry refusal is
/// held until the whole frame has decoded and its decoded bytes are bound to the verified plaintext, so a
/// frame that is not whole or not the verified bytes is refused as THAT, and nothing it says is trusted.
/// The descriptor ends at offset 0.
fn prevalidate_frame(
    one: &mut StagedPlaintextV1,
    frame: &ActiveFrameV1,
    generation: &str,
    budget: CustodyFrameBudgetV1,
    deadline: Instant,
    table: &mut ActivePathTableV1,
) -> Result<ActiveFrameSummaryV1, GitBootstrapErrorV1> {
    let class = frame.class;
    reverify_staged_v1(one, deadline)?;
    let header = CustodyFrameHeaderV1::new(class, generation).map_err(|_| frame_invalid(class))?;
    let expired = Cell::new(false);
    let refuse = || {
        if expired.get() {
            GitBootstrapErrorV1::Git(CustodyGitError::Timeout)
        } else {
            frame_invalid(class)
        }
    };
    let mut counts = FrameCountsV1::default();
    let mut entry_digest = ActiveEntryDigestV1::new(class);
    let mut fault: Option<ActiveFrameFaultV1> = None;
    let mut paced = PacedFrameReaderV1 {
        inner: (&one.file).take(one.length.saturating_add(1)),
        deadline,
        expired: &expired,
        context: digest::Context::new(&digest::SHA256),
        count: 0,
    };
    {
        let reader = BufReader::with_capacity(HASH_BUFFER_BYTES_V1, &mut paced);
        let mut decoder =
            CustodyFrameDecoderV1::new(reader, &header, budget).map_err(|_| refuse())?;
        loop {
            before_deadline(deadline)?;
            let Some(entry) = decoder.next_entry().map_err(|_| refuse())? else {
                break;
            };
            let (path, kind, length, content) = match entry {
                CustodyFrameEntryV1::Directory { path, .. } => {
                    (path, ActiveEntryKindV1::Directory, 0, None)
                }
                CustodyFrameEntryV1::Symlink { path, .. } => {
                    (path, ActiveEntryKindV1::Symlink, 0, None)
                }
                CustodyFrameEntryV1::Regular {
                    path,
                    length,
                    content,
                    ..
                } => (path, ActiveEntryKindV1::Regular, length, Some(content)),
            };
            #[cfg(test)]
            run_git_hook(
                GitHookPointV1::FrameEntry,
                "",
                Path::new(OsStr::from_bytes(path.as_bytes())),
            );
            if fault.is_none() {
                match table.admit(class, path.as_bytes(), kind) {
                    Ok(()) => {
                        counts.entries = counts.entries.saturating_add(1);
                        match kind {
                            ActiveEntryKindV1::Directory => {
                                counts.directories = counts.directories.saturating_add(1);
                            }
                            _ => {
                                counts.regular_files = counts.regular_files.saturating_add(1);
                                counts.content_bytes = counts.content_bytes.saturating_add(length);
                            }
                        }
                        entry_digest.record(kind, path.as_bytes(), length);
                    }
                    Err(found) => fault = Some(found),
                }
            }
            if let Some(mut content) = content {
                io::copy(&mut content, &mut io::sink()).map_err(|_| refuse())?;
            }
        }
    }
    // The decoder met the verified trailer and the end of the stream. The bytes it consumed must be
    // exactly the verified plaintext: a frame replaced after its rehash is refused here.
    let (count, context) = (paced.count, paced.context);
    let mut decoded_sha256 = [0_u8; 32];
    decoded_sha256.copy_from_slice(context.finish().as_ref());
    if count != one.length || decoded_sha256 != one.sha256 {
        return Err(GitBootstrapErrorV1::StagedInputChanged {
            name: lossy(one.name.as_bytes()),
        });
    }
    if let Some(fault) = fault {
        return Err(GitBootstrapErrorV1::ActiveFrame { class, fault });
    }
    observe_deadline(deadline, "frame-decoded")?;
    observe_deadline(deadline, "frame-rewound")?;
    one.file.seek(SeekFrom::Start(0)).map_err(|error| {
        GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Io(format!(
            "{}: {error}",
            lossy(one.name.as_bytes())
        )))
    })?;
    Ok(ActiveFrameSummaryV1 {
        class,
        staged: frame.staged,
        length: one.length,
        sha256: one.sha256,
        entries: counts.entries,
        directories: counts.directories,
        regular_files: counts.regular_files,
        content_bytes: counts.content_bytes,
        entries_sha256: entry_digest.finish(),
    })
}

/// Prevalidate the retained active frames of `verified`, before anything is written for the restored Git
/// plane: identify the `RefsAndHead` and `StashAndReflogs` payloads from the authenticated coverage and
/// the staged roles (a Captured class needs its one frame, an Empty one none, every other state
/// refuses); refuse an Empty `RefsAndHead` class with [`GitBootstrapErrorV1::EmptyRefsAndHead`], because
/// complete phase G needs a captured `HEAD` and synthesizes none; then, for every active frame, rehash
/// the retained descriptor, decode it completely through its verified trailer into a null sink, and
/// judge every entry (see [`ActivePathTableV1`]). A Captured refs class must hold exactly one `HEAD`
/// regular file. Ref grammar and agreement with the manifest belong to a later pass.
///
/// Nothing is written and nothing is charged: the one ledger, the pins, and the destination are exactly
/// as they were, whatever the outcome, and the caller keeps the one verified capsule. Every step keeps
/// the ONE absolute `deadline` the caller passes through the whole of phase G. On success each staged
/// descriptor is at offset 0. A refusal is in pass A's failure shape: it never began active
/// materialization, it carries `completed`, the last step the caller's flow completed whole (this pass
/// completes none), and the one ledger's charge.
pub(crate) fn prevalidate_active_frames_v1(
    verified: &mut VerifiedCapsuleV1,
    frame_budget: CustodyFrameBudgetV1,
    deadline: Instant,
    completed: Option<GitPlaneStepV1>,
) -> Result<ActiveFramePlanV1, GitBootstrapFailureV1> {
    prevalidate_active_frames_bounded_v1(
        verified,
        frame_budget,
        deadline,
        completed,
        ACTIVE_PATH_TABLE_BYTES_V1,
    )
}

/// [`prevalidate_active_frames_v1`] with the aggregate path table's byte bound as a parameter, so its
/// refusal is exercised at a small size.
pub(crate) fn prevalidate_active_frames_bounded_v1(
    verified: &mut VerifiedCapsuleV1,
    frame_budget: CustodyFrameBudgetV1,
    deadline: Instant,
    completed: Option<GitPlaneStepV1>,
    table_bound: u64,
) -> Result<ActiveFramePlanV1, GitBootstrapFailureV1> {
    prevalidate_frames(verified, frame_budget, deadline, table_bound)
        .map_err(|cause| pre_materialization_failure(verified, cause, completed))
}

/// A refusal in pass A's failure shape for a pass that writes nothing: it never began active
/// materialization, it carries `completed`, the last step the caller's flow completed whole, and the one
/// ledger's charge, read and never changed. Pass B1a and pass B1b share it.
fn pre_materialization_failure(
    verified: &VerifiedCapsuleV1,
    cause: GitBootstrapErrorV1,
    completed: Option<GitPlaneStepV1>,
) -> GitBootstrapFailureV1 {
    GitBootstrapFailureV1 {
        cause,
        last_completed: completed,
        active_materialization_began: false,
        ledger_used: verified.destination.ledger.borrow().used(),
    }
}

fn prevalidate_frames(
    verified: &mut VerifiedCapsuleV1,
    frame_budget: CustodyFrameBudgetV1,
    deadline: Instant,
    table_bound: u64,
) -> Result<ActiveFramePlanV1, GitBootstrapErrorV1> {
    observe_deadline(deadline, "frames-entry")?;
    let active = classify_active_frames(verified)?;
    if !active
        .iter()
        .any(|frame| frame.class == CustodyCoverageClassV1::RefsAndHead)
    {
        return Err(GitBootstrapErrorV1::EmptyRefsAndHead);
    }
    let generation = verified.control.manifest.generation_id().to_owned();
    let mut table = ActivePathTableV1::new(table_bound);
    let mut frames = Vec::with_capacity(active.len());
    for frame in &active {
        let one = verified.staged.get_mut(frame.staged).ok_or_else(|| {
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Io(
                "an active frame names no staged payload".into(),
            ))
        })?;
        frames.push(prevalidate_frame(
            one,
            frame,
            &generation,
            frame_budget,
            deadline,
            &mut table,
        )?);
    }
    table
        .finish()
        .map_err(|fault| GitBootstrapErrorV1::ActiveFrame {
            class: CustodyCoverageClassV1::RefsAndHead,
            fault,
        })?;
    // The last statement ahead of the infallible result.
    observe_deadline(deadline, "frames-final")?;
    Ok(ActiveFramePlanV1 { frames })
}

// ---------------------------------------------------------------------------------------------
// Pass B1b: pure refs agreement and inert config
// ---------------------------------------------------------------------------------------------

/// Git's own bound on the symbolic refs one lookup may follow (`SYMREF_MAXDEPTH`): a chain of five
/// symbolic refs ending in a value (or in a missing ref) resolves, a sixth refuses.
const REFS_SYMREF_HOPS_V1: usize = 5;
const REFS_BUFFER_BYTES_V1: usize = 8 * 1024;

/// The bounds of one refs agreement. Every input is read through them: no line, file, record count, or
/// retained byte count can exceed its cap, so no input is buffered whole without admission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RefsLimitsV1 {
    /// The longest one line may be, without its LF.
    pub(crate) line_bytes: u64,
    /// `HEAD`, `ORIG_HEAD`, and each loose ref file.
    pub(crate) loose_file_bytes: u64,
    pub(crate) packed_file_bytes: u64,
    pub(crate) fetch_head_bytes: u64,
    /// One reflog file; a reflog is read a row at a time and never retained.
    pub(crate) log_file_bytes: u64,
    /// Admitted records: loose files (`HEAD` and `ORIG_HEAD` included), packed rows, peeled rows,
    /// `FETCH_HEAD` rows, and reflog files.
    pub(crate) entries: u64,
    /// What the retained refs may cost in all, by [`refs_entry_cost_v1`].
    pub(crate) retained_bytes: u64,
}

impl RefsLimitsV1 {
    pub(crate) const fn v1() -> Self {
        Self {
            line_bytes: 8 * 1024,
            loose_file_bytes: 16 * 1024,
            packed_file_bytes: 64 * 1024 * 1024,
            fetch_head_bytes: 1024 * 1024,
            log_file_bytes: 64 * 1024 * 1024,
            entries: 262_144,
            retained_bytes: 64 * 1024 * 1024,
        }
    }
}

/// What one retained record costs: two copies of its name (its map key and its case key), its value, and
/// a fixed allowance for the map's bookkeeping.
pub(crate) fn refs_entry_cost_v1(name_len: usize, value_len: usize) -> u64 {
    ACTIVE_ENTRY_OVERHEAD_BYTES_V1
        .saturating_add(2 * name_len as u64)
        .saturating_add(value_len as u64)
}

/// Which cap a refusal names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RefsLimitKindV1 {
    Line,
    LooseFile,
    PackedFile,
    FetchHeadFile,
    LogFile,
    Entries,
    Retained,
}

/// Why a name is not a ref name: it is under `refs/`, has no empty component, follows the portable ASCII
/// spelling rule, and follows Git's check-ref-format rules.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RefNameFaultV1 {
    #[error("not under refs/")]
    NotUnderRefs,
    #[error("an empty component")]
    EmptyComponent,
    #[error("a component starts with a dot")]
    LeadingDot,
    #[error("a component ends in .lock")]
    LockSuffix,
    #[error("contains ..")]
    DoubleDot,
    #[error("contains a byte Git forbids in a ref name")]
    ForbiddenByte,
    #[error("contains @{{")]
    AtBrace,
    #[error("not a portable spelling ({0})")]
    Portable(PortableFaultV1),
}

/// Why one file's text is not the grammar of its kind.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RowFaultV1 {
    #[error("the file is empty")]
    EmptyFile,
    #[error("a line is not terminated by LF")]
    PartialLine,
    #[error("a one-line file has more")]
    ExtraLine,
    #[error("not a lowercase object id of the format's width")]
    ObjectId,
    #[error("the symbolic target is not a ref name ({0})")]
    SymbolicTarget(RefNameFaultV1),
    #[error("a malformed packed-refs header")]
    PackedHeader,
    #[error("a packed row is not `<oid> <name>`")]
    PackedRow,
    #[error("rows are not in order under the `sorted` trait")]
    PackedOrder,
    #[error("a name is packed twice")]
    PackedDuplicate,
    #[error("a peeled row does not follow a ref row")]
    PeeledOrphan,
    #[error("a malformed FETCH_HEAD row")]
    FetchHeadRow,
    #[error("a malformed reflog row")]
    LogRow,
}

/// Why the refs are refused. Paths and names are rendered lossily, for the message only.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RefsFaultV1 {
    #[error("{path}: not a refs file of the active subset")]
    UnexpectedFile { path: String },
    #[error("{path}: admitted twice")]
    Duplicate { path: String },
    #[error("{path}: not a valid ref name ({why})")]
    RefName { path: String, why: RefNameFaultV1 },
    #[error("{path}: {why}")]
    Row { path: String, why: RowFaultV1 },
    #[error("a {what:?} limit of {bound} was exceeded")]
    Limit { what: RefsLimitKindV1, bound: u64 },
    #[error("{path}: the source could not be read")]
    Unreadable { path: String },
    #[error("no valid HEAD was captured")]
    MissingHead,
    #[error("{name}: conflicts with the ref {with} above it")]
    Conflict { name: String, with: String },
    #[error("{name}: collides with {with} on a portable filesystem")]
    Collision { name: String, with: String },
    #[error("{name}: its symbolic chain is a cycle")]
    Cycle { name: String },
    #[error("{name}: its symbolic chain is longer than {REFS_SYMREF_HOPS_V1} refs")]
    TooDeep { name: String },
    #[error("{name}: names {target}, which does not exist")]
    Dangling { name: String, target: String },
    #[error("{name}: names the object {oid}, which is not in the verified inventory")]
    TargetAbsent { name: String, oid: String },
    #[error("{name}: the object {oid} is a {found}, not {expected}")]
    TargetKind {
        name: String,
        oid: String,
        found: &'static str,
        expected: &'static str,
    },
    #[error("{name}: a peeled row follows an object that is not a tag")]
    PeeledNotTag { name: String },
    #[error("{name}: a tag has no peeled row under the fully-peeled trait")]
    PeelMissing { name: String },
    #[error("{name}: the peeled object is itself a tag")]
    PeeledIsTag { name: String },
    #[error("the manifest's ref {name} is not a valid name ({why})")]
    ManifestName { name: String, why: RefNameFaultV1 },
    #[error("the manifest's ref {name} names an invalid symbolic target {target} ({why})")]
    ManifestTarget {
        name: String,
        target: String,
        why: RefNameFaultV1,
    },
    #[error("the manifest's ref {name} was not restored")]
    MissingOriginal { name: String },
    #[error("the manifest's ref {name} is {expected}, and the restored one is {found}")]
    OriginalMismatch {
        name: String,
        expected: String,
        found: String,
    },
}

fn refs_fault(fault: RefsFaultV1) -> GitBootstrapErrorV1 {
    GitBootstrapErrorV1::Refs(fault)
}

fn row_fault(path: &str, why: RowFaultV1) -> GitBootstrapErrorV1 {
    refs_fault(RefsFaultV1::Row {
        path: path.to_owned(),
        why,
    })
}

fn limit_fault(what: RefsLimitKindV1, bound: u64) -> GitBootstrapErrorV1 {
    refs_fault(RefsFaultV1::Limit { what, bound })
}

/// The ref-name rule for every name under `refs/`: at least one component after `refs/`, no empty
/// component, the portable spelling rule of this slice, and Git's check-ref-format rules: no component
/// that starts with a dot or ends in `.lock`, no `..`, none of `~ ^ : ? * [`, and no `@{`.
pub(crate) fn check_ref_name_v1(name: &[u8]) -> Result<(), RefNameFaultV1> {
    if !name.starts_with(b"refs/") {
        return Err(RefNameFaultV1::NotUnderRefs);
    }
    if name.split(|byte| *byte == b'/').any(<[u8]>::is_empty) {
        return Err(RefNameFaultV1::EmptyComponent);
    }
    portable_path(name).map_err(RefNameFaultV1::Portable)?;
    for component in name.split(|byte| *byte == b'/') {
        if component.starts_with(b".") {
            return Err(RefNameFaultV1::LeadingDot);
        }
        if component.ends_with(b".lock") {
            return Err(RefNameFaultV1::LockSuffix);
        }
    }
    if name.windows(2).any(|pair| pair == b"..") {
        return Err(RefNameFaultV1::DoubleDot);
    }
    if name.iter().any(|byte| b"~^:?*[".contains(byte)) {
        return Err(RefNameFaultV1::ForbiddenByte);
    }
    if name.windows(2).any(|pair| pair == b"@{") {
        return Err(RefNameFaultV1::AtBrace);
    }
    Ok(())
}

/// A deadline-paced, byte-capped source: every read observes the ONE deadline first, and at most `cap + 1`
/// bytes are ever requested from the source underneath.
struct PacedSourceV1<'a> {
    inner: io::Take<&'a mut dyn io::Read>,
    deadline: Instant,
    expired: &'a Cell<bool>,
}

impl io::Read for PacedSourceV1<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if before_deadline(self.deadline).is_err() {
            self.expired.set(true);
            return Err(io::Error::new(
                ErrorKind::TimedOut,
                "the phase-G deadline passed",
            ));
        }
        self.inner.read(buffer)
    }
}

/// One file read a line at a time. A line is at most `line_cap` bytes without its LF and the file at most
/// `file_cap` bytes; a source error, a deadline, a partial final line, and either cap are each refused
/// with their own type, and nothing is read once any of them is.
struct LinesV1<'a> {
    reader: BufReader<PacedSourceV1<'a>>,
    expired: &'a Cell<bool>,
    path: &'a str,
    file_cap: u64,
    file_kind: RefsLimitKindV1,
    line_cap: u64,
    consumed: u64,
}

impl<'a> LinesV1<'a> {
    fn new(
        source: &'a mut dyn io::Read,
        expired: &'a Cell<bool>,
        deadline: Instant,
        path: &'a str,
        file_cap: u64,
        file_kind: RefsLimitKindV1,
        line_cap: u64,
    ) -> Self {
        let paced = PacedSourceV1 {
            inner: source.take(file_cap.saturating_add(1)),
            deadline,
            expired,
        };
        Self {
            reader: BufReader::with_capacity(REFS_BUFFER_BYTES_V1, paced),
            expired,
            path,
            file_cap,
            file_kind,
            line_cap,
            consumed: 0,
        }
    }

    fn read_error(&self) -> GitBootstrapErrorV1 {
        if self.expired.get() {
            GitBootstrapErrorV1::Git(CustodyGitError::Timeout)
        } else {
            refs_fault(RefsFaultV1::Unreadable {
                path: self.path.to_owned(),
            })
        }
    }

    /// The next line without its LF, or `None` at the end of the file.
    fn next_line(&mut self) -> Result<Option<Vec<u8>>, GitBootstrapErrorV1> {
        let room = self.line_cap.saturating_add(2);
        let mut line = Vec::new();
        let read = io::BufRead::read_until(&mut (&mut self.reader).take(room), b'\n', &mut line)
            .map_err(|_| self.read_error())? as u64;
        if read == 0 {
            return Ok(None);
        }
        self.consumed = self.consumed.saturating_add(read);
        if self.consumed > self.file_cap {
            return Err(limit_fault(self.file_kind, self.file_cap));
        }
        if line.last() == Some(&b'\n') {
            line.pop();
            if line.len() as u64 > self.line_cap {
                return Err(limit_fault(RefsLimitKindV1::Line, self.line_cap));
            }
            return Ok(Some(line));
        }
        if read >= room {
            return Err(limit_fault(RefsLimitKindV1::Line, self.line_cap));
        }
        Err(row_fault(self.path, RowFaultV1::PartialLine))
    }

    /// Whether any byte remains in the file.
    fn has_more(&mut self) -> Result<bool, GitBootstrapErrorV1> {
        io::BufRead::fill_buf(&mut self.reader)
            .map(|buffer| !buffer.is_empty())
            .map_err(|_| self.read_error())
    }
}

/// A restored ref's own value: a direct object id, or the name of the ref it points to.
#[derive(Clone, Debug, PartialEq, Eq)]
enum RefValueV1 {
    Direct(String),
    Symbolic(Vec<u8>),
}

impl RefValueV1 {
    fn len(&self) -> usize {
        match self {
            Self::Direct(oid) => oid.len(),
            Self::Symbolic(target) => target.len(),
        }
    }

    fn found(&self) -> FoundV1<'_> {
        match self {
            Self::Direct(oid) => FoundV1::Direct(oid),
            Self::Symbolic(target) => FoundV1::Symbolic(target),
        }
    }
}

/// A ref's value, borrowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FoundV1<'a> {
    Direct(&'a str),
    Symbolic(&'a [u8]),
}

impl FoundV1<'_> {
    fn describe(&self) -> String {
        match self {
            Self::Direct(oid) => format!("direct {oid}"),
            Self::Symbolic(target) => format!("symbolic {}", lossy(target)),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct PackedHeaderV1 {
    peeled: bool,
    fully_peeled: bool,
    sorted: bool,
}

#[derive(Debug)]
struct PackedRowV1 {
    oid: String,
    peeled: Option<String>,
}

#[derive(Debug, Default)]
struct PackedRefsV1 {
    header: PackedHeaderV1,
    rows: BTreeMap<Vec<u8>, PackedRowV1>,
}

/// `# pack-refs with:` and then any of the three traits, each at most once, each preceded by a space;
/// Git writes a trailing space after the last.
fn parse_packed_header(line: &[u8]) -> Option<PackedHeaderV1> {
    let rest = line.strip_prefix(b"# pack-refs with:")?;
    if rest.first().is_some_and(|byte| *byte != b' ') {
        return None;
    }
    let mut header = PackedHeaderV1::default();
    for token in rest
        .split(|byte| *byte == b' ')
        .filter(|token| !token.is_empty())
    {
        let flag = match token {
            b"peeled" => &mut header.peeled,
            b"fully-peeled" => &mut header.fully_peeled,
            b"sorted" => &mut header.sorted,
            _ => return None,
        };
        if std::mem::replace(flag, true) {
            return None;
        }
    }
    Some(header)
}

/// One reflog row: `<old> <new> <identity> <timestamp> <zone>` and optionally a TAB and a message, with
/// both object ids at the format's width, an identity that ends in `>` after a `<`, digits for the
/// timestamp, and a signed four-digit zone. The message is free text without a NUL.
fn is_log_row(line: &[u8], format: GitObjectFormatV1) -> bool {
    let width = hex_width(format);
    let header = line
        .iter()
        .position(|byte| *byte == b'\t')
        .map_or(line, |tab| &line[..tab]);
    if line.contains(&0) || header.len() < 2 * width + 2 {
        return false;
    }
    let hex = |text: &[u8]| std::str::from_utf8(text).is_ok_and(|text| is_object_id(text, format));
    if !hex(&header[..width])
        || header[width] != b' '
        || !hex(&header[width + 1..2 * width + 1])
        || header[2 * width + 1] != b' '
    {
        return false;
    }
    let rest = &header[2 * width + 2..];
    let Some(zone_at) = rest.iter().rposition(|byte| *byte == b' ') else {
        return false;
    };
    let (rest, zone) = (&rest[..zone_at], &rest[zone_at + 1..]);
    let Some(stamp_at) = rest.iter().rposition(|byte| *byte == b' ') else {
        return false;
    };
    let (identity, stamp) = (&rest[..stamp_at], &rest[stamp_at + 1..]);
    let zone_ok = zone.len() == 5
        && (zone[0] == b'+' || zone[0] == b'-')
        && zone[1..].iter().all(u8::is_ascii_digit);
    let stamp_ok = !stamp.is_empty() && stamp.len() <= 20 && stamp.iter().all(u8::is_ascii_digit);
    zone_ok && stamp_ok && identity.last() == Some(&b'>') && identity.contains(&b'<')
}

/// What a name means to the pure builder: which file of the active subset it is.
enum RouteV1<'a> {
    Head,
    OrigHead,
    FetchHead,
    Packed,
    /// A loose ref, by its full name under `refs/`.
    Loose(&'a [u8]),
    /// A reflog, by the name of the ref it logs (`HEAD`, or a name under `refs/`).
    Log(&'a [u8]),
}

fn route_refs_file(path: &[u8]) -> Option<RouteV1<'_>> {
    match path {
        b"HEAD" => Some(RouteV1::Head),
        b"ORIG_HEAD" => Some(RouteV1::OrigHead),
        b"FETCH_HEAD" => Some(RouteV1::FetchHead),
        b"packed-refs" => Some(RouteV1::Packed),
        b"logs/HEAD" => Some(RouteV1::Log(b"HEAD")),
        _ if path.starts_with(b"refs/") => Some(RouteV1::Loose(path)),
        _ => path
            .strip_prefix(b"logs/")
            .filter(|name| name.starts_with(b"refs/"))
            .map(RouteV1::Log),
    }
}

/// The object kinds the refs rules require: a branch, a detached or resolved HEAD, and `ORIG_HEAD` name a
/// commit.
fn required_kind(name: &[u8]) -> Option<&'static str> {
    (name == b"HEAD" || name == b"ORIG_HEAD" || name.starts_with(b"refs/heads/"))
        .then_some("commit")
}

/// What resolving the symbolic refs found: the commit HEAD resolved to (none when unborn), and the names
/// whose chains end at a ref that does not exist.
#[derive(Debug, Default)]
struct ResolutionV1 {
    head: Option<String>,
    unborn: BTreeSet<Vec<u8>>,
}

/// The pure refs foundation. Files of the active subset are admitted one at a time, each read through its
/// caps and the one deadline and parsed in a single streaming pass; [`Self::finish`] then judges the
/// whole. It runs no Git, reads no configuration, opens no path, and holds only bounded, accounted state.
#[derive(Debug)]
pub(crate) struct RefsBuilderV1 {
    format: GitObjectFormatV1,
    limits: RefsLimitsV1,
    deadline: Instant,
    entries: u64,
    retained: u64,
    head: Option<RefValueV1>,
    orig_head: Option<String>,
    fetch_head: Option<Vec<String>>,
    packed: Option<PackedRefsV1>,
    loose: BTreeMap<Vec<u8>, RefValueV1>,
    logs: BTreeSet<Vec<u8>>,
    log_rows: u64,
    peeled_rows: u64,
}

impl RefsBuilderV1 {
    pub(crate) fn new(format: GitObjectFormatV1, limits: RefsLimitsV1, deadline: Instant) -> Self {
        Self {
            format,
            limits,
            deadline,
            entries: 0,
            retained: 0,
            head: None,
            orig_head: None,
            fetch_head: None,
            packed: None,
            loose: BTreeMap::new(),
            logs: BTreeSet::new(),
            log_rows: 0,
            peeled_rows: 0,
        }
    }

    fn count_entry(&mut self) -> Result<(), GitBootstrapErrorV1> {
        if self.entries >= self.limits.entries {
            return Err(limit_fault(RefsLimitKindV1::Entries, self.limits.entries));
        }
        self.entries += 1;
        Ok(())
    }

    fn charge(&mut self, bytes: u64) -> Result<(), GitBootstrapErrorV1> {
        let total = self.retained.saturating_add(bytes);
        if total > self.limits.retained_bytes {
            return Err(limit_fault(
                RefsLimitKindV1::Retained,
                self.limits.retained_bytes,
            ));
        }
        self.retained = total;
        Ok(())
    }

    fn object_id(&self, text: &[u8]) -> Option<String> {
        std::str::from_utf8(text)
            .ok()
            .filter(|text| is_object_id(text, self.format))
            .map(str::to_owned)
    }

    /// Admit one regular file of the active subset, read from `reader`. The deadline is observed before
    /// the first read and before every read after it, and no more than the file's cap is ever requested.
    pub(crate) fn admit_file(
        &mut self,
        path: &[u8],
        reader: &mut dyn io::Read,
    ) -> Result<(), GitBootstrapErrorV1> {
        before_deadline(self.deadline)?;
        let display = lossy(path);
        let Some(route) = route_refs_file(path) else {
            return Err(refs_fault(RefsFaultV1::UnexpectedFile { path: display }));
        };
        let duplicate = || refs_fault(RefsFaultV1::Duplicate { path: lossy(path) });
        let (cap, kind) = match route {
            RouteV1::Packed => (self.limits.packed_file_bytes, RefsLimitKindV1::PackedFile),
            RouteV1::FetchHead => (self.limits.fetch_head_bytes, RefsLimitKindV1::FetchHeadFile),
            RouteV1::Log(_) => (self.limits.log_file_bytes, RefsLimitKindV1::LogFile),
            _ => (self.limits.loose_file_bytes, RefsLimitKindV1::LooseFile),
        };
        match &route {
            RouteV1::Head if self.head.is_some() => return Err(duplicate()),
            RouteV1::OrigHead if self.orig_head.is_some() => return Err(duplicate()),
            RouteV1::FetchHead if self.fetch_head.is_some() => return Err(duplicate()),
            RouteV1::Packed if self.packed.is_some() => return Err(duplicate()),
            RouteV1::Loose(name) if self.loose.contains_key(*name) => return Err(duplicate()),
            RouteV1::Log(_) if self.logs.contains(path) => return Err(duplicate()),
            RouteV1::Loose(name) => {
                check_ref_name_v1(name).map_err(|why| {
                    refs_fault(RefsFaultV1::RefName {
                        path: display.clone(),
                        why,
                    })
                })?;
            }
            RouteV1::Log(name) if *name != b"HEAD" => {
                check_ref_name_v1(name).map_err(|why| {
                    refs_fault(RefsFaultV1::RefName {
                        path: lossy(name),
                        why,
                    })
                })?;
            }
            _ => {}
        }
        // A file is one record, except packed-refs and FETCH_HEAD, whose rows are the records.
        if !matches!(route, RouteV1::Packed | RouteV1::FetchHead) {
            self.count_entry()?;
        }
        let expired = Cell::new(false);
        let mut lines = LinesV1::new(
            reader,
            &expired,
            self.deadline,
            &display,
            cap,
            kind,
            self.limits.line_bytes,
        );
        match route {
            RouteV1::Head => {
                let value = self.parse_loose(&mut lines, true)?;
                self.charge(refs_entry_cost_v1(path.len(), value.len()))?;
                self.head = Some(value);
            }
            RouteV1::OrigHead => {
                let RefValueV1::Direct(oid) = self.parse_loose(&mut lines, false)? else {
                    return Err(row_fault(&display, RowFaultV1::ObjectId));
                };
                self.charge(refs_entry_cost_v1(path.len(), oid.len()))?;
                self.orig_head = Some(oid);
            }
            RouteV1::Loose(name) => {
                let value = self.parse_loose(&mut lines, true)?;
                self.charge(refs_entry_cost_v1(name.len(), value.len()))?;
                self.loose.insert(name.to_vec(), value);
            }
            RouteV1::Packed => {
                let packed = self.parse_packed(&mut lines)?;
                self.packed = Some(packed);
            }
            RouteV1::FetchHead => {
                let rows = self.parse_fetch_head(&mut lines)?;
                self.fetch_head = Some(rows);
            }
            RouteV1::Log(_) => {
                self.charge(refs_entry_cost_v1(path.len(), 0))?;
                self.parse_log(&mut lines)?;
                self.logs.insert(path.to_vec());
            }
        }
        Ok(())
    }

    /// One line: a direct object id, or (when `symbolic` is allowed) `ref: ` and a full ref name.
    fn parse_loose(
        &self,
        lines: &mut LinesV1<'_>,
        symbolic: bool,
    ) -> Result<RefValueV1, GitBootstrapErrorV1> {
        let path = lines.path;
        let Some(line) = lines.next_line()? else {
            return Err(row_fault(path, RowFaultV1::EmptyFile));
        };
        let value = match line.strip_prefix(b"ref: ").filter(|_| symbolic) {
            Some(target) => {
                check_ref_name_v1(target)
                    .map_err(|why| row_fault(path, RowFaultV1::SymbolicTarget(why)))?;
                RefValueV1::Symbolic(target.to_vec())
            }
            None => RefValueV1::Direct(
                self.object_id(&line)
                    .ok_or_else(|| row_fault(path, RowFaultV1::ObjectId))?,
            ),
        };
        if lines.has_more()? {
            return Err(row_fault(path, RowFaultV1::ExtraLine));
        }
        Ok(value)
    }

    fn parse_packed(
        &mut self,
        lines: &mut LinesV1<'_>,
    ) -> Result<PackedRefsV1, GitBootstrapErrorV1> {
        let path = lines.path;
        let width = hex_width(self.format);
        let mut packed = PackedRefsV1::default();
        let mut first = true;
        let mut peelable: Option<Vec<u8>> = None;
        let mut previous: Option<Vec<u8>> = None;
        while let Some(line) = lines.next_line()? {
            let at_start = std::mem::replace(&mut first, false);
            if at_start && line.starts_with(b"# pack-refs with:") {
                packed.header = parse_packed_header(&line)
                    .ok_or_else(|| row_fault(path, RowFaultV1::PackedHeader))?;
                continue;
            }
            if line.first() == Some(&b'#') {
                return Err(row_fault(path, RowFaultV1::PackedHeader));
            }
            self.count_entry()?;
            if let Some(oid) = line.strip_prefix(b"^") {
                let Some(name) = peelable.take() else {
                    return Err(row_fault(path, RowFaultV1::PeeledOrphan));
                };
                let oid = self
                    .object_id(oid)
                    .ok_or_else(|| row_fault(path, RowFaultV1::ObjectId))?;
                self.charge(width as u64)?;
                self.peeled_rows += 1;
                if let Some(row) = packed.rows.get_mut(&name) {
                    row.peeled = Some(oid);
                }
                continue;
            }
            let split = line
                .iter()
                .position(|byte| *byte == b' ')
                .ok_or_else(|| row_fault(path, RowFaultV1::PackedRow))?;
            let (oid, name) = (&line[..split], &line[split + 1..]);
            let oid = self
                .object_id(oid)
                .ok_or_else(|| row_fault(path, RowFaultV1::ObjectId))?;
            check_ref_name_v1(name).map_err(|why| {
                refs_fault(RefsFaultV1::RefName {
                    path: lossy(name),
                    why,
                })
            })?;
            if packed.header.sorted {
                if previous.as_deref().is_some_and(|before| name <= before) {
                    return Err(row_fault(path, RowFaultV1::PackedOrder));
                }
            } else if packed.rows.contains_key(name) {
                return Err(row_fault(path, RowFaultV1::PackedDuplicate));
            }
            self.charge(refs_entry_cost_v1(name.len(), width))?;
            packed
                .rows
                .insert(name.to_vec(), PackedRowV1 { oid, peeled: None });
            previous = Some(name.to_vec());
            peelable = Some(name.to_vec());
        }
        Ok(packed)
    }

    /// `<oid> TAB (empty | not-for-merge) TAB <note>` per row; the note is free text without a NUL.
    fn parse_fetch_head(
        &mut self,
        lines: &mut LinesV1<'_>,
    ) -> Result<Vec<String>, GitBootstrapErrorV1> {
        let path = lines.path;
        let width = hex_width(self.format);
        let mut rows = Vec::new();
        while let Some(line) = lines.next_line()? {
            self.count_entry()?;
            let malformed = || row_fault(path, RowFaultV1::FetchHeadRow);
            if line.contains(&0) {
                return Err(malformed());
            }
            let first = line
                .iter()
                .position(|byte| *byte == b'\t')
                .ok_or_else(malformed)?;
            let rest = &line[first + 1..];
            let second = rest
                .iter()
                .position(|byte| *byte == b'\t')
                .ok_or_else(malformed)?;
            let oid = self
                .object_id(&line[..first])
                .ok_or_else(|| row_fault(path, RowFaultV1::ObjectId))?;
            if !matches!(&rest[..second], b"" | b"not-for-merge") {
                return Err(malformed());
            }
            self.charge(refs_entry_cost_v1(0, width))?;
            rows.push(oid);
        }
        Ok(rows)
    }

    /// Reflog rows are validated as they stream by and never retained.
    fn parse_log(&mut self, lines: &mut LinesV1<'_>) -> Result<(), GitBootstrapErrorV1> {
        let path = lines.path;
        while let Some(line) = lines.next_line()? {
            if !is_log_row(&line, self.format) {
                return Err(row_fault(path, RowFaultV1::LogRow));
            }
            self.log_rows += 1;
        }
        Ok(())
    }

    /// A ref's own value by name: a loose ref wins over a packed one of the same name.
    fn effective(&self, name: &[u8]) -> Option<FoundV1<'_>> {
        match self.loose.get(name) {
            Some(value) => Some(value.found()),
            None => self
                .packed
                .as_ref()
                .and_then(|packed| packed.rows.get(name))
                .map(|row| FoundV1::Direct(&row.oid)),
        }
    }

    /// The value of a manifest ref's name, where the three pseudorefs have their own files.
    fn found(&self, name: &[u8]) -> Option<FoundV1<'_>> {
        match name {
            b"HEAD" => self.head.as_ref().map(RefValueV1::found),
            b"ORIG_HEAD" => self.orig_head.as_deref().map(FoundV1::Direct),
            b"FETCH_HEAD" => self
                .fetch_head
                .as_ref()
                .and_then(|rows| rows.first())
                .map(|oid| FoundV1::Direct(oid)),
            _ => self.effective(name),
        }
    }

    /// Every name a ref has, loose or packed, once.
    fn names(&self) -> BTreeSet<&[u8]> {
        self.loose
            .keys()
            .map(Vec::as_slice)
            .chain(
                self.packed
                    .iter()
                    .flat_map(|packed| packed.rows.keys().map(Vec::as_slice)),
            )
            .collect()
    }

    /// A ref and a ref under it conflict, and spellings that differ only by ASCII case collide.
    fn check_structure(&self) -> Result<(), GitBootstrapErrorV1> {
        let names = self.names();
        let mut keys: BTreeMap<Vec<u8>, &[u8]> = BTreeMap::new();
        for name in &names {
            for (at, byte) in name.iter().enumerate() {
                if *byte == b'/' && names.contains(&name[..at]) {
                    return Err(refs_fault(RefsFaultV1::Conflict {
                        name: lossy(name),
                        with: lossy(&name[..at]),
                    }));
                }
            }
            if let Some(earlier) = keys.insert(name.to_ascii_lowercase(), name) {
                return Err(refs_fault(RefsFaultV1::Collision {
                    name: lossy(name),
                    with: lossy(earlier),
                }));
            }
        }
        Ok(())
    }

    /// One direct target: in the inventory, of a known kind, and of the kind its name requires.
    fn check_target<'i>(
        &self,
        name: &[u8],
        oid: &str,
        inventory: &'i BTreeMap<String, &'static str>,
    ) -> Result<&'i str, GitBootstrapErrorV1> {
        let found = inventory.get(oid).copied().ok_or_else(|| {
            refs_fault(RefsFaultV1::TargetAbsent {
                name: lossy(name),
                oid: oid.to_owned(),
            })
        })?;
        let wrong = |expected: &'static str| {
            refs_fault(RefsFaultV1::TargetKind {
                name: lossy(name),
                oid: oid.to_owned(),
                found,
                expected,
            })
        };
        if found == "unknown" {
            return Err(wrong("a known kind"));
        }
        match required_kind(name) {
            Some(expected) if expected != found => Err(wrong(expected)),
            _ => Ok(found),
        }
    }

    /// Every ACTIVE direct target the restored refs carry, wherever it comes from, against the inventory:
    /// everything Git can resolve (so not a packed row that a loose ref shadows).
    fn check_targets(
        &self,
        inventory: &BTreeMap<String, &'static str>,
    ) -> Result<u64, GitBootstrapErrorV1> {
        let mut checked: BTreeSet<&str> = BTreeSet::new();
        if let Some(RefValueV1::Direct(oid)) = &self.head {
            self.check_target(b"HEAD", oid, inventory)?;
            checked.insert(oid);
        }
        if let Some(oid) = &self.orig_head {
            self.check_target(b"ORIG_HEAD", oid, inventory)?;
            checked.insert(oid);
        }
        for oid in self.fetch_head.iter().flatten() {
            self.check_target(b"FETCH_HEAD", oid, inventory)?;
            checked.insert(oid);
        }
        for (name, value) in &self.loose {
            if let RefValueV1::Direct(oid) = value {
                self.check_target(name, oid, inventory)?;
                checked.insert(oid);
            }
        }
        if let Some(packed) = &self.packed {
            for (name, row) in &packed.rows {
                // A loose ref of the same name hides this row: Git never resolves it, so it is not an
                // active target. Its grammar was judged as it streamed in; its object, kind, and peeled
                // value are judged only when nothing shadows it (a pruned stale object is routine).
                if self.loose.contains_key(name) {
                    continue;
                }
                let kind = self.check_target(name, &row.oid, inventory)?;
                checked.insert(&row.oid);
                let tag = || refs_fault(RefsFaultV1::PeeledNotTag { name: lossy(name) });
                match &row.peeled {
                    Some(peeled) => {
                        if kind != "tag" {
                            return Err(tag());
                        }
                        if self.check_target(name, peeled, inventory)? == "tag" {
                            return Err(refs_fault(RefsFaultV1::PeeledIsTag { name: lossy(name) }));
                        }
                        checked.insert(peeled);
                    }
                    None if packed.header.fully_peeled && kind == "tag" => {
                        return Err(refs_fault(RefsFaultV1::PeelMissing { name: lossy(name) }));
                    }
                    None => {}
                }
            }
        }
        Ok(checked.len() as u64)
    }

    /// Follow one symbolic ref: at most [`REFS_SYMREF_HOPS_V1`] refs, a cycle refused as a cycle. The result
    /// is the object it resolves to, or the name of the missing ref its chain ends at.
    fn resolve<'s>(
        &'s self,
        start: &'s [u8],
        first: FoundV1<'s>,
    ) -> Result<Result<&'s str, &'s [u8]>, GitBootstrapErrorV1> {
        let mut seen: Vec<&[u8]> = vec![start];
        let mut value = first;
        let mut hops = 0;
        loop {
            match value {
                FoundV1::Direct(oid) => return Ok(Ok(oid)),
                FoundV1::Symbolic(target) => {
                    if seen.contains(&target) {
                        return Err(refs_fault(RefsFaultV1::Cycle { name: lossy(start) }));
                    }
                    if hops == REFS_SYMREF_HOPS_V1 {
                        return Err(refs_fault(RefsFaultV1::TooDeep { name: lossy(start) }));
                    }
                    hops += 1;
                    seen.push(target);
                    match self.effective(target) {
                        Some(next) => value = next,
                        None => return Ok(Err(target)),
                    }
                }
            }
        }
    }

    /// Resolve HEAD and every loose symbolic ref. A resolved ref must name an object of the kind its name
    /// requires; a chain that ends at a missing ref is unborn, which only HEAD, or a ref the manifest
    /// calls `Unborn`, may be.
    fn check_resolution(
        &self,
        manifest: &CustodyManifestV1,
        inventory: &BTreeMap<String, &'static str>,
    ) -> Result<ResolutionV1, GitBootstrapErrorV1> {
        let declared: BTreeSet<&[u8]> = manifest
            .original_refs()
            .iter()
            .filter(|record| matches!(record.target(), CustodyRefTargetV1::Unborn))
            .map(|record| record.name().as_bytes())
            .collect();
        let mut resolution = ResolutionV1::default();
        let starts = self
            .head
            .iter()
            .map(|head| (&b"HEAD"[..], head.found()))
            .chain(
                self.loose
                    .iter()
                    .map(|(name, value)| (name.as_slice(), value.found())),
            )
            .filter(|(_, value)| matches!(value, FoundV1::Symbolic(_)));
        for (name, value) in starts {
            match self.resolve(name, value)? {
                Ok(oid) => {
                    let found = inventory.get(oid).copied().unwrap_or("unknown");
                    if let Some(expected) = required_kind(name).filter(|kind| *kind != found) {
                        return Err(refs_fault(RefsFaultV1::TargetKind {
                            name: lossy(name),
                            oid: oid.to_owned(),
                            found,
                            expected,
                        }));
                    }
                    if name == b"HEAD" {
                        resolution.head = Some(oid.to_owned());
                    }
                }
                Err(missing) => {
                    if name != b"HEAD" && !declared.contains(name) {
                        return Err(refs_fault(RefsFaultV1::Dangling {
                            name: lossy(name),
                            target: lossy(missing),
                        }));
                    }
                    resolution.unborn.insert(name.to_vec());
                }
            }
        }
        Ok(resolution)
    }

    /// Every manifest ref, by name and by target: present, valid, and the very value the manifest records.
    fn check_manifest(
        &self,
        manifest: &CustodyManifestV1,
        unborn: &BTreeSet<Vec<u8>>,
    ) -> Result<u64, GitBootstrapErrorV1> {
        for record in manifest.original_refs() {
            let name = record.name().as_bytes();
            if !matches!(name, b"HEAD" | b"ORIG_HEAD" | b"FETCH_HEAD") {
                check_ref_name_v1(name).map_err(|why| {
                    refs_fault(RefsFaultV1::ManifestName {
                        name: lossy(name),
                        why,
                    })
                })?;
            }
            if let CustodyRefTargetV1::Symbolic(target) = record.target() {
                check_ref_name_v1(target.as_bytes()).map_err(|why| {
                    refs_fault(RefsFaultV1::ManifestTarget {
                        name: lossy(name),
                        target: lossy(target.as_bytes()),
                        why,
                    })
                })?;
            }
            let found = self
                .found(name)
                .ok_or_else(|| refs_fault(RefsFaultV1::MissingOriginal { name: lossy(name) }))?;
            let (expected, agrees) = match record.target() {
                CustodyRefTargetV1::Direct(object) => (
                    format!("direct {}", object.object_id()),
                    found == FoundV1::Direct(object.object_id()),
                ),
                CustodyRefTargetV1::Symbolic(target) => (
                    format!("symbolic {}", lossy(target.as_bytes())),
                    found == FoundV1::Symbolic(target.as_bytes()),
                ),
                CustodyRefTargetV1::Unborn => (
                    "unborn".to_owned(),
                    matches!(found, FoundV1::Symbolic(_)) && unborn.contains(name),
                ),
            };
            if !agrees {
                return Err(refs_fault(RefsFaultV1::OriginalMismatch {
                    name: lossy(name),
                    expected,
                    found: found.describe(),
                }));
            }
        }
        Ok(manifest.original_refs().len() as u64)
    }

    /// Judge everything admitted against `manifest` and its object inventory: exactly one valid HEAD;
    /// refs that neither conflict nor collide; every active direct target in the inventory, of a valid
    /// kind, with consistent peeled rows; every symbolic chain bounded and resolved (or unborn where that
    /// is allowed); and every manifest ref present with exactly its recorded target. The deadline is
    /// observed between phases.
    ///
    /// Where the manifest's closed vocabulary is silent, these are the readings (each refused, never
    /// guessed, when the restored refs disagree): a manifest `Direct` ref is a ref whose OWN value is that
    /// object (a symbolic ref that merely resolves to it disagrees); `Symbolic(name)` is a ref whose own
    /// value names exactly `name`, resolvable or not; `Unborn` is a symbolic ref whose chain ends at a ref
    /// that does not exist; the value of `FETCH_HEAD` is its first row's object (as Git resolves it) and
    /// of `ORIG_HEAD` its one object; a branch under `refs/heads/`, HEAD (detached or resolved), and
    /// `ORIG_HEAD` name commits; a packed peeled row follows a tag and names a non-tag; and ref names that
    /// differ only by ASCII case collide. A refusal leaves the builder unusable: its caller stops.
    pub(crate) fn finish(
        &self,
        manifest: &CustodyManifestV1,
    ) -> Result<RefsAgreementV1, GitBootstrapErrorV1> {
        let inventory = manifest_inventory(manifest, self.format)?;
        let head = self
            .head
            .as_ref()
            .ok_or_else(|| refs_fault(RefsFaultV1::MissingHead))?;
        self.check_structure()?;
        observe_deadline(self.deadline, "refs-structure")?;
        let direct_targets = self.check_targets(&inventory)?;
        observe_deadline(self.deadline, "refs-targets")?;
        let resolution = self.check_resolution(manifest, &inventory)?;
        observe_deadline(self.deadline, "refs-resolved")?;
        let manifest_refs = self.check_manifest(manifest, &resolution.unborn)?;
        observe_deadline(self.deadline, "refs-manifest")?;
        let packed = self.packed.as_ref();
        Ok(RefsAgreementV1 {
            head: match head {
                RefValueV1::Direct(oid) => HeadStateV1::Detached { oid: oid.clone() },
                RefValueV1::Symbolic(target) => HeadStateV1::Symbolic {
                    target: lossy(target),
                    resolved: resolution.head,
                },
            },
            loose_refs: self.loose.len() as u64,
            packed_refs: packed.map_or(0, |packed| packed.rows.len() as u64),
            peeled_rows: self.peeled_rows,
            effective_refs: self.names().len() as u64,
            symbolic_refs: self
                .loose
                .values()
                .filter(|value| matches!(value, RefValueV1::Symbolic(_)))
                .count() as u64,
            orig_head: self.orig_head.clone(),
            fetch_head_rows: self.fetch_head.as_ref().map_or(0, |rows| rows.len() as u64),
            reflog_files: self.logs.len() as u64,
            reflog_rows: self.log_rows,
            direct_targets,
            manifest_refs,
        })
    }
}

/// What HEAD is in the restored refs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HeadStateV1 {
    /// HEAD holds an object id of the inventory's commits.
    Detached { oid: String },
    /// HEAD names a branch; `resolved` is the commit its chain ends at, or `None` when the chain ends
    /// at a ref that does not exist (an unborn HEAD).
    Symbolic {
        target: String,
        resolved: Option<String>,
    },
}

/// What the agreement proved, in counts; the refs themselves are not retained.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RefsAgreementV1 {
    pub(crate) head: HeadStateV1,
    /// Loose files under `refs/`.
    pub(crate) loose_refs: u64,
    pub(crate) packed_refs: u64,
    pub(crate) peeled_rows: u64,
    /// Distinct ref names under `refs/`, loose or packed.
    pub(crate) effective_refs: u64,
    /// Loose symbolic refs under `refs/`.
    pub(crate) symbolic_refs: u64,
    pub(crate) orig_head: Option<String>,
    pub(crate) fetch_head_rows: u64,
    pub(crate) reflog_files: u64,
    pub(crate) reflog_rows: u64,
    /// Distinct direct targets found in the verified inventory.
    pub(crate) direct_targets: u64,
    /// The manifest's original refs, each cross-checked.
    pub(crate) manifest_refs: u64,
}

/// The retained frames' agreement with the manifest and its inventory: the production input of the refs
/// foundation. It re-reads exactly the frames pass B1a admitted (`plan` must be that plan, for this
/// capsule), each rehashed from its retained descriptor and decoded again, completely, through its
/// verified trailer, with every regular file of the active subset fed to a [`RefsBuilderV1`]; then
/// [`RefsBuilderV1::finish`] judges the whole. A frame that is not the verified bytes is refused as that
/// ahead of anything it says. An Empty `RefsAndHead` class is [`GitBootstrapErrorV1::EmptyRefsAndHead`];
/// no HEAD is ever synthesized.
///
/// Nothing is written, charged, or reserved: the one ledger, the pins, and the destination are exactly as
/// they were, whatever the outcome, and the caller keeps the one verified capsule. The ONE absolute
/// `deadline` is observed at every step. On success each staged descriptor is at offset 0. A refusal is in
/// pass A's failure shape: it never began active materialization, it carries `completed`, and the one
/// ledger's charge.
pub(crate) fn agree_active_refs_v1(
    verified: &mut VerifiedCapsuleV1,
    plan: &ActiveFramePlanV1,
    object_format: GitObjectFormatV1,
    frame_budget: CustodyFrameBudgetV1,
    deadline: Instant,
    completed: Option<GitPlaneStepV1>,
) -> Result<RefsAgreementV1, GitBootstrapFailureV1> {
    agree_active_refs_bounded_v1(
        verified,
        plan,
        object_format,
        frame_budget,
        deadline,
        completed,
        RefsLimitsV1::v1(),
    )
}

/// [`agree_active_refs_v1`] with the limits as a parameter, so each cap is exercised at a small size.
pub(crate) fn agree_active_refs_bounded_v1(
    verified: &mut VerifiedCapsuleV1,
    plan: &ActiveFramePlanV1,
    object_format: GitObjectFormatV1,
    frame_budget: CustodyFrameBudgetV1,
    deadline: Instant,
    completed: Option<GitPlaneStepV1>,
    limits: RefsLimitsV1,
) -> Result<RefsAgreementV1, GitBootstrapFailureV1> {
    agree_frames(
        verified,
        plan,
        object_format,
        frame_budget,
        deadline,
        limits,
    )
    .map_err(|cause| pre_materialization_failure(verified, cause, completed))
}

fn plan_mismatch(what: &str) -> GitBootstrapErrorV1 {
    GitBootstrapErrorV1::ActivePlanMismatch(what.to_owned())
}

fn agree_frames(
    verified: &mut VerifiedCapsuleV1,
    plan: &ActiveFramePlanV1,
    format: GitObjectFormatV1,
    frame_budget: CustodyFrameBudgetV1,
    deadline: Instant,
    limits: RefsLimitsV1,
) -> Result<RefsAgreementV1, GitBootstrapErrorV1> {
    observe_deadline(deadline, "refs-entry")?;
    let active = classify_active_frames(verified)?;
    if !active
        .iter()
        .any(|frame| frame.class == CustodyCoverageClassV1::RefsAndHead)
    {
        return Err(GitBootstrapErrorV1::EmptyRefsAndHead);
    }
    // The plan is this capsule's: the same frames, in the same order, at the same staged payloads, with
    // the length and SHA-256 the verified capsule recorded for each.
    let planned: Vec<(CustodyCoverageClassV1, usize)> = plan
        .frames
        .iter()
        .map(|frame| (frame.class, frame.staged))
        .collect();
    let expected: Vec<(CustodyCoverageClassV1, usize)> = active
        .iter()
        .map(|frame| (frame.class, frame.staged))
        .collect();
    if planned != expected {
        return Err(plan_mismatch(
            "the frames are not the capsule's active frames",
        ));
    }
    for summary in &plan.frames {
        let recorded = verified
            .staged
            .get(summary.staged)
            .ok_or_else(|| plan_mismatch("a frame names no staged payload"))?;
        if recorded.length != summary.length || recorded.sha256 != summary.sha256 {
            return Err(plan_mismatch("a frame is not the verified plaintext"));
        }
    }
    let generation = verified.control.manifest.generation_id().to_owned();
    let mut builder = RefsBuilderV1::new(format, limits, deadline);
    for summary in &plan.frames {
        let one = verified
            .staged
            .get_mut(summary.staged)
            .ok_or_else(|| plan_mismatch("a frame names no staged payload"))?;
        feed_frame(
            one,
            summary,
            &generation,
            frame_budget,
            deadline,
            &mut builder,
        )?;
    }
    let agreement = builder.finish(&verified.control.manifest)?;
    // The last statement ahead of the infallible result.
    observe_deadline(deadline, "refs-final")?;
    Ok(agreement)
}

/// Decode one retained frame again, completely, and feed its regular files to `builder`. The first refusal
/// of the refs themselves is HELD until the whole frame has decoded and its decoded bytes are bound to
/// the verified plaintext, so a frame that is not whole or not the verified bytes is refused as THAT, and
/// nothing it says is trusted. The frame must also be the one the plan describes. The descriptor ends at
/// offset 0.
fn feed_frame(
    one: &mut StagedPlaintextV1,
    summary: &ActiveFrameSummaryV1,
    generation: &str,
    budget: CustodyFrameBudgetV1,
    deadline: Instant,
    builder: &mut RefsBuilderV1,
) -> Result<(), GitBootstrapErrorV1> {
    let class = summary.class;
    reverify_staged_v1(one, deadline)?;
    let header = CustodyFrameHeaderV1::new(class, generation).map_err(|_| frame_invalid(class))?;
    let expired = Cell::new(false);
    let refuse = || {
        if expired.get() {
            GitBootstrapErrorV1::Git(CustodyGitError::Timeout)
        } else {
            frame_invalid(class)
        }
    };
    let mut counts = FrameCountsV1::default();
    let mut entry_digest = ActiveEntryDigestV1::new(class);
    let mut fault: Option<GitBootstrapErrorV1> = None;
    let mut paced = PacedFrameReaderV1 {
        inner: (&one.file).take(one.length.saturating_add(1)),
        deadline,
        expired: &expired,
        context: digest::Context::new(&digest::SHA256),
        count: 0,
    };
    {
        let reader = BufReader::with_capacity(HASH_BUFFER_BYTES_V1, &mut paced);
        let mut decoder =
            CustodyFrameDecoderV1::new(reader, &header, budget).map_err(|_| refuse())?;
        loop {
            before_deadline(deadline)?;
            let Some(entry) = decoder.next_entry().map_err(|_| refuse())? else {
                break;
            };
            let (path, kind, length, content) = match entry {
                CustodyFrameEntryV1::Directory { path, .. } => {
                    (path, ActiveEntryKindV1::Directory, 0, None)
                }
                CustodyFrameEntryV1::Symlink { path, .. } => {
                    (path, ActiveEntryKindV1::Symlink, 0, None)
                }
                CustodyFrameEntryV1::Regular {
                    path,
                    length,
                    content,
                    ..
                } => (path, ActiveEntryKindV1::Regular, length, Some(content)),
            };
            #[cfg(test)]
            run_git_hook(
                GitHookPointV1::FrameEntry,
                "",
                Path::new(OsStr::from_bytes(path.as_bytes())),
            );
            counts.entries = counts.entries.saturating_add(1);
            match kind {
                ActiveEntryKindV1::Directory => {
                    counts.directories = counts.directories.saturating_add(1);
                }
                _ => {
                    counts.regular_files = counts.regular_files.saturating_add(1);
                    counts.content_bytes = counts.content_bytes.saturating_add(length);
                }
            }
            entry_digest.record(kind, path.as_bytes(), length);
            if let Some(mut content) = content {
                if fault.is_none() {
                    match builder.admit_file(path.as_bytes(), &mut content) {
                        Ok(()) => {}
                        Err(timeout @ GitBootstrapErrorV1::Git(CustodyGitError::Timeout)) => {
                            return Err(timeout);
                        }
                        Err(GitBootstrapErrorV1::Refs(RefsFaultV1::Unreadable { .. })) => {
                            return Err(refuse());
                        }
                        Err(found) => fault = Some(found),
                    }
                }
                io::copy(&mut content, &mut io::sink()).map_err(|_| refuse())?;
            } else if kind == ActiveEntryKindV1::Symlink && fault.is_none() {
                fault = Some(refs_fault(RefsFaultV1::UnexpectedFile {
                    path: lossy(path.as_bytes()),
                }));
            }
        }
    }
    // The decoder met the verified trailer and the end of the stream. The bytes it consumed must be
    // exactly the verified plaintext: a frame replaced after its rehash is refused here.
    let (count, context) = (paced.count, paced.context);
    let mut decoded_sha256 = [0_u8; 32];
    decoded_sha256.copy_from_slice(context.finish().as_ref());
    if count != one.length || decoded_sha256 != one.sha256 {
        return Err(GitBootstrapErrorV1::StagedInputChanged {
            name: lossy(one.name.as_bytes()),
        });
    }
    if (
        summary.entries,
        summary.directories,
        summary.regular_files,
        summary.content_bytes,
        summary.entries_sha256,
    ) != (
        counts.entries,
        counts.directories,
        counts.regular_files,
        counts.content_bytes,
        entry_digest.finish(),
    ) {
        return Err(plan_mismatch(
            "a frame's entries are not the ones B1a admitted",
        ));
    }
    if let Some(fault) = fault {
        return Err(fault);
    }
    observe_deadline(deadline, "refs-decoded")?;
    observe_deadline(deadline, "refs-rewound")?;
    one.file.seek(SeekFrom::Start(0)).map_err(|error| {
        GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Io(format!(
            "{}: {error}",
            lossy(one.name.as_bytes())
        )))
    })?;
    Ok(())
}

/// The active repository config, byte for byte: `[core]` with `repositoryformatversion` 0 (SHA-1) or 1
/// (SHA-256), `bare = false`, `filemode = true`, `logallrefupdates = true`, `fsmonitor = false`, and for
/// SHA-256 an `[extensions]` section with `objectformat = sha256`. LF line ends, tab-indented
/// `key = value` lines, one trailing LF. It names no remote, include, filter, hooks path, or credential
/// helper, and it is a function of the format alone: archived configuration is never consulted.
pub(crate) fn encode_inert_config_v1(format: GitObjectFormatV1) -> Vec<u8> {
    let version = match format {
        GitObjectFormatV1::Sha1 => 0,
        GitObjectFormatV1::Sha256 => 1,
    };
    let mut text = format!(
        "[core]\n\trepositoryformatversion = {version}\n\tbare = false\n\tfilemode = true\n\tlogallrefupdates = true\n\tfsmonitor = false\n"
    );
    if format == GitObjectFormatV1::Sha256 {
        text.push_str("[extensions]\n\tobjectformat = sha256\n");
    }
    text.into_bytes()
}

// ---------------------------------------------------------------------------------------------
// Pass B2a: charged second object store
// ---------------------------------------------------------------------------------------------

/// One directory of the final object store, in creation order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ObjectDirV1 {
    Repository,
    Git,
    Objects,
    Pack,
    Info,
}

impl ObjectDirV1 {
    const ALL: [Self; 5] = [
        Self::Repository,
        Self::Git,
        Self::Objects,
        Self::Pack,
        Self::Info,
    ];

    const fn index(self) -> usize {
        match self {
            Self::Repository => 0,
            Self::Git => 1,
            Self::Objects => 2,
            Self::Pack => 3,
            Self::Info => 4,
        }
    }

    /// The name beneath its parent.
    const fn leaf(self) -> &'static str {
        match self {
            Self::Repository => "repository",
            Self::Git => ".git",
            Self::Objects => "objects",
            Self::Pack => "pack",
            Self::Info => "info",
        }
    }

    /// The destination-relative name: what the ledger's collision refusal, the device seam, and every
    /// custody refusal call it.
    const fn name(self) -> &'static str {
        match self {
            Self::Repository => "repository",
            Self::Git => "repository/.git",
            Self::Objects => "repository/.git/objects",
            Self::Pack => "repository/.git/objects/pack",
            Self::Info => "repository/.git/objects/info",
        }
    }

    /// The directory it is created beneath: `None` is the destination root.
    const fn parent(self) -> Option<Self> {
        match self {
            Self::Repository => None,
            Self::Git => Some(Self::Repository),
            Self::Objects => Some(Self::Git),
            Self::Pack | Self::Info => Some(Self::Objects),
        }
    }
}

/// The directory pins the helper has created so far. A refusal reports them as they stand; a success
/// freezes them as a [`FinalObjectPinsV1`].
#[derive(Debug, Default)]
pub(crate) struct PartialObjectPinsV1 {
    pub(crate) repository: Option<PinnedDirectoryV1>,
    pub(crate) git: Option<PinnedDirectoryV1>,
    pub(crate) objects: Option<PinnedDirectoryV1>,
    pub(crate) pack: Option<PinnedDirectoryV1>,
    pub(crate) info: Option<PinnedDirectoryV1>,
}

impl PartialObjectPinsV1 {
    fn set(&mut self, dir: ObjectDirV1, pin: PinnedDirectoryV1) {
        let slot = match dir {
            ObjectDirV1::Repository => &mut self.repository,
            ObjectDirV1::Git => &mut self.git,
            ObjectDirV1::Objects => &mut self.objects,
            ObjectDirV1::Pack => &mut self.pack,
            ObjectDirV1::Info => &mut self.info,
        };
        *slot = Some(pin);
    }

    /// All five pins, or the pins back as they were.
    fn freeze(self) -> Result<FinalObjectPinsV1, Box<Self>> {
        match self {
            Self {
                repository: Some(repository),
                git: Some(git),
                objects: Some(objects),
                pack: Some(pack),
                info: Some(info),
            } => Ok(FinalObjectPinsV1 {
                repository,
                git,
                objects,
                pack,
                info,
            }),
            other => Err(Box::new(other)),
        }
    }
}

/// The five retained pins of the final object store: `repository/`, its `.git/`, `objects/`, and
/// `objects/pack` and `objects/info`.
#[derive(Debug)]
pub(crate) struct FinalObjectPinsV1 {
    pub(crate) repository: PinnedDirectoryV1,
    pub(crate) git: PinnedDirectoryV1,
    pub(crate) objects: PinnedDirectoryV1,
    pub(crate) pack: PinnedDirectoryV1,
    pub(crate) info: PinnedDirectoryV1,
}

impl FinalObjectPinsV1 {
    fn thaw(self) -> PartialObjectPinsV1 {
        PartialObjectPinsV1 {
            repository: Some(self.repository),
            git: Some(self.git),
            objects: Some(self.objects),
            pack: Some(self.pack),
            info: Some(self.info),
        }
    }
}

/// The pins a recheck looks at, borrowed from either a partial or a frozen set.
#[derive(Clone, Copy)]
pub(super) struct PinViewV1<'p> {
    pins: [Option<&'p PinnedDirectoryV1>; 5],
}

impl<'p> PinViewV1<'p> {
    fn of_partial(pins: &'p PartialObjectPinsV1) -> Self {
        Self {
            pins: [
                pins.repository.as_ref(),
                pins.git.as_ref(),
                pins.objects.as_ref(),
                pins.pack.as_ref(),
                pins.info.as_ref(),
            ],
        }
    }

    pub(super) fn of_final(pins: &'p FinalObjectPinsV1) -> Self {
        Self {
            pins: [
                Some(&pins.repository),
                Some(&pins.git),
                Some(&pins.objects),
                Some(&pins.pack),
                Some(&pins.info),
            ],
        }
    }

    fn get(&self, dir: ObjectDirV1) -> Option<&'p PinnedDirectoryV1> {
        self.pins[dir.index()]
    }
}

/// What the helper charged to the one ledger and has not given back: entry allowances and logical bytes.
/// The ledger moved by exactly `entries * ENTRY_ALLOWANCE_BYTES_V1 + bytes`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ObjectStoreChargeV1 {
    pub(crate) entries: u64,
    pub(crate) bytes: u64,
}

/// One object file copied whole and verified: its retained descriptor (read-write, at offset 0), and its
/// length, SHA-256, device, and inode, measured from it.
#[derive(Debug)]
pub(crate) struct MaterializedObjectFileV1 {
    /// Relative to `.git`, and to the bootstrap alike: `objects/pack/pack-<hash>.pack`.
    pub(crate) relative: String,
    pub(crate) file: File,
    pub(crate) length: u64,
    pub(crate) sha256: [u8; 32],
    pub(crate) dev: u64,
    pub(crate) ino: u64,
}

/// The one object file that was being created or written when a refusal came: the bytes written so far
/// are on disk and stay there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InFlightObjectFileV1 {
    pub(crate) relative: String,
    pub(crate) expected_length: u64,
    /// Exactly the bytes that reached the file: it advances with every successful write, including the
    /// prefix of a chunk whose write then failed, and the ledger keeps exactly these bytes charged.
    pub(crate) written: u64,
}

/// The second object store, created and verified. This is a helper's value for the later passes, not
/// phase G's result: no HEAD, config, refs, or Git child belongs to it.
#[derive(Debug)]
pub(crate) struct FinalObjectStoreV1 {
    /// The phase-G step this completes.
    pub(crate) step: GitPlaneStepV1,
    pub(crate) pins: FinalObjectPinsV1,
    /// The copied files, in copy order (`.pack`, `.idx`, `.rev`); empty for an Empty object database.
    pub(crate) files: Vec<MaterializedObjectFileV1>,
    pub(crate) charged: ObjectStoreChargeV1,
}

/// What a refused invocation left: the pins that exist, the files that are whole, the file that was in
/// flight, and what it still has charged. Nothing here is ever removed.
#[derive(Debug, Default)]
pub(crate) struct ObjectStorePartialV1 {
    pub(crate) pins: PartialObjectPinsV1,
    pub(crate) files: Vec<MaterializedObjectFileV1>,
    pub(crate) in_flight: Option<InFlightObjectFileV1>,
    pub(crate) charged: ObjectStoreChargeV1,
}

/// A refused or failed second object store, with how far it got. `last_completed` is the caller's own
/// last completed step, carried unchanged (this helper completes [`GitPlaneStepV1::Objects`] only whole).
#[derive(Debug)]
pub(crate) struct ObjectStoreFailureV1 {
    pub(crate) cause: GitBootstrapErrorV1,
    pub(crate) last_completed: Option<GitPlaneStepV1>,
    /// Whether this invocation may have created an entry: false for every refusal before the first
    /// create, and for a collision at the very first entry.
    pub(crate) active_materialization_began: bool,
    /// The one ledger's charge when the failure was returned.
    pub(crate) ledger_used: u64,
    pub(crate) partial: ObjectStorePartialV1,
}

/// The reads and writes of the copy go through a buffer of this size, unless a test makes it smaller.
fn copy_chunk_bytes() -> usize {
    #[cfg(test)]
    if let Some(bytes) = COPY_CHUNK.with(std::cell::Cell::get) {
        return bytes;
    }
    HASH_BUFFER_BYTES_V1
}

/// Create the output file beneath the pack pin, create-new and no-follow.
fn create_output_file(pack: &PinnedDirectoryV1, leaf: &str) -> Result<File, FsCustodyError> {
    #[cfg(test)]
    if OBJECT_FAULT.with(std::cell::Cell::get) == Some(ObjectFaultV1::CreateFile) {
        return Err(FsCustodyError::Io(
            "custody restore object store file".to_owned(),
            io::Error::from(ErrorKind::PermissionDenied),
        ));
    }
    pack.create_new_regular_child(OsStr::new(leaf), "custody restore object store file")
}

/// One chunk read of the retained source descriptor.
fn read_chunk_step(source: &File, buffer: &mut [u8]) -> io::Result<usize> {
    #[cfg(test)]
    if let Some(interrupted) = injected_interrupt(ObjectCallV1::Read) {
        return Err(interrupted);
    }
    let mut reader = source;
    reader.read(buffer)
}

/// One read of the length probe on the retained source descriptor.
fn read_probe_step(source: &File, buffer: &mut [u8]) -> io::Result<usize> {
    #[cfg(test)]
    if let Some(interrupted) = injected_interrupt(ObjectCallV1::Probe) {
        return Err(interrupted);
    }
    let mut reader = source;
    reader.read(buffer)
}

/// One write of `bytes` to the output file, which already holds `written` bytes.
fn write_step(output: &mut File, bytes: &[u8], written: u64) -> io::Result<usize> {
    use std::io::Write as _;

    #[cfg(not(test))]
    let _ = written;
    #[cfg(test)]
    if let Some(interrupted) = injected_interrupt(ObjectCallV1::Write) {
        return Err(interrupted);
    }
    #[cfg(test)]
    match OBJECT_FAULT.with(std::cell::Cell::get) {
        Some(ObjectFaultV1::ShortWrites(most)) => {
            let take = bytes.len().min(most.max(1));
            return output.write(&bytes[..take]);
        }
        Some(ObjectFaultV1::WriteAfter(limit)) | Some(ObjectFaultV1::WriteZeroAfter(limit)) => {
            let room = limit.saturating_sub(written);
            if room == 0 {
                return if OBJECT_FAULT.with(std::cell::Cell::get)
                    == Some(ObjectFaultV1::WriteZeroAfter(limit))
                {
                    Ok(0)
                } else {
                    Err(io::Error::other("an injected write fault"))
                };
            }
            let take = usize::try_from(room).map_or(bytes.len(), |room| room.min(bytes.len()));
            return output.write(&bytes[..take]);
        }
        _ => {}
    }
    output.write(bytes)
}

/// Give back `bytes` of a chunk that was reserved on the ledger and provably never written (a failed or
/// zero-length write reports that none of them reached the file): the ledger and the facts both drop them.
fn give_back_unwritten(
    dest: &super::RestoreDestinationV1,
    charged: &mut ObjectStoreChargeV1,
    bytes: usize,
) {
    let bytes = bytes as u64;
    dest.ledger.borrow_mut().release(bytes);
    charged.bytes = charged.bytes.saturating_sub(bytes);
}

/// Flush the output file's contents to stable storage.
fn sync_output_file(output: &File, relative: &str) -> Result<(), GitBootstrapErrorV1> {
    #[cfg(test)]
    if OBJECT_FAULT.with(std::cell::Cell::get) == Some(ObjectFaultV1::SyncFile) {
        return Err(io_refusal(
            relative,
            &io::Error::other("an injected file sync fault"),
        ));
    }
    output
        .sync_all()
        .map_err(|error| io_refusal(relative, &error))
}

/// Flush the pack directory's new entry to stable storage.
fn sync_pack_directory(pack: &PinnedDirectoryV1) -> Result<(), GitBootstrapErrorV1> {
    #[cfg(test)]
    if OBJECT_FAULT.with(std::cell::Cell::get) == Some(ObjectFaultV1::SyncDirectory) {
        return Err(GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Io(
            "an injected directory sync fault".to_owned(),
        )));
    }
    pack.sync("custody restore object store directory")
        .map_err(fs_error)
}

fn leaf_of(relative: &str) -> &str {
    relative.rsplit('/').next().unwrap_or(relative)
}

fn io_refusal(relative: &str, error: &io::Error) -> GitBootstrapErrorV1 {
    GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Io(format!("{relative}: {error}")))
}

fn object_input(why: impl Into<String>) -> GitBootstrapErrorV1 {
    GitBootstrapErrorV1::ObjectStoreInput(why.into())
}

fn copy_refusal(relative: &str, detail: impl Into<String>) -> GitBootstrapErrorV1 {
    GitBootstrapErrorV1::ObjectCopy {
        relative: relative.to_owned(),
        detail: detail.into(),
    }
}

/// The plan is this capsule's complete active prevalidation: the capsule's active classes include a
/// Captured `RefsAndHead` (an Empty one is [`GitBootstrapErrorV1::EmptyRefsAndHead`], and no HEAD is ever
/// synthesized), and the plan names exactly the capsule's active frames, in order, at the same staged
/// payloads, with the length and SHA-256 the verified capsule recorded for each. Pure.
fn require_prevalidated(
    boot: &VerifiedGitBootstrapV1,
    plan: &ActiveFramePlanV1,
) -> Result<(), GitBootstrapErrorV1> {
    let active = classify_active_frames(&boot.verified)?;
    if !active
        .iter()
        .any(|frame| frame.class == CustodyCoverageClassV1::RefsAndHead)
    {
        return Err(GitBootstrapErrorV1::EmptyRefsAndHead);
    }
    let planned: Vec<(CustodyCoverageClassV1, usize)> = plan
        .frames
        .iter()
        .map(|frame| (frame.class, frame.staged))
        .collect();
    let expected: Vec<(CustodyCoverageClassV1, usize)> = active
        .iter()
        .map(|frame| (frame.class, frame.staged))
        .collect();
    if planned != expected {
        return Err(plan_mismatch(
            "the frames are not the capsule's active frames",
        ));
    }
    for summary in &plan.frames {
        let recorded = boot
            .verified
            .staged
            .get(summary.staged)
            .ok_or_else(|| plan_mismatch("a frame names no staged payload"))?;
        if recorded.length != summary.length || recorded.sha256 != summary.sha256 {
            return Err(plan_mismatch("a frame is not the verified plaintext"));
        }
    }
    if boot.active_frames != active {
        return Err(object_input(
            "the bootstrap's active frames are not the capsule's",
        ));
    }
    Ok(())
}

/// The bootstrap is a proven object store: strict fsck ran, the closure evidence agrees with the
/// manifest's inventory and coverage, the provenance and the objects agree, and a Captured pack is the
/// hash-named `.pack`, `.idx`, and `.rev` of the bootstrap's retained pack directory, in that order. The
/// pack name is a lower-case object-id-width hexadecimal string, never a path. Pure.
fn require_proven_bootstrap(boot: &VerifiedGitBootstrapV1) -> Result<(), GitBootstrapErrorV1> {
    let manifest = &boot.verified.control.manifest;
    let inventory = manifest.original_objects().len();
    if !boot.closure.fsck_strict {
        return Err(object_input("strict fsck was not proven"));
    }
    if boot.closure.inventory_objects != inventory {
        return Err(object_input(format!(
            "the closure evidence covers {} objects, and the manifest lists {inventory}",
            boot.closure.inventory_objects
        )));
    }
    let database = coverage_state(manifest, CustodyCoverageClassV1::ObjectDatabase);
    match (
        &boot.objects,
        boot.provenance,
        boot.closure.closure,
        database,
    ) {
        (
            BootstrapObjectsV1::Captured { pack_hash, files },
            GitFormatProvenanceV1::CapsuleObjectsAndPack,
            GitClosureProofV1::Reachable { objects },
            Some(CustodyStateClassV1::Captured),
        ) => {
            if inventory == 0 || objects != inventory {
                return Err(object_input(
                    "the reachable closure is not the manifest's inventory",
                ));
            }
            if !is_object_id(pack_hash, boot.object_format) {
                return Err(object_input(
                    "the pack name is not an object id of the caller's format",
                ));
            }
            if files.len() != 3 {
                return Err(object_input(format!(
                    "a Captured bootstrap admits three files, not {}",
                    files.len()
                )));
            }
            for (file, extension) in files.iter().zip(["pack", "idx", "rev"]) {
                if file.relative != format!("{PACK_DIRECTORY_V1}/pack-{pack_hash}.{extension}") {
                    return Err(object_input(format!(
                        "{} is not the admitted .{extension}",
                        file.relative
                    )));
                }
            }
            if !boot.directories.contains_key(PACK_DIRECTORY_V1) {
                return Err(object_input(
                    "the bootstrap's pack directory is not retained",
                ));
            }
        }
        (
            BootstrapObjectsV1::Empty,
            GitFormatProvenanceV1::CallerSelectedEmpty,
            GitClosureProofV1::Vacuous,
            Some(CustodyStateClassV1::Empty),
        ) => {
            if inventory != 0 {
                return Err(object_input("an Empty object database lists objects"));
            }
        }
        _ => {
            return Err(object_input(
                "the objects, provenance, closure, and manifest coverage disagree",
            ))
        }
    }
    Ok(())
}

/// The admitted bootstrap files, in copy order; none for Empty.
fn admitted_files(boot: &VerifiedGitBootstrapV1) -> &[AdmittedObjectFileV1] {
    match &boot.objects {
        BootstrapObjectsV1::Captured { files, .. } => files,
        BootstrapObjectsV1::Empty => &[],
    }
}

/// Every admitted bootstrap file is still the entry its retained pack directory holds, and its retained
/// descriptor is still that regular file at the admitted length. A link, a directory, a special file,
/// another inode, or another length refuses; the replacement is never opened. Each file is preceded by the
/// one deadline's guard.
pub(super) fn source_files_unchanged(
    boot: &VerifiedGitBootstrapV1,
    pace: PaceV1<'_>,
) -> Result<(), GitBootstrapErrorV1> {
    for admitted in admitted_files(boot) {
        pace("objects-source")?;
        let pack = boot
            .directories
            .get(PACK_DIRECTORY_V1)
            .ok_or_else(|| object_input("the bootstrap's pack directory is not retained"))?;
        let stat = pack
            .child_metadata_no_follow(
                OsStr::new(leaf_of(&admitted.relative)),
                "custody restore admitted object file",
            )
            .map_err(fs_error)?
            .ok_or_else(|| {
                digest_identity(&admitted.relative, "no longer the admitted object file")
            })?;
        match stat.kind {
            ChildKindV1::Symlink => {
                return Err(GitBootstrapErrorV1::Restore(
                    CustodyRestoreErrorV1::SymlinkRefused {
                        name: admitted.relative.clone(),
                    },
                ))
            }
            ChildKindV1::Regular => {}
            ChildKindV1::Directory | ChildKindV1::Special(_) => {
                return Err(digest_identity(
                    &admitted.relative,
                    "no longer the admitted object file",
                ))
            }
        }
        if (stat.dev, stat.ino, stat.size) != (admitted.dev, admitted.ino, admitted.length) {
            return Err(digest_identity(
                &admitted.relative,
                "no longer the admitted object file",
            ));
        }
        let held = admitted
            .file
            .metadata()
            .map_err(|error| io_refusal(&admitted.relative, &error))?;
        let held = ChildStatV1::from_metadata(&held);
        if (held.kind, held.dev, held.ino, held.size)
            != (
                ChildKindV1::Regular,
                admitted.dev,
                admitted.ino,
                admitted.length,
            )
        {
            return Err(digest_identity(
                &admitted.relative,
                "the retained descriptor is no longer the admitted object file",
            ));
        }
    }
    Ok(())
}

/// Every output directory that exists is still the entry its parent holds, parents first, and every file
/// already copied is still the regular file its retained pin lists, of the length and identity it was
/// measured with. A link or another object refuses; nothing is followed.
pub(super) fn output_chains(
    dest: &super::RestoreDestinationV1,
    pins: PinViewV1<'_>,
    files: &[MaterializedObjectFileV1],
    pace: PaceV1<'_>,
) -> Result<(), GitBootstrapErrorV1> {
    for dir in ObjectDirV1::ALL {
        let Some(pin) = pins.get(dir) else { continue };
        pace("objects-chain")?;
        let parent = match dir.parent() {
            None => &dest.root,
            Some(parent) => pins.get(parent).ok_or_else(|| {
                CustodyRestoreErrorV1::IdentityChanged(format!(
                    "{}: its parent is not retained",
                    dir.name()
                ))
            })?,
        };
        recheck_entry(parent, dir.leaf().as_bytes(), pin, dir.name().as_bytes())?;
    }
    for file in files {
        pace("objects-chain")?;
        let pack = pins.get(ObjectDirV1::Pack).ok_or_else(|| {
            CustodyRestoreErrorV1::IdentityChanged(format!(
                "{}: the pack directory is not retained",
                file.relative
            ))
        })?;
        let stat = pack
            .child_metadata_no_follow(
                OsStr::new(leaf_of(&file.relative)),
                "custody restore object store file",
            )
            .map_err(fs_error)?
            .ok_or_else(|| digest_identity(&file.relative, "the copied file's entry is gone"))?;
        match stat.kind {
            ChildKindV1::Symlink => {
                return Err(GitBootstrapErrorV1::Restore(
                    CustodyRestoreErrorV1::SymlinkRefused {
                        name: file.relative.clone(),
                    },
                ))
            }
            ChildKindV1::Regular => {}
            ChildKindV1::Directory | ChildKindV1::Special(_) => {
                return Err(digest_identity(
                    &file.relative,
                    "the entry is no longer the copied file",
                ))
            }
        }
        if (stat.dev, stat.ino, stat.size) != (file.dev, file.ino, file.length) {
            return Err(digest_identity(
                &file.relative,
                "the entry is no longer the copied file",
            ));
        }
    }
    Ok(())
}

/// One run of the helper: the retained bootstrap it reads, and what it has created and charged so far.
struct StoreRunV1<'a> {
    boot: &'a VerifiedGitBootstrapV1,
    deadline: Instant,
    pins: PartialObjectPinsV1,
    files: Vec<MaterializedObjectFileV1>,
    in_flight: Option<InFlightObjectFileV1>,
    charged: ObjectStoreChargeV1,
    began: bool,
}

impl<'a> StoreRunV1<'a> {
    fn dest(&self) -> &'a super::RestoreDestinationV1 {
        &self.boot.verified.destination
    }

    /// The recheck between writes: every output directory and file, then every admitted source file, then
    /// the staging, HOME, XDG, and bootstrap chains, and the destination root's identity LAST. Each step
    /// is held to the one deadline.
    fn chains(
        &self,
        pins: PinViewV1<'_>,
        files: &[MaterializedObjectFileV1],
        pace: PaceV1<'_>,
    ) -> Result<(), GitBootstrapErrorV1> {
        output_chains(self.dest(), pins, files, pace)?;
        source_files_unchanged(self.boot, pace)?;
        sweep_chains(
            self.dest(),
            Some(&self.boot.home),
            Some(&self.boot.xdg),
            &self.boot.directories,
            pace,
        )
    }

    /// [`Self::chains`] over what exists now, then the clock: the recheck ends in the destination root's
    /// identity, which can block, so nothing the caller does next (a create, a charge, a write) may follow
    /// an expired deadline. Only the final gate, which has nothing fallible after it, omits this guard.
    fn recheck(&self) -> Result<(), GitBootstrapErrorV1> {
        let deadline = self.deadline;
        let pace = |step: &'static str| observe_deadline(deadline, step);
        self.chains(PinViewV1::of_partial(&self.pins), &self.files, &pace)?;
        observe_deadline(deadline, "objects-checked")
    }

    /// The full gate: the mount census first, then [`Self::chains`], then the clock (see
    /// [`Self::recheck`]).
    fn gate(&self) -> Result<(), GitBootstrapErrorV1> {
        let deadline = self.deadline;
        let pace = |step: &'static str| observe_deadline(deadline, step);
        pace("objects-gate")?;
        refuse_mounts_within(&self.dest().root)?;
        self.chains(PinViewV1::of_partial(&self.pins), &self.files, &pace)?;
        observe_deadline(deadline, "objects-checked")
    }

    /// The final gate over the frozen pins: the mount census, the output and source chains, every other
    /// retained chain, and the destination root's identity LAST, the guard immediately before it being
    /// the last statement ahead of it.
    fn final_gate(&self, frozen: &FinalObjectPinsV1) -> Result<(), GitBootstrapErrorV1> {
        #[cfg(test)]
        run_git_hook(
            GitHookPointV1::BeforeObjectsFinalGate,
            "",
            self.dest().root.canonical_path(),
        );
        let deadline = self.deadline;
        let pace = |step: &'static str| observe_deadline(deadline, step);
        pace("objects-final")?;
        refuse_mounts_within(&self.dest().root)?;
        self.chains(PinViewV1::of_final(frozen), &self.files, &pace)
    }

    /// Create one directory of the hierarchy: the clock, a recheck of everything retained, then the
    /// create-new beneath its retained parent (its entry reserved first, given back only when the create
    /// provably made nothing), and a recheck that includes the new pin.
    fn create_directory(&mut self, dir: ObjectDirV1) -> Result<(), GitBootstrapErrorV1> {
        observe_deadline(self.deadline, "objects-directory")?;
        self.recheck()?;
        let dest = self.dest();
        let parent = match dir.parent() {
            None => &dest.root,
            Some(parent) => PinViewV1::of_partial(&self.pins)
                .get(parent)
                .ok_or_else(|| {
                    object_input(format!("{}: its parent is not retained", dir.name()))
                })?,
        };
        #[cfg(test)]
        run_git_hook(
            GitHookPointV1::BeforeObjectCreate,
            "directory",
            &parent.canonical_path().join(dir.leaf()),
        );
        let created = create_staging_directory(
            parent,
            &dest.ledger,
            dest.dev,
            dir.leaf().as_bytes(),
            dir.name().as_bytes(),
        );
        match created {
            Ok(pin) => {
                self.began = true;
                self.charged.entries += 1;
                self.pins.set(dir, pin);
            }
            Err(error) => {
                // A collision and a refused reservation made nothing and charge nothing. Any other error
                // may follow a successful create, so its entry stays charged.
                if !matches!(
                    error,
                    CustodyRestoreErrorV1::StagingCollision { .. }
                        | CustodyRestoreErrorV1::Budget(_)
                ) {
                    self.began = true;
                    self.charged.entries += 1;
                }
                return Err(error.into());
            }
        }
        self.recheck()
    }

    /// Copy the admitted file `admitted` into the new pack directory: the full gate, the clock, the entry
    /// reserved, the create-new, the device, a recheck; then the stream, one bounded chunk at a time, each
    /// chunk charged before it is written, ending in the exact length and SHA-256; the file and its
    /// directory synced; the written file read back against the admitted digest; and a recheck.
    ///
    /// A blocking call cannot be preempted, so the deadline can pass inside one. Whatever it returns, no
    /// further call on the descriptor follows without the one deadline being read first: the retry of an
    /// interrupted read, write, or probe, the rest of a chunk after a short write, and the first probe
    /// after the last write. A write that stops there keeps the bytes the file accepted charged and gives
    /// back the rest of its reservation.
    fn copy_file(&mut self, admitted: &AdmittedObjectFileV1) -> Result<(), GitBootstrapErrorV1> {
        // Every file follows all five directories, so materialization began when the first was created.
        let deadline = self.deadline;
        let relative = admitted.relative.as_str();
        let leaf = leaf_of(relative);
        observe_deadline(deadline, "objects-file")?;
        self.gate()?;
        let dest = self.dest();
        let name = joined(ObjectDirV1::Pack.name().as_bytes(), leaf.as_bytes());
        let pack = self
            .pins
            .pack
            .as_ref()
            .ok_or_else(|| object_input("the pack directory is not retained"))?;
        // The retained source descriptor is read from its start.
        (&admitted.file)
            .seek(SeekFrom::Start(0))
            .map_err(|error| io_refusal(relative, &error))?;

        dest.ledger
            .borrow_mut()
            .reserve_entries(1)
            .map_err(ledger_error)?;
        self.charged.entries += 1;
        #[cfg(test)]
        run_git_hook(
            GitHookPointV1::BeforeObjectCreate,
            "file",
            &pack.canonical_path().join(leaf),
        );
        let mut output = match create_output_file(pack, leaf) {
            Ok(output) => output,
            Err(error) => {
                // A create that met an existing entry made nothing, so its allowance goes back;
                // any other error may follow a successful create and keeps the charge.
                if matches!(error, FsCustodyError::TargetExists(_)) {
                    dest.ledger.borrow_mut().release(ENTRY_ALLOWANCE_BYTES_V1);
                    self.charged.entries = self.charged.entries.saturating_sub(1);
                }
                return Err(staging_refusal(error, &name).into());
            }
        };
        let length = admitted.length;
        self.in_flight = Some(InFlightObjectFileV1 {
            relative: relative.to_owned(),
            expected_length: length,
            written: 0,
        });
        let created = ChildStatV1::from_metadata(
            &output
                .metadata()
                .map_err(|error| io_refusal(relative, &error))?,
        );
        let observed = created.dev;
        #[cfg(test)]
        let observed = super::CREATED_DEV
            .with(|devices| devices.borrow().get(name.as_slice()).copied())
            .unwrap_or(observed);
        if observed != dest.dev {
            return Err(CustodyRestoreErrorV1::DeviceCrossing { name: lossy(&name) }.into());
        }
        self.recheck()?;

        // The stream: at most one chunk in memory, the clock before every chunk, each chunk charged
        // before it is written, and never more than the admitted length.
        let chunk = copy_chunk_bytes();
        let mut buffer = vec![0_u8; chunk];
        let mut context = digest::Context::new(&digest::SHA256);
        let source = &admitted.file;
        let mut written = 0_u64;
        #[cfg(test)]
        let output_path = pack.canonical_path().join(leaf);
        while written < length {
            #[cfg(test)]
            run_git_hook(
                GitHookPointV1::BeforeObjectChunk,
                "",
                &pack.canonical_path().join(leaf),
            );
            before_deadline(deadline)?;
            let want = usize::try_from(length - written).map_or(chunk, |left| left.min(chunk));
            let read = loop {
                let step = read_chunk_step(source, &mut buffer[..want]);
                #[cfg(test)]
                run_git_hook(GitHookPointV1::ObjectIo, "read", &output_path);
                match step {
                    Ok(read) => break read,
                    // An interrupted read did nothing, and the call can have blocked past the deadline: its
                    // retry is held to the one deadline like every other call.
                    Err(error) if error.kind() == ErrorKind::Interrupted => {
                        before_deadline(deadline)?
                    }
                    Err(error) => return Err(io_refusal(relative, &error)),
                }
            };
            if read == 0 {
                return Err(copy_refusal(
                    relative,
                    format!("the source ended after {written} of {length} bytes"),
                ));
            }
            // The read can block: the clock is read again before the chunk is charged and written.
            observe_deadline(deadline, "objects-read")?;
            dest.ledger
                .borrow_mut()
                .reserve(read as u64)
                .map_err(ledger_error)?;
            self.charged.bytes += read as u64;
            // Explicit writes, so a failure partway keeps the bytes that did reach the file: the fact in
            // flight advances with each successful write, and the part of the chunk that was reserved and
            // never written is given back, so the charge is exactly what is on disk. A write that returned
            // (a prefix, or an interruption that wrote nothing) is followed by another call only while the
            // one deadline is still ahead; once it has passed, the accepted prefix stays charged and the
            // rest of the chunk goes back like a failed write's.
            let mut offset = 0_usize;
            let mut stopped: Option<GitBootstrapErrorV1> = None;
            while offset < read {
                let step = write_step(&mut output, &buffer[offset..read], written);
                #[cfg(test)]
                run_git_hook(GitHookPointV1::ObjectIo, "write", &output_path);
                match step {
                    Ok(0) => {
                        stopped =
                            Some(io_refusal(relative, &io::Error::from(ErrorKind::WriteZero)));
                    }
                    Ok(wrote) => {
                        offset += wrote;
                        written += wrote as u64;
                        if let Some(flight) = self.in_flight.as_mut() {
                            flight.written = written;
                        }
                        if offset < read {
                            stopped = before_deadline(deadline).err();
                        }
                    }
                    Err(error) if error.kind() == ErrorKind::Interrupted => {
                        stopped = before_deadline(deadline).err();
                    }
                    Err(error) => {
                        stopped = Some(io_refusal(relative, &error));
                    }
                }
                if stopped.is_some() {
                    break;
                }
            }
            if let Some(error) = stopped {
                give_back_unwritten(dest, &mut self.charged, read - offset);
                return Err(error);
            }
            context.update(&buffer[..read]);
        }
        // The source holds exactly the admitted length, and the bytes streamed are the admitted digest. The
        // last write can have blocked past the deadline, so the first probe call is held to it, and so is
        // every retry of an interrupted one.
        before_deadline(deadline)?;
        let mut probe = [0_u8; 1];
        let extra = loop {
            let step = read_probe_step(source, &mut probe);
            #[cfg(test)]
            run_git_hook(GitHookPointV1::ObjectIo, "probe", &output_path);
            match step {
                Ok(read) => break read,
                Err(error) if error.kind() == ErrorKind::Interrupted => before_deadline(deadline)?,
                Err(error) => return Err(io_refusal(relative, &error)),
            }
        };
        if extra != 0 {
            return Err(copy_refusal(
                relative,
                format!("the source is longer than the admitted {length} bytes"),
            ));
        }
        let mut streamed = [0_u8; 32];
        streamed.copy_from_slice(context.finish().as_ref());
        if streamed != admitted.sha256 {
            return Err(copy_refusal(
                relative,
                "the bytes read are not the admitted SHA-256",
            ));
        }

        observe_deadline(deadline, "objects-sync")?;
        sync_output_file(&output, relative)?;
        #[cfg(test)]
        run_git_hook(
            GitHookPointV1::ObjectSynced,
            "file",
            &pack.canonical_path().join(leaf),
        );
        // The file's sync can block: the clock is read again before the directory's.
        observe_deadline(deadline, "objects-sync-directory")?;
        sync_pack_directory(pack)?;
        #[cfg(test)]
        run_git_hook(
            GitHookPointV1::ObjectSynced,
            "directory",
            pack.canonical_path(),
        );

        // The written file, read back from its own descriptor, is the admitted bytes.
        observe_deadline(deadline, "objects-verify")?;
        output
            .seek(SeekFrom::Start(0))
            .map_err(|error| io_refusal(relative, &error))?;
        let (read_back, back_sha256) = stream_digest(&mut output, length, deadline, relative)?;
        if read_back != length || back_sha256 != admitted.sha256 {
            return Err(copy_refusal(
                relative,
                "the written file is not the admitted bytes",
            ));
        }
        output
            .seek(SeekFrom::Start(0))
            .map_err(|error| io_refusal(relative, &error))?;
        let held = ChildStatV1::from_metadata(
            &output
                .metadata()
                .map_err(|error| io_refusal(relative, &error))?,
        );
        if (held.kind, held.size, held.dev) != (ChildKindV1::Regular, length, dest.dev) {
            return Err(copy_refusal(
                relative,
                "the written file is not a regular file of the admitted length on the root's device",
            ));
        }
        self.in_flight = None;
        self.files.push(MaterializedObjectFileV1 {
            relative: relative.to_owned(),
            file: output,
            length,
            sha256: admitted.sha256,
            dev: held.dev,
            ino: held.ino,
        });
        self.recheck()
    }

    fn run(&mut self, plan: &ActiveFramePlanV1) -> Result<FinalObjectStoreV1, GitBootstrapErrorV1> {
        observe_deadline(self.deadline, "objects-entry")?;
        let boot = self.boot;
        require_prevalidated(boot, plan)?;
        require_proven_bootstrap(boot)?;
        self.gate()?;
        for dir in ObjectDirV1::ALL {
            self.create_directory(dir)?;
        }
        for admitted in admitted_files(boot) {
            self.copy_file(admitted)?;
        }
        let frozen = std::mem::take(&mut self.pins).freeze().map_err(|pins| {
            self.pins = *pins;
            object_input("the object-store hierarchy is incomplete")
        })?;
        match self.final_gate(&frozen) {
            Ok(()) => Ok(FinalObjectStoreV1 {
                step: GitPlaneStepV1::Objects,
                pins: frozen,
                files: std::mem::take(&mut self.files),
                charged: self.charged,
            }),
            Err(error) => {
                self.pins = frozen.thaw();
                Err(error)
            }
        }
    }
}

/// Create the second object store: `repository/.git/objects/{pack,info}`, and, for a Captured object
/// database, the bootstrap's three admitted files copied into `objects/pack`.
///
/// Nothing is created unless `plan` is this capsule's complete active prevalidation (an Empty
/// `RefsAndHead` is [`GitBootstrapErrorV1::EmptyRefsAndHead`]) and `bootstrap` is a proven object store.
/// Every directory and file is create-new beneath a retained pin and its entry is charged to the one
/// ledger first; each copied chunk is charged before it is written, so the second copy is charged as the
/// first was. Only the three admitted regular files are streamed, from their retained descriptors; the
/// bootstrap's HEAD, config, and hooks are never copied. Nothing is renamed, overwritten, deleted, or
/// cleaned up, and no Git child runs: the final HEAD and config are deliberately absent. Every step keeps
/// the ONE absolute `deadline`; the destination root's identity is the last fallible check and only
/// infallible construction follows it.
///
/// A refusal keeps every created byte and is in [`ObjectStoreFailureV1`]'s shape: the caller's `completed`
/// step, whether materialization began, the one ledger's charge, and typed partial effects. A second
/// invocation over the same destination refuses at `repository` as a collision, touching nothing.
pub(crate) fn materialize_object_store_v1(
    bootstrap: &VerifiedGitBootstrapV1,
    plan: &ActiveFramePlanV1,
    deadline: Instant,
    completed: Option<GitPlaneStepV1>,
) -> Result<FinalObjectStoreV1, Box<ObjectStoreFailureV1>> {
    let mut run = StoreRunV1 {
        boot: bootstrap,
        deadline,
        pins: PartialObjectPinsV1::default(),
        files: Vec::new(),
        in_flight: None,
        charged: ObjectStoreChargeV1::default(),
        began: false,
    };
    match run.run(plan) {
        Ok(done) => Ok(done),
        Err(cause) => Err(Box::new(ObjectStoreFailureV1 {
            cause,
            last_completed: completed,
            active_materialization_began: run.began,
            ledger_used: bootstrap.verified.destination.ledger.borrow().used(),
            partial: ObjectStorePartialV1 {
                pins: run.pins,
                files: run.files,
                in_flight: run.in_flight,
                charged: run.charged,
            },
        })),
    }
}

// ---------------------------------------------------------------------------------------------
// Pass B2b: exact active metadata and final ref agreement
// ---------------------------------------------------------------------------------------------

/// The one file the active `.git` receives that no frame carries: the inert config.
const CONFIG_LEAF_V1: &str = "config";

/// A metadata file copied whole is the same fact as an object file copied whole: its retained read-write
/// descriptor at offset 0, and the length, SHA-256, device, and inode measured from it. Its `relative` name
/// is beneath `.git` (`HEAD`, `refs/heads/main`, `config`).
pub(crate) type MaterializedMetadataFileV1 = MaterializedObjectFileV1;
/// The metadata file that was being created or written when a refusal came.
pub(crate) type InFlightMetadataFileV1 = InFlightObjectFileV1;
/// What the step charged to the one ledger and has not given back.
pub(crate) type MetadataChargeV1 = ObjectStoreChargeV1;

/// The active metadata, restored and agreed with the manifest, and the config created. This is a helper's
/// value for the final passes, not phase G's result: no Git child has run on it, and no final proof of the
/// restored repository belongs to it.
#[derive(Debug)]
pub(crate) struct FinalMetadataV1 {
    /// The phase-G step this completes: [`GitPlaneStepV1::Config`].
    pub(crate) step: GitPlaneStepV1,
    /// Every metadata directory this step created and pinned, by its path beneath `.git` (`refs`,
    /// `refs/heads`, `logs`, ...). Each is on the root's device.
    pub(crate) directories: BTreeMap<String, PinnedDirectoryV1>,
    /// Every metadata file, in creation order (the frames' regular files, then `config`), each retained
    /// read-write at offset 0.
    pub(crate) files: Vec<MaterializedMetadataFileV1>,
    /// What the restored refs proved, read from the retained final descriptors.
    pub(crate) agreement: RefsAgreementV1,
    pub(crate) charged: MetadataChargeV1,
}

impl FinalMetadataV1 {
    /// The config, the last file created.
    pub(crate) fn config(&self) -> Option<&MaterializedMetadataFileV1> {
        self.files
            .last()
            .filter(|file| file.relative == CONFIG_LEAF_V1)
    }
}

/// What a refused invocation left: the directories that exist and are pinned, the files that are whole, the
/// file in flight, and what it still has charged. Nothing here is ever removed.
#[derive(Debug, Default)]
pub(crate) struct MetadataPartialV1 {
    pub(crate) directories: BTreeMap<String, PinnedDirectoryV1>,
    pub(crate) files: Vec<MaterializedMetadataFileV1>,
    pub(crate) in_flight: Option<InFlightMetadataFileV1>,
    pub(crate) charged: MetadataChargeV1,
}

/// A refused or failed metadata step, with how far it got. `last_completed` is the last step that completed
/// whole: the caller's own at entry, then [`GitPlaneStepV1::Metadata`] once every frame is restored, then
/// [`GitPlaneStepV1::Refs`] once the refs agree; the config step completes only in a success.
#[derive(Debug)]
pub(crate) struct MetadataFailureV1 {
    pub(crate) cause: GitBootstrapErrorV1,
    pub(crate) last_completed: Option<GitPlaneStepV1>,
    /// Whether THIS invocation may have created an entry. The object copy before it already began active
    /// materialization, so the caller's own flag is the stronger one.
    pub(crate) active_materialization_began: bool,
    /// The one ledger's charge when the failure was returned.
    pub(crate) ledger_used: u64,
    pub(crate) partial: MetadataPartialV1,
}

fn metadata_input(why: impl Into<String>) -> GitBootstrapErrorV1 {
    GitBootstrapErrorV1::MetadataInput(why.into())
}

fn metadata_copy(relative: &str, detail: impl Into<String>) -> GitBootstrapErrorV1 {
    GitBootstrapErrorV1::MetadataCopy {
        relative: relative.to_owned(),
        detail: detail.into(),
    }
}

/// `relative` split into the directory it is created in (`None` is `.git` itself) and its leaf.
pub(super) fn split_relative(relative: &str) -> (Option<&str>, &str) {
    match relative.rsplit_once('/') {
        Some((parent, leaf)) => (Some(parent), leaf),
        None => (None, relative),
    }
}

/// The destination-relative name of an entry beneath `.git`.
pub(super) fn metadata_name(relative: &str) -> String {
    format!("{}/{relative}", ObjectDirV1::Git.name())
}

/// The retained pin of `parent` (`None` is `.git`), which must be one this invocation created and pinned.
pub(super) fn pin_beneath<'p>(
    git: &'p PinnedDirectoryV1,
    directories: &'p BTreeMap<String, PinnedDirectoryV1>,
    parent: Option<&str>,
) -> Result<&'p PinnedDirectoryV1, GitBootstrapErrorV1> {
    match parent {
        None => Ok(git),
        Some(parent) => directories.get(parent).ok_or_else(|| {
            metadata_input(format!(
                "{}: its parent is not a retained directory of this restore",
                metadata_name(parent)
            ))
        }),
    }
}

/// `parent`'s entry `leaf` must still be the regular file measured as `(dev, ino, length)`. A link or any
/// other object refuses, and nothing is followed.
pub(super) fn recheck_regular_entry(
    parent: &PinnedDirectoryV1,
    relative: &str,
    measured: (u64, u64, u64),
) -> Result<(), GitBootstrapErrorV1> {
    let name = metadata_name(relative);
    let stat = parent
        .child_metadata_no_follow(
            OsStr::new(split_relative(relative).1),
            "custody restore metadata file",
        )
        .map_err(fs_error)?
        .ok_or_else(|| digest_identity(&name, "the metadata file's entry is gone"))?;
    match stat.kind {
        ChildKindV1::Symlink => {
            return Err(GitBootstrapErrorV1::Restore(
                CustodyRestoreErrorV1::SymlinkRefused { name },
            ))
        }
        ChildKindV1::Regular => {}
        ChildKindV1::Directory | ChildKindV1::Special(_) => {
            return Err(digest_identity(
                &name,
                "the entry is no longer the restored file",
            ))
        }
    }
    if (stat.dev, stat.ino, stat.size) != measured {
        return Err(digest_identity(
            &name,
            "the entry is no longer the restored file",
        ));
    }
    Ok(())
}

/// One decoded frame entry, as the step judges it.
struct FrameEntryV1<'e> {
    class: CustodyCoverageClassV1,
    path: &'e [u8],
    kind: ActiveEntryKindV1,
    length: u64,
}

/// A refusal that ends a decode at once instead of being held until the stream is verified: the deadline
/// (nothing more is read after it), and the decoder's own failure (it is poisoned and cannot be drained). Every
/// other fault comes from what was DONE with a decoded entry, and is held.
fn ends_the_decode(error: &GitBootstrapErrorV1) -> bool {
    matches!(
        error,
        GitBootstrapErrorV1::Git(CustodyGitError::Timeout)
            | GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::FrameInvalid { .. })
    )
}

/// A refusal that ends a parse of a retained file at once instead of being held until the file is read to its
/// end: the deadline, and a read the descriptor itself failed. Every other fault is about what the bytes say.
fn ends_the_parse(error: &GitBootstrapErrorV1) -> bool {
    matches!(
        error,
        GitBootstrapErrorV1::Git(CustodyGitError::Timeout)
            | GitBootstrapErrorV1::Refs(RefsFaultV1::Unreadable { .. })
    )
}

/// The retained descriptor of a restored file, bounded to one byte past its recorded length, as the parsers
/// read it: every byte that passes is hashed and counted.
struct DigestedReaderV1<'a> {
    inner: io::Take<&'a File>,
    context: digest::Context,
    count: u64,
    /// The file's path, for the test hook.
    #[cfg(test)]
    hook_path: std::path::PathBuf,
}

impl io::Read for DigestedReaderV1<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buffer);
        #[cfg(test)]
        run_git_hook(GitHookPointV1::AfterRefsRead, "", &self.hook_path);
        let read = read?;
        self.context.update(&buffer[..read]);
        self.count = self.count.saturating_add(read as u64);
        Ok(read)
    }
}

/// Read `reader` to its end, discarding the bytes: the one absolute deadline is read before every read, so a
/// retry of an interrupted read is held to it as well.
fn drain_stream(
    reader: &mut dyn io::Read,
    deadline: Instant,
    relative: &str,
) -> Result<(), GitBootstrapErrorV1> {
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES_V1];
    loop {
        before_deadline(deadline)?;
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(io_refusal(relative, &error)),
        }
    }
}

/// What a recheck covers of the metadata this step made.
#[derive(Clone, Copy)]
enum MetaScopeV1<'s> {
    /// Every directory and file made so far.
    All,
    /// Only the chain of directories down to this one (`None` is `.git` itself): the bounded recheck
    /// between two entries, whose cost does not grow with the number of entries made.
    Chain(Option<&'s str>),
}

/// Where one file's bytes go: the destination, the one deadline, and the file's own names.
pub(super) struct SinkV1<'s> {
    pub(super) dest: &'s super::RestoreDestinationV1,
    pub(super) deadline: Instant,
    pub(super) relative: &'s str,
    /// The file's path, for the test hooks.
    pub(super) hook_path: &'s Path,
    pub(super) length: u64,
}

/// Stream exactly `sink.length` bytes of `source` into `output`, at most one chunk in memory: the clock
/// before every chunk and again after its read, each chunk charged to the one ledger BEFORE it is written,
/// and each write an explicit call, so the fact in flight advances with every successful write and a
/// failed or zero-length write stops the copy with the part of the chunk that was reserved and never
/// written given back. A write that returned (a prefix, or an interruption that wrote nothing) is followed
/// by another call only while the one deadline is still ahead. Returns the SHA-256 of the bytes written.
pub(super) fn stream_into(
    sink: &SinkV1<'_>,
    charged: &mut MetadataChargeV1,
    in_flight: &mut Option<InFlightMetadataFileV1>,
    output: &mut File,
    source: &mut dyn io::Read,
    source_fault: &dyn Fn() -> GitBootstrapErrorV1,
) -> Result<[u8; 32], GitBootstrapErrorV1> {
    let (dest, deadline, relative, length) = (sink.dest, sink.deadline, sink.relative, sink.length);
    #[cfg(not(test))]
    let _ = sink.hook_path;
    let chunk = copy_chunk_bytes();
    let mut buffer = vec![0_u8; chunk];
    let mut context = digest::Context::new(&digest::SHA256);
    let mut written = 0_u64;
    while written < length {
        #[cfg(test)]
        run_git_hook(GitHookPointV1::BeforeObjectChunk, "", sink.hook_path);
        before_deadline(deadline)?;
        let want = usize::try_from(length - written).map_or(chunk, |left| left.min(chunk));
        let step = source.read(&mut buffer[..want]);
        #[cfg(test)]
        run_git_hook(GitHookPointV1::ObjectIo, "read", sink.hook_path);
        let read = step.map_err(|_| source_fault())?;
        if read == 0 {
            return Err(metadata_copy(
                relative,
                format!("the frame's content ended after {written} of {length} bytes"),
            ));
        }
        // The read can block: the clock is read again before the chunk is charged and written.
        observe_deadline(deadline, "metadata-read")?;
        dest.ledger
            .borrow_mut()
            .reserve(read as u64)
            .map_err(ledger_error)?;
        charged.bytes += read as u64;
        let mut offset = 0_usize;
        let mut stopped: Option<GitBootstrapErrorV1> = None;
        while offset < read {
            let step = write_step(output, &buffer[offset..read], written);
            #[cfg(test)]
            run_git_hook(GitHookPointV1::ObjectIo, "write", sink.hook_path);
            match step {
                Ok(0) => {
                    stopped = Some(io_refusal(relative, &io::Error::from(ErrorKind::WriteZero)));
                }
                Ok(wrote) => {
                    offset += wrote;
                    written += wrote as u64;
                    if let Some(flight) = in_flight.as_mut() {
                        flight.written = written;
                    }
                    if offset < read {
                        stopped = before_deadline(deadline).err();
                    }
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => {
                    stopped = before_deadline(deadline).err();
                }
                Err(error) => {
                    stopped = Some(io_refusal(relative, &error));
                }
            }
            if stopped.is_some() {
                break;
            }
        }
        if let Some(error) = stopped {
            give_back_unwritten(dest, charged, read - offset);
            return Err(error);
        }
        context.update(&buffer[..read]);
    }
    // The last write can have blocked past the deadline: nothing follows it on the descriptor without the
    // clock.
    before_deadline(deadline)?;
    let mut sha256 = [0_u8; 32];
    sha256.copy_from_slice(context.finish().as_ref());
    Ok(sha256)
}

/// The retained metadata outputs are the objects the earlier passes made: the object copy completed its
/// step, and its files are exactly the bootstrap's admitted files, in order, with the admitted length and
/// SHA-256. Pure.
fn require_final_objects(
    boot: &VerifiedGitBootstrapV1,
    objects: &FinalObjectStoreV1,
) -> Result<(), GitBootstrapErrorV1> {
    if objects.step != GitPlaneStepV1::Objects {
        return Err(metadata_input("the object copy has not completed its step"));
    }
    let admitted = admitted_files(boot);
    if objects.files.len() != admitted.len() {
        return Err(metadata_input(format!(
            "the object copy holds {} files, and the bootstrap admitted {}",
            objects.files.len(),
            admitted.len()
        )));
    }
    for (made, admitted) in objects.files.iter().zip(admitted) {
        if (&made.relative, made.length, made.sha256)
            != (&admitted.relative, admitted.length, admitted.sha256)
        {
            return Err(metadata_input(format!(
                "{} is not the copy of the admitted file",
                made.relative
            )));
        }
    }
    Ok(())
}

/// One run of the metadata step: the retained bootstrap and object copy it builds on, and what it has
/// created, charged, and completed so far.
struct MetaRunV1<'a> {
    boot: &'a VerifiedGitBootstrapV1,
    objects: &'a FinalObjectStoreV1,
    deadline: Instant,
    frame_budget: CustodyFrameBudgetV1,
    table_bound: u64,
    limits: RefsLimitsV1,
    directories: BTreeMap<String, PinnedDirectoryV1>,
    files: Vec<MaterializedMetadataFileV1>,
    in_flight: Option<InFlightMetadataFileV1>,
    charged: MetadataChargeV1,
    began: bool,
    completed: Option<GitPlaneStepV1>,
}

impl<'a> MetaRunV1<'a> {
    fn dest(&self) -> &'a super::RestoreDestinationV1 {
        &self.boot.verified.destination
    }

    /// Every directory this step made is still the entry its parent holds, and (for [`MetaScopeV1::All`])
    /// every file is still the regular file its parent lists, of the identity and length it was measured
    /// with. Each step is held to the one deadline.
    fn metadata_chains(
        &self,
        scope: MetaScopeV1<'_>,
        pace: PaceV1<'_>,
    ) -> Result<(), GitBootstrapErrorV1> {
        let git = &self.objects.pins.git;
        let directory = |relative: &str| -> Result<(), GitBootstrapErrorV1> {
            let (parent, leaf) = split_relative(relative);
            let pin = pin_beneath(git, &self.directories, Some(relative))?;
            let above = pin_beneath(git, &self.directories, parent)?;
            recheck_entry(
                above,
                leaf.as_bytes(),
                pin,
                metadata_name(relative).as_bytes(),
            )?;
            Ok(())
        };
        match scope {
            MetaScopeV1::All => {
                for relative in self.directories.keys() {
                    pace("metadata-chain")?;
                    directory(relative)?;
                }
                for file in &self.files {
                    pace("metadata-chain")?;
                    let parent =
                        pin_beneath(git, &self.directories, split_relative(&file.relative).0)?;
                    recheck_regular_entry(
                        parent,
                        &file.relative,
                        (file.dev, file.ino, file.length),
                    )?;
                }
            }
            MetaScopeV1::Chain(Some(tail)) => {
                for (at, _) in tail.match_indices('/').chain([(tail.len(), "")]) {
                    pace("metadata-chain")?;
                    directory(&tail[..at])?;
                }
            }
            MetaScopeV1::Chain(None) => {}
        }
        Ok(())
    }

    /// The recheck: the object copy's chains, then the metadata chains in `scope`, then every admitted
    /// bootstrap file, then the staging, HOME, XDG, and bootstrap chains and the destination root's
    /// identity LAST.
    fn chains(&self, scope: MetaScopeV1<'_>, pace: PaceV1<'_>) -> Result<(), GitBootstrapErrorV1> {
        output_chains(
            self.dest(),
            PinViewV1::of_final(&self.objects.pins),
            &self.objects.files,
            pace,
        )?;
        self.metadata_chains(scope, pace)?;
        source_files_unchanged(self.boot, pace)?;
        sweep_chains(
            self.dest(),
            Some(&self.boot.home),
            Some(&self.boot.xdg),
            &self.boot.directories,
            pace,
        )
    }

    /// [`Self::chains`] over `scope`, then the clock: the recheck ends in the destination root's identity,
    /// which can block, so nothing the caller does next (a create, a charge, a write) may follow an expired
    /// deadline. Only the final gate, which has nothing fallible after it, omits this guard.
    fn recheck(&self, scope: MetaScopeV1<'_>) -> Result<(), GitBootstrapErrorV1> {
        let deadline = self.deadline;
        let pace = |step: &'static str| observe_deadline(deadline, step);
        self.chains(scope, &pace)?;
        observe_deadline(deadline, "metadata-checked")
    }

    /// The full gate: the mount census first, then every chain, then the clock (see [`Self::recheck`]).
    fn gate(&self) -> Result<(), GitBootstrapErrorV1> {
        let deadline = self.deadline;
        let pace = |step: &'static str| observe_deadline(deadline, step);
        pace("metadata-gate")?;
        refuse_mounts_within(&self.dest().root)?;
        self.chains(MetaScopeV1::All, &pace)?;
        observe_deadline(deadline, "metadata-checked")
    }

    /// The final gate: the mount census, every chain, and the destination root's identity LAST, the guard
    /// immediately before it being the last statement ahead of it.
    fn final_gate(&self) -> Result<(), GitBootstrapErrorV1> {
        #[cfg(test)]
        run_git_hook(
            GitHookPointV1::BeforeMetadataFinalGate,
            "",
            self.dest().root.canonical_path(),
        );
        let deadline = self.deadline;
        let pace = |step: &'static str| observe_deadline(deadline, step);
        pace("metadata-final")?;
        refuse_mounts_within(&self.dest().root)?;
        self.chains(MetaScopeV1::All, &pace)
    }

    /// Create one directory: a bounded recheck, then the create-new beneath its retained parent (its entry
    /// reserved first, given back only when the create provably made nothing), and a recheck that includes
    /// the new pin.
    fn make_directory(&mut self, relative: &str) -> Result<(), GitBootstrapErrorV1> {
        let (parent_relative, leaf) = split_relative(relative);
        self.recheck(MetaScopeV1::Chain(parent_relative))?;
        let dest = self.dest();
        let parent = pin_beneath(&self.objects.pins.git, &self.directories, parent_relative)?;
        #[cfg(test)]
        run_git_hook(
            GitHookPointV1::BeforeObjectCreate,
            "directory",
            &parent.canonical_path().join(leaf),
        );
        let created = create_staging_directory(
            parent,
            &dest.ledger,
            dest.dev,
            leaf.as_bytes(),
            metadata_name(relative).as_bytes(),
        );
        match created {
            Ok(pin) => {
                self.began = true;
                self.charged.entries += 1;
                self.directories.insert(relative.to_owned(), pin);
            }
            Err(error) => {
                // A collision and a refused reservation made nothing and charge nothing. Any other error
                // may follow a successful create, so its entry stays charged.
                if !matches!(
                    error,
                    CustodyRestoreErrorV1::StagingCollision { .. }
                        | CustodyRestoreErrorV1::Budget(_)
                ) {
                    self.began = true;
                    self.charged.entries += 1;
                }
                return Err(error.into());
            }
        }
        self.recheck(MetaScopeV1::Chain(Some(relative)))
    }

    /// Create one regular file beneath its retained parent and stream `length` bytes of `source` into it:
    /// a bounded recheck, the entry reserved, the create-new, the device, a recheck, the stream, and the
    /// file's own entry checked by name before it is kept. A collision is never reused: the preexisting
    /// entry is left exactly as it was.
    fn make_file(
        &mut self,
        relative: &str,
        length: u64,
        source: &mut dyn io::Read,
        source_fault: &dyn Fn() -> GitBootstrapErrorV1,
    ) -> Result<(), GitBootstrapErrorV1> {
        let deadline = self.deadline;
        let (parent_relative, leaf) = split_relative(relative);
        self.recheck(MetaScopeV1::Chain(parent_relative))?;
        let dest = self.dest();
        let name = metadata_name(relative);
        let parent = pin_beneath(&self.objects.pins.git, &self.directories, parent_relative)?;
        dest.ledger
            .borrow_mut()
            .reserve_entries(1)
            .map_err(ledger_error)?;
        self.charged.entries += 1;
        let hook_path = parent.canonical_path().join(leaf);
        #[cfg(test)]
        run_git_hook(GitHookPointV1::BeforeObjectCreate, "file", &hook_path);
        let mut output = match create_output_file(parent, leaf) {
            Ok(output) => output,
            Err(error) => {
                // A create that met an existing entry made nothing, so its allowance goes back; any other
                // error may follow a successful create and keeps the charge.
                if matches!(error, FsCustodyError::TargetExists(_)) {
                    dest.ledger.borrow_mut().release(ENTRY_ALLOWANCE_BYTES_V1);
                    self.charged.entries = self.charged.entries.saturating_sub(1);
                } else {
                    self.began = true;
                }
                return Err(staging_refusal(error, name.as_bytes()).into());
            }
        };
        self.began = true;
        self.in_flight = Some(InFlightMetadataFileV1 {
            relative: relative.to_owned(),
            expected_length: length,
            written: 0,
        });
        let created = ChildStatV1::from_metadata(
            &output
                .metadata()
                .map_err(|error| io_refusal(&name, &error))?,
        );
        let observed = created.dev;
        #[cfg(test)]
        let observed = super::CREATED_DEV
            .with(|devices| devices.borrow().get(name.as_bytes()).copied())
            .unwrap_or(observed);
        if observed != dest.dev {
            return Err(CustodyRestoreErrorV1::DeviceCrossing { name }.into());
        }
        self.recheck(MetaScopeV1::Chain(parent_relative))?;

        let sink = SinkV1 {
            dest,
            deadline,
            relative,
            hook_path: &hook_path,
            length,
        };
        let sha256 = stream_into(
            &sink,
            &mut self.charged,
            &mut self.in_flight,
            &mut output,
            source,
            source_fault,
        )?;
        let held = ChildStatV1::from_metadata(
            &output
                .metadata()
                .map_err(|error| io_refusal(&name, &error))?,
        );
        if (held.kind, held.size, held.dev) != (ChildKindV1::Regular, length, dest.dev) {
            return Err(metadata_copy(
                relative,
                "the written file is not a regular file of the frame's length on the root's device",
            ));
        }
        output
            .seek(SeekFrom::Start(0))
            .map_err(|error| io_refusal(&name, &error))?;
        recheck_regular_entry(parent, relative, (held.dev, held.ino, length))?;
        self.in_flight = None;
        self.files.push(MaterializedMetadataFileV1 {
            relative: relative.to_owned(),
            file: output,
            length,
            sha256,
            dev: held.dev,
            ino: held.ino,
        });
        self.recheck(MetaScopeV1::Chain(parent_relative))
    }

    /// Judge one decoded entry by the pass B1a rules again and create it: the admission BEFORE anything is
    /// created (the exact allowlist in its actual class, the entry type, the portable spelling, the
    /// collisions, the parent, and no symlink), the clock again (the decode can block), and then the
    /// directory or the file, streamed from `content`.
    fn restore_entry(
        &mut self,
        table: &mut ActivePathTableV1,
        entry: &FrameEntryV1<'_>,
        content: Option<CustodyFrameContentReaderV1<'_>>,
        source_fault: &dyn Fn() -> GitBootstrapErrorV1,
    ) -> Result<(), GitBootstrapErrorV1> {
        table
            .admit(entry.class, entry.path, entry.kind)
            .map_err(|fault| GitBootstrapErrorV1::ActiveFrame {
                class: entry.class,
                fault,
            })?;
        observe_deadline(self.deadline, "metadata-item")?;
        let relative = std::str::from_utf8(entry.path)
            .map_err(|_| metadata_input("an admitted frame path is not UTF-8"))?;
        match (entry.kind, content) {
            (ActiveEntryKindV1::Directory, _) => self.make_directory(relative),
            (ActiveEntryKindV1::Regular, Some(mut content)) => {
                self.make_file(relative, entry.length, &mut content, source_fault)
            }
            _ => Err(GitBootstrapErrorV1::ActiveFrame {
                class: entry.class,
                fault: ActiveFrameFaultV1::Symlink {
                    path: lossy(entry.path),
                },
            }),
        }
    }

    /// One retained active frame, restored: the clock, a recheck of everything made, the descriptor
    /// reverified against its recorded length and SHA-256 immediately before a second COMPLETE decode, and
    /// then every decoded entry judged by the pass B1a rules again and created. The decoder is drained
    /// through its verified trailer, the bytes it consumed are bound to the verified plaintext, and its
    /// entries to the ones prevalidation admitted. The first refusal of what was done with an entry is HELD
    /// (nothing after it is judged or created) until all of that has been verified, and then reported;
    /// the deadline and the decoder's own failure end the decode at once. Every byte already created stays.
    fn materialize_frame(
        &mut self,
        summary: &ActiveFrameSummaryV1,
        generation: &str,
        table: &mut ActivePathTableV1,
    ) -> Result<(), GitBootstrapErrorV1> {
        let class = summary.class;
        let deadline = self.deadline;
        observe_deadline(deadline, "metadata-frame")?;
        self.recheck(MetaScopeV1::All)?;
        let recorded = self
            .boot
            .verified
            .staged
            .get(summary.staged)
            .ok_or_else(|| plan_mismatch("a frame names no staged payload"))?;
        // The retained descriptor is read through a duplicate of itself, which shares its open file
        // description: no staged name is ever opened again.
        let mut one = StagedPlaintextV1 {
            name: recorded.name.clone(),
            role: recorded.role.clone(),
            file: recorded
                .file
                .try_clone()
                .map_err(|error| io_refusal(&lossy(recorded.name.as_bytes()), &error))?,
            length: recorded.length,
            sha256: recorded.sha256,
        };
        reverify_staged_v1(&mut one, deadline)?;
        let header =
            CustodyFrameHeaderV1::new(class, generation).map_err(|_| frame_invalid(class))?;
        let expired = Cell::new(false);
        let refuse = || {
            if expired.get() {
                GitBootstrapErrorV1::Git(CustodyGitError::Timeout)
            } else {
                frame_invalid(class)
            }
        };
        let mut counts = FrameCountsV1::default();
        let mut entry_digest = ActiveEntryDigestV1::new(class);
        // The first refusal of the entries themselves (an admission fault, or a failed create or write) is
        // HELD until the whole frame has decoded and its bytes are bound to the verified plaintext, so a
        // frame that is not whole or not the verified bytes is refused as THAT, whatever it said first.
        let mut held: Option<GitBootstrapErrorV1> = None;
        let mut paced = PacedFrameReaderV1 {
            inner: (&one.file).take(one.length.saturating_add(1)),
            deadline,
            expired: &expired,
            context: digest::Context::new(&digest::SHA256),
            count: 0,
        };
        {
            let reader = BufReader::with_capacity(HASH_BUFFER_BYTES_V1, &mut paced);
            let mut decoder = CustodyFrameDecoderV1::new(reader, &header, self.frame_budget)
                .map_err(|_| refuse())?;
            loop {
                before_deadline(deadline)?;
                let Some(entry) = decoder.next_entry().map_err(|_| refuse())? else {
                    break;
                };
                let (path, kind, length, content) = match entry {
                    CustodyFrameEntryV1::Directory { path, .. } => {
                        (path, ActiveEntryKindV1::Directory, 0, None)
                    }
                    CustodyFrameEntryV1::Symlink { path, .. } => {
                        (path, ActiveEntryKindV1::Symlink, 0, None)
                    }
                    CustodyFrameEntryV1::Regular {
                        path,
                        length,
                        content,
                        ..
                    } => (path, ActiveEntryKindV1::Regular, length, Some(content)),
                };
                #[cfg(test)]
                run_git_hook(
                    GitHookPointV1::FrameEntry,
                    "",
                    Path::new(OsStr::from_bytes(path.as_bytes())),
                );
                counts.entries = counts.entries.saturating_add(1);
                match kind {
                    ActiveEntryKindV1::Directory => {
                        counts.directories = counts.directories.saturating_add(1);
                    }
                    _ => {
                        counts.regular_files = counts.regular_files.saturating_add(1);
                        counts.content_bytes = counts.content_bytes.saturating_add(length);
                    }
                }
                entry_digest.record(kind, path.as_bytes(), length);
                // Once a fault is held nothing more is judged or created: the rest of the frame is only
                // decoded, so its trailer and its bytes are still verified (an unread remainder of the
                // current content is drained and verified by the decoder's next call).
                if held.is_some() {
                    continue;
                }
                let entry = FrameEntryV1 {
                    class,
                    path: path.as_bytes(),
                    kind,
                    length,
                };
                match self.restore_entry(table, &entry, content, &refuse) {
                    Ok(()) => {}
                    Err(error) if ends_the_decode(&error) => return Err(error),
                    Err(error) => held = Some(error),
                }
            }
        }
        // The decoder met the verified trailer and the end of the stream. The bytes it consumed must be
        // exactly the verified plaintext: a frame replaced after its rehash is refused here.
        let (count, context) = (paced.count, paced.context);
        let mut decoded_sha256 = [0_u8; 32];
        decoded_sha256.copy_from_slice(context.finish().as_ref());
        if count != one.length || decoded_sha256 != one.sha256 {
            return Err(GitBootstrapErrorV1::StagedInputChanged {
                name: lossy(one.name.as_bytes()),
            });
        }
        if (
            summary.entries,
            summary.directories,
            summary.regular_files,
            summary.content_bytes,
            summary.entries_sha256,
        ) != (
            counts.entries,
            counts.directories,
            counts.regular_files,
            counts.content_bytes,
            entry_digest.finish(),
        ) {
            return Err(plan_mismatch(
                "a frame's entries are not the ones B1a admitted",
            ));
        }
        if let Some(error) = held {
            return Err(error);
        }
        observe_deadline(deadline, "metadata-decoded")?;
        one.file
            .seek(SeekFrom::Start(0))
            .map_err(|error| io_refusal(&lossy(one.name.as_bytes()), &error))?;
        Ok(())
    }

    /// Cross-check EVERY restored ref against the manifest and the verified inventory, reading only the
    /// retained FINAL descriptors: the clock, a recheck of everything made, and then each file read ONCE,
    /// through a reader that hashes and counts exactly the bytes the pass B1b parsers consume, which then
    /// judge the whole. The first refusal of the refs themselves is HELD until the file has been read to its
    /// end and its digest and length are compared with the ones it was written with, so a changed file is
    /// refused as that ahead of anything it says, and no byte the parser saw escapes the digest.
    fn check_refs(&mut self) -> Result<RefsAgreementV1, GitBootstrapErrorV1> {
        let deadline = self.deadline;
        observe_deadline(deadline, "metadata-refs")?;
        self.recheck(MetaScopeV1::All)?;
        let mut builder = RefsBuilderV1::new(self.boot.object_format, self.limits, deadline);
        for file in &mut self.files {
            file.file
                .seek(SeekFrom::Start(0))
                .map_err(|error| io_refusal(&file.relative, &error))?;
            let mut reader = DigestedReaderV1 {
                inner: (&file.file).take(file.length.saturating_add(1)),
                context: digest::Context::new(&digest::SHA256),
                count: 0,
                #[cfg(test)]
                hook_path: self.objects.pins.git.canonical_path().join(&file.relative),
            };
            let held = match builder.admit_file(file.relative.as_bytes(), &mut reader) {
                Ok(()) => None,
                Err(error) if ends_the_parse(&error) => return Err(error),
                Err(error) => Some(error),
            };
            // Whatever the parsers left unread is read now, so the digest covers the whole file.
            drain_stream(&mut reader, deadline, &file.relative)?;
            let (count, context) = (reader.count, reader.context);
            let mut sha256 = [0_u8; 32];
            sha256.copy_from_slice(context.finish().as_ref());
            if count != file.length || sha256 != file.sha256 {
                return Err(metadata_copy(
                    &file.relative,
                    "the retained descriptor is not the bytes that were written",
                ));
            }
            if let Some(error) = held {
                return Err(error);
            }
            file.file
                .seek(SeekFrom::Start(0))
                .map_err(|error| io_refusal(&file.relative, &error))?;
        }
        let agreement = builder.finish(&self.boot.verified.control.manifest)?;
        observe_deadline(deadline, "metadata-agreed")?;
        Ok(agreement)
    }

    /// The inert config, created ONCE, only after the refs agree: the gate, then the one create-new of
    /// `config` beneath `.git` carrying exactly the golden bytes of the caller's format, read back from its
    /// own descriptor. The bootstrap's config, and any archived one, is never consulted.
    fn create_config(&mut self) -> Result<(), GitBootstrapErrorV1> {
        let deadline = self.deadline;
        observe_deadline(deadline, "metadata-config")?;
        self.gate()?;
        let bytes = encode_inert_config_v1(self.boot.object_format);
        let mut sha256 = [0_u8; 32];
        sha256.copy_from_slice(digest::digest(&digest::SHA256, &bytes).as_ref());
        let mut source: &[u8] = &bytes;
        self.make_file(CONFIG_LEAF_V1, bytes.len() as u64, &mut source, &|| {
            metadata_input("the config source failed")
        })?;
        let file = self
            .files
            .last_mut()
            .filter(|file| file.relative == CONFIG_LEAF_V1)
            .ok_or_else(|| metadata_input("the config was not kept"))?;
        file.file
            .seek(SeekFrom::Start(0))
            .map_err(|error| io_refusal(&file.relative, &error))?;
        let (length, read_back) =
            stream_digest(&mut file.file, file.length, deadline, &file.relative)?;
        if length != file.length || read_back != sha256 || file.sha256 != sha256 {
            return Err(metadata_copy(
                &file.relative,
                "the written config is not the golden bytes",
            ));
        }
        file.file
            .seek(SeekFrom::Start(0))
            .map_err(|error| io_refusal(&file.relative, &error))?;
        self.recheck(MetaScopeV1::Chain(None))
    }

    fn run(&mut self, plan: &ActiveFramePlanV1) -> Result<FinalMetadataV1, GitBootstrapErrorV1> {
        let deadline = self.deadline;
        observe_deadline(deadline, "metadata-entry")?;
        let boot = self.boot;
        require_prevalidated(boot, plan)?;
        require_proven_bootstrap(boot)?;
        require_final_objects(boot, self.objects)?;
        self.gate()?;
        let generation = boot.verified.control.manifest.generation_id().to_owned();
        let mut table = ActivePathTableV1::new(self.table_bound);
        for summary in &plan.frames {
            self.materialize_frame(summary, &generation, &mut table)?;
        }
        table
            .finish()
            .map_err(|fault| GitBootstrapErrorV1::ActiveFrame {
                class: CustodyCoverageClassV1::RefsAndHead,
                fault,
            })?;
        self.gate()?;
        self.completed = Some(GitPlaneStepV1::Metadata);

        let agreement = self.check_refs()?;
        self.completed = Some(GitPlaneStepV1::Refs);

        self.create_config()?;
        self.final_gate()?;
        Ok(FinalMetadataV1 {
            step: GitPlaneStepV1::Config,
            directories: std::mem::take(&mut self.directories),
            files: std::mem::take(&mut self.files),
            agreement,
            charged: self.charged,
        })
    }
}

/// Restore the active metadata beneath the retained `.git` of `objects`, agree it with the manifest's
/// original refs, and create the inert config.
///
/// Nothing is created unless `plan` is this capsule's complete active prevalidation (an Empty `RefsAndHead`
/// is [`GitBootstrapErrorV1::EmptyRefsAndHead`]), `bootstrap` is a proven object store, and `objects` is
/// the copy of it that [`materialize_object_store_v1`] made. Each retained active frame is rehashed from
/// its descriptor immediately before it is decoded a second time, every entry is judged by the pass B1a
/// rules again before it is created, and every directory and file is create-new beneath a retained pin that
/// this invocation created (the entry, and then each chunk, charged to the one ledger first). The bytes are
/// the frame's, exactly: no symlink, no normalization, no archived or bootstrap config, hook, or other
/// behavior-affecting entry ever exists. After every frame is restored, every file is read back from its
/// retained descriptor and fed to the pass B1b parsers, which agree the whole with the manifest's original
/// refs and the verified inventory; only then is the config created, once, with the golden bytes of the
/// caller's format. No Git child runs, the repository's only child stays `.git`, and the bootstrap is never
/// touched. Nothing is renamed, overwritten, deleted, or cleaned up.
///
/// Files are created with the create-new primitive's owner-private mode and directories with its `0700`; the
/// frames' recorded modes are not applied, and no file or directory is individually synced here (the sync of
/// the new directory entry that each directory create performs is the primitive's own). Nothing in this slice
/// depends on either.
///
/// Every step keeps the ONE absolute `deadline`; the destination root's identity is the last fallible check
/// and only infallible construction follows it. A refusal keeps every created byte and is in
/// [`MetadataFailureV1`]'s shape: the last step completed whole, whether this invocation began creating,
/// the one ledger's charge, and typed partial effects. A second invocation over the same `.git` refuses at
/// its first entry as a collision, touching nothing.
pub(crate) fn materialize_active_metadata_v1(
    bootstrap: &VerifiedGitBootstrapV1,
    objects: &FinalObjectStoreV1,
    plan: &ActiveFramePlanV1,
    frame_budget: CustodyFrameBudgetV1,
    deadline: Instant,
    completed: Option<GitPlaneStepV1>,
) -> Result<FinalMetadataV1, Box<MetadataFailureV1>> {
    materialize_active_metadata_bounded_v1(
        bootstrap,
        objects,
        plan,
        frame_budget,
        deadline,
        completed,
        ACTIVE_PATH_TABLE_BYTES_V1,
        RefsLimitsV1::v1(),
    )
}

/// [`materialize_active_metadata_v1`] with the aggregate path table's byte bound and the refs limits as
/// parameters, so each is exercised at a small size.
#[allow(clippy::too_many_arguments)]
pub(crate) fn materialize_active_metadata_bounded_v1(
    bootstrap: &VerifiedGitBootstrapV1,
    objects: &FinalObjectStoreV1,
    plan: &ActiveFramePlanV1,
    frame_budget: CustodyFrameBudgetV1,
    deadline: Instant,
    completed: Option<GitPlaneStepV1>,
    table_bound: u64,
    limits: RefsLimitsV1,
) -> Result<FinalMetadataV1, Box<MetadataFailureV1>> {
    let mut run = MetaRunV1 {
        boot: bootstrap,
        objects,
        deadline,
        frame_budget,
        table_bound,
        limits,
        directories: BTreeMap::new(),
        files: Vec::new(),
        in_flight: None,
        charged: MetadataChargeV1::default(),
        began: false,
        completed,
    };
    match run.run(plan) {
        Ok(done) => Ok(done),
        Err(cause) => Err(Box::new(MetadataFailureV1 {
            cause,
            last_completed: run.completed,
            active_materialization_began: run.began,
            ledger_used: bootstrap.verified.destination.ledger.borrow().used(),
            partial: MetadataPartialV1 {
                directories: run.directories,
                files: run.files,
                in_flight: run.in_flight,
                charged: run.charged,
            },
        })),
    }
}

// ---------------------------------------------------------------------------------------------
// Pass B2c: final closure, typed result and wrapper
// ---------------------------------------------------------------------------------------------

/// How the final tree differs from what the restore made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FinalTreeFaultV1 {
    /// An entry the restore did not make.
    Unexpected,
    /// An entry the restore made is gone.
    Missing,
    /// An entry that is neither a directory nor a regular file.
    Special,
    /// More entries than the restore made.
    Bound,
    /// A name that is no longer the retained inode.
    Replaced,
    /// The retained inode, but not the length or SHA-256 it was restored with.
    Bytes,
}

/// The final database's proof: the evidence (every inventory object, reached, strict fsck) and each final
/// child's recorded evidence, in order. Exit status alone is never evidence.
#[derive(Debug)]
pub(crate) struct FinalClosureV1 {
    pub(crate) evidence: GitClosureEvidenceV1,
    pub(crate) runs: Vec<GitRunRecordV1>,
}

/// Phase G's result, retained for payload restoration. It holds the ORIGINAL verified capsule (inside
/// [`Self::bootstrap`], with its one ledger and every staged descriptor, each back at offset 0), every
/// staging, HOME, XDG, and bootstrap pin, the final repository's pins and every materialized descriptor, the
/// format provenance, the bootstrap's closure evidence, the pre-write agreement, and the final database's
/// closure evidence. It is NOT a restore record: no payload, index, evidence, or record exists.
#[derive(Debug)]
pub(crate) struct RestoredGitPlaneV1 {
    /// The phase-G step this completes: [`GitPlaneStepV1::FinalGate`].
    pub(crate) step: GitPlaneStepV1,
    pub(crate) bootstrap: VerifiedGitBootstrapV1,
    pub(crate) plan: ActiveFramePlanV1,
    /// What the frames agreed with the manifest before anything was written beneath the repository.
    pub(crate) prewrite_agreement: RefsAgreementV1,
    pub(crate) objects: FinalObjectStoreV1,
    pub(crate) metadata: FinalMetadataV1,
    pub(crate) closure: FinalClosureV1,
}

impl RestoredGitPlaneV1 {
    /// The original verified capsule.
    pub(crate) fn capsule(&self) -> &VerifiedCapsuleV1 {
        &self.bootstrap.verified
    }

    pub(crate) fn object_format(&self) -> GitObjectFormatV1 {
        self.bootstrap.object_format
    }

    pub(crate) fn provenance(&self) -> GitFormatProvenanceV1 {
        self.bootstrap.provenance
    }

    /// The one ledger's charge.
    pub(crate) fn ledger_used(&self) -> u64 {
        self.bootstrap.verified.destination.ledger.borrow().used()
    }
}

/// What a refused object copy left, or the object store it made.
#[derive(Debug, Default)]
pub(crate) enum PlaneObjectsV1 {
    #[default]
    NotStarted,
    Refused(ObjectStorePartialV1),
    Whole(FinalObjectStoreV1),
}

/// What a refused metadata step left, or the metadata it made.
#[derive(Debug, Default)]
pub(crate) enum PlaneMetadataV1 {
    #[default]
    NotStarted,
    Refused(MetadataPartialV1),
    Whole(FinalMetadataV1),
}

/// Every value a refused restore still holds. Nothing here is ever removed: the bytes it describes stay on
/// disk, and dropping these closes only descriptors.
#[derive(Debug, Default)]
pub(crate) struct GitPlanePartialV1 {
    /// The bootstrap value, with the original capsule, its pins, and its one ledger. Absent only when the
    /// bootstrap itself refused: it consumed the capsule, and reports its own figure.
    pub(crate) bootstrap: Option<VerifiedGitBootstrapV1>,
    /// B1a's plan, once the frames were prevalidated.
    pub(crate) plan: Option<ActiveFramePlanV1>,
    pub(crate) objects: PlaneObjectsV1,
    pub(crate) metadata: PlaneMetadataV1,
    /// The final children that returned a result, in order.
    pub(crate) final_runs: Vec<GitRunRecordV1>,
}

/// A refused or failed restore, with how far it got. `last_completed` is the last step that completed whole
/// ([`GitPlaneStepV1::FinalGate`] is only ever a success), and `active_materialization_began` is whether any
/// entry beneath the repository may have been created. Every byte created before the failure is preserved,
/// and a second restore over the same destination refuses at phase V.
#[derive(Debug)]
pub(crate) struct GitPlaneRestoreFailureV1 {
    pub(crate) cause: GitBootstrapErrorV1,
    pub(crate) last_completed: Option<GitPlaneStepV1>,
    pub(crate) active_materialization_began: bool,
    /// The one ledger's charge when the failure was returned.
    pub(crate) ledger_used: u64,
    pub(crate) partial: GitPlanePartialV1,
}

fn final_input(why: impl Into<String>) -> GitBootstrapErrorV1 {
    GitBootstrapErrorV1::FinalInput(why.into())
}

fn final_tree(name: String, fault: FinalTreeFaultV1) -> GitBootstrapErrorV1 {
    GitBootstrapErrorV1::FinalTree { name, fault }
}

/// The destination-relative name of the final directory or file at `relative` beneath the Git directory
/// (`""` is the Git directory itself).
fn final_name(relative: &str) -> String {
    if relative.is_empty() {
        ObjectDirV1::Git.name().to_owned()
    } else {
        metadata_name(relative)
    }
}

/// A listing that ran past the census bound is a refusal of its own, naming the directory that was listed;
/// every other listing error is custody's.
fn final_listing_error(error: FsCustodyError, listed: String) -> GitBootstrapErrorV1 {
    match error {
        FsCustodyError::EnumerationLimitExceeded { .. } => {
            final_tree(listed, FinalTreeFaultV1::Bound)
        }
        other => fs_error(other),
    }
}

/// Whether `error` is the typed timeout the one deadline produces (a guard's, the runner's, a bounded read's).
fn is_deadline_timeout(error: &GitBootstrapErrorV1) -> bool {
    matches!(error, GitBootstrapErrorV1::Git(CustodyGitError::Timeout))
}

/// A path beneath the Git directory as its EXACT bytes (the empty path is the Git directory itself): the one
/// identity the census gives a name. A directory entry is one byte component (no `/`, no NUL), so a parent's bytes,
/// a `/`, and one exact component keep both boundaries and the join is injective, which a lossy rendering is not
/// (every maximal invalid sequence and a literal U+FFFD render alike). Text is made from it only to name a finding
/// ([`Self::render`]); no lookup, membership, missing check, fact, or duplicate ever reads that text back.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct GitCensusPathV1(Vec<u8>);

impl GitCensusPathV1 {
    /// The Git directory itself.
    fn root() -> Self {
        Self(Vec::new())
    }

    /// The key of a name the restore made (an admitted relative path): its own bytes, taken once.
    fn expected(relative: &str) -> Self {
        Self(relative.as_bytes().to_vec())
    }

    fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// The entry of this directory named by one exact component.
    fn child(&self, name: &[u8]) -> Self {
        let mut bytes = self.0.clone();
        if !bytes.is_empty() {
            bytes.push(b'/');
        }
        bytes.extend_from_slice(name);
        Self(bytes)
    }

    /// The directory this path is an entry of (`None` for the Git directory itself).
    fn parent(&self) -> Option<Self> {
        if self.0.is_empty() {
            return None;
        }
        Some(match self.0.iter().rposition(|byte| *byte == b'/') {
            Some(at) => Self(self.0[..at].to_vec()),
            None => Self::root(),
        })
    }

    /// The exact bytes of the last component.
    fn leaf(&self) -> &[u8] {
        match self.0.iter().rposition(|byte| *byte == b'/') {
            Some(at) => &self.0[at + 1..],
            None => &self.0,
        }
    }

    /// The text that names this path in a finding (lossy for a name that is not UTF-8): a diagnostic only.
    fn render(&self) -> String {
        lossy(&self.0)
    }
}

/// A directory of the census: the repository, or the Git directory or one the restore made beneath it. The two
/// namespaces are variants, so no string sentinel stands for either.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum CensusDirectoryKeyV1 {
    Repository,
    Git(GitCensusPathV1),
}

impl CensusDirectoryKeyV1 {
    /// The key of its entry named by one exact raw component.
    fn entry(&self, name: &[u8]) -> CensusEntryKeyV1 {
        match self {
            Self::Repository => CensusEntryKeyV1::RepositoryChild(name.to_vec()),
            Self::Git(path) => CensusEntryKeyV1::Git(path.child(name)),
        }
    }

    /// The destination-relative text a finding, a fact's refusal, and the device seam call this directory.
    fn seam(&self) -> String {
        match self {
            Self::Repository => ObjectDirV1::Repository.name().to_owned(),
            Self::Git(path) => final_name(&path.render()),
        }
    }
}

/// An entry the census lists, by its exact bytes: a child of the repository, or a path beneath the Git directory.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum CensusEntryKeyV1 {
    RepositoryChild(Vec<u8>),
    Git(GitCensusPathV1),
}

/// Where a finding of the census was made.
#[derive(Clone, Debug, PartialEq, Eq)]
enum CensusFaultAtV1 {
    Directory(CensusDirectoryKeyV1),
    Entry(CensusEntryKeyV1),
}

/// What kind of finding it is, read from the typed error and never from a name it carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CensusFaultClassV1 {
    Tree(FinalTreeFaultV1),
    Link,
    Identity,
    Device,
    Custody,
}

/// The typed identity of one finding of the census: where it was made and what kind it is. Two findings are the
/// same finding exactly when these are equal; the text of their public errors is never compared, so two raw names
/// that render alike are two findings that may look alike.
#[derive(Clone, Debug, PartialEq, Eq)]
struct CensusFaultKeyV1 {
    at: CensusFaultAtV1,
    class: CensusFaultClassV1,
}

impl CensusFaultKeyV1 {
    fn of(at: CensusFaultAtV1, error: &GitBootstrapErrorV1) -> Self {
        let class = match error {
            GitBootstrapErrorV1::FinalTree { fault, .. } => CensusFaultClassV1::Tree(*fault),
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::SymlinkRefused { .. }) => {
                CensusFaultClassV1::Link
            }
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::IdentityChanged(_)) => {
                CensusFaultClassV1::Identity
            }
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::DeviceCrossing { .. }) => {
                CensusFaultClassV1::Device
            }
            _ => CensusFaultClassV1::Custody,
        };
        Self { at, class }
    }
}

/// One finding a completion keeps: its typed identity and the public error built for it.
#[derive(Debug)]
struct CensusFindingV1 {
    key: CensusFaultKeyV1,
    error: GitBootstrapErrorV1,
}

/// One regular file the restore made, as it was measured when it was made, and the descriptor the restore
/// retained of it (the very file object, read-write, that its store owns): the one thing a failure that is
/// already known may read the file's bytes from, because a name may be another inode by then.
#[derive(Clone, Copy)]
struct MadeFileV1<'a> {
    dev: u64,
    ino: u64,
    length: u64,
    sha256: [u8; 32],
    file: &'a File,
}

/// Everything the restore made beneath the repository, by its exact path beneath the Git directory (the empty
/// path is the Git directory itself): each directory's retained pin, and each regular file's measured identity,
/// length, and SHA-256. The one account the final tree is held to. Its names are the admitted ASCII text the
/// restore made, taken once as bytes ([`GitCensusPathV1::expected`]): a listed name is a member only by equal
/// bytes, never by what it renders as.
struct MadeTreeV1<'a> {
    repository: &'a PinnedDirectoryV1,
    git: &'a PinnedDirectoryV1,
    directories: BTreeMap<GitCensusPathV1, &'a PinnedDirectoryV1>,
    files: BTreeMap<GitCensusPathV1, MadeFileV1<'a>>,
}

impl<'a> MadeTreeV1<'a> {
    fn of(
        objects: &'a FinalObjectStoreV1,
        metadata: &'a FinalMetadataV1,
    ) -> Result<Self, GitBootstrapErrorV1> {
        let pins = &objects.pins;
        let mut directories: BTreeMap<GitCensusPathV1, &PinnedDirectoryV1> = BTreeMap::from([
            (GitCensusPathV1::root(), &pins.git),
            (GitCensusPathV1::expected("objects"), &pins.objects),
            (GitCensusPathV1::expected("objects/pack"), &pins.pack),
            (GitCensusPathV1::expected("objects/info"), &pins.info),
        ]);
        for (relative, pin) in &metadata.directories {
            if directories
                .insert(GitCensusPathV1::expected(relative), pin)
                .is_some()
            {
                return Err(final_input(format!(
                    "{relative} is a directory of both stores"
                )));
            }
        }
        let mut files = BTreeMap::new();
        for file in objects.files.iter().chain(&metadata.files) {
            let made = MadeFileV1 {
                dev: file.dev,
                ino: file.ino,
                length: file.length,
                sha256: file.sha256,
                file: &file.file,
            };
            if files
                .insert(GitCensusPathV1::expected(&file.relative), made)
                .is_some()
            {
                return Err(final_input(format!(
                    "{} is a file of both stores",
                    file.relative
                )));
            }
        }
        if let Some(path) = files.keys().find(|path| directories.contains_key(*path)) {
            return Err(final_input(format!(
                "{} is both a directory and a file",
                path.render()
            )));
        }
        Ok(Self {
            repository: &pins.repository,
            git: &pins.git,
            directories,
            files,
        })
    }
}

/// A census taken after a child must equal the baseline taken before it: an added, removed, replaced, or resized
/// entry, a file whose mode or timestamps moved, and a directory whose identity, mode, or timestamps changed
/// are each the refusal of a read-only child that changed the final repository.
fn final_unchanged(
    baseline: &ProofFactsV1,
    measured: &ProofFactsV1,
    label: &'static str,
) -> Result<(), GitBootstrapErrorV1> {
    if measured == baseline {
        return Ok(());
    }
    let changed = match measured
        .files
        .iter()
        .find(|(path, fact)| baseline.files.get(*path) != Some(*fact))
    {
        Some((path, _)) => format!(
            "{} was replaced, resized, or had its mode or timestamps changed",
            path.render()
        ),
        None => match measured
            .directories
            .iter()
            .find(|(key, fact)| baseline.directories.get(*key) != Some(*fact))
        {
            Some((key, _)) => {
                format!(
                    "{} had its mode, timestamps, or identity changed",
                    key.seam()
                )
            }
            None => "its entries changed".to_owned(),
        },
    };
    Err(GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Budget(
        format!("{label}: a read-only child changed the final repository: {changed}"),
    )))
}

/// What one census of the final tree saw: every regular file, by its exact path beneath the Git directory, with
/// the facts its own no-follow descriptor reported, and every directory (the repository and each retained
/// directory), by its typed key, with the non-atime facts its retained descriptor reported (identity, mode, and
/// timestamps: a `chmod` or a touch of a restored directory is an effect). Two censuses are equal exactly when
/// nothing changed.
#[derive(Debug, Default, PartialEq, Eq)]
struct ProofFactsV1 {
    files: BTreeMap<GitCensusPathV1, FileFactV1>,
    directories: BTreeMap<CensusDirectoryKeyV1, FileFactV1>,
}

/// The listed entry `stat` must be the retained directory `pin`, on the root's device. A link is refused and
/// never followed, a special file is refused, and a regular file or another directory is a replacement.
/// `child` names the entry in a refusal, and `seam` is its destination-relative name for the device seam.
fn require_final_directory(
    stat: &ChildStatV1,
    child: &str,
    seam: &str,
    pin: &PinnedDirectoryV1,
    dev: u64,
) -> Result<(), GitBootstrapErrorV1> {
    match stat.kind {
        ChildKindV1::Directory => {}
        ChildKindV1::Symlink => {
            return Err(GitBootstrapErrorV1::Restore(
                CustodyRestoreErrorV1::SymlinkRefused {
                    name: child.to_owned(),
                },
            ))
        }
        ChildKindV1::Special(_) => {
            return Err(final_tree(child.to_owned(), FinalTreeFaultV1::Special))
        }
        ChildKindV1::Regular => {
            return Err(final_tree(child.to_owned(), FinalTreeFaultV1::Replaced))
        }
    }
    if Some(stat.dev) != pin.identity().dev || Some(stat.ino) != pin.identity().ino {
        return Err(GitBootstrapErrorV1::Restore(
            CustodyRestoreErrorV1::IdentityChanged(format!(
                "{child}: the entry is no longer the retained directory"
            )),
        ));
    }
    if observed_dev(seam, stat.dev) != dev {
        return Err(GitBootstrapErrorV1::Restore(
            CustodyRestoreErrorV1::DeviceCrossing {
                name: seam.to_owned(),
            },
        ));
    }
    Ok(())
}

/// The descriptor the restore retained of a file it made must still be that file, regular, of the length it was
/// made with: its own `fstat`, never a name. A descriptor that cannot be `fstat`ed is the typed refusal of its
/// file; one that is not the file the restore made, or of another length, is that file's own finding.
fn require_made_file(
    file: &File,
    made: &MadeFileV1<'_>,
    relative: &str,
) -> Result<(), GitBootstrapErrorV1> {
    let held = file
        .metadata()
        .map_err(|error| io_refusal(relative, &error))?;
    let held = ChildStatV1::from_metadata(&held);
    if held.kind != ChildKindV1::Regular || (held.dev, held.ino) != (made.dev, made.ino) {
        return Err(digest_identity(
            relative,
            "the retained descriptor is no longer the file the restore made",
        ));
    }
    if held.size != made.length {
        return Err(final_tree(relative.to_owned(), FinalTreeFaultV1::Bytes));
    }
    Ok(())
}

/// A proof stream's SHAPE outranks its exit status: output that is not UTF-8, ends in a partial line, or holds a
/// malformed row is a [`GitBootstrapErrorV1::GitOutput`] whatever status the child exited with, because it
/// says the stream itself broke the contract. A well-formed stream that disagrees with the manifest does not:
/// when the child also failed, its status is the better diagnosis (a crash after a clean prefix is a failed
/// child, not every later object missing). On a success status the parser's own verdict stands.
fn shape_then_status<T>(
    result: &GitRunResultV1,
    command: &'static str,
    parsed: Result<T, GitBootstrapErrorV1>,
) -> Result<T, GitBootstrapErrorV1> {
    match parsed {
        Err(error @ GitBootstrapErrorV1::GitOutput(_)) => Err(error),
        other => {
            require_success(result, command)?;
            other
        }
    }
}

/// [`output_chains`] for the final gates: the same directories and files in the same order and with the same
/// refusals, every one its own unit of `pass`, with the same clock before each.
fn final_output_chains(
    dest: &super::RestoreDestinationV1,
    pins: PinViewV1<'_>,
    files: &[MaterializedObjectFileV1],
    pace: PaceV1<'_>,
    pass: &ChainPassV1,
) -> Result<(), GitBootstrapErrorV1> {
    for dir in ObjectDirV1::ALL {
        let Some(pin) = pins.get(dir) else { continue };
        pass.unit(|| {
            pace("objects-chain")?;
            let parent = match dir.parent() {
                None => &dest.root,
                Some(parent) => pins.get(parent).ok_or_else(|| {
                    CustodyRestoreErrorV1::IdentityChanged(format!(
                        "{}: its parent is not retained",
                        dir.name()
                    ))
                })?,
            };
            recheck_entry(parent, dir.leaf().as_bytes(), pin, dir.name().as_bytes())?;
            Ok(())
        })?;
    }
    for file in files {
        pass.unit(|| {
            pace("objects-chain")?;
            let pack = pins.get(ObjectDirV1::Pack).ok_or_else(|| {
                CustodyRestoreErrorV1::IdentityChanged(format!(
                    "{}: the pack directory is not retained",
                    file.relative
                ))
            })?;
            let stat = pack
                .child_metadata_no_follow(
                    OsStr::new(leaf_of(&file.relative)),
                    "custody restore object store file",
                )
                .map_err(fs_error)?
                .ok_or_else(|| {
                    digest_identity(&file.relative, "the copied file's entry is gone")
                })?;
            match stat.kind {
                ChildKindV1::Symlink => {
                    return Err(GitBootstrapErrorV1::Restore(
                        CustodyRestoreErrorV1::SymlinkRefused {
                            name: file.relative.clone(),
                        },
                    ))
                }
                ChildKindV1::Regular => {}
                ChildKindV1::Directory | ChildKindV1::Special(_) => {
                    return Err(digest_identity(
                        &file.relative,
                        "the entry is no longer the copied file",
                    ))
                }
            }
            if (stat.dev, stat.ino, stat.size) != (file.dev, file.ino, file.length) {
                return Err(digest_identity(
                    &file.relative,
                    "the entry is no longer the copied file",
                ));
            }
            Ok(())
        })?;
    }
    Ok(())
}

/// [`source_files_unchanged`] for the final gates, the same two reads of each admitted bootstrap file in the
/// same order and with the same refusals, each its OWN unit of `pass` behind its own clock (one call per unit):
/// the entry by name (`pace` runs before its `fstatat`), then the file's retained descriptor (`pace` runs between
/// the two, so no call is made after a slow one without the clock). A deadline that stops the second leaves the
/// first attempted, and a completion makes only the second; the two are independent, so a refusal of one never
/// hides the other.
fn final_source_files_unchanged(
    boot: &VerifiedGitBootstrapV1,
    pace: PaceV1<'_>,
    pass: &ChainPassV1,
) -> Result<(), GitBootstrapErrorV1> {
    for admitted in admitted_files(boot) {
        pass.unit(|| {
            pace("final-source")?;
            let pack = boot
                .directories
                .get(PACK_DIRECTORY_V1)
                .ok_or_else(|| object_input("the bootstrap's pack directory is not retained"))?;
            let leaf = OsStr::new(leaf_of(&admitted.relative));
            let stat = pack
                .child_metadata_no_follow(leaf, "custody restore admitted object file")
                .map_err(fs_error)?;
            #[cfg(test)]
            run_git_hook(
                GitHookPointV1::FinalSourceStatted,
                "",
                &pack.canonical_path().join(leaf),
            );
            let stat = stat.ok_or_else(|| {
                digest_identity(&admitted.relative, "no longer the admitted object file")
            })?;
            match stat.kind {
                ChildKindV1::Symlink => {
                    return Err(GitBootstrapErrorV1::Restore(
                        CustodyRestoreErrorV1::SymlinkRefused {
                            name: admitted.relative.clone(),
                        },
                    ))
                }
                ChildKindV1::Regular => {}
                ChildKindV1::Directory | ChildKindV1::Special(_) => {
                    return Err(digest_identity(
                        &admitted.relative,
                        "no longer the admitted object file",
                    ))
                }
            }
            if (stat.dev, stat.ino, stat.size) != (admitted.dev, admitted.ino, admitted.length) {
                return Err(digest_identity(
                    &admitted.relative,
                    "no longer the admitted object file",
                ));
            }
            Ok(())
        })?;
        pass.unit(|| {
            pace("final-source-statted")?;
            let held = admitted
                .file
                .metadata()
                .map_err(|error| io_refusal(&admitted.relative, &error))?;
            let held = ChildStatV1::from_metadata(&held);
            if (held.kind, held.dev, held.ino, held.size)
                != (
                    ChildKindV1::Regular,
                    admitted.dev,
                    admitted.ino,
                    admitted.length,
                )
            {
                return Err(digest_identity(
                    &admitted.relative,
                    "the retained descriptor is no longer the admitted object file",
                ));
            }
            Ok(())
        })?;
    }
    Ok(())
}

/// [`sweep_chains`] for the final gates: the same chains in the same order, the destination root's identity
/// LAST. Every retained EDGE is its own unit of `pass`, ONE guard and ONE call each, attempted once: the work
/// directory's entry beneath the root, the plain directory's beneath the work directory, and each retained staging
/// directory's beneath its retained parent (the plain directory, or the retained pin of its key's parent: the keys
/// are in byte order, so a parent comes before its child), then HOME, XDG, each bootstrap directory, and the root.
/// No edge is checked again for another's sake (the work and plain entries, and every prefix of a key, used to be
/// replayed for each staging key, so one refusal both hid the next edge and was repeated for each key). A completion
/// goes on past a refusal, and the root is always the last unit.
fn final_sweep_chains(
    dest: &super::RestoreDestinationV1,
    home: Option<&PinnedDirectoryV1>,
    xdg: Option<&PinnedDirectoryV1>,
    dirs: &BTreeMap<String, PinnedDirectoryV1>,
    pace: PaceV1<'_>,
    pass: &ChainPassV1,
) -> Result<(), GitBootstrapErrorV1> {
    let work = RESTORE_WORK_DIR_NAME_V1.as_bytes();
    pass.unit(|| {
        pace("chain")?;
        recheck_entry(&dest.root, work, &dest.work, work)?;
        #[cfg(test)]
        run_git_hook(
            GitHookPointV1::FinalStagingChecked,
            "",
            dest.root.canonical_path(),
        );
        Ok(())
    })?;
    pass.unit(|| {
        pace("chain-step")?;
        recheck_entry(
            &dest.work,
            RESTORE_PLAIN_DIR_NAME_V1.as_bytes(),
            &dest.plain,
            &plain_relative(b""),
        )?;
        #[cfg(test)]
        run_git_hook(
            GitHookPointV1::FinalStagingChecked,
            "",
            dest.work.canonical_path(),
        );
        Ok(())
    })?;
    let staging = dest.staging.borrow();
    for (key, pin) in staging.iter() {
        pass.unit(|| {
            pace("chain-step")?;
            let (parent, component): (&PinnedDirectoryV1, &[u8]) =
                match key.iter().rposition(|byte| *byte == b'/') {
                    Some(at) => {
                        let above = staging.get(&key[..at]).ok_or_else(|| {
                            CustodyRestoreErrorV1::IdentityChanged(format!(
                                "{}: a staging directory is not retained",
                                lossy(&plain_relative(&key[..at]))
                            ))
                        })?;
                        (&**above, &key[at + 1..])
                    }
                    None => (&dest.plain, key.as_slice()),
                };
            recheck_entry(parent, component, pin, &plain_relative(key))?;
            #[cfg(test)]
            run_git_hook(
                GitHookPointV1::FinalStagingChecked,
                "",
                parent.canonical_path(),
            );
            Ok(())
        })?;
    }
    let work_name = RESTORE_WORK_DIR_NAME_V1.as_bytes();
    for (name, pin) in [(HOME_NAME_V1, home), (XDG_NAME_V1, xdg)] {
        if let Some(pin) = pin {
            pass.unit(|| {
                pace("chain")?;
                recheck_entry(
                    &dest.work,
                    name.as_bytes(),
                    pin,
                    &joined(work_name, name.as_bytes()),
                )?;
                Ok(())
            })?;
        }
    }
    let bootstrap_name = joined(work_name, BOOTSTRAP_NAME_V1.as_bytes());
    for (relative, pin) in dirs {
        pass.unit(|| {
            pace("chain")?;
            if relative.is_empty() {
                recheck_entry(
                    &dest.work,
                    BOOTSTRAP_NAME_V1.as_bytes(),
                    pin,
                    &bootstrap_name,
                )?;
                return Ok(());
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
            Ok(())
        })?;
    }
    pass.unit(|| {
        pace("root")?;
        pinned_root_unchanged(&dest.root)
            .map_err(|detail| CustodyRestoreErrorV1::IdentityChanged(detail).into())
    })
}

// ---------------------------------------------------------------------------------------------
// Observation progress: units, phases, and the completion of a known failure
// ---------------------------------------------------------------------------------------------

/// The text of the route refusal the runner callbacks of a final child are returned as when their gate refuses. The
/// runner returns a callback's own error as it is when the binary did not drift, so this text, and only this,
/// identifies a refusal THIS file made, as against any other error of the runner.
const FINAL_GATE_REFUSED_V1: &str = "the final repository gate refused";

/// Whether `error` is the route refusal a callback's refusal was returned as (see [`FINAL_GATE_REFUSED_V1`]).
fn is_final_gate_refusal(error: &CustodyGitError) -> bool {
    matches!(error, CustodyGitError::RouteRefusal(text) if text == FINAL_GATE_REFUSED_V1)
}

/// One pass over the retained chains, in custody order. Every retained edge (and the destination root's identity,
/// which is the last) is ONE unit ([`Self::unit`]), and a unit is SETTLED when it ran to a verdict, a pass or a
/// custody refusal, and not when a deadline guard stopped it first. A gate's pass is held to the one deadline and
/// ends at its first refusal, which is the child's failure; a completion's pass goes on past every refusal and keeps
/// each, so no failing edge hides another and the root is always observed, last. A completion resumes after the
/// leading units an earlier observation of the same phase settled: a unit is never observed twice after a child.
struct ChainPassV1 {
    /// How many leading units to skip.
    resume: usize,
    /// Whether a refusal is kept and the pass goes on (a completion), or ends it (a gate).
    exhaustive: bool,
    /// The index of the next unit.
    next: Cell<usize>,
    /// How many leading units ran to a verdict.
    settled: Cell<usize>,
    /// The refusals a completion kept, in custody order.
    found: RefCell<Vec<GitBootstrapErrorV1>>,
}

impl ChainPassV1 {
    /// A gate's pass: from the first unit, ending at the first refusal.
    fn gate() -> Self {
        Self::of(0, false)
    }

    /// A completion's pass: resuming after `resume` settled units, keeping every refusal.
    fn completing(resume: usize) -> Self {
        Self::of(resume, true)
    }

    fn of(resume: usize, exhaustive: bool) -> Self {
        Self {
            resume,
            exhaustive,
            next: Cell::new(0),
            settled: Cell::new(0),
            found: RefCell::new(Vec::new()),
        }
    }

    /// Runs one unit, unless an earlier observation already settled it.
    fn unit(
        &self,
        run: impl FnOnce() -> Result<(), GitBootstrapErrorV1>,
    ) -> Result<(), GitBootstrapErrorV1> {
        let index = self.next.get();
        self.next.set(index + 1);
        if index < self.resume {
            self.settled.set(index + 1);
            return Ok(());
        }
        match run() {
            Ok(()) => {
                self.settled.set(index + 1);
                Ok(())
            }
            // A guard the deadline tripped stopped the unit before it ran to a verdict: it is NOT settled.
            Err(error) if is_deadline_timeout(&error) => Err(error),
            Err(error) => {
                self.settled.set(index + 1);
                if self.exhaustive {
                    self.found.borrow_mut().push(error);
                    Ok(())
                } else {
                    Err(error)
                }
            }
        }
    }
}

/// What one gate of a final child (a runner callback's, or the settlement's terminal one) ran to a verdict.
struct GateSeenV1 {
    /// The mount census ran to a verdict (a refusal is its verdict).
    mounts: Cell<bool>,
    /// The retained chains.
    chains: ChainPassV1,
}

impl GateSeenV1 {
    fn new() -> Self {
        Self {
            mounts: Cell::new(false),
            chains: ChainPassV1::gate(),
        }
    }
}

/// What the census of the exact tree has run to a verdict, CALL by call, and the facts those calls returned. A call
/// is ATTEMPTED when it ran, whether a custody refusal followed it or a later guard found the deadline spent: its
/// fact is kept here and the call is never made again, while every call that has not run is still owed, one at a
/// time, in the census's own order. A custody finding of one unit settles no other: a gate's pass ends at the first
/// one (it is the child's failure), and a completion ([`ProofRunV1::proof_census`] with `collect`) keeps each in
/// `findings` and goes on to the next unit, never descending an unexpected or replaced entry (a directory is only
/// ever listed through its RETAINED pin). The comparison with the baseline needs a COMPLETE census that found
/// nothing: after any finding it is unavailable, and so it is when the listing bound, which every listing shares,
/// is spent (`ended`: nothing more can be listed within it).
///
/// Every key is the exact bytes of the object a unit observes ([`CensusDirectoryKeyV1`], [`CensusEntryKeyV1`],
/// [`GitCensusPathV1`]): a returned fact, a completion, and a membership belong to that one raw entry and to no
/// other whose name merely renders alike, and a missing check is its own set, never an entry of `done`.
#[derive(Debug, Default)]
struct CensusV1 {
    /// What the calls that returned reported: every directory's and every regular file's facts.
    facts: ProofFactsV1,
    /// Every entry a listing returned (the repository's and each retained directory's), by its exact key.
    seen: BTreeSet<CensusEntryKeyV1>,
    /// The listing budget every directory's listing draws on, once the first listing is about to be made.
    remaining: Option<u64>,
    /// The directories whose `fstat` call ran, and what the ones that returned reported.
    stat_ran: BTreeSet<CensusDirectoryKeyV1>,
    stats: BTreeMap<CensusDirectoryKeyV1, ChildStatV1>,
    /// The directories whose listing call ran, and the names, in byte order, the ones that returned held.
    list_ran: BTreeSet<CensusDirectoryKeyV1>,
    listed: BTreeMap<CensusDirectoryKeyV1, Vec<Vec<u8>>>,
    /// The `fstatat` of each listed entry that returned, awaiting its verdict.
    entry_stats: BTreeMap<CensusEntryKeyV1, ChildStatV1>,
    /// The descriptor of each expected regular file whose `open` returned, awaiting its own `fstat`.
    opened: BTreeMap<CensusEntryKeyV1, File>,
    /// The entries whose verdict was reached (a pass or a finding).
    done: BTreeSet<CensusEntryKeyV1>,
    /// The names the restore made whose missing check ran.
    missing: BTreeSet<GitCensusPathV1>,
    /// The findings a completion kept, in the census's order, each with its typed identity.
    findings: Vec<CensusFindingV1>,
    /// Whether any unit ended in a finding (in any phase).
    faulted: bool,
    /// Whether the shared listing bound was spent: the census can go no further.
    ended: bool,
    missing_git_checked: bool,
    missing_checked: bool,
    /// Whether every unit ran to the end.
    finished: bool,
    /// Whether the comparison with the baseline was made.
    compared: bool,
}

impl CensusV1 {
    /// A finding of one unit, made at `at`: a gate's pass ends at it, and a completion keeps it (with its typed
    /// identity) and goes on.
    fn fault(
        &mut self,
        at: CensusFaultAtV1,
        error: GitBootstrapErrorV1,
        collect: bool,
    ) -> Result<(), GitBootstrapErrorV1> {
        self.faulted = true;
        if collect {
            let key = CensusFaultKeyV1::of(at, &error);
            self.findings.push(CensusFindingV1 { key, error });
            Ok(())
        } else {
            Err(error)
        }
    }
}

/// What the paced digest of a settlement ran to a verdict, file by file in path order (the pass ends at its first
/// refusal, so at most one file is partial): `settled` leading files had their digest reach a verdict ABOUT THE
/// ORIGINAL BYTES (a pass, or a finding derived from the bytes read through a descriptor bound to the measured file),
/// and `read` holds the count and SHA-256 of the next file's bytes when its read RETURNED and a guard then found the
/// deadline spent: those bytes were read, so a completion carries their verdict and never reads them again. A NAME
/// refusal (the open, or the binding `fstat` of the descriptor the name opened) is no observation of the retained
/// file, a guard that stops the read leaves no complete hash, and neither settles the file: the retained descriptor
/// is still owed its own digest.
#[derive(Debug, Default)]
struct DigestSeenV1 {
    settled: Cell<usize>,
    read: Cell<Option<(u64, [u8; 32])>>,
}

impl DigestSeenV1 {
    /// The file whose digest was being made reached its verdict: it is settled and its carried bytes are spent.
    fn settle(&self) {
        self.settled.set(self.settled.get() + 1);
        self.read.set(None);
    }
}

/// What the observations made AFTER a final child's exit ran to a verdict, by stage and unit. A completion makes
/// only the rest of them.
#[derive(Debug, Default)]
struct SettledV1 {
    /// The mount census.
    mounts: bool,
    /// The census of the exact tree, call by call, and whether its comparison with the baseline was made.
    census: CensusV1,
    /// How many restored files, in path order, had their digest run to a verdict, and the bytes the next returned.
    digests: DigestSeenV1,
    /// How many retained-chain units, in custody order, ran to a verdict (the root's identity is the last).
    chains: usize,
}

/// How a final child ended, for its completion: whether it may have run (its pre-spawn gate passed, so the runner
/// went on), and what the observations made since it exited already settled.
#[derive(Debug)]
struct ChildExitV1 {
    may_have_run: bool,
    settled: SettledV1,
}

/// What the paced settlement of a returned, classified-ok result ran to a verdict, in its own phase: its census and
/// comparison, its digests, and its terminal gate. The after-exit gate of the same child does NOT stand for any of
/// it: the settlement did further I/O after that gate returned.
struct SettlementSeenV1 {
    census: RefCell<CensusV1>,
    digests: DigestSeenV1,
    gate: GateSeenV1,
}

impl SettlementSeenV1 {
    fn new() -> Self {
        Self {
            census: RefCell::new(CensusV1::default()),
            digests: DigestSeenV1::default(),
            gate: GateSeenV1::new(),
        }
    }

    fn into_settled(self) -> SettledV1 {
        SettledV1 {
            mounts: self.gate.mounts.get(),
            census: self.census.into_inner(),
            digests: self.digests,
            chains: self.gate.chains.settled.get(),
        }
    }
}

/// One run of the final proof: the retained values it reads, the account of what the restore made, the ONE
/// absolute deadline, and the children it has run.
struct ProofRunV1<'a> {
    boot: &'a VerifiedGitBootstrapV1,
    objects: &'a FinalObjectStoreV1,
    metadata: &'a FinalMetadataV1,
    made: MadeTreeV1<'a>,
    deadline: Instant,
    runs: Vec<GitRunRecordV1>,
}

impl<'a> ProofRunV1<'a> {
    fn new(
        boot: &'a VerifiedGitBootstrapV1,
        objects: &'a FinalObjectStoreV1,
        metadata: &'a FinalMetadataV1,
        deadline: Instant,
    ) -> Result<Self, GitBootstrapErrorV1> {
        if objects.step != GitPlaneStepV1::Objects || metadata.step != GitPlaneStepV1::Config {
            return Err(final_input("the stores have not completed their steps"));
        }
        Ok(Self {
            boot,
            objects,
            metadata,
            made: MadeTreeV1::of(objects, metadata)?,
            deadline,
            runs: Vec::new(),
        })
    }

    fn dest(&self) -> &'a super::RestoreDestinationV1 {
        &self.boot.verified.destination
    }

    fn into_runs(self) -> Vec<GitRunRecordV1> {
        self.runs
    }

    /// Every retained chain, one UNIT per retained edge, in custody order: the output directories and the files
    /// copied, the metadata directories and files, every admitted bootstrap file, then the staging, HOME, XDG, and
    /// bootstrap chains with the destination root's identity LAST. `pace` runs before every filesystem call a step
    /// makes, so no call is made after a slow one without the clock: a step of one call has one guard, and the two
    /// that make more ([`final_source_files_unchanged`] is two units) have one guard each.
    /// `pass` says what a refusal does ([`ChainPassV1`]): a gate's pass ends at the first one, a completion's keeps
    /// every one and goes on to the next edge and, last, the root.
    fn proof_chains(
        &self,
        pace: PaceV1<'_>,
        pass: &ChainPassV1,
    ) -> Result<(), GitBootstrapErrorV1> {
        final_output_chains(
            self.dest(),
            PinViewV1::of_final(&self.objects.pins),
            &self.objects.files,
            pace,
            pass,
        )?;
        let git = &self.objects.pins.git;
        for (relative, pin) in &self.metadata.directories {
            pass.unit(|| {
                pace("final-chain")?;
                let (parent, leaf) = split_relative(relative);
                let above = pin_beneath(git, &self.metadata.directories, parent)?;
                recheck_entry(
                    above,
                    leaf.as_bytes(),
                    pin,
                    metadata_name(relative).as_bytes(),
                )?;
                Ok(())
            })?;
        }
        for file in &self.metadata.files {
            pass.unit(|| {
                pace("final-chain")?;
                let parent = pin_beneath(
                    git,
                    &self.metadata.directories,
                    split_relative(&file.relative).0,
                )?;
                recheck_regular_entry(parent, &file.relative, (file.dev, file.ino, file.length))
            })?;
        }
        final_source_files_unchanged(self.boot, pace, pass)?;
        final_sweep_chains(
            self.dest(),
            Some(&self.boot.home),
            Some(&self.boot.xdg),
            &self.boot.directories,
            pace,
            pass,
        )
    }

    /// The ONE gate of the final children: the mount census, then every retained chain, held to the ONE
    /// absolute deadline: observed before the census, right after it (the first step of the chains), before
    /// every chain, and immediately before the root's identity, which stays the LAST fallible call. Both runner
    /// callbacks run it, because neither knows the child's outcome (the post-exit one of a child that is about
    /// to succeed precedes a success), and so does a result that is about to become a success. A refusal here
    /// makes the child's failure known, and a known failure is completed without the clock
    /// ([`Self::proof_settle`]); no gate is ever left out of the deadline. `seen` records what the gate ran to a
    /// verdict, so that completion makes only the observations this one did not: the mount census once it has run
    /// (its refusal is its verdict), and each chain unit it reached (the one that refused included).
    fn proof_deadline_gate(&self, seen: &GateSeenV1) -> Result<(), GitBootstrapErrorV1> {
        let deadline = self.deadline;
        observe_deadline(deadline, "final-gate")?;
        let mounts = refuse_mounts_within(&self.dest().root);
        seen.mounts.set(true);
        mounts?;
        self.proof_chains(&|step| observe_deadline(deadline, step), &seen.chains)
    }

    /// The census of the exact tree as one gate's pass: the facts of one complete census, or its first finding or its
    /// first guard that finds the deadline spent ([`Self::proof_census`] with a fresh [`CensusV1`]).
    fn proof_measure(&self, pace: PaceV1<'_>) -> Result<ProofFactsV1, GitBootstrapErrorV1> {
        let mut census = CensusV1::default();
        self.proof_census(pace, &mut census, false)?;
        Ok(census.facts)
    }

    /// The census of the exact tree, through retained pins only, resumable call by call ([`CensusV1`]). The
    /// repository holds exactly the Git directory; every directory is the retained pin its parent lists; every
    /// regular file is opened no-follow, `fstat`ed, and required to be the inode and length the restore made;
    /// anything else (an entry the restore did not make, a missing one, a link, a special file, another device, or
    /// more entries than the restore made) is a finding. Every unit is ONE call behind ITS OWN guard: `pace` runs
    /// before the repository's and each directory's `fstat`, before each listing, before each entry's `fstatat`,
    /// before an expected file's `open`, and before the `fstat` of the descriptor it opened, so no call is made after a
    /// slow one without the clock. A unit whose call already ran is not made again; its returned fact is used.
    ///
    /// `collect` false is a gate's pass: it returns at its first finding or guard. `collect` true is a completion:
    /// each finding is kept in the census and the next unit runs (a listing bound spent ends the census). A unit never
    /// descends an unexpected, replaced, linked, or special entry.
    fn proof_census(
        &self,
        pace: PaceV1<'_>,
        c: &mut CensusV1,
        collect: bool,
    ) -> Result<(), GitBootstrapErrorV1> {
        if c.ended || c.finished {
            return Ok(());
        }
        let dev = self.dest().dev;
        // One entry for the Git directory beneath the repository, every other directory, and every file:
        // one more than the restore made is named, and anything past that is a census bound.
        c.remaining
            .get_or_insert((self.made.directories.len() + self.made.files.len()) as u64 + 1);

        let repository = CensusDirectoryKeyV1::Repository;
        self.census_stat(
            pace,
            c,
            &repository,
            self.made.repository,
            "custody restore final repository",
            collect,
        )?;
        self.census_list(
            pace,
            c,
            &repository,
            self.made.repository,
            "custody restore final repository listing",
            dev,
            collect,
        )?;
        if c.ended {
            return Ok(());
        }
        if let Some(names) = c.listed.get(&repository).cloned() {
            for name in &names {
                self.census_repository_entry(pace, c, name, dev, collect)?;
            }
        }
        if !c.missing_git_checked {
            c.missing_git_checked = true;
            let git = CensusEntryKeyV1::RepositoryChild(b".git".to_vec());
            if c.listed.contains_key(&repository) && !c.seen.contains(&git) {
                c.fault(
                    CensusFaultAtV1::Entry(git),
                    final_tree(final_name(""), FinalTreeFaultV1::Missing),
                    collect,
                )?;
            }
        }

        for (path, directory) in &self.made.directories {
            let key = CensusDirectoryKeyV1::Git(path.clone());
            self.census_stat(
                pace,
                c,
                &key,
                directory,
                "custody restore final directory",
                collect,
            )?;
            self.census_list(
                pace,
                c,
                &key,
                directory,
                "custody restore final listing",
                dev,
                collect,
            )?;
            if c.ended {
                return Ok(());
            }
            if let Some(names) = c.listed.get(&key).cloned() {
                for name in &names {
                    self.census_entry(pace, c, path, directory, name, dev, collect)?;
                }
            }
        }
        // A name the restore made that no returned listing held (a name under a directory whose listing never
        // returned is not judged: nothing says it is missing). Each missing name is its own unit, recorded in the
        // set of missing checks and never among the entries.
        if !c.missing_checked {
            for path in self
                .made
                .directories
                .keys()
                .filter(|path| !path.is_root())
                .chain(self.made.files.keys())
            {
                let parent =
                    CensusDirectoryKeyV1::Git(path.parent().unwrap_or_else(GitCensusPathV1::root));
                let entry = CensusEntryKeyV1::Git(path.clone());
                if c.listed.contains_key(&parent)
                    && !c.seen.contains(&entry)
                    && c.missing.insert(path.clone())
                {
                    c.fault(
                        CensusFaultAtV1::Entry(entry),
                        final_tree(path.render(), FinalTreeFaultV1::Missing),
                        collect,
                    )?;
                }
            }
            c.missing_checked = true;
        }
        c.finished = true;
        Ok(())
    }

    /// The `fstat` of the retained directory `pin` (the repository, or a made directory), one unit: its guard, its
    /// call, and the fact it returned (kept the moment it returns, under the directory's typed key).
    fn census_stat(
        &self,
        pace: PaceV1<'_>,
        c: &mut CensusV1,
        key: &CensusDirectoryKeyV1,
        pin: &PinnedDirectoryV1,
        label: &str,
        collect: bool,
    ) -> Result<(), GitBootstrapErrorV1> {
        if c.stat_ran.contains(key) {
            return Ok(());
        }
        pace("final-census")?;
        c.stat_ran.insert(key.clone());
        match pin.current_metadata(label) {
            Ok(stat) => {
                let stat = observed_stat(stat);
                c.facts
                    .directories
                    .insert(key.clone(), FileFactV1::of(&stat));
                c.stats.insert(key.clone(), stat);
                #[cfg(test)]
                run_git_hook(GitHookPointV1::FinalCensusStatted, "", pin.canonical_path());
                Ok(())
            }
            Err(error) => c.fault(
                CensusFaultAtV1::Directory(key.clone()),
                fs_error(error),
                collect,
            ),
        }
    }

    /// The listing of the retained directory `pin`, one unit: its guard (which also stands before the device check
    /// of the `fstat` that returned), its call, and the names it returned, in byte order, so the findings a
    /// completion collects come in one order. Every returned name is recorded as the entry of THIS directory it
    /// is, by its exact bytes. It draws on the one listing budget; a listing that ran past it is a `Bound` finding
    /// and ends the census.
    #[allow(clippy::too_many_arguments)]
    fn census_list(
        &self,
        pace: PaceV1<'_>,
        c: &mut CensusV1,
        key: &CensusDirectoryKeyV1,
        pin: &PinnedDirectoryV1,
        label: &str,
        dev: u64,
        collect: bool,
    ) -> Result<(), GitBootstrapErrorV1> {
        if c.list_ran.contains(key) {
            return Ok(());
        }
        pace("final-statted")?;
        c.list_ran.insert(key.clone());
        if let Some(stat) = c.stats.get(key) {
            let seam = key.seam();
            if observed_dev(&seam, stat.dev) != dev {
                return c.fault(
                    CensusFaultAtV1::Directory(key.clone()),
                    GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::DeviceCrossing {
                        name: seam,
                    }),
                    collect,
                );
            }
        }
        let mut left = c.remaining.unwrap_or(0);
        let listed = pin.list_child_names(&mut left, label);
        c.remaining = Some(left);
        match listed {
            Ok(mut names) => {
                names.sort();
                #[cfg(test)]
                run_git_hook(GitHookPointV1::FinalCensusListed, "", pin.canonical_path());
                let names: Vec<Vec<u8>> =
                    names.iter().map(|name| name.as_bytes().to_vec()).collect();
                for name in &names {
                    c.seen.insert(key.entry(name));
                }
                c.listed.insert(key.clone(), names);
                Ok(())
            }
            Err(error) => {
                let error = final_listing_error(error, key.seam());
                if matches!(
                    error,
                    GitBootstrapErrorV1::FinalTree {
                        fault: FinalTreeFaultV1::Bound,
                        ..
                    }
                ) {
                    c.ended = true;
                }
                c.fault(CensusFaultAtV1::Directory(key.clone()), error, collect)
            }
        }
    }

    /// One name the repository lists: it must be the Git directory, which is checked through the retained pin of
    /// its parent (one `fstatat`), and any other name is an unexpected entry (no call, and never descended). The
    /// unit belongs to the exact raw name, so two raw names that render alike are two units.
    fn census_repository_entry(
        &self,
        pace: PaceV1<'_>,
        c: &mut CensusV1,
        name: &[u8],
        dev: u64,
        collect: bool,
    ) -> Result<(), GitBootstrapErrorV1> {
        let entry = CensusEntryKeyV1::RepositoryChild(name.to_vec());
        if c.done.contains(&entry) {
            return Ok(());
        }
        pace("final-census")?;
        c.done.insert(entry.clone());
        if name != b".git" {
            return c.fault(
                CensusFaultAtV1::Entry(entry),
                final_tree(lossy(name), FinalTreeFaultV1::Unexpected),
                collect,
            );
        }
        let stat = match self
            .made
            .repository
            .child_metadata_no_follow(OsStr::new(".git"), "custody restore final entry")
        {
            Err(error) => return c.fault(CensusFaultAtV1::Entry(entry), fs_error(error), collect),
            Ok(None) => {
                return c.fault(
                    CensusFaultAtV1::Entry(entry),
                    final_tree(final_name(""), FinalTreeFaultV1::Missing),
                    collect,
                )
            }
            Ok(Some(stat)) => stat,
        };
        match require_final_directory(&stat, ".git", &final_name(""), self.made.git, dev) {
            Ok(()) => Ok(()),
            Err(error) => c.fault(CensusFaultAtV1::Entry(entry), error, collect),
        }
    }

    /// One name a made directory (`parent`, the retained `directory`) lists: its `fstatat` (one unit, whose returned
    /// fact is kept under the exact entry and whose hook stands right after it), then its kind. A link, a special
    /// file, an unexpected or replaced entry, and a directory that is not the retained pin its name has are each a
    /// finding of THAT raw name only; none is descended or opened (a directory is listed through its retained pin
    /// when the census reaches it). An expected regular file goes on to [`Self::census_file`].
    #[allow(clippy::too_many_arguments)]
    fn census_entry(
        &self,
        pace: PaceV1<'_>,
        c: &mut CensusV1,
        parent: &GitCensusPathV1,
        directory: &PinnedDirectoryV1,
        name: &[u8],
        dev: u64,
        collect: bool,
    ) -> Result<(), GitBootstrapErrorV1> {
        let child = parent.child(name);
        let entry = CensusEntryKeyV1::Git(child.clone());
        if c.done.contains(&entry) {
            return Ok(());
        }
        let component = OsStr::from_bytes(name);
        let stat = match c.entry_stats.get(&entry).copied() {
            Some(stat) => stat,
            None => {
                pace("final-census")?;
                match directory.child_metadata_no_follow(component, "custody restore final entry") {
                    Err(error) => {
                        c.done.insert(entry.clone());
                        return c.fault(CensusFaultAtV1::Entry(entry), fs_error(error), collect);
                    }
                    Ok(None) => {
                        c.done.insert(entry.clone());
                        return c.fault(
                            CensusFaultAtV1::Entry(entry),
                            final_tree(child.render(), FinalTreeFaultV1::Missing),
                            collect,
                        );
                    }
                    Ok(Some(stat)) => {
                        c.entry_stats.insert(entry.clone(), stat);
                        #[cfg(test)]
                        run_git_hook(
                            GitHookPointV1::BeforeFinalCensusOpen,
                            "",
                            &directory.canonical_path().join(component),
                        );
                        stat
                    }
                }
            }
        };
        if stat.kind == ChildKindV1::Regular {
            return self.census_file(pace, c, directory, component, &child, &stat, dev, collect);
        }
        c.done.insert(entry.clone());
        let verdict = match stat.kind {
            ChildKindV1::Symlink => Err(GitBootstrapErrorV1::Restore(
                CustodyRestoreErrorV1::SymlinkRefused {
                    name: child.render(),
                },
            )),
            ChildKindV1::Special(_) => Err(final_tree(child.render(), FinalTreeFaultV1::Special)),
            ChildKindV1::Directory | ChildKindV1::Regular => {
                // A name the restore made as a file is a replacement; any other name is unexpected.
                match self.made.directories.get(&child) {
                    Some(pin) => {
                        let text = child.render();
                        require_final_directory(&stat, &text, &final_name(&text), pin, dev)
                    }
                    None => {
                        let fault = if self.made.files.contains_key(&child) {
                            FinalTreeFaultV1::Replaced
                        } else {
                            FinalTreeFaultV1::Unexpected
                        };
                        Err(final_tree(child.render(), fault))
                    }
                }
            }
        };
        match verdict {
            Ok(()) => Ok(()),
            Err(error) => c.fault(CensusFaultAtV1::Entry(entry), error, collect),
        }
    }

    /// An expected regular file's two calls, each its own unit: the `open` of the name no-follow through its retained
    /// directory (its hook right after it; the descriptor is kept under the exact entry the moment it returns), and
    /// the `fstat` of THAT descriptor, which must be the entry the listing named, on the root's device, the very
    /// inode the restore made, at the length it made. A name the restore made as a directory is a replacement; any
    /// other is unexpected.
    #[allow(clippy::too_many_arguments)]
    fn census_file(
        &self,
        pace: PaceV1<'_>,
        c: &mut CensusV1,
        directory: &PinnedDirectoryV1,
        component: &OsStr,
        child: &GitCensusPathV1,
        stat: &ChildStatV1,
        dev: u64,
        collect: bool,
    ) -> Result<(), GitBootstrapErrorV1> {
        let entry = CensusEntryKeyV1::Git(child.clone());
        let Some(made) = self.made.files.get(child) else {
            c.done.insert(entry.clone());
            let fault = if self.made.directories.contains_key(child) {
                FinalTreeFaultV1::Replaced
            } else {
                FinalTreeFaultV1::Unexpected
            };
            return c.fault(
                CensusFaultAtV1::Entry(entry),
                final_tree(child.render(), fault),
                collect,
            );
        };
        if !c.opened.contains_key(&entry) {
            // The entry's `fstatat` has returned: a clock before the descriptor is opened.
            pace("final-entry-statted")?;
            match directory.open_regular_file(component, "custody restore final file") {
                Err(error) => {
                    c.done.insert(entry.clone());
                    let error = if refused_link(&error) {
                        GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::SymlinkRefused {
                            name: child.render(),
                        })
                    } else {
                        fs_error(error)
                    };
                    return c.fault(CensusFaultAtV1::Entry(entry), error, collect);
                }
                Ok(file) => {
                    c.opened.insert(entry.clone(), file);
                    #[cfg(test)]
                    run_git_hook(
                        GitHookPointV1::FinalFileOpened,
                        "",
                        &directory.canonical_path().join(component),
                    );
                }
            }
        }
        pace("final-opened")?;
        let Some(file) = c.opened.remove(&entry) else {
            return Ok(());
        };
        c.done.insert(entry.clone());
        let text = child.render();
        let opened = match file.metadata() {
            Ok(opened) => opened,
            Err(error) => {
                return c.fault(
                    CensusFaultAtV1::Entry(entry),
                    io_refusal(&text, &error),
                    collect,
                )
            }
        };
        let fact = observed_stat(ChildStatV1::from_metadata(&opened));
        if fact.kind != ChildKindV1::Regular || fact.dev != stat.dev || fact.ino != stat.ino {
            return c.fault(
                CensusFaultAtV1::Entry(entry),
                GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::IdentityChanged(format!(
                    "{text}: the opened file is not the listed entry"
                ))),
                collect,
            );
        }
        if observed_dev(&final_name(&text), fact.dev) != dev {
            return c.fault(
                CensusFaultAtV1::Entry(entry),
                GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::DeviceCrossing {
                    name: final_name(&text),
                }),
                collect,
            );
        }
        if (fact.dev, fact.ino) != (made.dev, made.ino) {
            return c.fault(
                CensusFaultAtV1::Entry(entry),
                final_tree(text, FinalTreeFaultV1::Replaced),
                collect,
            );
        }
        if fact.size != made.length {
            return c.fault(
                CensusFaultAtV1::Entry(entry),
                final_tree(text, FinalTreeFaultV1::Bytes),
                collect,
            );
        }
        c.facts.files.insert(child.clone(), FileFactV1::of(&fact));
        Ok(())
    }

    /// The SHA-256 of every file `facts` holds, each streamed from a fresh no-follow descriptor opened
    /// through the RETAINED pin of its directory and bound to the census (its own `fstat`, before the first
    /// read and after the last, must equal the file's measured non-atime facts, and exactly the measured byte
    /// count must be read), and each compared with the digest the file was restored with. Every read keeps
    /// the ONE absolute deadline; `pace` runs before every file, after its open, and after its read has returned.
    /// This is the pass that serves a success, so it ends at its first refusal.
    fn proof_digests(
        &self,
        facts: &ProofFactsV1,
        pace: PaceV1<'_>,
    ) -> Result<(), GitBootstrapErrorV1> {
        self.proof_digests_counted(facts, pace, &DigestSeenV1::default())
    }

    /// [`Self::proof_digests`], recording in `seen` how far each file's digest got ([`DigestSeenV1`]).
    fn proof_digests_counted(
        &self,
        facts: &ProofFactsV1,
        pace: PaceV1<'_>,
        seen: &DigestSeenV1,
    ) -> Result<(), GitBootstrapErrorV1> {
        for (path, fact) in &facts.files {
            self.proof_digest_file(path, fact, pace, seen)?;
        }
        Ok(())
    }

    /// The digest of ONE file of the census, held to the one deadline ([`Self::proof_digests`]). It reaches a
    /// VERDICT about the original bytes only once its read has returned through a descriptor bound to the measured
    /// file: the bytes read (count and SHA-256) are recorded in `seen` the moment the read returns, before the guard
    /// that follows it, and the file is settled when its verdict is reached (a pass, or a finding about the bytes or
    /// the descriptor that read them). A refusal of the NAME (the open, or the binding `fstat`), a guard that stops
    /// the read, and a read that fails each leave the file unsettled and unrecorded: they say nothing about the
    /// retained descriptor.
    fn proof_digest_file(
        &self,
        path: &GitCensusPathV1,
        fact: &FileFactV1,
        pace: PaceV1<'_>,
        seen: &DigestSeenV1,
    ) -> Result<(), GitBootstrapErrorV1> {
        pace("final-digest")?;
        // Every lookup and the open use the file's exact bytes; its text only names it in a refusal.
        let relative = path.render();
        let leaf = OsStr::from_bytes(path.leaf());
        let directory = path
            .parent()
            .and_then(|parent| self.made.directories.get(&parent))
            .ok_or_else(|| final_input(format!("{relative}: its directory is not retained")))?;
        let made = self
            .made
            .files
            .get(path)
            .ok_or_else(|| final_input(format!("{relative}: not a file the restore made")))?;
        let mut file = directory
            .open_regular_file(leaf, "custody restore final file digest")
            .map_err(|error| {
                if refused_link(&error) {
                    GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::SymlinkRefused {
                        name: relative.clone(),
                    })
                } else {
                    fs_error(error)
                }
            })?;
        #[cfg(test)]
        run_git_hook(
            GitHookPointV1::FinalDigestFileOpened,
            "",
            &directory.canonical_path().join(leaf),
        );
        pace("final-digest-opened")?;
        require_measured_file(&file, fact, &relative, "before it was read")?;
        #[cfg(test)]
        run_git_hook(
            GitHookPointV1::FinalDigestOpened,
            "",
            &directory.canonical_path().join(leaf),
        );
        let (read, sha256) = stream_digest(&mut file, fact.length, self.deadline, &relative)?;
        seen.read.set(Some((read, sha256)));
        #[cfg(test)]
        run_git_hook(
            GitHookPointV1::FinalDigestHashed,
            "",
            &directory.canonical_path().join(leaf),
        );
        pace("final-digest-read")?;
        if read != fact.length {
            seen.settle();
            return Err(digest_identity(
                &relative,
                &format!(
                    "{read} bytes were read, and the census measured {}",
                    fact.length
                ),
            ));
        }
        if let Err(error) = require_measured_file(&file, fact, &relative, "after it was read") {
            seen.settle();
            return Err(error);
        }
        seen.settle();
        if sha256 != made.sha256 {
            return Err(final_tree(relative, FinalTreeFaultV1::Bytes));
        }
        Ok(())
    }

    /// Run one read-only child rooted at the retained repository, and settle what it left, on EVERY outcome.
    /// A read-only child owns a zero-effect allowance: before it, the tree is censused and every file's
    /// SHA-256 is held to the restore's (a replaced entry stops the child); both runner callbacks run the
    /// deadline gate; and `classify` judges a returned result (status, then output). Nothing is reserved, so
    /// nothing is released: the one ledger is never touched. The baseline is held to the ONE absolute deadline
    /// at every call (a guard after each, and one before the runner, a filesystem call of its own, is called):
    /// a baseline that returns after it never lets a child start. The two callbacks record what their gates ran
    /// to a verdict, per phase, so a failure that is known afterwards is completed for exactly what no gate saw
    /// ([`Self::proof_reconcile`]). The runner evaluates the binary's drift and the post-exit callback
    /// independently and lets the drift dominate, so a callback's refusal replaces only the route refusal this file
    /// made for it: any other runner error stays the primary and the callback's refusal is the first finding
    /// beside it.
    fn proof_child<T>(
        &mut self,
        runner: &GitRunnerV1,
        names: &GitRootNamesV1,
        label: &'static str,
        request: GitRunRequestV1,
        classify: impl FnOnce(&GitRunResultV1) -> Result<T, GitBootstrapErrorV1>,
    ) -> Result<T, GitBootstrapErrorV1> {
        #[cfg(test)]
        run_git_hook(
            GitHookPointV1::BeforeFinalChildCensus,
            label,
            self.made.repository.canonical_path(),
        );
        let deadline = self.deadline;
        let pace = |step: &'static str| observe_deadline(deadline, step);
        let baseline = self.proof_measure(&pace)?;
        self.proof_digests(&baseline, &pace)?;
        observe_deadline(deadline, "final-baselined")?;
        let refusal: RefCell<Option<GitBootstrapErrorV1>> = RefCell::new(None);
        // What the two callbacks saw. The pre-spawn gate only says the child may now run (it is no evidence of what
        // the child leaves); the post-exit gate's progress is what the child left that a gate has already seen.
        let before_passed = Cell::new(false);
        let after = GateSeenV1::new();
        let outcome = {
            let this = &*self;
            let check = |point_label: &'static str, is_after: bool| {
                #[cfg(test)]
                run_git_hook(
                    if is_after {
                        GitHookPointV1::AfterFinalChild
                    } else {
                        GitHookPointV1::BeforeFinalChild
                    },
                    point_label,
                    this.made.repository.canonical_path(),
                );
                #[cfg(not(test))]
                let _ = point_label;
                let before = GateSeenV1::new();
                let seen = if is_after { &after } else { &before };
                match this.proof_deadline_gate(seen) {
                    Ok(()) => {
                        if !is_after {
                            before_passed.set(true);
                        }
                        Ok(())
                    }
                    Err(error) => {
                        *refusal.borrow_mut() = Some(error);
                        Err(CustodyGitError::RouteRefusal(FINAL_GATE_REFUSED_V1.into()))
                    }
                }
            };
            runner.run(
                this.made.repository,
                names,
                request,
                || check(label, false),
                || check(label, true),
            )
        };
        let refusal = refusal.into_inner();
        let exit = ChildExitV1 {
            may_have_run: before_passed.get(),
            settled: SettledV1 {
                mounts: after.mounts.get(),
                chains: after.chains.settled.get(),
                ..SettledV1::default()
            },
        };
        // Every runner error is one outcome of a child that may have run: a callback refusal, a timeout, a cap,
        // binary drift, a failed stream, and a refusal before the spawn alike.
        let (verdict, beside) = match outcome {
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
                match refusal {
                    None => (classify(&result), None),
                    Some(error) => (Err(error), None),
                }
            }
            Err(error) => match refusal {
                None => (Err(error.into()), None),
                Some(callback) if is_final_gate_refusal(&error) => (Err(callback), None),
                Some(callback) => (Err(error.into()), Some(callback)),
            },
        };
        self.proof_settle(&baseline, label, verdict, beside, exit)
    }

    /// Settle a read-only child, once, on any outcome: the tree must be EXACTLY what the pre-child census
    /// found, so an added, removed, replaced, or resized entry, a file whose mode or timestamps moved, and a
    /// file whose bytes changed in place (which only its digest can show) are each a reconciliation failure.
    /// A result that is about to be a success is settled on the ONE absolute deadline at every call
    /// ([`Self::proof_settle_paced`]) and can only become a success there. EVERY failure of the child, whether
    /// already known (a runner error, a callback refusal, a nonzero status, a parser refusal) or made known by
    /// the paced settlement (the typed timeout, or a custody finding), enters [`Self::proof_reconcile`] at this
    /// ONE site, once, with what had already run to a verdict; that helper keeps the failure beside whatever else
    /// it finds and can never return a value. `beside` is a callback's refusal that the runner's own error
    /// outranked, `exit` how the child ended.
    fn proof_settle<T>(
        &self,
        baseline: &ProofFactsV1,
        label: &'static str,
        verdict: Result<T, GitBootstrapErrorV1>,
        beside: Option<GitBootstrapErrorV1>,
        exit: ChildExitV1,
    ) -> Result<T, GitBootstrapErrorV1> {
        let (primary, exit) = match verdict {
            Err(primary) => (primary, exit),
            Ok(value) => {
                let seen = SettlementSeenV1::new();
                match self.proof_settle_paced(baseline, label, &seen) {
                    Ok(()) => return Ok(value),
                    Err(error) => (
                        error,
                        ChildExitV1 {
                            may_have_run: true,
                            settled: seen.into_settled(),
                        },
                    ),
                }
            }
        };
        Err(self.proof_reconcile(baseline, label, primary, beside, exit))
    }

    /// The settlement of a result that is about to be a success, held to the ONE absolute deadline at every call
    /// (a guard before each call of the census and of the digests, so nothing is made after a slow one without the
    /// clock): the census, its comparison with the baseline, the digests, and the success gate, in that order,
    /// ending at the first failure. `seen` records what ran to a verdict in this phase, CALL by call (the census's
    /// returned facts, each digest's returned bytes, the gate's units), so a completion never repeats a call that
    /// ran and still makes every call that did not.
    fn proof_settle_paced(
        &self,
        baseline: &ProofFactsV1,
        label: &'static str,
        seen: &SettlementSeenV1,
    ) -> Result<(), GitBootstrapErrorV1> {
        let deadline = self.deadline;
        let paced = |step: &'static str| observe_deadline(deadline, step);
        self.proof_census(&paced, &mut seen.census.borrow_mut(), false)?;
        {
            let mut census = seen.census.borrow_mut();
            census.compared = true;
            final_unchanged(baseline, &census.facts, label)?;
        }
        // Every fact is equal, so this reads the very files, at the very lengths, that the baseline
        // read, under the SAME absolute deadline: there is no exception after a child.
        self.proof_digests_counted(&seen.census.borrow().facts, &paced, &seen.digests)?;
        self.proof_deadline_gate(&seen.gate)
    }

    /// The ONE failure-only completion of a final child: `primary` is a failure that is already known, and
    /// every observation of what the child may have left that no earlier phase ran to a verdict is made WITHOUT
    /// the clock (a spent deadline cannot stop it), each independently of the others, in the master's gate order:
    /// the mount census; the census of the exact tree, resumed after the calls that already returned (each unit one
    /// call, each finding kept, an unexpected or replaced entry never descended), and its comparison with the
    /// baseline (which needs a complete census that found nothing); the digest of every restored file, in path
    /// order, by its RETAINED descriptor (it needs neither the census nor a name, and it carries the bytes a paced
    /// read already returned); then every retained chain edge, one unit each, the destination root's identity LAST.
    /// Each is finite: a bounded mount table, at most the entries the restore made plus one, one bounded read per
    /// file, one call per retained edge. Nothing is written, reserved, released, retried, or started.
    ///
    /// `exit` says what ran to a verdict since the child exited, call by call (a unit whose call ran is attempted
    /// whether a refusal or a guard followed it; a unit a deadline guard stopped before its call is not) and whether
    /// the child may have run at all (when it did not, only the tree and the digests are owed). A finding that fails
    /// is kept, in the order above, beside the primary (a callback's refusal first, via `beside`), nested in the
    /// existing [`GitBootstrapErrorV1::ChildAndReconcile`] with the first finding outermost, and the result is
    /// ALWAYS a failure: its type cannot be a verified value, and the primary is never dropped.
    fn proof_reconcile(
        &self,
        baseline: &ProofFactsV1,
        label: &'static str,
        primary: GitBootstrapErrorV1,
        beside: Option<GitBootstrapErrorV1>,
        exit: ChildExitV1,
    ) -> GitBootstrapErrorV1 {
        // The ONE clock-free pace of the final region, used only after a failure is known.
        let pace: PaceV1<'_> = &unpaced;
        let mut settled = exit.settled;
        let mut found: Vec<GitBootstrapErrorV1> = beside.into_iter().collect();
        if exit.may_have_run && !settled.mounts {
            if let Err(error) = refuse_mounts_within(&self.dest().root) {
                found.push(error.into());
            }
        }
        // A completion collects each finding and cannot fail on a clock; a refusal it returned all the same is kept.
        if let Err(error) = self.proof_census(pace, &mut settled.census, true) {
            found.push(error);
        }
        let mut kept: Vec<CensusFaultKeyV1> = Vec::new();
        for finding in settled.census.findings.drain(..) {
            kept.push(finding.key);
            found.push(finding.error);
        }
        if settled.census.finished && !settled.census.faulted && !settled.census.compared {
            settled.census.compared = true;
            if let Err(error) = final_unchanged(baseline, &settled.census.facts, label) {
                found.push(error);
            }
        }
        self.proof_retained_digests(&settled.digests, &mut found, &kept);
        if exit.may_have_run {
            let pass = ChainPassV1::completing(settled.chains);
            // Every call is inside a unit and a completion keeps each refusal, so this returns nothing; a refusal
            // that were returned is kept all the same.
            if let Err(error) = self.proof_chains(pace, &pass) {
                found.push(error);
            }
            found.extend(pass.found.take());
        }
        let reconcile = found.into_iter().rev().reduce(|later, earlier| {
            GitBootstrapErrorV1::ChildAndReconcile {
                primary: Box::new(earlier),
                reconcile: Box::new(later),
            }
        });
        match reconcile {
            None => primary,
            Some(reconcile) => GitBootstrapErrorV1::ChildAndReconcile {
                primary: Box::new(primary),
                reconcile: Box::new(reconcile),
            },
        }
    }

    /// The path of the restored file `path` beneath its retained directory, for a test hook.
    #[cfg(test)]
    fn final_file_path(&self, path: &GitCensusPathV1) -> Option<std::path::PathBuf> {
        path.parent()
            .and_then(|parent| self.made.directories.get(&parent))
            .map(|directory| {
                directory
                    .canonical_path()
                    .join(OsStr::from_bytes(path.leaf()))
            })
    }

    /// The digest of every restored file after the ones the paced digests settled (in path order, which is the order
    /// they settle in), each by its retained descriptor ([`Self::proof_retained_digest`]), each independent of the
    /// others: a file whose digest refuses is kept, and the next file is observed. The first file owed carries the
    /// bytes its paced read returned, if one did. A fault the census of the tree already kept for the same file (a
    /// resized file is both) is reported once: `kept` holds the typed identity of each finding the census kept, and
    /// a file's tree fault is that finding when its exact path and its fault are equal to one's, never by the text
    /// either carries.
    fn proof_retained_digests(
        &self,
        seen: &DigestSeenV1,
        found: &mut Vec<GitBootstrapErrorV1>,
        kept: &[CensusFaultKeyV1],
    ) {
        let from = seen.settled.get();
        for (position, (path, made)) in self.made.files.iter().enumerate().skip(from) {
            #[cfg(test)]
            if let Some(at) = self.final_file_path(path) {
                run_git_hook(GitHookPointV1::FinalDigestFileOpened, "", &at);
            }
            let carried = if position == from {
                seen.read.get()
            } else {
                None
            };
            if let Err(error) = self.proof_retained_digest(path, made, carried) {
                let again = match &error {
                    GitBootstrapErrorV1::FinalTree { fault, .. } => {
                        kept.contains(&CensusFaultKeyV1 {
                            at: CensusFaultAtV1::Entry(CensusEntryKeyV1::Git(path.clone())),
                            class: CensusFaultClassV1::Tree(*fault),
                        })
                    }
                    _ => false,
                };
                if !again {
                    found.push(error);
                }
            }
        }
    }

    /// The SHA-256 of the file the restore made at `path`, read from the descriptor the restore RETAINED, at
    /// explicit offsets: it moves no shared cursor, reopens no name (a name may be another inode, a link, or a FIFO
    /// by now), and reads no clock, because it serves a failure that is already known. It reads at most the length the
    /// restore made plus one byte, so a file that grew is never read whole, in a finite number of calls, and a
    /// read that fails for any reason, a signal's interruption included, is one typed refusal of that file and is
    /// never repeated. It can only refuse: a descriptor that is no longer the file the restore made, a length or
    /// bytes that changed, and a read that failed are each that file's own finding, and the other files are
    /// observed regardless.
    ///
    /// `carried` is the count and SHA-256 of bytes a paced digest already READ from this file through a descriptor
    /// bound to the measured file, before a guard found the deadline spent: they are an observation of the original
    /// bytes, so they are judged here and never read again (a second read could only see a later state, and the
    /// descriptor's own `fstat` is made as the one call that is still owed).
    fn proof_retained_digest(
        &self,
        path: &GitCensusPathV1,
        made: &MadeFileV1<'_>,
        carried: Option<(u64, [u8; 32])>,
    ) -> Result<(), GitBootstrapErrorV1> {
        let file = made.file;
        // The file's text names it in a refusal and in the test seams; the descriptor is what is read.
        let relative = path.render();
        let relative = relative.as_str();
        let (count, sha256) = match carried {
            Some(read) => read,
            None => {
                require_made_file(file, made, relative)?;
                #[cfg(test)]
                let at = self.final_file_path(path);
                #[cfg(test)]
                if let Some(at) = &at {
                    run_git_hook(GitHookPointV1::FinalDigestOpened, "", at);
                }
                let limit = made.length.saturating_add(1);
                let mut context = digest::Context::new(&digest::SHA256);
                let mut buffer = vec![0_u8; HASH_BUFFER_BYTES_V1];
                let mut count = 0_u64;
                while count < limit {
                    let want = usize::try_from(limit - count)
                        .map_or(buffer.len(), |left| left.min(buffer.len()));
                    #[cfg(test)]
                    let read = match reconcile_read_call(relative) {
                        Some(error) => Err(error),
                        None => file.read_at(&mut buffer[..want], count),
                    };
                    #[cfg(not(test))]
                    let read = file.read_at(&mut buffer[..want], count);
                    match read {
                        Ok(0) => break,
                        Ok(read) => {
                            count = count.saturating_add(read as u64);
                            context.update(&buffer[..read]);
                        }
                        Err(error) => return Err(io_refusal(relative, &error)),
                    }
                }
                #[cfg(test)]
                if let Some(at) = &at {
                    run_git_hook(GitHookPointV1::FinalDigestHashed, "", at);
                }
                let mut sha256 = [0_u8; 32];
                sha256.copy_from_slice(context.finish().as_ref());
                (count, sha256)
            }
        };
        if count > made.length {
            return Err(digest_identity(
                relative,
                &format!(
                    "{count} bytes were read, and the census measured {}",
                    made.length
                ),
            ));
        }
        if count < made.length {
            return Err(final_tree(relative.to_owned(), FinalTreeFaultV1::Bytes));
        }
        require_made_file(file, made, relative)?;
        if sha256 != made.sha256 {
            return Err(final_tree(relative.to_owned(), FinalTreeFaultV1::Bytes));
        }
        Ok(())
    }

    /// `cat-file --batch-all-objects`: exactly the manifest's `(oid, kind)` set, in the FINAL database.
    fn proof_inventory(
        &mut self,
        runner: &GitRunnerV1,
        names: &GitRootNamesV1,
        format: GitObjectFormatV1,
        expected: &BTreeMap<String, &'static str>,
    ) -> Result<(), GitBootstrapErrorV1> {
        let request = GitRunRequestV1::new(
            GitCommandV1::CatFileAllObjects,
            Vec::new(),
            stdout_limit_for_objects(expected.len()),
            GIT_STDERR_LIMIT_V1,
            self.deadline,
        );
        self.proof_child(
            runner,
            names,
            "cat-file --batch-all-objects",
            request,
            |result| {
                let parsed = match result.captured_stdout() {
                    Some(stdout) => {
                        compare_inventory(stdout, expected, format, "the final repository")
                    }
                    None => Err(GitBootstrapErrorV1::GitOutput(
                        "cat-file stdout was streamed".into(),
                    )),
                };
                shape_then_status(result, "cat-file --batch-all-objects", parsed)
            },
        )
    }

    /// `rev-list --objects --missing=print` over EVERY inventory object, in the FINAL database: the reached
    /// set is exactly the inventory.
    fn proof_reachability(
        &mut self,
        runner: &GitRunnerV1,
        names: &GitRootNamesV1,
        format: GitObjectFormatV1,
        expected: &BTreeMap<String, &'static str>,
    ) -> Result<usize, GitBootstrapErrorV1> {
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
            self.deadline,
        );
        self.proof_child(
            runner,
            names,
            "rev-list --missing=print",
            request,
            |result| {
                let parsed = match result.captured_stdout() {
                    Some(stdout) => compare_closure(stdout, expected, format),
                    None => Err(GitBootstrapErrorV1::GitOutput(
                        "rev-list stdout was streamed".into(),
                    )),
                };
                shape_then_status(result, "rev-list --missing=print", parsed)
            },
        )
    }

    /// `fsck --strict`, in the FINAL database: its whole stream classified before its exit status.
    fn proof_fsck(
        &mut self,
        runner: &GitRunnerV1,
        names: &GitRootNamesV1,
        objects: usize,
    ) -> Result<(), GitBootstrapErrorV1> {
        let request = GitRunRequestV1::new(
            GitCommandV1::FsckStrict,
            Vec::new(),
            stdout_limit_for_objects(objects),
            GIT_STDERR_LIMIT_V1,
            self.deadline,
        );
        self.proof_child(runner, names, "fsck --strict", request, |result| {
            classify_fsck(result)?;
            require_success(result, "fsck --strict")
        })
    }

    /// Every staged descriptor goes back to offset 0, as phase V returned it. The ONE absolute deadline's guard
    /// is read before the first rewind (the last child's gate has just ended) and again after EACH rewind
    /// returns, so no rewind follows a slow one without the clock, and the closing gate's entry follows the
    /// last.
    fn proof_rewind_staged(&self) -> Result<(), GitBootstrapErrorV1> {
        observe_deadline(self.deadline, "final-rewind")?;
        for one in &self.boot.verified.staged {
            (&one.file)
                .seek(SeekFrom::Start(0))
                .map_err(|error| io_refusal(&lossy(one.name.as_bytes()), &error))?;
            observe_deadline(self.deadline, "final-rewound")?;
        }
        Ok(())
    }

    /// The closing gate, in this order: the mount census; the exact tree; the digest of every restored file;
    /// every retained chain; and the destination root's identity LAST. Every step is held to the ONE absolute
    /// deadline: it is observed on entry, right after the mount census, inside the census and the digests
    /// (before each filesystem call after the first of a unit, and after the last), right after each of them,
    /// and inside the chains (before each call); the sweep's guard immediately before the root's identity is
    /// the last statement ahead of that identity, which stays the LAST fallible call.
    fn proof_closing_gate(&self) -> Result<(), GitBootstrapErrorV1> {
        let deadline = self.deadline;
        let pace = |step: &'static str| observe_deadline(deadline, step);
        observe_deadline(deadline, "final-closing")?;
        refuse_mounts_within(&self.dest().root)?;
        observe_deadline(deadline, "final-mounts")?;
        let facts = self.proof_measure(&pace)?;
        observe_deadline(deadline, "final-tree")?;
        self.proof_digests(&facts, &pace)?;
        observe_deadline(deadline, "final-digested")?;
        self.proof_chains(&pace, &ChainPassV1::gate())
    }

    /// The final proof: the inventory, the reachability of every inventory object (vacuous for an Empty
    /// database: no request), and the strict fsck, each in the final database after HEAD, refs, and config
    /// exist; then the staged rewinds and the closing gate. Nothing fallible follows the closing gate.
    fn proof_run(
        &mut self,
        runner: &GitRunnerV1,
        format: GitObjectFormatV1,
        expected: &BTreeMap<String, &'static str>,
        captured: bool,
    ) -> Result<GitClosureEvidenceV1, GitBootstrapErrorV1> {
        observe_deadline(self.deadline, "final-entry")?;
        let names = GitRootNamesV1::new(".git", ".git", ".git")?;
        self.proof_inventory(runner, &names, format, expected)?;
        let closure = if captured {
            GitClosureProofV1::Reachable {
                objects: self.proof_reachability(runner, &names, format, expected)?,
            }
        } else {
            GitClosureProofV1::Vacuous
        };
        self.proof_fsck(runner, &names, expected.len())?;
        self.proof_rewind_staged()?;
        #[cfg(test)]
        run_git_hook(
            GitHookPointV1::BeforePlaneFinalGate,
            "",
            self.dest().root.canonical_path(),
        );
        self.proof_closing_gate()?;
        Ok(GitClosureEvidenceV1 {
            inventory_objects: expected.len(),
            closure,
            fsck_strict: true,
        })
    }
}

fn plane_failure(
    cause: GitBootstrapErrorV1,
    last_completed: Option<GitPlaneStepV1>,
    active_materialization_began: bool,
    ledger_used: u64,
    partial: GitPlanePartialV1,
) -> Box<GitPlaneRestoreFailureV1> {
    Box::new(GitPlaneRestoreFailureV1 {
        cause,
        last_completed,
        active_materialization_began,
        ledger_used,
        partial,
    })
}

/// A refusal of a pass that writes nothing (B1a or B1b), with pass A's value and what was planned so far.
fn prewrite_failure(
    boot: VerifiedGitBootstrapV1,
    plan: Option<ActiveFramePlanV1>,
    failure: GitBootstrapFailureV1,
) -> Box<GitPlaneRestoreFailureV1> {
    plane_failure(
        failure.cause,
        failure.last_completed,
        failure.active_materialization_began,
        failure.ledger_used,
        GitPlanePartialV1 {
            bootstrap: Some(boot),
            plan,
            ..GitPlanePartialV1::default()
        },
    )
}

/// Restore phase G: consume the ORIGINAL verified capsule, prove its Git objects in an isolated bootstrap,
/// prevalidate and agree the active frames, create the final object store and the exact active metadata, and
/// prove the final database, all on the ONE caller-supplied absolute `deadline` and the one ledger.
///
/// The order is fixed. The bootstrap (pass A) runs first and closes; the frames are then prevalidated (B1a)
/// and their refs agreed with the manifest (B1b), so nothing is created beneath the repository for a capsule
/// whose frames are refused (an Empty `RefsAndHead` class is [`GitBootstrapErrorV1::EmptyRefsAndHead`], and
/// no HEAD is synthesized); the object copy (B2a) and the metadata with the inert config (B2b) follow; and
/// only then do the final children run, rooted at the retained repository. Every helper receives the same
/// `deadline`, `frame_budget`, and ledger, and the wrapper makes no instant, no budget, and no ledger of its
/// own. A staged name is never reopened.
///
/// A refusal is a [`GitPlaneRestoreFailureV1`] with every retained value and every created byte kept.
pub(crate) fn restore_git_plane_v1(
    verified: VerifiedCapsuleV1,
    runner: &GitRunnerV1,
    object_format: GitObjectFormatV1,
    frame_budget: CustodyFrameBudgetV1,
    deadline: Instant,
) -> Result<RestoredGitPlaneV1, Box<GitPlaneRestoreFailureV1>> {
    restore_git_plane_bounded_v1(
        verified,
        runner,
        object_format,
        frame_budget,
        deadline,
        ACTIVE_PATH_TABLE_BYTES_V1,
        RefsLimitsV1::v1(),
    )
}

/// [`restore_git_plane_v1`] with the aggregate path table's byte bound and the refs limits as parameters, so
/// each is exercised at a small size. Each is handed to every helper that applies it.
pub(crate) fn restore_git_plane_bounded_v1(
    verified: VerifiedCapsuleV1,
    runner: &GitRunnerV1,
    object_format: GitObjectFormatV1,
    frame_budget: CustodyFrameBudgetV1,
    deadline: Instant,
    table_bound: u64,
    limits: RefsLimitsV1,
) -> Result<RestoredGitPlaneV1, Box<GitPlaneRestoreFailureV1>> {
    let mut boot =
        bootstrap_git_objects_v1(verified, runner, object_format, deadline).map_err(|failure| {
            plane_failure(
                failure.cause,
                failure.last_completed,
                failure.active_materialization_began,
                failure.ledger_used,
                GitPlanePartialV1::default(),
            )
        })?;
    let completed = Some(GitPlaneStepV1::BootstrapGate);
    let plan = match prevalidate_active_frames_bounded_v1(
        &mut boot.verified,
        frame_budget,
        deadline,
        completed,
        table_bound,
    ) {
        Ok(plan) => plan,
        Err(failure) => return Err(prewrite_failure(boot, None, failure)),
    };
    let prewrite = match agree_active_refs_bounded_v1(
        &mut boot.verified,
        &plan,
        object_format,
        frame_budget,
        deadline,
        completed,
        limits,
    ) {
        Ok(agreement) => agreement,
        Err(failure) => return Err(prewrite_failure(boot, Some(plan), failure)),
    };
    let objects = match materialize_object_store_v1(&boot, &plan, deadline, completed) {
        Ok(objects) => objects,
        Err(refused) => {
            let ObjectStoreFailureV1 {
                cause,
                last_completed,
                active_materialization_began,
                ledger_used,
                partial,
            } = *refused;
            return Err(plane_failure(
                cause,
                last_completed,
                active_materialization_began,
                ledger_used,
                GitPlanePartialV1 {
                    bootstrap: Some(boot),
                    plan: Some(plan),
                    objects: PlaneObjectsV1::Refused(partial),
                    ..GitPlanePartialV1::default()
                },
            ));
        }
    };
    let metadata = match materialize_active_metadata_bounded_v1(
        &boot,
        &objects,
        &plan,
        frame_budget,
        deadline,
        Some(GitPlaneStepV1::Objects),
        table_bound,
        limits,
    ) {
        Ok(metadata) => metadata,
        Err(refused) => {
            let MetadataFailureV1 {
                cause,
                last_completed,
                ledger_used,
                partial,
                ..
            } = *refused;
            // The object copy that completed is itself active materialization.
            return Err(plane_failure(
                cause,
                last_completed,
                true,
                ledger_used,
                GitPlanePartialV1 {
                    bootstrap: Some(boot),
                    plan: Some(plan),
                    objects: PlaneObjectsV1::Whole(objects),
                    metadata: PlaneMetadataV1::Refused(partial),
                    ..GitPlanePartialV1::default()
                },
            ));
        }
    };
    let captured = matches!(boot.objects, BootstrapObjectsV1::Captured { .. });
    let (proved, final_runs) = match ProofRunV1::new(&boot, &objects, &metadata, deadline) {
        Err(cause) => (Err(cause), Vec::new()),
        Ok(mut run) => {
            let proved = match manifest_inventory(&boot.verified.control.manifest, object_format) {
                Ok(expected) => run.proof_run(runner, object_format, &expected, captured),
                Err(cause) => Err(cause),
            };
            (proved, run.into_runs())
        }
    };
    match proved {
        Ok(evidence) => Ok(RestoredGitPlaneV1 {
            step: GitPlaneStepV1::FinalGate,
            bootstrap: boot,
            plan,
            prewrite_agreement: prewrite,
            objects,
            metadata,
            closure: FinalClosureV1 {
                evidence,
                runs: final_runs,
            },
        }),
        Err(cause) => {
            let ledger_used = boot.verified.destination.ledger.borrow().used();
            Err(plane_failure(
                cause,
                Some(GitPlaneStepV1::Config),
                true,
                ledger_used,
                GitPlanePartialV1 {
                    bootstrap: Some(boot),
                    plan: Some(plan),
                    objects: PlaneObjectsV1::Whole(objects),
                    metadata: PlaneMetadataV1::Whole(metadata),
                    final_runs,
                },
            ))
        }
    }
}
