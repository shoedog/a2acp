//! The ADR-0041 coverage plan, v1 (slice 2B2b2b1).
//!
//! [`plan_coverage_v1`] turns a pinned, quiescent clone into a coverage plan:
//! - canonical coverage rows for all fourteen classes;
//! - the `cargo-target-v1` exclusion and dependency records;
//! - one frame receipt per captured, walked class.
//!
//! **Ownership (task §3).** Every top-level git-directory entry is routed by one closed table,
//! [`route_git_dir_entry`], and each class walk includes its own top-level entries as whole
//! subtrees and skips the rest. The worktree walk emits the pinned git directory as a childless
//! connector, recognized by identity at any depth, and skips only the policy-excluded root
//! `target`. So every entry is owned by exactly one class, is excluded under `cargo-target-v1`,
//! or makes its owning class `unresolved`.
//!
//! **Evidence (§2, §3.3).** Gitlinks, `.gitmodules`, nested repositories, LFS attributes, linked
//! worktrees, alternates, and locks are detected and never hidden. Gitlinks are listed exactly by
//! one read-only probe, `ls-files --stage -z`, over a copy of the source index in a template-less
//! bare repository inside a bounded planning scratch.
//!
//! **Effects.** The source is never written: every source read is descriptor-relative and
//! no-follow, through a retained pin or a fresh pin proved to be the same directory. The planner
//! writes only inside the caller's planning scratch, after proving it disjoint from the whole
//! protected set and rechecking that set, and it charges every scratch byte to one ledger.
//!
//! **Failure semantics (§4.5).** A [`CustodyCoverageErrorV1`] means no plan is trustworthy.
//! Everything class-local, such as an unreadable entry, a probe failure, or a walker refusal,
//! becomes an `unresolved` row instead.

use crate::custody_export::{
    pin_alternate_chain, preflight_scratch_root, protected_directories, remeasure_git_directory_in,
    CustodyExportErrorV1, GitDirectoryBudgetV1, PinnedObjectStoreV1, ScratchLedgerV1,
};
use crate::custody_frame::{
    CustodyFrameBudgetV1, CustodyFrameDecoderV1, CustodyFrameEncoderV1, CustodyFrameEntryV1,
    CustodyFrameErrorV1, CustodyFrameHeaderV1, CustodyFramePathV1,
};
use crate::custody_git::{
    CustodyGitError, GitCommandV1, GitObjectFormatV1, GitRootNamesV1, GitRouteRequestV1,
    GitRunRequestV1, GitRunResultV1, GitRunnerV1,
};
use crate::custody_inventory::{CustodyReasonCodeV1, CustodyStateClassV1};
use crate::custody_seal::{
    CustodyCoverageClassV1, CustodyCoverageEntryV1, CustodyDependencyV1, CustodyExclusionV1,
    CustodyGitObjectFormatV1, CustodyOriginalObjectV1,
};
use crate::custody_walk::{
    walk_tree_v1, CustodyWalkErrorV1, WalkDecisionV1, WalkParkV1, WalkReceiptV1, WalkSelectionV1,
};
use crate::execution_policy::Sha256HexV1;
use crate::fs_custody::{
    pinned_root_unchanged, ChildKindV1, ChildStatV1, FsCustodyError, PinnedDirectoryV1,
};
use ring::digest;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

const LABEL: &str = "custody coverage plan";

const WORK_DIR_NAME: &str = "work";
const HOME_DIR_NAME: &str = "home";
const XDG_DIR_NAME: &str = "xdg";
/// The index probe's bare repository: one component beneath `work/`, and its `GIT_DIR` (§2.2).
const PROBE_GIT_DIR_NAME: &str = "index-probe.git";

const INDEX_NAME: &[u8] = b"index";
const SHARED_INDEX_PREFIX: &[u8] = b"sharedindex.";

/// §2.3: the probe's stdout bound is `min(128 × copied index bytes + 64 KiB, 256 MiB)`.
const PROBE_STDOUT_BYTES_PER_INDEX_BYTE_V1: u64 = 128;
const PROBE_STDOUT_FLOOR_V1: u64 = 64 * 1024;
const PROBE_STDOUT_CEILING_V1: u64 = 256 * 1024 * 1024;
const PROBE_STDERR_LIMIT_V1: usize = 64 * 1024;
/// `init --bare` prints one line.
const INIT_STDOUT_LIMIT_V1: usize = 16 * 1024;
const COPY_BUFFER_BYTES_V1: usize = 64 * 1024;

/// §3.3: the LFS detector's in-memory sink bound.
const LFS_DETECTOR_SINK_BYTES_V1: u64 = 1024 * 1024;
const LFS_FILTER_V1: &[u8] = b"filter=lfs";
/// `info/attributes` is read through the pin up to the detector's sink bound.
const INFO_ATTRIBUTES_MAX_BYTES_V1: u64 = LFS_DETECTOR_SINK_BYTES_V1;
/// A gitfile is one `gitdir: <path>` line.
const GITFILE_MAX_BYTES_V1: u64 = 4096;
const GITFILE_PREFIX_V1: &[u8] = b"gitdir: ";
/// The exporter reads an alternates file to the same bound.
const ALTERNATES_MAX_BYTES_V1: u64 = 64 * 1024;

const CARGO_TARGET_EXCLUSION_ID_V1: &str = "cargo-target-v1";
const CARGO_TARGET_POLICY_VERSION_V1: &str = "cargo-target-v1";
/// The snake-case wire name of `CustodyCoverageClassV1::ReproducibleOutputs` (§3.4). Manifest
/// validation requires only a non-empty string, so the spec fixes this exact value.
const CARGO_TARGET_CONTENT_CLASS_V1: &str = "reproducible_outputs";
const WORKTREE_FILE_DEPENDENCY_KIND_V1: &str = "worktree-file";
const CARGO_TARGET_NAME_V1: &[u8] = b"target";
/// `(dependency id, root file name)`, in dependency-id order. `rust-toolchain` is bound only when
/// the root holds a regular `rust-toolchain.toml`.
const CARGO_DEPENDENCIES_V1: [(&str, &str); 2] =
    [("cargo-lock", "Cargo.lock"), ("cargo-toml", "Cargo.toml")];
const RUST_TOOLCHAIN_DEPENDENCY_V1: (&str, &str) = ("rust-toolchain", "rust-toolchain.toml");

/// The six classes walked from the git-directory root, in canonical class order.
const GIT_DIR_CLASSES_V1: [CustodyCoverageClassV1; 6] = [
    CustodyCoverageClassV1::RefsAndHead,
    CustodyCoverageClassV1::Index,
    CustodyCoverageClassV1::StashAndReflogs,
    CustodyCoverageClassV1::InProgressGitOperations,
    CustodyCoverageClassV1::GitConfigurationAndHooks,
    CustodyCoverageClassV1::BridgeEvidence,
];

/// Every class whose evidence needs the git directory's top-level census. When the census cannot
/// list the root within the entry budget, each of them is `unresolved`.
const CENSUS_CLASSES_V1: [CustodyCoverageClassV1; 10] = [
    CustodyCoverageClassV1::RefsAndHead,
    CustodyCoverageClassV1::ObjectDatabase,
    CustodyCoverageClassV1::Index,
    CustodyCoverageClassV1::StashAndReflogs,
    CustodyCoverageClassV1::InProgressGitOperations,
    CustodyCoverageClassV1::LinkedWorktrees,
    CustodyCoverageClassV1::NestedRepositoriesAndSubmodules,
    CustodyCoverageClassV1::LfsAndExternalPayloads,
    CustodyCoverageClassV1::GitConfigurationAndHooks,
    CustodyCoverageClassV1::BridgeEvidence,
];

/// The top-level git-directory entries whose mere presence with contents is dependency evidence.
const DEPENDENCY_DIRECTORIES_V1: [&[u8]; 3] = [b"worktrees", b"modules", b"lfs"];

// ---------------------------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------------------------

/// The caller's planning-scratch budget (§2.2). Every byte the planner writes below the planning
/// scratch root, plus a fixed allowance per created entry, is charged against it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CustodyPlanningBudgetV1 {
    pub(crate) max_scratch_bytes: u64,
}

/// The caller's declaration that no external evidence is recorded for this unit (§4.4). It has
/// no `Default`: the declaration is a required, explicit input.
#[derive(Debug)]
pub(crate) struct NoExternalEvidenceRecordedV1(());

impl NoExternalEvidenceRecordedV1 {
    pub(crate) const fn declare() -> Self {
        Self(())
    }
}

/// The caller's declaration that the unit has external evidence the planner cannot resolve.
#[derive(Debug)]
pub(crate) struct ExternalEvidenceUnresolvedV1(());

impl ExternalEvidenceUnresolvedV1 {
    pub(crate) const fn declare() -> Self {
        Self(())
    }
}

/// The required external-evidence input (§4.4). Both declarations are constructible only inside
/// `bridge-core`, and neither is a default.
#[derive(Debug)]
pub(crate) enum CustodyExternalEvidenceV1 {
    NoneRecorded(NoExternalEvidenceRecordedV1),
    Unresolved(ExternalEvidenceUnresolvedV1),
}

impl From<NoExternalEvidenceRecordedV1> for CustodyExternalEvidenceV1 {
    fn from(declaration: NoExternalEvidenceRecordedV1) -> Self {
        Self::NoneRecorded(declaration)
    }
}

impl From<ExternalEvidenceUnresolvedV1> for CustodyExternalEvidenceV1 {
    fn from(declaration: ExternalEvidenceUnresolvedV1) -> Self {
        Self::Unresolved(declaration)
    }
}

/// The pinned source: the repository root, its git directory, the primary object store, and the
/// recursive alternate chain. Together they are the full protected set (§2.2).
#[derive(Debug)]
pub(crate) struct CustodyCoverageSourcesV1 {
    source_repository: PinnedDirectoryV1,
    source_git_dir: PinnedDirectoryV1,
    primary_store: PinnedObjectStoreV1,
    alternate_stores: Vec<PinnedObjectStoreV1>,
}

impl CustodyCoverageSourcesV1 {
    /// Pins the three roots and follows the primary store's alternates to a fixed point with 2B2's
    /// `pin_alternate_chain`. For a bare source, `source_repository` is the git directory.
    pub(crate) fn pin(
        source_repository: &Path,
        source_git_dir: &Path,
        primary_store: &Path,
    ) -> Result<Self, CustodyCoverageErrorV1> {
        let pin_refusal =
            |error: CustodyExportErrorV1| CustodyCoverageErrorV1::Io(error.to_string());
        let primary_store = PinnedObjectStoreV1::open(primary_store).map_err(pin_refusal)?;
        let alternate_stores = pin_alternate_chain(&primary_store).map_err(pin_refusal)?;
        let open = |path: &Path| {
            PinnedDirectoryV1::open(path, LABEL)
                .map_err(|error| CustodyCoverageErrorV1::Io(error.to_string()))
        };
        Ok(Self {
            source_repository: open(source_repository)?,
            source_git_dir: open(source_git_dir)?,
            primary_store,
            alternate_stores,
        })
    }

    /// 2B2's protected-set helper over these pins.
    fn protected(&self) -> Vec<&PinnedDirectoryV1> {
        protected_directories(
            &self.source_repository,
            &self.source_git_dir,
            &self.primary_store,
            &self.alternate_stores,
        )
    }
}

/// One planning request (§4.1).
#[derive(Debug)]
pub(crate) struct CustodyCoverageRequestV1 {
    pub(crate) sources: CustodyCoverageSourcesV1,
    pub(crate) generation_id: String,
    pub(crate) frame_budget: CustodyFrameBudgetV1,
    /// Bounds the names each walk lists, skipped entries included, and the census.
    pub(crate) entry_budget: u64,
    pub(crate) planning_budget: CustodyPlanningBudgetV1,
    pub(crate) scratch_root: PathBuf,
    pub(crate) external_evidence: CustodyExternalEvidenceV1,
    pub(crate) object_format: CustodyGitObjectFormatV1,
    /// The manifest object inventory, used only for the object-database row.
    pub(crate) object_inventory: Vec<CustodyOriginalObjectV1>,
    pub(crate) git_route: GitRouteRequestV1,
    /// The runner's deadline for every probe child.
    pub(crate) deadline: Instant,
}

// ---------------------------------------------------------------------------------------------
// Plan and refusals
// ---------------------------------------------------------------------------------------------

/// One captured, walked class's receipt: the frame summary and the walker's inventory digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CustodyClassReceiptV1 {
    pub(crate) class: CustodyCoverageClassV1,
    pub(crate) frame_length: u64,
    pub(crate) frame_sha256: [u8; 32],
    pub(crate) inventory_digest: [u8; 32],
}

/// What the index probe established about gitlinks (§2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CustodyGitlinkEvidenceV1 {
    /// The source has no `index`, so no probe ran: empty gitlink evidence.
    NoIndex,
    /// The probe listed `records` index entries, of which `gitlinks` have mode `160000`.
    Listed { records: u64, gitlinks: u64 },
    /// The probe could not list the index, and the `index` row is `unresolved`.
    Unresolved,
}

/// The coverage plan (§4.1).
#[derive(Debug)]
pub(crate) struct CustodyCoveragePlanV1 {
    coverage: Vec<CustodyCoverageEntryV1>,
    exclusions: Vec<CustodyExclusionV1>,
    dependencies: Vec<CustodyDependencyV1>,
    receipts: Vec<CustodyClassReceiptV1>,
    gitlinks: CustodyGitlinkEvidenceV1,
    scratch_bytes_used: u64,
}

impl CustodyCoveragePlanV1 {
    /// Canonical rows, one per class, in class order.
    pub(crate) fn coverage(&self) -> &[CustodyCoverageEntryV1] {
        &self.coverage
    }

    pub(crate) fn exclusions(&self) -> &[CustodyExclusionV1] {
        &self.exclusions
    }

    /// Canonical dependency records, in dependency-id order.
    pub(crate) fn dependencies(&self) -> &[CustodyDependencyV1] {
        &self.dependencies
    }

    /// One receipt per captured, walked class, in class order.
    pub(crate) fn receipts(&self) -> &[CustodyClassReceiptV1] {
        &self.receipts
    }

    pub(crate) fn gitlinks(&self) -> CustodyGitlinkEvidenceV1 {
        self.gitlinks
    }

    /// The planning-scratch ledger's final charge.
    pub(crate) fn scratch_bytes_used(&self) -> u64 {
        self.scratch_bytes_used
    }
}

/// A failure that makes no plan trustworthy (§4.5). Everything class-local is an `unresolved`
/// row instead.
#[derive(Debug, thiserror::Error)]
pub(crate) enum CustodyCoverageErrorV1 {
    /// The planning scratch root failed its preflight or overlaps the protected set.
    #[error("planning scratch refused: {0}")]
    InvalidScratch(String),
    /// A pinned source root, or an object store's alternates content, changed.
    #[error("a pinned source root changed: {0}")]
    SourceRootDrift(String),
    /// A planning-ledger reservation, or a post-exit re-measure, exceeded the caller budget.
    #[error("planning scratch budget refused: {0}")]
    PlanningScratchBudget(String),
    /// The index probe's runner or probe repository failed outside the class-local cells.
    #[error("index probe infrastructure failed: {0}")]
    ProbeInfrastructure(String),
    /// A class walk skipped other than exactly the entries the other classes own.
    #[error(
        "class-skip accounting mismatch for {class:?}: expected {expected}, walked {observed}"
    )]
    AccountingMismatch {
        class: CustodyCoverageClassV1,
        expected: u64,
        observed: u64,
    },
    /// An I/O failure on a pinned root itself, or in the planning scratch.
    #[error("custody coverage io failed: {0}")]
    Io(String),
}

type PlanResult<T> = Result<T, CustodyCoverageErrorV1>;

// ---------------------------------------------------------------------------------------------
// The git-directory table (§3.1)
// ---------------------------------------------------------------------------------------------

/// Where one top-level git-directory entry belongs, and the unresolved reason it forces on that
/// class, if any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GitDirRouteV1 {
    pub(crate) class: CustodyCoverageClassV1,
    pub(crate) unresolved: Option<CustodyReasonCodeV1>,
}

/// The §3.1 table's plain rows: exact names and `*` prefixes, each to its owning class.
fn git_dir_owner(name: &[u8]) -> Option<CustodyCoverageClassV1> {
    use CustodyCoverageClassV1 as Class;
    Some(match name {
        b"HEAD" | b"ORIG_HEAD" | b"FETCH_HEAD" | b"packed-refs" | b"refs" | b"shallow" => {
            Class::RefsAndHead
        }
        b"logs" => Class::StashAndReflogs,
        b"index" => Class::Index,
        b"config" | b"config.worktree" | b"description" | b"hooks" | b"info" | b"branches"
        | b"remotes" | b"rr-cache" | b"gc.log" => Class::GitConfigurationAndHooks,
        b"MERGE_HEAD"
        | b"MERGE_MSG"
        | b"MERGE_MODE"
        | b"MERGE_RR"
        | b"MERGE_AUTOSTASH"
        | b"AUTO_MERGE"
        | b"CHERRY_PICK_HEAD"
        | b"REVERT_HEAD"
        | b"REBASE_HEAD"
        | b"SQUASH_MSG"
        | b"COMMIT_EDITMSG"
        | b"TAG_EDITMSG"
        | b"NOTES_MERGE_REF"
        | b"NOTES_MERGE_PARTIAL"
        | b"NOTES_MERGE_WORKTREE"
        | b"rebase-merge"
        | b"rebase-apply"
        | b"sequencer" => Class::InProgressGitOperations,
        b"a2a-bridge" => Class::BridgeEvidence,
        b"objects" => Class::ObjectDatabase,
        b"worktrees" | b"commondir" => Class::LinkedWorktrees,
        b"modules" => Class::NestedRepositoriesAndSubmodules,
        b"lfs" => Class::LfsAndExternalPayloads,
        _ if name.starts_with(SHARED_INDEX_PREFIX) => Class::Index,
        _ if name.starts_with(b"BISECT_") => Class::InProgressGitOperations,
        _ if name.starts_with(b"A2A_") => Class::BridgeEvidence,
        _ => return None,
    })
}

/// Routes one top-level git-directory entry by the closed §3.1 table, including its special
/// rows. It is a pure function of the name, so the census and every class walk agree.
pub(crate) fn route_git_dir_entry(name: &[u8]) -> GitDirRouteV1 {
    use CustodyCoverageClassV1 as Class;
    use CustodyReasonCodeV1 as Reason;
    let lock_stem = if name == b"gc.pid" {
        Some(&b"gc"[..])
    } else {
        name.strip_suffix(b".lock")
    };
    if let Some(stem) = lock_stem {
        return GitDirRouteV1 {
            class: git_dir_owner(stem).unwrap_or(Class::GitConfigurationAndHooks),
            unresolved: Some(Reason::WriterUncontrolled),
        };
    }
    let Some(class) = git_dir_owner(name) else {
        return GitDirRouteV1 {
            class: Class::GitConfigurationAndHooks,
            unresolved: Some(Reason::ContentUnresolved),
        };
    };
    let unresolved = match name {
        // The source is itself a linked-worktree git directory.
        b"commondir" => Some(Reason::DependencyUnresolved),
        b"shallow" => Some(Reason::ContentUnresolved),
        _ => None,
    };
    GitDirRouteV1 { class, unresolved }
}

fn last_component(path: &[u8]) -> &[u8] {
    path.rsplit(|byte| *byte == b'/').next().unwrap_or(path)
}

fn is_top_level(path: &[u8]) -> bool {
    !path.contains(&b'/')
}

fn park(reason: CustodyReasonCodeV1, path: &CustodyFramePathV1) -> WalkDecisionV1 {
    WalkDecisionV1::Park(WalkParkV1::new(reason, path.clone()))
}

/// One git-directory class walk (§3.1): the class's own top-level entries as whole subtrees, and
/// a skip for every entry another class owns.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GitDirClassSelectionV1 {
    pub(crate) class: CustodyCoverageClassV1,
}

impl WalkSelectionV1 for GitDirClassSelectionV1 {
    fn decide(&self, path: &CustodyFramePathV1, _stat: &ChildStatV1) -> WalkDecisionV1 {
        let bytes = path.as_bytes();
        if is_top_level(bytes) {
            let route = route_git_dir_entry(bytes);
            if route.class != self.class {
                return WalkDecisionV1::Skip;
            }
            return match route.unresolved {
                Some(reason) => park(reason, path),
                None => WalkDecisionV1::Include,
            };
        }
        if last_component(bytes).ends_with(b".lock") {
            return park(CustodyReasonCodeV1::WriterUncontrolled, path);
        }
        if bytes == b"info/grafts" {
            return park(CustodyReasonCodeV1::ContentUnresolved, path);
        }
        WalkDecisionV1::Include
    }
}

// ---------------------------------------------------------------------------------------------
// The worktree table (§3.2) and the detectors (§3.3)
// ---------------------------------------------------------------------------------------------

/// SMELL-1: only a root `target` that is a directory is ever policy-excluded.
fn is_excludable_target(stat: &ChildStatV1) -> bool {
    stat.kind == ChildKindV1::Directory
}

fn is_pinned_git_dir(stat: &ChildStatV1, git_dir: (u64, u64)) -> bool {
    stat.kind == ChildKindV1::Directory && (stat.dev, stat.ino) == git_dir
}

/// The worktree walk (§3.2). Every decision depends only on `(path, stat)` and the constants
/// fixed at plan start: the pinned git directory's identity and the `cargo-target-v1`
/// precondition.
#[derive(Clone, Copy, Debug)]
pub(crate) struct WorktreeSelectionV1 {
    pub(crate) git_dir: (u64, u64),
    pub(crate) cargo_target_excluded: bool,
}

impl WalkSelectionV1 for WorktreeSelectionV1 {
    fn decide(&self, path: &CustodyFramePathV1, stat: &ChildStatV1) -> WalkDecisionV1 {
        // The connector: the pinned git directory itself, at any depth, whatever its name.
        if is_pinned_git_dir(stat, self.git_dir) {
            return WalkDecisionV1::IncludeEntryOnly;
        }
        let bytes = path.as_bytes();
        if is_top_level(bytes) {
            if bytes == b".git" {
                // A gitfile is ordinary bytes; its target is checked separately. Any other root
                // `.git` is a git directory other than the pinned one.
                return if stat.kind == ChildKindV1::Regular {
                    WalkDecisionV1::Include
                } else {
                    park(CustodyReasonCodeV1::DependencyUnresolved, path)
                };
            }
            if bytes == CARGO_TARGET_NAME_V1
                && self.cargo_target_excluded
                && is_excludable_target(stat)
            {
                return WalkDecisionV1::Skip;
            }
            if bytes == b".gitmodules" {
                return park(CustodyReasonCodeV1::DependencyUnresolved, path);
            }
        } else if last_component(bytes) == b".git" {
            return park(CustodyReasonCodeV1::DependencyUnresolved, path);
        }
        WalkDecisionV1::Include
    }
}

/// The dependency class a worktree park evidences, beside the worktree itself.
fn worktree_park_evidence(path: &CustodyFramePathV1) -> Option<CustodyCoverageClassV1> {
    let bytes = path.as_bytes();
    if bytes == b".git" {
        Some(CustodyCoverageClassV1::LinkedWorktrees)
    } else if bytes == b".gitmodules" || last_component(bytes) == b".git" {
        Some(CustodyCoverageClassV1::NestedRepositoriesAndSubmodules)
    } else {
        None
    }
}

/// The LFS-attributes detector (§3.3). It prunes exactly what the class walks prune (the pinned
/// git directory, every nested `.git`, and the excluded root `target`), descends every other
/// directory, and keeps only `.gitattributes` files.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LfsDetectorSelectionV1 {
    pub(crate) git_dir: (u64, u64),
    pub(crate) cargo_target_excluded: bool,
}

impl WalkSelectionV1 for LfsDetectorSelectionV1 {
    fn decide(&self, path: &CustodyFramePathV1, stat: &ChildStatV1) -> WalkDecisionV1 {
        if is_pinned_git_dir(stat, self.git_dir) {
            return WalkDecisionV1::Skip;
        }
        let bytes = path.as_bytes();
        let name = last_component(bytes);
        if name == b".git" {
            return WalkDecisionV1::Skip;
        }
        if bytes == CARGO_TARGET_NAME_V1 && self.cargo_target_excluded && is_excludable_target(stat)
        {
            return WalkDecisionV1::Skip;
        }
        if stat.kind == ChildKindV1::Directory || name == b".gitattributes" {
            return WalkDecisionV1::Include;
        }
        WalkDecisionV1::Skip
    }
}

/// The object lock sentinel (§3.3): directories are descended, a `*.lock` file parks, and every
/// other file is skipped unread.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ObjectLockSentinelV1;

impl WalkSelectionV1 for ObjectLockSentinelV1 {
    fn decide(&self, path: &CustodyFramePathV1, stat: &ChildStatV1) -> WalkDecisionV1 {
        if stat.kind == ChildKindV1::Directory {
            return WalkDecisionV1::Include;
        }
        if last_component(path.as_bytes()).ends_with(b".lock") {
            return park(CustodyReasonCodeV1::WriterUncontrolled, path);
        }
        WalkDecisionV1::Skip
    }
}

/// §4.3: a walker refusal is a class-local reason, never a plan error.
fn walk_refusal_reason(error: &CustodyWalkErrorV1) -> CustodyReasonCodeV1 {
    match error {
        CustodyWalkErrorV1::Park(park) => park.reason(),
        CustodyWalkErrorV1::MountBoundary { .. } => CustodyReasonCodeV1::MountBoundary,
        CustodyWalkErrorV1::SourceDrift { .. } => CustodyReasonCodeV1::IdentityChanged,
        CustodyWalkErrorV1::Frame(_)
        | CustodyWalkErrorV1::EntryLimit
        | CustodyWalkErrorV1::UnsupportedEntry { .. }
        | CustodyWalkErrorV1::Io(_) => CustodyReasonCodeV1::ContentUnresolved,
    }
}

// ---------------------------------------------------------------------------------------------
// The index probe (§2)
// ---------------------------------------------------------------------------------------------

/// The runner calls of the index probe, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ProbeStageV1 {
    /// `GitRunnerV1::admit`, whose internal `version` run precedes every other stage.
    Admission,
    InitBare,
    LsFilesStageZ,
}

impl ProbeStageV1 {
    fn label(self) -> &'static str {
        match self {
            Self::Admission => "runner admission",
            Self::InitBare => "init --bare",
            Self::LsFilesStageZ => "ls-files --stage -z",
        }
    }
}

/// Where one runner failure lands (§4.5's runner-failure table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RunnerOutcomeV1 {
    /// `ProbeInfrastructure`: the plan is refused.
    Infrastructure,
    /// `index` becomes `unresolved` with `ContentUnresolved`.
    IndexUnresolved,
}

/// §4.5's runner-failure table, exhaustive and deterministic. Only the probe listing's own child
/// outcomes are class-local; a failure of admission, of `init --bare`, or of the route, command,
/// or effect seam under any stage refuses the plan.
pub(crate) fn classify_runner_error(
    stage: ProbeStageV1,
    error: &CustodyGitError,
) -> RunnerOutcomeV1 {
    let child_outcome = matches!(
        error,
        CustodyGitError::Timeout
            | CustodyGitError::Stream(_)
            | CustodyGitError::StdoutLimit { .. }
            | CustodyGitError::StderrLimit { .. }
    );
    match stage {
        ProbeStageV1::LsFilesStageZ if child_outcome => RunnerOutcomeV1::IndexUnresolved,
        ProbeStageV1::Admission | ProbeStageV1::InitBare | ProbeStageV1::LsFilesStageZ => {
            RunnerOutcomeV1::Infrastructure
        }
    }
}

/// `min(128 × copied index bytes + 64 KiB, 256 MiB)` (§2.3).
pub(crate) fn probe_stdout_limit(copied_index_bytes: u64) -> usize {
    let bound = copied_index_bytes
        .saturating_mul(PROBE_STDOUT_BYTES_PER_INDEX_BYTE_V1)
        .saturating_add(PROBE_STDOUT_FLOOR_V1)
        .min(PROBE_STDOUT_CEILING_V1);
    usize::try_from(bound).unwrap_or(usize::MAX)
}

/// A strict `ls-files --stage -z` listing (§2.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct IndexListingV1 {
    pub(crate) records: u64,
    pub(crate) gitlinks: u64,
}

/// Why a listing is refused. Any refusal makes `index` `unresolved`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IndexListingFaultV1 {
    /// The record is shorter than its fixed fields.
    Short,
    /// The mode is not six octal digits.
    Mode,
    /// A field separator is not the one the grammar requires.
    Separator,
    /// The object id is not lowercase hex of the source format's length.
    ObjectId,
    /// The stage is not `0` to `3`.
    Stage,
    /// The path violates the 2B2b1 frame path policy, including its 4096-byte ceiling.
    Path,
    /// Bytes follow the last NUL terminator.
    TrailingBytes,
}

/// Parses strict records `^[0-7]{6} <hex oid> [0-3]\t<path>\0`, each path under the 2B2b1 path
/// policy, and counts the gitlinks (mode `160000`).
pub(crate) fn parse_index_listing(
    bytes: &[u8],
    hex_length: usize,
) -> Result<IndexListingV1, IndexListingFaultV1> {
    let mut listing = IndexListingV1 {
        records: 0,
        gitlinks: 0,
    };
    let mut rest = bytes;
    while !rest.is_empty() {
        let end = rest
            .iter()
            .position(|byte| *byte == 0)
            .ok_or(IndexListingFaultV1::TrailingBytes)?;
        if parse_index_record(&rest[..end], hex_length)? {
            listing.gitlinks += 1;
        }
        listing.records += 1;
        rest = &rest[end + 1..];
    }
    Ok(listing)
}

/// One record without its NUL. Returns whether it is a gitlink.
fn parse_index_record(record: &[u8], hex_length: usize) -> Result<bool, IndexListingFaultV1> {
    // mode, space, object id, space, stage, tab.
    let fixed = 6 + 1 + hex_length + 1 + 1 + 1;
    if record.len() < fixed {
        return Err(IndexListingFaultV1::Short);
    }
    let mode = &record[..6];
    if !mode.iter().all(|byte| (b'0'..=b'7').contains(byte)) {
        return Err(IndexListingFaultV1::Mode);
    }
    if record[6] != b' ' {
        return Err(IndexListingFaultV1::Separator);
    }
    let object_id = &record[7..7 + hex_length];
    if !object_id
        .iter()
        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(IndexListingFaultV1::ObjectId);
    }
    if record[7 + hex_length] != b' ' {
        return Err(IndexListingFaultV1::Separator);
    }
    if !(b'0'..=b'3').contains(&record[8 + hex_length]) {
        return Err(IndexListingFaultV1::Stage);
    }
    if record[9 + hex_length] != b'\t' {
        return Err(IndexListingFaultV1::Separator);
    }
    CustodyFramePathV1::from_bytes(&record[fixed..]).map_err(|_| IndexListingFaultV1::Path)?;
    Ok(mode == b"160000")
}

fn hex_length(format: CustodyGitObjectFormatV1) -> usize {
    match format {
        CustodyGitObjectFormatV1::Sha1 => 40,
        CustodyGitObjectFormatV1::Sha256 => 64,
    }
}

fn git_object_format(format: CustodyGitObjectFormatV1) -> GitObjectFormatV1 {
    match format {
        CustodyGitObjectFormatV1::Sha1 => GitObjectFormatV1::Sha1,
        CustodyGitObjectFormatV1::Sha256 => GitObjectFormatV1::Sha256,
    }
}

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

/// A ledger or re-measure refusal is the planning budget's; anything else from the shared 2B2
/// helpers is scratch I/O.
fn scratch_refusal(error: CustodyExportErrorV1) -> CustodyCoverageErrorV1 {
    match error {
        CustodyExportErrorV1::ScratchLedger(detail)
        | CustodyExportErrorV1::UnexpectedGitFile(detail) => {
            CustodyCoverageErrorV1::PlanningScratchBudget(detail)
        }
        other => CustodyCoverageErrorV1::Io(other.to_string()),
    }
}

fn io_refusal(error: &FsCustodyError) -> CustodyCoverageErrorV1 {
    CustodyCoverageErrorV1::Io(error.to_string())
}

/// A 2B2 store recheck: identity or alternates-content drift is `SourceRootDrift`.
fn recheck_refusal(error: CustodyExportErrorV1) -> CustodyCoverageErrorV1 {
    match error {
        CustodyExportErrorV1::IdentityDrift(detail) => {
            CustodyCoverageErrorV1::SourceRootDrift(detail)
        }
        // An alternates file that grew past its bound since it was pinned changed content.
        CustodyExportErrorV1::SourceState(detail) => {
            CustodyCoverageErrorV1::SourceRootDrift(detail.to_owned())
        }
        other => CustodyCoverageErrorV1::Io(other.to_string()),
    }
}

fn retained_identity(pin: &PinnedDirectoryV1) -> PlanResult<(u64, u64)> {
    match (pin.identity().dev, pin.identity().ino) {
        (Some(dev), Some(ino)) => Ok((dev, ino)),
        _ => Err(CustodyCoverageErrorV1::Io(format!(
            "{}: the retained directory identity (dev/ino) is unavailable",
            pin.canonical_path().display()
        ))),
    }
}

fn file_stat(file: &File) -> io::Result<ChildStatV1> {
    file.metadata()
        .map(|metadata| ChildStatV1::from_metadata(&metadata))
}

fn is_not_found(error: &FsCustodyError) -> bool {
    matches!(error, FsCustodyError::Io(_, error) if error.kind() == io::ErrorKind::NotFound)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn sha256_hex(bytes: [u8; 32]) -> Sha256HexV1 {
    use std::fmt::Write as _;
    let mut hex = String::with_capacity(64);
    for byte in bytes {
        let _ = write!(hex, "{byte:02x}");
    }
    Sha256HexV1::parse(hex).expect("64 lowercase hex digits are a SHA-256")
}

/// Reads one regular file beneath `directory`, through the pin with no-follow, one bounded
/// component at a time. `Ok(None)` means some component is absent. Any other open or read
/// failure, or a file longer than `max_bytes`, is `Err`.
fn read_bounded_child(
    directory: &PinnedDirectoryV1,
    components: &[&str],
    max_bytes: u64,
) -> Result<Option<Vec<u8>>, ()> {
    let Some((file_name, parents)) = components.split_last() else {
        return Err(());
    };
    let mut opened: Option<PinnedDirectoryV1> = None;
    for parent in parents {
        let current = opened.as_ref().unwrap_or(directory);
        match current.open_existing_child_directory(OsStr::new(parent), LABEL) {
            Ok(next) => opened = Some(next),
            Err(error) if is_not_found(&error) => return Ok(None),
            Err(_) => return Err(()),
        }
    }
    let current = opened.as_ref().unwrap_or(directory);
    let file = match current.open_regular_file(OsStr::new(file_name), LABEL) {
        Ok(file) => file,
        Err(error) if is_not_found(&error) => return Ok(None),
        Err(_) => return Err(()),
    };
    let mut bytes = Vec::new();
    file.take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() as u64 > max_bytes {
        return Err(());
    }
    Ok(Some(bytes))
}

/// A SHA-256 sink for streamed file content.
struct HashingSinkV1 {
    context: digest::Context,
    length: u64,
}

impl Write for HashingSinkV1 {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.context.update(bytes);
        self.length += bytes.len() as u64;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Plan state
// ---------------------------------------------------------------------------------------------

/// Which walk a seam event names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WalkKindV1 {
    Class(CustodyCoverageClassV1),
    LfsDetector,
    ObjectLockSentinel,
}

/// A root pinned afresh for exactly one walk. `serial` is unique within the plan.
struct FreshPinV1 {
    pin: PinnedDirectoryV1,
    serial: u64,
}

/// Walks one pinned root with its own pin. Walks run strictly one at a time (§4.2).
fn walk_one<W: Write>(
    kind: WalkKindV1,
    root: &FreshPinV1,
    selection: &dyn WalkSelectionV1,
    entry_budget: u64,
    encoder: CustodyFrameEncoderV1<W>,
) -> Result<WalkReceiptV1, CustodyWalkErrorV1> {
    #[cfg(test)]
    seam::walk_event(seam::WalkEventV1::Begin {
        walk: kind,
        pin: root.serial,
    });
    let outcome = walk_tree_v1(&root.pin, selection, entry_budget, encoder);
    #[cfg(test)]
    seam::walk_event(seam::WalkEventV1::End {
        walk: kind,
        pin: root.serial,
    });
    #[cfg(not(test))]
    let _ = kind;
    outcome
}

/// The git directory's top-level census: every name, its routing, and its listing stat.
struct GitDirCensusV1 {
    entries: Vec<(Vec<u8>, GitDirRouteV1)>,
}

impl GitDirCensusV1 {
    /// The class-skip equation's right side: the entries the other classes own.
    fn owned_by_others(&self, class: CustodyCoverageClassV1) -> u64 {
        self.entries
            .iter()
            .filter(|(_, route)| route.class != class)
            .count() as u64
    }

    fn has(&self, name: &[u8]) -> bool {
        self.entries.iter().any(|(entry, _)| entry == name)
    }

    /// `index`, then every `sharedindex.*`, in byte order.
    fn index_files(&self) -> Vec<&[u8]> {
        let mut files: Vec<&[u8]> = self
            .entries
            .iter()
            .map(|(name, _)| name.as_slice())
            .filter(|name| *name == INDEX_NAME || name.starts_with(SHARED_INDEX_PREFIX))
            .collect();
        files.sort_unstable_by_key(|name| (*name != INDEX_NAME, *name));
        files
    }
}

/// One `cargo-target-v1` dependency file, bound by SHA-256 and by the stat it was hashed under.
struct DependencyBindingV1 {
    dependency_id: &'static str,
    file_name: &'static str,
    sha256: [u8; 32],
    stat: ChildStatV1,
}

/// The worktree constants fixed at plan start (§3.2).
struct WorktreePlanV1 {
    git_dir: (u64, u64),
    cargo_target_excluded: bool,
    dependencies: Vec<DependencyBindingV1>,
}

enum CopyOutcomeV1 {
    Copied(u64),
    Unresolved(CustodyReasonCodeV1),
}

struct PlannerV1<'r> {
    request: &'r CustodyCoverageRequestV1,
    scratch: PinnedDirectoryV1,
    ledger: RefCell<ScratchLedgerV1>,
    reasons: BTreeMap<CustodyCoverageClassV1, BTreeSet<CustodyReasonCodeV1>>,
    walked: BTreeMap<CustodyCoverageClassV1, WalkReceiptV1>,
    next_serial: u64,
}

/// Plans one pinned, quiescent source (§4). See the module documentation.
pub(crate) fn plan_coverage_v1(
    request: &CustodyCoverageRequestV1,
) -> PlanResult<CustodyCoveragePlanV1> {
    let sources = &request.sources;

    // §2.2 overlap preflight over the full protected set, with 2B2's predicate. It creates nothing.
    let scratch = preflight_scratch_root(&request.scratch_root, &sources.protected())
        .map_err(|error| CustodyCoverageErrorV1::InvalidScratch(error.to_string()))?;

    // §2.2 pre-write drift barrier, before the first ledger reservation or scratch write.
    pre_write_barrier(sources)?;
    #[cfg(test)]
    seam::point(&seam::PlanPointV1::BarrierPassed);

    let mut planner = PlannerV1 {
        request,
        scratch,
        ledger: RefCell::new(ScratchLedgerV1::new(
            request.planning_budget.max_scratch_bytes,
        )),
        reasons: BTreeMap::new(),
        walked: BTreeMap::new(),
        next_serial: 0,
    };

    let census = planner.census()?;
    #[cfg(test)]
    seam::point(&seam::PlanPointV1::CensusTaken);
    let worktree = planner.observe_worktree()?;

    let gitlinks = match &census {
        Some(census) => planner.probe_gitlinks(census)?,
        None => CustodyGitlinkEvidenceV1::Unresolved,
    };

    if let Some(census) = &census {
        planner.walk_git_dir_classes(census)?;
    }
    if let Some(worktree) = &worktree {
        planner.walk_worktree(worktree)?;
        #[cfg(test)]
        seam::point(&seam::PlanPointV1::WorktreeWalked);
        planner.recheck_dependencies(worktree);
    }
    planner.detect_lfs(worktree.as_ref())?;
    planner.detect_object_locks()?;
    planner.detect_alternates();
    if matches!(
        request.external_evidence,
        CustodyExternalEvidenceV1::Unresolved(_)
    ) {
        planner.mark(
            CustodyCoverageClassV1::ExternalEvidence,
            CustodyReasonCodeV1::DependencyUnresolved,
        );
    }
    Ok(planner.assemble(worktree.as_ref(), gitlinks))
}

/// §2.2: exactly 2B2's `capability.recheck()` over the protected set. Each member is its own
/// check: the repository root, the git directory, the primary store (identity and alternates
/// content), and every pinned alternate store (the same).
fn pre_write_barrier(sources: &CustodyCoverageSourcesV1) -> PlanResult<()> {
    pinned_root_unchanged(&sources.source_repository)
        .map_err(CustodyCoverageErrorV1::SourceRootDrift)?;
    pinned_root_unchanged(&sources.source_git_dir)
        .map_err(CustodyCoverageErrorV1::SourceRootDrift)?;
    sources.primary_store.recheck().map_err(recheck_refusal)?;
    for store in &sources.alternate_stores {
        store.recheck().map_err(recheck_refusal)?;
    }
    Ok(())
}

impl PlannerV1<'_> {
    fn mark(&mut self, class: CustodyCoverageClassV1, reason: CustodyReasonCodeV1) {
        self.reasons.entry(class).or_default().insert(reason);
    }

    fn is_unresolved(&self, class: CustodyCoverageClassV1) -> bool {
        self.reasons.contains_key(&class)
    }

    /// Opens `retained`'s directory afresh for one walk and proves it is the pinned directory. A
    /// root that no longer resolves to its pin is `SourceRootDrift`.
    fn fresh_pin(&mut self, retained: &PinnedDirectoryV1) -> PlanResult<FreshPinV1> {
        let pin = match PinnedDirectoryV1::open(retained.canonical_path(), LABEL) {
            Ok(pin) => pin,
            Err(error) => {
                return Err(match pinned_root_unchanged(retained) {
                    Err(drift) => CustodyCoverageErrorV1::SourceRootDrift(drift),
                    Ok(()) => io_refusal(&error),
                })
            }
        };
        if !pin.identity().matches(retained.identity()) {
            return Err(CustodyCoverageErrorV1::SourceRootDrift(format!(
                "{} no longer resolves to its pinned directory",
                retained.canonical_path().display()
            )));
        }
        Ok(self.serial(pin))
    }

    fn serial(&mut self, pin: PinnedDirectoryV1) -> FreshPinV1 {
        self.next_serial += 1;
        FreshPinV1 {
            pin,
            serial: self.next_serial,
        }
    }

    /// Lists the git directory's top level once (§3.1), through its retained pin, routing every
    /// entry. The census is not a walk: it only lists and states the top level, before any walk
    /// runs. The special rows and the non-walked dependency directories mark their classes here;
    /// the class walks re-derive the same routing and park on the same entries. `None` means the
    /// root could not be listed within the entry budget, and every census class is then
    /// `unresolved`.
    fn census(&mut self) -> PlanResult<Option<GitDirCensusV1>> {
        let request = self.request;
        let root = &request.sources.source_git_dir;
        let mut remaining = request.entry_budget;
        let listing = root.list_child_names(&mut remaining, LABEL);
        #[cfg(test)]
        let listing = if seam::census_listing_fault() {
            Err(FsCustodyError::Io(
                LABEL.to_owned(),
                io::Error::from(io::ErrorKind::PermissionDenied),
            ))
        } else {
            listing
        };
        let mut names = match listing {
            Ok(names) => names,
            Err(FsCustodyError::EnumerationLimitExceeded { .. }) => {
                for class in CENSUS_CLASSES_V1 {
                    self.mark(class, CustodyReasonCodeV1::ContentUnresolved);
                }
                return Ok(None);
            }
            Err(error) => return Err(io_refusal(&error)),
        };
        names.sort();
        let mut entries = Vec::with_capacity(names.len());
        for name in names {
            let route = route_git_dir_entry(name.as_bytes());
            let stat = match root.child_metadata_no_follow(name.as_os_str(), LABEL) {
                Ok(Some(stat)) => stat,
                Ok(None) => {
                    self.mark(route.class, CustodyReasonCodeV1::IdentityChanged);
                    continue;
                }
                Err(error) => return Err(io_refusal(&error)),
            };
            if let Some(reason) = route.unresolved {
                self.mark(route.class, reason);
            }
            if DEPENDENCY_DIRECTORIES_V1.contains(&name.as_bytes()) {
                if let Some(reason) = dependency_directory_evidence(root, name.as_os_str(), &stat) {
                    self.mark(route.class, reason);
                }
            }
            entries.push((name.as_bytes().to_vec(), route));
        }
        Ok(Some(GitDirCensusV1 { entries }))
    }

    /// Observes the worktree's plan-start constants (§3.2, §3.4). `None` for a bare source, whose
    /// repository root is its git directory.
    fn observe_worktree(&mut self) -> PlanResult<Option<WorktreePlanV1>> {
        let sources = &self.request.sources;
        let git_dir = retained_identity(&sources.source_git_dir)?;
        if retained_identity(&sources.source_repository)? == git_dir {
            return Ok(None);
        }
        let root = &sources.source_repository;
        let stat = |name: &str| {
            root.child_metadata_no_follow(OsStr::new(name), LABEL)
                .map_err(|error| io_refusal(&error))
        };
        let regular = |stat: &Option<ChildStatV1>| {
            stat.as_ref()
                .is_some_and(|stat| stat.kind == ChildKindV1::Regular)
        };

        if stat(".git")?.is_some_and(|stat| stat.kind == ChildKindV1::Regular)
            && !gitfile_names_pinned_git_dir(sources)
        {
            self.mark(
                CustodyCoverageClassV1::LinkedWorktrees,
                CustodyReasonCodeV1::DependencyUnresolved,
            );
        }

        let cargo_target_excluded = CARGO_DEPENDENCIES_V1
            .iter()
            .map(|(_, file_name)| stat(file_name))
            .collect::<PlanResult<Vec<_>>>()?
            .iter()
            .all(regular)
            && stat("target")?.is_some_and(|stat| is_excludable_target(&stat));

        let mut dependencies = Vec::new();
        if cargo_target_excluded {
            let mut files = CARGO_DEPENDENCIES_V1.to_vec();
            if regular(&stat(RUST_TOOLCHAIN_DEPENDENCY_V1.1)?) {
                files.push(RUST_TOOLCHAIN_DEPENDENCY_V1);
            }
            for (dependency_id, file_name) in files {
                match bind_dependency(root, dependency_id, file_name) {
                    Ok(binding) => dependencies.push(binding),
                    Err(reason) => self.mark(CustodyCoverageClassV1::ReproducibleOutputs, reason),
                }
            }
        }
        Ok(Some(WorktreePlanV1 {
            git_dir,
            cargo_target_excluded,
            dependencies,
        }))
    }

    /// The gitlink probe (§2.2–§2.4).
    fn probe_gitlinks(&mut self, census: &GitDirCensusV1) -> PlanResult<CustodyGitlinkEvidenceV1> {
        if !census.has(INDEX_NAME) {
            return Ok(CustodyGitlinkEvidenceV1::NoIndex);
        }
        let request = self.request;

        self.reserve_entries(1)?;
        let work = self
            .scratch
            .create_new_child_directory(OsStr::new(WORK_DIR_NAME), LABEL)
            .map_err(|error| io_refusal(&error))?;
        self.reserve_entries(2)?;
        for name in [HOME_DIR_NAME, XDG_DIR_NAME] {
            work.create_new_child_directory(OsStr::new(name), LABEL)
                .map_err(|error| io_refusal(&error))?;
        }
        let names = GitRootNamesV1::new(HOME_DIR_NAME, XDG_DIR_NAME, PROBE_GIT_DIR_NAME)
            .map_err(|error| CustodyCoverageErrorV1::ProbeInfrastructure(error.to_string()))?;

        let runner = match admit_runner(request, &work, &names) {
            Ok(runner) => runner,
            Err(error) => return self.runner_refusal(ProbeStageV1::Admission, &error),
        };

        // 2B2's init reservation: the nine entries and the logical `HEAD`/`config` bound, before
        // the spawn, then remeasured and reconciled to the bytes Git wrote.
        let mut budget = GitDirectoryBudgetV1::for_init(PROBE_GIT_DIR_NAME);
        self.reserve_entries(budget.entries)?;
        self.reserve(budget.logical_bytes)?;
        let init = run_stage(
            &runner,
            &work,
            &names,
            ProbeStageV1::InitBare,
            GitRunRequestV1::new(
                GitCommandV1::InitBare {
                    dir: PROBE_GIT_DIR_NAME.to_owned(),
                    object_format: git_object_format(request.object_format),
                },
                Vec::new(),
                INIT_STDOUT_LIMIT_V1,
                PROBE_STDERR_LIMIT_V1,
                request.deadline,
            ),
        );
        let init = match init {
            Ok(init) => init,
            Err(error) => return self.runner_refusal(ProbeStageV1::InitBare, &error),
        };
        // A failed initialization refuses the plan whatever it left behind, so its exit status is
        // checked first; a successful one is remeasured and reconciled.
        if !init.status.success() {
            return Err(CustodyCoverageErrorV1::ProbeInfrastructure(format!(
                "init --bare exited {:?}",
                init.status.code()
            )));
        }
        remeasure_git_directory_in(&work, &mut budget, &self.ledger).map_err(scratch_refusal)?;

        let probe = work
            .open_existing_child_directory(OsStr::new(PROBE_GIT_DIR_NAME), LABEL)
            .map_err(|error| io_refusal(&error))?;
        let mut copied = 0_u64;
        for name in census.index_files() {
            match self.copy_index_file(&probe, name, &mut budget)? {
                CopyOutcomeV1::Copied(bytes) => copied = copied.saturating_add(bytes),
                CopyOutcomeV1::Unresolved(reason) => {
                    self.mark(CustodyCoverageClassV1::Index, reason);
                    return Ok(CustodyGitlinkEvidenceV1::Unresolved);
                }
            }
        }

        let listing = run_stage(
            &runner,
            &work,
            &names,
            ProbeStageV1::LsFilesStageZ,
            GitRunRequestV1::new(
                GitCommandV1::LsFilesStageZ,
                Vec::new(),
                probe_stdout_limit(copied),
                PROBE_STDERR_LIMIT_V1,
                request.deadline,
            ),
        );
        let listing = match listing {
            Ok(listing) => listing,
            Err(error) => return self.runner_refusal(ProbeStageV1::LsFilesStageZ, &error),
        };
        // The probe writes nothing; the re-measure proves it.
        remeasure_git_directory_in(&work, &mut budget, &self.ledger).map_err(scratch_refusal)?;
        // §2.3: the exit status is checked before any parsing.
        if !listing.status.success() {
            self.mark(
                CustodyCoverageClassV1::Index,
                CustodyReasonCodeV1::ContentUnresolved,
            );
            return Ok(CustodyGitlinkEvidenceV1::Unresolved);
        }
        let stdout = listing.captured_stdout().unwrap_or_default();
        match parse_index_listing(stdout, hex_length(request.object_format)) {
            Ok(listed) => {
                if listed.gitlinks != 0 {
                    self.mark(
                        CustodyCoverageClassV1::NestedRepositoriesAndSubmodules,
                        CustodyReasonCodeV1::DependencyUnresolved,
                    );
                }
                Ok(CustodyGitlinkEvidenceV1::Listed {
                    records: listed.records,
                    gitlinks: listed.gitlinks,
                })
            }
            Err(_) => {
                self.mark(
                    CustodyCoverageClassV1::Index,
                    CustodyReasonCodeV1::ContentUnresolved,
                );
                Ok(CustodyGitlinkEvidenceV1::Unresolved)
            }
        }
    }

    fn reserve(&self, bytes: u64) -> PlanResult<()> {
        self.ledger
            .borrow_mut()
            .reserve(bytes)
            .map_err(scratch_refusal)
    }

    fn reserve_entries(&self, entries: u64) -> PlanResult<()> {
        self.ledger
            .borrow_mut()
            .reserve_entries(entries)
            .map_err(scratch_refusal)
    }

    /// §4.5's runner-failure table applied to one stage's refusal.
    fn runner_refusal(
        &mut self,
        stage: ProbeStageV1,
        error: &CustodyGitError,
    ) -> PlanResult<CustodyGitlinkEvidenceV1> {
        match classify_runner_error(stage, error) {
            RunnerOutcomeV1::Infrastructure => Err(CustodyCoverageErrorV1::ProbeInfrastructure(
                format!("{}: {error}", stage.label()),
            )),
            RunnerOutcomeV1::IndexUnresolved => {
                self.mark(
                    CustodyCoverageClassV1::Index,
                    CustodyReasonCodeV1::ContentUnresolved,
                );
                Ok(CustodyGitlinkEvidenceV1::Unresolved)
            }
        }
    }

    /// Copies one source index file, read through the pinned git directory, into the probe. Its
    /// bytes and one entry allowance are reserved before the copy, and it is added to the probe's
    /// expected file set so the re-measure admits exactly it.
    fn copy_index_file(
        &self,
        probe: &PinnedDirectoryV1,
        name: &[u8],
        budget: &mut GitDirectoryBudgetV1,
    ) -> PlanResult<CopyOutcomeV1> {
        let name_os = OsStr::from_bytes(name);
        let Ok(mut source) = self
            .request
            .sources
            .source_git_dir
            .open_regular_file(name_os, LABEL)
        else {
            return Ok(CopyOutcomeV1::Unresolved(
                CustodyReasonCodeV1::ContentUnresolved,
            ));
        };
        let Ok(opened) = file_stat(&source) else {
            return Ok(CopyOutcomeV1::Unresolved(
                CustodyReasonCodeV1::ContentUnresolved,
            ));
        };
        self.reserve(opened.size)?;
        self.reserve_entries(1)?;
        budget
            .files
            .insert(String::from_utf8_lossy(name).into_owned());
        budget.logical_bytes = budget
            .logical_bytes
            .checked_add(opened.size)
            .ok_or_else(|| {
                CustodyCoverageErrorV1::PlanningScratchBudget("the probe budget overflowed".into())
            })?;
        budget.entries = budget.entries.saturating_add(1);
        let mut target = probe
            .create_new_regular_child(name_os, LABEL)
            .map_err(|error| io_refusal(&error))?;

        // Copy exactly the opened size, then probe one byte that is never written, so the copy
        // can never exceed its reservation.
        let mut buffer = vec![0_u8; COPY_BUFFER_BYTES_V1];
        let mut copied = 0_u64;
        let mut bounded = (&mut source).take(opened.size);
        loop {
            let read = match bounded.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    return Ok(CopyOutcomeV1::Unresolved(
                        CustodyReasonCodeV1::ContentUnresolved,
                    ))
                }
            };
            target
                .write_all(&buffer[..read])
                .map_err(|error| CustodyCoverageErrorV1::Io(format!("index copy: {error}")))?;
            copied += read as u64;
        }
        let grew = match source.read(&mut buffer[..1]) {
            Ok(read) => read != 0,
            Err(_) => {
                return Ok(CopyOutcomeV1::Unresolved(
                    CustodyReasonCodeV1::ContentUnresolved,
                ))
            }
        };
        let unchanged = file_stat(&source).is_ok_and(|after| after == opened);
        if copied != opened.size || grew || !unchanged {
            return Ok(CopyOutcomeV1::Unresolved(
                CustodyReasonCodeV1::IdentityChanged,
            ));
        }
        Ok(CopyOutcomeV1::Copied(copied))
    }

    /// One walk per git-directory class, each with its own fresh pin (§3.1, §4.2).
    fn walk_git_dir_classes(&mut self, census: &GitDirCensusV1) -> PlanResult<()> {
        for class in GIT_DIR_CLASSES_V1 {
            let root = self.fresh_pin(&self.request.sources.source_git_dir)?;
            let selection = GitDirClassSelectionV1 { class };
            match self.class_walk(class, &root, &selection) {
                Ok(receipt) => {
                    require_class_skips(class, census.owned_by_others(class), &receipt)?;
                    self.walked.insert(class, receipt);
                }
                Err(error) => self.mark(class, walk_refusal_reason(&error)),
            }
        }
        Ok(())
    }

    fn walk_worktree(&mut self, worktree: &WorktreePlanV1) -> PlanResult<()> {
        let class = CustodyCoverageClassV1::Worktree;
        let root = self.fresh_pin(&self.request.sources.source_repository)?;
        let selection = WorktreeSelectionV1 {
            git_dir: worktree.git_dir,
            cargo_target_excluded: worktree.cargo_target_excluded,
        };
        match self.class_walk(class, &root, &selection) {
            Ok(receipt) => {
                // No other class owns a worktree entry; only the excluded `target` is skipped.
                let expected = u64::from(worktree.cargo_target_excluded);
                require_class_skips(class, expected, &receipt)?;
                self.walked.insert(class, receipt);
            }
            Err(error) => {
                if let CustodyWalkErrorV1::Park(park) = &error {
                    if let Some(evidenced) = worktree_park_evidence(park.path()) {
                        self.mark(evidenced, park.reason());
                    }
                }
                self.mark(class, walk_refusal_reason(&error));
            }
        }
        Ok(())
    }

    /// A class walk into a counting, hashing encoder over a sink that stores nothing.
    fn class_walk(
        &self,
        class: CustodyCoverageClassV1,
        root: &FreshPinV1,
        selection: &dyn WalkSelectionV1,
    ) -> Result<WalkReceiptV1, CustodyWalkErrorV1> {
        let header = CustodyFrameHeaderV1::new(class, &self.request.generation_id)?;
        let encoder = CustodyFrameEncoderV1::new(io::sink(), &header, self.request.frame_budget)?;
        walk_one(
            WalkKindV1::Class(class),
            root,
            selection,
            self.request.entry_budget,
            encoder,
        )
    }

    /// Each dependency file must still have the stat it was hashed under after the worktree walk
    /// captured it, so the bound digest is the captured content's.
    fn recheck_dependencies(&mut self, worktree: &WorktreePlanV1) {
        let root = &self.request.sources.source_repository;
        let drifted = worktree.dependencies.iter().any(|binding| {
            !matches!(
                root.child_metadata_no_follow(OsStr::new(binding.file_name), LABEL),
                Ok(Some(stat)) if stat == binding.stat
            )
        });
        if drifted {
            self.mark(
                CustodyCoverageClassV1::ReproducibleOutputs,
                CustodyReasonCodeV1::IdentityChanged,
            );
        }
    }

    /// The LFS detector walk and the `info/attributes` scan (§3.3).
    fn detect_lfs(&mut self, worktree: Option<&WorktreePlanV1>) -> PlanResult<()> {
        let lfs = CustodyCoverageClassV1::LfsAndExternalPayloads;
        if let Some(worktree) = worktree {
            let root = self.fresh_pin(&self.request.sources.source_repository)?;
            let selection = LfsDetectorSelectionV1 {
                git_dir: worktree.git_dir,
                cargo_target_excluded: worktree.cargo_target_excluded,
            };
            let mut sink = Vec::new();
            let outcome = CustodyFrameHeaderV1::new(lfs, &self.request.generation_id)
                .and_then(|header| {
                    let budget = CustodyFrameBudgetV1::new(
                        self.request.frame_budget.max_entries(),
                        LFS_DETECTOR_SINK_BYTES_V1,
                    )?;
                    Ok((header, budget))
                })
                .map_err(CustodyWalkErrorV1::from)
                .and_then(|(header, budget)| {
                    let encoder = CustodyFrameEncoderV1::new(&mut sink, &header, budget)?;
                    let receipt = walk_one(
                        WalkKindV1::LfsDetector,
                        &root,
                        &selection,
                        self.request.entry_budget,
                        encoder,
                    )?;
                    Ok((header, budget, receipt))
                });
            match outcome {
                // A detector skips by selection, not by ownership, so its receipt is exempt from
                // the class-skip equation (§3.3): its completeness is its pruning.
                Ok((header, budget, _exempt)) => {
                    match attributes_frame_names_lfs(&sink, &header, budget) {
                        Ok(true) => self.mark(lfs, CustodyReasonCodeV1::DependencyUnresolved),
                        Ok(false) => {}
                        Err(_) => self.mark(
                            CustodyCoverageClassV1::Worktree,
                            CustodyReasonCodeV1::ContentUnresolved,
                        ),
                    }
                }
                // A detector budget or walker refusal leaves the worktree's evidence incomplete.
                Err(error) => self.mark(
                    CustodyCoverageClassV1::Worktree,
                    walk_refusal_reason(&error),
                ),
            }
        }
        match read_bounded_child(
            &self.request.sources.source_git_dir,
            &["info", "attributes"],
            INFO_ATTRIBUTES_MAX_BYTES_V1,
        ) {
            Ok(Some(bytes)) if contains(&bytes, LFS_FILTER_V1) => {
                self.mark(lfs, CustodyReasonCodeV1::DependencyUnresolved);
            }
            Ok(_) => {}
            Err(()) => self.mark(lfs, CustodyReasonCodeV1::ContentUnresolved),
        }
        Ok(())
    }

    /// The object lock sentinel (§3.3). Its frame holds directories only and is discarded; the
    /// frame format cannot carry `object_database`, so its header names the neighbouring
    /// `alternates_and_shared_stores` class.
    fn detect_object_locks(&mut self) -> PlanResult<()> {
        let class = CustodyCoverageClassV1::ObjectDatabase;
        let git_dir = self.fresh_pin(&self.request.sources.source_git_dir)?;
        let objects = match git_dir
            .pin
            .open_existing_child_directory(OsStr::new("objects"), LABEL)
        {
            Ok(objects) => self.serial(objects),
            Err(_) => {
                self.mark(class, CustodyReasonCodeV1::ContentUnresolved);
                return Ok(());
            }
        };
        drop(git_dir);
        let outcome = CustodyFrameHeaderV1::new(
            CustodyCoverageClassV1::AlternatesAndSharedStores,
            &self.request.generation_id,
        )
        .and_then(|header| {
            CustodyFrameEncoderV1::new(io::sink(), &header, self.request.frame_budget)
        })
        .map_err(CustodyWalkErrorV1::from)
        .and_then(|encoder| {
            walk_one(
                WalkKindV1::ObjectLockSentinel,
                &objects,
                &ObjectLockSentinelV1,
                self.request.entry_budget,
                encoder,
            )
        });
        if let Err(error) = outcome {
            self.mark(class, walk_refusal_reason(&error));
        }
        Ok(())
    }

    /// `objects/info/alternates`, read through the pinned git directory with no-follow (§3.1).
    /// Any non-comment line is dependency evidence, and so is any store the request pinned in the
    /// primary store's alternate chain.
    fn detect_alternates(&mut self) {
        let class = CustodyCoverageClassV1::AlternatesAndSharedStores;
        if !self.request.sources.alternate_stores.is_empty() {
            self.mark(class, CustodyReasonCodeV1::DependencyUnresolved);
        }
        match read_bounded_child(
            &self.request.sources.source_git_dir,
            &["objects", "info", "alternates"],
            ALTERNATES_MAX_BYTES_V1,
        ) {
            Ok(Some(bytes)) if names_an_alternate(&bytes) => {
                self.mark(class, CustodyReasonCodeV1::DependencyUnresolved);
            }
            Ok(_) => {}
            Err(()) => self.mark(class, CustodyReasonCodeV1::ContentUnresolved),
        }
    }

    fn assemble(
        self,
        worktree: Option<&WorktreePlanV1>,
        gitlinks: CustodyGitlinkEvidenceV1,
    ) -> CustodyCoveragePlanV1 {
        let reproducible = CustodyCoverageClassV1::ReproducibleOutputs;
        let excluded = !self.is_unresolved(reproducible)
            && worktree.is_some_and(|worktree| worktree.cargo_target_excluded);
        let mut coverage = Vec::with_capacity(CustodyCoverageClassV1::ALL.len());
        let mut receipts = Vec::new();
        for class in CustodyCoverageClassV1::ALL {
            let row = if let Some(reasons) = self.reasons.get(&class) {
                CustodyCoverageEntryV1::new(
                    class,
                    CustodyStateClassV1::Unresolved,
                    reasons.iter().copied().collect(),
                    None,
                )
            } else if class == reproducible && excluded {
                CustodyCoverageEntryV1::new(
                    class,
                    CustodyStateClassV1::ExcludedReproducible,
                    Vec::new(),
                    Some(CARGO_TARGET_EXCLUSION_ID_V1.to_owned()),
                )
            } else if class == CustodyCoverageClassV1::ObjectDatabase {
                let state = if self.request.object_inventory.is_empty() {
                    CustodyStateClassV1::Empty
                } else {
                    CustodyStateClassV1::Captured
                };
                CustodyCoverageEntryV1::new(class, state, Vec::new(), None)
            } else if let Some(receipt) = self
                .walked
                .get(&class)
                .filter(|receipt| receipt.summary().entries() != 0)
            {
                receipts.push(CustodyClassReceiptV1 {
                    class,
                    frame_length: receipt.summary().frame_bytes(),
                    frame_sha256: receipt.summary().frame_sha256(),
                    inventory_digest: receipt.inventory_sha256(),
                });
                CustodyCoverageEntryV1::new(class, CustodyStateClassV1::Captured, Vec::new(), None)
            } else {
                CustodyCoverageEntryV1::new(class, CustodyStateClassV1::Empty, Vec::new(), None)
            };
            coverage.push(row.expect("a planned row satisfies the coverage-row invariants"));
        }

        let (exclusions, dependencies) = match worktree {
            Some(worktree) if excluded => cargo_target_records(&worktree.dependencies),
            _ => (Vec::new(), Vec::new()),
        };
        CustodyCoveragePlanV1 {
            coverage,
            exclusions,
            dependencies,
            receipts,
            gitlinks,
            scratch_bytes_used: self.ledger.borrow().used(),
        }
    }
}

/// §4.3's class-skip equation for one capture walk.
fn require_class_skips(
    class: CustodyCoverageClassV1,
    expected: u64,
    receipt: &WalkReceiptV1,
) -> PlanResult<()> {
    if receipt.skipped_entries() != expected {
        return Err(CustodyCoverageErrorV1::AccountingMismatch {
            class,
            expected,
            observed: receipt.skipped_entries(),
        });
    }
    Ok(())
}

/// `worktrees/`, `modules/`, or `lfs/` with any content, or of any other kind than a directory,
/// is dependency evidence; an unreadable one is `ContentUnresolved` (§3.1).
fn dependency_directory_evidence(
    root: &PinnedDirectoryV1,
    name: &OsStr,
    stat: &ChildStatV1,
) -> Option<CustodyReasonCodeV1> {
    if stat.kind != ChildKindV1::Directory {
        return Some(CustodyReasonCodeV1::DependencyUnresolved);
    }
    let Ok(directory) = root.open_existing_child_directory(name, LABEL) else {
        return Some(CustodyReasonCodeV1::ContentUnresolved);
    };
    let mut one = 1_u64;
    match directory.list_child_names(&mut one, LABEL) {
        Ok(names) if names.is_empty() => None,
        Ok(_) | Err(FsCustodyError::EnumerationLimitExceeded { .. }) => {
            Some(CustodyReasonCodeV1::DependencyUnresolved)
        }
        Err(_) => Some(CustodyReasonCodeV1::ContentUnresolved),
    }
}

/// Whether the root `.git` gitfile names the pinned git directory (§3.2).
///
/// The `gitdir:` target is resolved by [`resolve_gitfile_target`] without stating a path or
/// following a symlink, and must be the pinned directory's canonical path. That path is the one its
/// identity was recorded under: the pre-write barrier proved it still resolves to the pinned
/// `(dev, ino)` before any write, and every later fresh pin of the git directory proves it again,
/// refusing the plan otherwise. So a gitfile that passes names the pinned directory by identity.
fn gitfile_names_pinned_git_dir(sources: &CustodyCoverageSourcesV1) -> bool {
    let Ok(Some(bytes)) =
        read_bounded_child(&sources.source_repository, &[".git"], GITFILE_MAX_BYTES_V1)
    else {
        return false;
    };
    let Some(mut target) = bytes.strip_prefix(GITFILE_PREFIX_V1) else {
        return false;
    };
    while let Some(trimmed) = target
        .strip_suffix(b"\n")
        .or_else(|| target.strip_suffix(b"\r"))
    {
        target = trimmed;
    }
    if target.is_empty() || target.contains(&0) || target.contains(&b'\n') {
        return false;
    }
    resolve_gitfile_target(
        sources.source_repository.canonical_path(),
        Path::new(OsStr::from_bytes(target)),
    )
    .is_some_and(|resolved| resolved == sources.source_git_dir.canonical_path())
}

/// Resolves a gitfile's `gitdir:` target the way Git does (relative to the directory holding the
/// gitfile), lexically and exactly:
/// - an absolute target is taken as it is;
/// - a relative target is joined to `root`, the worktree's canonical path, and each *leading* `..`
///   removes one component of it. A canonical path holds no symlink, so that is the physical
///   parent, as it is for Git's `../repo/.git` and relative-worktree paths.
///
/// A `..` after a component the target names itself could cross a symlink, whose physical parent
/// lexical removal cannot know, and so could name another directory than it appears to. It is
/// refused (`None`), as is any `..` in an absolute target and a `..` that climbs past `/`.
fn resolve_gitfile_target(root: &Path, target: &Path) -> Option<PathBuf> {
    let mut resolved = if target.is_absolute() {
        PathBuf::from("/")
    } else {
        root.to_path_buf()
    };
    let mut climbing = !target.is_absolute();
    for component in target.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir if climbing => {
                if !resolved.pop() {
                    return None;
                }
            }
            Component::ParentDir | Component::Prefix(_) => return None,
            Component::Normal(name) => {
                climbing = false;
                resolved.push(name);
            }
        }
    }
    Some(resolved)
}

/// Hashes one dependency file through the worktree pin, bound to the stat it was read under.
fn bind_dependency(
    root: &PinnedDirectoryV1,
    dependency_id: &'static str,
    file_name: &'static str,
) -> Result<DependencyBindingV1, CustodyReasonCodeV1> {
    let mut file = root
        .open_regular_file(OsStr::new(file_name), LABEL)
        .map_err(|_| CustodyReasonCodeV1::ContentUnresolved)?;
    let opened = file_stat(&file).map_err(|_| CustodyReasonCodeV1::ContentUnresolved)?;
    let mut sink = HashingSinkV1 {
        context: digest::Context::new(&digest::SHA256),
        length: 0,
    };
    io::copy(&mut file, &mut sink).map_err(|_| CustodyReasonCodeV1::ContentUnresolved)?;
    let unchanged = file_stat(&file).is_ok_and(|after| after == opened);
    if !unchanged || sink.length != opened.size {
        return Err(CustodyReasonCodeV1::IdentityChanged);
    }
    let mut sha256 = [0_u8; 32];
    sha256.copy_from_slice(sink.context.finish().as_ref());
    Ok(DependencyBindingV1 {
        dependency_id,
        file_name,
        sha256,
        stat: opened,
    })
}

/// §3.4's exact records.
fn cargo_target_records(
    bindings: &[DependencyBindingV1],
) -> (Vec<CustodyExclusionV1>, Vec<CustodyDependencyV1>) {
    let exclusion = CustodyExclusionV1::new(
        CARGO_TARGET_EXCLUSION_ID_V1,
        CARGO_TARGET_CONTENT_CLASS_V1,
        CARGO_TARGET_POLICY_VERSION_V1,
        bindings
            .iter()
            .map(|binding| binding.dependency_id.to_owned())
            .collect(),
    )
    .expect("the cargo-target-v1 exclusion always names its dependencies");
    let dependencies = bindings
        .iter()
        .map(|binding| {
            CustodyDependencyV1::new(
                binding.dependency_id,
                WORKTREE_FILE_DEPENDENCY_KIND_V1,
                sha256_hex(binding.sha256),
                CustodyStateClassV1::Captured,
                Vec::new(),
            )
            .expect("a captured dependency record carries no reasons")
        })
        .collect();
    (vec![exclusion], dependencies)
}

fn names_an_alternate(bytes: &[u8]) -> bool {
    bytes.split(|byte| *byte == b'\n').any(|line| {
        let line = line.trim_ascii();
        !line.is_empty() && line[0] != b'#'
    })
}

/// Decodes the LFS detector's frame and scans every `.gitattributes` for `filter=lfs`.
fn attributes_frame_names_lfs(
    frame: &[u8],
    header: &CustodyFrameHeaderV1,
    budget: CustodyFrameBudgetV1,
) -> Result<bool, CustodyFrameErrorV1> {
    let mut decoder = CustodyFrameDecoderV1::new(frame, header, budget)?;
    let mut found = false;
    while let Some(entry) = decoder.next_entry()? {
        if let CustodyFrameEntryV1::Regular {
            path, mut content, ..
        } = entry
        {
            let mut bytes = Vec::new();
            content.read_to_end(&mut bytes).map_err(|error| {
                CustodyFrameErrorV1::from_io_error(&error)
                    .unwrap_or(CustodyFrameErrorV1::Io(error.kind()))
            })?;
            if last_component(path.as_bytes()) == b".gitattributes"
                && contains(&bytes, LFS_FILTER_V1)
            {
                found = true;
            }
        }
    }
    Ok(found)
}

/// Admits the runner at `work/`, or the seam's injected admission refusal.
fn admit_runner(
    request: &CustodyCoverageRequestV1,
    work: &PinnedDirectoryV1,
    names: &GitRootNamesV1,
) -> Result<GitRunnerV1, CustodyGitError> {
    #[cfg(test)]
    if let Some(error) = seam::runner_fault(ProbeStageV1::Admission) {
        return Err(error);
    }
    GitRunnerV1::admit(request.git_route.clone(), work, names, request.deadline)
}

/// Runs one probe child, rooted at `work/`, or returns the seam's injected refusal.
fn run_stage(
    runner: &GitRunnerV1,
    work: &PinnedDirectoryV1,
    names: &GitRootNamesV1,
    stage: ProbeStageV1,
    request: GitRunRequestV1,
) -> Result<GitRunResultV1, CustodyGitError> {
    #[cfg(test)]
    if let Some(error) = seam::runner_fault(stage) {
        return Err(error);
    }
    let result = runner.run(work, names, request, || Ok(()), || Ok(()));
    #[cfg(test)]
    if let Ok(result) = &result {
        seam::record_run(stage, result);
    }
    #[cfg(not(test))]
    let _ = stage;
    result
}

// ---------------------------------------------------------------------------------------------
// Test seam
// ---------------------------------------------------------------------------------------------

/// The planner's test-only seam: a record of every walk's pin and every probe run, a point hook
/// between plan stages, an injected runner refusal per stage, and an injected census failure.
#[cfg(test)]
pub(crate) mod seam {
    use super::{CustodyGitError, GitRunResultV1, ProbeStageV1, WalkKindV1};
    use std::cell::RefCell;
    use std::ffi::OsString;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum WalkEventV1 {
        Begin { walk: WalkKindV1, pin: u64 },
        End { walk: WalkKindV1, pin: u64 },
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum PlanPointV1 {
        /// The pre-write barrier passed; nothing has been written.
        BarrierPassed,
        /// The git directory's census is complete; no probe or walk has run.
        CensusTaken,
        /// The worktree walk is complete; the dependency files are not yet re-stated.
        WorktreeWalked,
    }

    /// One completed probe run.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(crate) struct ProbeRunV1 {
        pub(crate) stage: ProbeStageV1,
        pub(crate) argv: Vec<OsString>,
        pub(crate) git_dir: Option<String>,
        pub(crate) exit_status: Option<i32>,
    }

    pub(crate) type PointHookV1 = Box<dyn FnMut(&PlanPointV1)>;
    pub(crate) type RunnerFaultV1 = Box<dyn Fn() -> CustodyGitError>;

    #[derive(Default)]
    pub(crate) struct PlanSeamsV1 {
        pub(crate) point: Option<PointHookV1>,
        pub(crate) runner_fault: Option<(ProbeStageV1, RunnerFaultV1)>,
        pub(crate) census_listing_fault: bool,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub(crate) struct PlanRecordV1 {
        pub(crate) walks: Vec<WalkEventV1>,
        pub(crate) runs: Vec<ProbeRunV1>,
    }

    struct StateV1 {
        seams: PlanSeamsV1,
        record: PlanRecordV1,
    }

    thread_local! {
        static STATE: RefCell<Option<StateV1>> = const { RefCell::new(None) };
    }

    /// Uninstalls the seam when dropped.
    pub(crate) struct InstalledV1(());

    impl InstalledV1 {
        pub(crate) fn record(&self) -> PlanRecordV1 {
            STATE.with(|state| {
                state
                    .borrow()
                    .as_ref()
                    .map(|state| state.record.clone())
                    .unwrap_or_default()
            })
        }
    }

    impl Drop for InstalledV1 {
        fn drop(&mut self) {
            STATE.with(|state| *state.borrow_mut() = None);
        }
    }

    pub(crate) fn install(seams: PlanSeamsV1) -> InstalledV1 {
        STATE.with(|state| {
            *state.borrow_mut() = Some(StateV1 {
                seams,
                record: PlanRecordV1::default(),
            });
        });
        InstalledV1(())
    }

    fn with_state<T>(update: impl FnOnce(&mut StateV1) -> T) -> Option<T> {
        STATE.with(|state| state.borrow_mut().as_mut().map(update))
    }

    pub(super) fn walk_event(event: WalkEventV1) {
        with_state(|state| state.record.walks.push(event));
    }

    /// Runs the hook with the state released, so it may change the tree freely.
    pub(super) fn point(point: &PlanPointV1) {
        let hook = with_state(|state| state.seams.point.take()).flatten();
        if let Some(mut hook) = hook {
            hook(point);
            with_state(|state| state.seams.point = Some(hook));
        }
    }

    pub(super) fn runner_fault(stage: ProbeStageV1) -> Option<CustodyGitError> {
        with_state(|state| match &state.seams.runner_fault {
            Some((armed, fault)) if *armed == stage => Some(fault()),
            _ => None,
        })
        .flatten()
    }

    pub(super) fn census_listing_fault() -> bool {
        with_state(|state| state.seams.census_listing_fault).unwrap_or(false)
    }

    pub(super) fn record_run(stage: ProbeStageV1, result: &GitRunResultV1) {
        with_state(|state| {
            state.record.runs.push(ProbeRunV1 {
                stage,
                argv: result.evidence.argv.clone(),
                git_dir: result.evidence.environment.get("GIT_DIR").cloned(),
                exit_status: result.evidence.exit_status,
            });
        });
    }
}

#[cfg(test)]
#[path = "custody_coverage_tests.rs"]
mod tests;
