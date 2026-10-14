//! ADR-0041 slice 2B3c, phase P: the payload plane.
//!
//! Phase P consumes the crate-private phase-G success value [`RestoredGitPlaneV1`] and restores the rest
//! of the active payload and the inert evidence plane into the same destination: the worktree frame into
//! `repository/`, the index frame into `repository/.git/` (`index` and `sharedindex.*` only), and the
//! three evidence-only classes into `evidence/<class wire name>/`.
//!
//! **Increment C2a: admission, prevalidation, and the plan.** [`plan_payload_plane_v1`] writes nothing.
//! It returns the phase-G value unchanged beside a pure, bounded [`PayloadPlanV1`] that the later
//! materialization consumes, or a typed [`PayloadPlaneRestoreFailureV1`] that keeps the phase-G value.
//!
//! **Admission** ([`PayloadPlaneStepV1::Admission`]). The Captured classes must lie in the exact
//! eight-class union: the three phase G consumed and the five phase P consumes. Any other Captured
//! class is [`PayloadPlaneErrorV1::UnsupportedCapturedClass`] (D5). Each of the five phase-P classes must
//! be exactly Captured with one staged payload, or Empty with none; every other state or shape is
//! [`PayloadPlaneErrorV1::UnsupportedCoverage`], never downgraded to Empty. Each phase-P payload's retained
//! descriptor is then rehashed against its staged length and SHA-256, and the first gate runs.
//!
//! **Prevalidation** ([`PayloadPlaneStepV1::Prevalidation`]). In step order (worktree, index, then the
//! evidence classes), each frame is rehashed again and decoded completely, through its verified trailer,
//! into a null sink. Every decoded entry feeds one bounded [`PayloadPlanTableV1`] keyed by its exact path
//! bytes, which applies the worktree connector rules (D3), the path rules (D6), the index names, and the
//! external-target classification (D7). Every byte the decoder consumed is counted and hashed, and once
//! the decoder reports the verified trailer the count and digest must equal the staged length and
//! SHA-256, as phase G's prevalidation requires ([`PayloadPlaneErrorV1::StagedInputChanged`]). An entry
//! refusal is held until that binding completes, so a frame that is not whole or not the verified bytes is
//! refused as that, and nothing it says is trusted. The step ends in the gate again.
//!
//! **Gates.** The mount census over the destination first, then every retained chain phase G left (its
//! object store and metadata beneath `repository/.git`, the bootstrap's admitted object files, and the
//! staging, HOME, XDG, and bootstrap directories), and the destination root's identity LAST, with the one
//! caller-supplied deadline observed before every step and immediately before that identity.
//!
//! **Effects of planning.** None. No entry is created, nothing is charged to the one ledger, no Git child
//! runs, and no staged name is reopened: every payload byte is read through its retained descriptor, which
//! is back at offset 0 on success. A refusal reports the one ledger's figure, which planning never moves.
//!
//! **Increment C2b: materialization.** [`materialize_payload_plane_v1`] plans first, exactly as above,
//! and then writes the plan in three steps, each ending in the gate: [`PayloadPlaneStepV1::Worktree`]
//! (the worktree frame into `repository/`, with D3's root gitfile into
//! `evidence/worktree_root_gitfile/gitfile`), [`PayloadPlaneStepV1::Index`] (`index` and
//! `sharedindex.*` into `repository/.git/`), and [`PayloadPlaneStepV1::Evidence`] (each evidence-only
//! class into `evidence/<class wire name>/`, D1). Each frame is rehashed from its retained descriptor and
//! decoded a THIRD time; every decoded entry must be the entry the plan admitted at that position before
//! anything is created for it, and every byte the decoder consumes is counted and hashed and, once the
//! decoder reports the verified trailer, bound to the staged length and SHA-256
//! ([`PayloadPlaneErrorV1::StagedInputChanged`]). A refusal of what was done with an entry is held until
//! that binding completes; the deadline and the decoder's own failure end the decode at once.
//!
//! **Writes.** Every entry is created create-new and no-follow beneath a retained pin: phase G's
//! `repository/` and `repository/.git`, the destination root (for `evidence/` alone), or a directory this
//! invocation created and pinned. Every regular file is created `0600` and every directory `0700`, and
//! each must keep that mode: its creation and every later gate require it. The frame's mode and the mode
//! the terminal gate is to apply are recorded, never applied (final modes are increment C3's). A symlink is written verbatim through the symlink primitive and never followed. A
//! preexisting entry in an intended slot, including two frame names that a case- or
//! normalization-folding destination makes one, is [`PayloadPlaneErrorV1::DestinationCollision`]; a
//! symlink the destination cannot hold is [`PayloadPlaneErrorV1::DestinationUnsupported`]. Every created
//! directory, file, and symlink must be on the destination root's device. Nothing is renamed, overwritten,
//! chmodded, deleted, or synced beyond the directory primitive's own sync, and no Git child runs; phase P
//! writes nothing beneath `repository/.git` but `index` and `sharedindex.*`. `evidence/` and a slot
//! directory exist only once an entry needs them, so an Empty class creates nothing.
//!
//! **The deadline.** The one caller-supplied deadline is read before every frame and every entry, between
//! chunks, and immediately after every create returns (once what it made is kept, charged, and recorded, or
//! in flight), so a slow create, write, or verification is never followed by another create past it.
//!
//! **Accounting.** The one ledger phase V created and phase G carried: each entry's allowance is reserved
//! before its create-new and given back only when the create provably made nothing (a collision, or a
//! symlink the destination cannot hold); each chunk's bytes are reserved before they are written, and only
//! the part of a chunk that never reached the file is given back. A symlink's target is within its entry
//! allowance.
//!
//! **Result.** [`MaterializedPayloadPlaneV1`]: the phase-G value, the plan, and every created entry's
//! identity (its device, inode, kind, length and SHA-256 or target bytes, frame mode, created mode, and
//! planned final mode) with every created directory's retained pin: what the terminal gate needs. A refusal
//! keeps all of it in [`PayloadPlanePartialV1`], with the file that was in flight; nothing is removed, and a
//! second invocation over the same destination refuses as a collision at its first create.

use super::git::{
    before_deadline, coverage_state, metadata_name, observe_deadline, output_chains, pin_beneath,
    recheck_regular_entry, reverify_staged_v1, source_files_unchanged, split_relative,
    GitBootstrapErrorV1, PaceV1, PacedFrameReaderV1, PinViewV1, RestoredGitPlaneV1,
    VerifiedGitBootstrapV1, BOOTSTRAP_NAME_V1, HOME_NAME_V1, XDG_NAME_V1,
};
use super::{
    joined, lossy, recheck_entry, refuse_mounts_within, CustodyRestoreErrorV1,
    RestoreDestinationV1, StagedPlaintextV1, VerifiedCapsuleV1, HASH_BUFFER_BYTES_V1,
    RESTORE_WORK_DIR_NAME_V1,
};
use crate::custody_capsule::CustodyCapsuleArtifactRoleV1;
use crate::custody_frame::{
    CustodyFrameBudgetV1, CustodyFrameComponentFaultV1, CustodyFrameDecoderV1, CustodyFrameEntryV1,
    CustodyFrameHeaderV1,
};
use crate::custody_git::CustodyGitError;
use crate::custody_inventory::CustodyStateClassV1;
use crate::custody_seal::CustodyCoverageClassV1;
use crate::fs_custody::{pinned_root_unchanged, ChildKindV1, PinnedDirectoryV1};
use ring::digest;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::{self, BufReader, Read as _, Seek as _, SeekFrom};
use std::os::unix::ffi::OsStrExt as _;
use std::time::Instant;
#[cfg(test)]
use std::{cell::RefCell, path::Path};

// ---------------------------------------------------------------------------------------------
// Names and bounds
// ---------------------------------------------------------------------------------------------

/// The most bytes the plan table may retain (D6). Each planned entry is charged two copies of its path
/// (its lookup key and its stored entry), its symlink target (none for another kind), and a fixed
/// allowance for the table's own bookkeeping, summed with checked arithmetic.
const PAYLOAD_PLAN_TABLE_BYTES_V1: u64 = 64 * 1024 * 1024;
const PAYLOAD_ENTRY_OVERHEAD_BYTES_V1: u64 = 64;

/// The five classes phase P restores, in the order their steps write them.
const PAYLOAD_CLASSES_V1: [CustodyCoverageClassV1; 5] = [
    CustodyCoverageClassV1::Worktree,
    CustodyCoverageClassV1::Index,
    CustodyCoverageClassV1::InProgressGitOperations,
    CustodyCoverageClassV1::GitConfigurationAndHooks,
    CustodyCoverageClassV1::BridgeEvidence,
];

/// The three classes phase G consumed. With the five above, the only classes that may be Captured (D5).
const GIT_PLANE_CLASSES_V1: [CustodyCoverageClassV1; 3] = [
    CustodyCoverageClassV1::RefsAndHead,
    CustodyCoverageClassV1::ObjectDatabase,
    CustodyCoverageClassV1::StashAndReflogs,
];

/// The worktree's connector name (D3).
const GIT_CONNECTOR_V1: &[u8] = b".git";
/// The only names the index frame may hold.
const INDEX_NAME_V1: &[u8] = b"index";
const SHARED_INDEX_PREFIX_V1: &[u8] = b"sharedindex.";

// ---------------------------------------------------------------------------------------------
// Typed progress and failure
// ---------------------------------------------------------------------------------------------

/// The steps of phase P, in order. Progress advances only when a whole step succeeds. Planning (C2a)
/// reaches [`Self::Prevalidation`] and materialization (C2b) [`Self::Evidence`]; the terminal gate is
/// increment C3's and is enumerated here so one progress type spans the slice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum PayloadPlaneStepV1 {
    Admission,
    Prevalidation,
    Worktree,
    Index,
    Evidence,
    TerminalGate,
}

/// Why one phase-P frame entry is refused. Paths are rendered lossily, for the message only.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum PayloadFrameFaultV1 {
    #[error("{path}: a component is {kind}")]
    InvalidComponent {
        path: String,
        kind: CustodyFrameComponentFaultV1,
    },
    #[error("{path}: its parent is not a directory entry of the same frame")]
    ParentNotDirectory { path: String },
    #[error("{path}: listed twice")]
    Duplicate { path: String },
    #[error("{path}: an entry beneath the worktree's `.git`")]
    BeneathGitConnector { path: String },
    #[error("the worktree's `.git` is a symlink")]
    GitConnectorSymlink,
    #[error("{path}: not `index` or a `sharedindex.*` name")]
    IndexName { path: String },
    #[error("{path}: an index file that is not a regular file")]
    IndexKind { path: String },
    #[error("the payload plan table exceeds its {bound}-byte bound")]
    PlanTableBound { bound: u64 },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PayloadPlaneErrorV1 {
    // Admission.
    #[error("phase P refuses a Captured {class:?} class: it is outside the eight-class union")]
    UnsupportedCapturedClass { class: CustodyCoverageClassV1 },
    #[error("a coverage state phase P cannot restore: {class:?}: {detail}")]
    UnsupportedCoverage {
        class: CustodyCoverageClassV1,
        detail: &'static str,
    },
    #[error("a staged input is not the verified plaintext: {name}")]
    StagedInputChanged { name: String },
    // Prevalidation.
    #[error("the {class:?} frame is refused: {fault}")]
    PayloadFrameInvalid {
        class: CustodyCoverageClassV1,
        fault: PayloadFrameFaultV1,
    },
    // Materialization.
    /// A create-new met an existing entry: something already held the slot, or the destination file
    /// system folds two distinct frame names (case, Unicode normalization) into one. The existing entry is
    /// left exactly as it was.
    #[error("a payload entry's slot is already taken: {name}")]
    DestinationCollision { name: String },
    /// The destination file system cannot hold this symlink's target; nothing was created for it, and the
    /// target is never truncated or skipped.
    #[error("the destination cannot hold the symlink {name} ({target_len}-byte target)")]
    DestinationUnsupported { name: String, target_len: usize },
    /// The third decode of a frame is not what prevalidation admitted, or the plan does not bind the
    /// staged payload it names.
    #[error("the {class:?} frame is not the one the plan admitted: {detail}")]
    PlanMismatch {
        class: CustodyCoverageClassV1,
        detail: String,
    },
    // The one deadline.
    #[error("the phase-P deadline passed")]
    Timeout,
    // The destination, its pins, and the frame codec.
    #[error(transparent)]
    Restore(#[from] CustodyRestoreErrorV1),
    /// A shared phase-G check refused in a way no variant above names: the retained value is not one
    /// restore's.
    #[error("a retained phase-G check refused: {0}")]
    GitPlane(Box<GitBootstrapErrorV1>),
}

impl From<GitBootstrapErrorV1> for PayloadPlaneErrorV1 {
    /// The shared phase-G helpers (the rehash, the deadline, and the retained-chain checks) refuse in
    /// phase G's vocabulary; each refusal keeps its meaning here.
    fn from(error: GitBootstrapErrorV1) -> Self {
        match error {
            GitBootstrapErrorV1::StagedInputChanged { name } => Self::StagedInputChanged { name },
            GitBootstrapErrorV1::Git(CustodyGitError::Timeout) => Self::Timeout,
            GitBootstrapErrorV1::Restore(error) => Self::Restore(error),
            other => Self::GitPlane(Box::new(other)),
        }
    }
}

/// Every value a refused phase P still holds. Nothing is ever removed: the entries it describes stay on
/// disk, and dropping these closes only descriptors.
#[derive(Debug)]
pub(crate) struct PayloadPlanePartialV1 {
    /// The phase-G value, with its capsule, its pins, its one ledger, and every staged descriptor.
    pub(crate) plane: RestoredGitPlaneV1,
    /// The plan, once prevalidation completed.
    pub(crate) plan: Option<PayloadPlanV1>,
    /// Every entry and pin materialization created so far, the file in flight, and the charge.
    pub(crate) created: PayloadCreatedV1,
}

/// A refused or failed phase P, with how far it got. `last_completed` is the last step that completed
/// whole, and `payload_materialization_began` is whether any payload entry may have been created (never,
/// before the materialization steps, and not for a collision at the very first create).
#[derive(Debug)]
pub(crate) struct PayloadPlaneRestoreFailureV1 {
    pub(crate) cause: PayloadPlaneErrorV1,
    pub(crate) last_completed: Option<PayloadPlaneStepV1>,
    pub(crate) payload_materialization_began: bool,
    /// The one ledger's charge when the failure was returned.
    pub(crate) ledger_used: u64,
    pub(crate) partial: PayloadPlanePartialV1,
}

// ---------------------------------------------------------------------------------------------
// The plan
// ---------------------------------------------------------------------------------------------

/// Where a planned entry is restored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum PayloadPlaneV1 {
    /// `repository/`: the worktree frame.
    Worktree,
    /// `repository/.git/`: the index frame.
    Index,
    /// `evidence/<class wire name>/`: one evidence-only class, inert.
    Evidence(CustodyCoverageClassV1),
}

impl PayloadPlaneV1 {
    /// The plane a phase-P class restores into; `None` for every other class.
    pub(crate) fn of(class: CustodyCoverageClassV1) -> Option<Self> {
        match class {
            CustodyCoverageClassV1::Worktree => Some(Self::Worktree),
            CustodyCoverageClassV1::Index => Some(Self::Index),
            CustodyCoverageClassV1::InProgressGitOperations
            | CustodyCoverageClassV1::GitConfigurationAndHooks
            | CustodyCoverageClassV1::BridgeEvidence => Some(Self::Evidence(class)),
            _ => None,
        }
    }
}

/// One phase-P class as admission found it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PayloadClassStateV1 {
    /// Exactly one staged payload, by index into the capsule's staged plaintexts.
    Captured { staged: usize },
    /// No payload: nothing is created for the class, not even its directory.
    Empty,
}

/// What the worktree's root `.git` entry is (D3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RootGitDispositionV1 {
    /// The childless `.git` directory: phase G already made `repository/.git`, so it is skipped.
    ConnectorDirectorySkipped,
    /// The root gitfile: its bytes go to `evidence/worktree_root_gitfile/gitfile`, and nothing is
    /// created at `repository/.git` for it.
    RootGitfileToEvidence,
    /// The worktree frame has no root `.git` entry, or the worktree class is Empty.
    Absent,
}

/// A planned entry's kind, with the frame's own facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PlannedKindV1 {
    /// `childless` is whether no later entry of the frame lies beneath it.
    Directory {
        mode: u32,
        childless: bool,
    },
    Regular {
        mode: u32,
        length: u64,
    },
    /// The target bytes verbatim, and whether they are external (D7).
    Symlink {
        target: Vec<u8>,
        external: bool,
    },
}

/// What phase P does with a planned entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlannedActionV1 {
    /// Created at its frame path beneath its plane's root.
    Create,
    /// D3: the worktree's childless `.git` directory is skipped.
    SkipConnector,
    /// D3: the worktree's root gitfile goes to `evidence/worktree_root_gitfile/gitfile`.
    RootGitfileToEvidence,
}

/// One admitted frame entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlannedEntryV1 {
    /// The frame path as its exact bytes: the one identity the plan gives a name, never a rendering.
    pub(crate) path: Vec<u8>,
    pub(crate) kind: PlannedKindV1,
    pub(crate) action: PlannedActionV1,
}

/// One whole, trailer-verified phase-P frame: which staged payload it is, the staged length and SHA-256
/// its decoded bytes were bound to, and every entry in frame order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlannedFrameV1 {
    pub(crate) class: CustodyCoverageClassV1,
    pub(crate) plane: PayloadPlaneV1,
    /// Index into the capsule's staged plaintexts.
    pub(crate) staged: usize,
    pub(crate) length: u64,
    pub(crate) sha256: [u8; 32],
    pub(crate) entries: Vec<PlannedEntryV1>,
    /// The sum of the regular files' lengths.
    pub(crate) content_bytes: u64,
}

/// A symlink whose target is absolute or lexically leaves its plane's root (D7). Its target is still
/// written verbatim; it is declared, never followed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ExternalSymlinkV1 {
    pub(crate) plane: PayloadPlaneV1,
    pub(crate) path: Vec<u8>,
    pub(crate) target: Vec<u8>,
}

/// The pure, bounded plan the materialization consumes: no descriptor, no pin, and no rendered name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PayloadPlanV1 {
    /// Each phase-P class, in step order, with its admitted state.
    pub(crate) classes: [(CustodyCoverageClassV1, PayloadClassStateV1); 5],
    /// One per Captured class, in step order.
    pub(crate) frames: Vec<PlannedFrameV1>,
    pub(crate) root_git: RootGitDispositionV1,
    /// The plan table's checked charge.
    pub(crate) table_bytes: u64,
}

impl PayloadPlanV1 {
    pub(crate) fn frame(&self, class: CustodyCoverageClassV1) -> Option<&PlannedFrameV1> {
        self.frames.iter().find(|frame| frame.class == class)
    }

    /// Every worktree directory with no entry beneath it that will be restored as an empty directory,
    /// in frame order: the skipped connector is not one. The frame carries no marker of a separate Git
    /// directory, so the plan makes no claim about which of these was one.
    pub(crate) fn childless_directories(&self) -> Vec<&[u8]> {
        self.frame(CustodyCoverageClassV1::Worktree)
            .into_iter()
            .flat_map(|frame| &frame.entries)
            .filter(|entry| {
                entry.action == PlannedActionV1::Create
                    && matches!(
                        entry.kind,
                        PlannedKindV1::Directory {
                            childless: true,
                            ..
                        }
                    )
            })
            .map(|entry| entry.path.as_slice())
            .collect()
    }

    /// Every external symlink of every plane, in plan order (D7).
    pub(crate) fn declared_external_references(&self) -> Vec<ExternalSymlinkV1> {
        self.frames
            .iter()
            .flat_map(|frame| {
                frame.entries.iter().filter_map(|entry| match &entry.kind {
                    PlannedKindV1::Symlink {
                        target,
                        external: true,
                    } => Some(ExternalSymlinkV1 {
                        plane: frame.plane,
                        path: entry.path.clone(),
                        target: target.clone(),
                    }),
                    _ => None,
                })
            })
            .collect()
    }
}

/// Phase P's C2a success value: the phase-G value, unchanged (every staged descriptor back at offset
/// 0), and the plan of everything the later steps restore. Nothing has been created.
#[derive(Debug)]
pub(crate) struct PlannedPayloadPlaneV1 {
    /// The phase-P step this completes: [`PayloadPlaneStepV1::Prevalidation`].
    pub(crate) step: PayloadPlaneStepV1,
    pub(crate) plane: RestoredGitPlaneV1,
    pub(crate) plan: PayloadPlanV1,
}

// ---------------------------------------------------------------------------------------------
// The plan table
// ---------------------------------------------------------------------------------------------

/// A decoded frame entry, as the plan table judges it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FrameEntryKindV1 {
    Directory { mode: u32 },
    Regular { mode: u32, length: u64 },
    Symlink { target: Vec<u8> },
}

/// D7: a symlink at a path whose parent lies `depth` directories beneath its plane's root is external
/// when its target is absolute, or when resolving the target lexically, component by component, ever
/// leaves that root. Nothing is looked up: `.` and empty components stay, and `..` goes up one.
fn is_external(depth: usize, target: &[u8]) -> bool {
    if target.first() == Some(&b'/') {
        return true;
    }
    let mut depth = depth;
    for component in target.split(|byte| *byte == b'/') {
        match component {
            b"" | b"." => {}
            b".." => match depth.checked_sub(1) {
                Some(up) => depth = up,
                None => return true,
            },
            _ => depth += 1,
        }
    }
    false
}

/// The one table every phase-P frame of a restore feeds, in step order. Each entry is judged against its
/// plane's rules (the worktree connector, D3; the index names), the path rules (D6: no empty, `.`, or
/// `..` component, a parent that is a directory entry of the same frame, and no path twice), and the
/// table's checked bound; a symlink's target is classified (D7). Keys are the exact path bytes, so two
/// names that render alike stay two names. An entry that is refused is not admitted.
#[derive(Debug)]
pub(crate) struct PayloadPlanTableV1 {
    /// Each admitted entry by its plane and exact path (the first copy of the path), at its position in
    /// its plane's entries.
    keys: BTreeMap<(PayloadPlaneV1, Vec<u8>), usize>,
    /// Each plane's admitted entries in frame order (the second copy of each path).
    entries: BTreeMap<PayloadPlaneV1, Vec<PlannedEntryV1>>,
    retained: u64,
    bound: u64,
}

impl PayloadPlanTableV1 {
    pub(crate) fn new(bound: u64) -> Self {
        Self {
            keys: BTreeMap::new(),
            entries: BTreeMap::new(),
            retained: 0,
            bound,
        }
    }

    /// A table that already retains `retained` bytes, so its checked arithmetic is exercised.
    #[cfg(test)]
    pub(crate) fn retaining_for_test(bound: u64, retained: u64) -> Self {
        Self {
            retained,
            ..Self::new(bound)
        }
    }

    /// The table's checked charge so far.
    pub(crate) fn retained(&self) -> u64 {
        self.retained
    }

    /// `plane`'s admitted entries, in frame order.
    pub(crate) fn entries(&self, plane: PayloadPlaneV1) -> &[PlannedEntryV1] {
        self.entries.get(&plane).map_or(&[], Vec::as_slice)
    }

    /// Moves `plane`'s admitted entries out once its frame is whole, so they are never held twice; their
    /// keys and charge stay.
    pub(crate) fn take_entries(&mut self, plane: PayloadPlaneV1) -> Vec<PlannedEntryV1> {
        self.entries.remove(&plane).unwrap_or_default()
    }

    /// Judge one entry of `plane`'s frame, and admit it if it passes every rule.
    pub(crate) fn admit(
        &mut self,
        plane: PayloadPlaneV1,
        path: &[u8],
        kind: FrameEntryKindV1,
    ) -> Result<(), PayloadFrameFaultV1> {
        let shown = || lossy(path);
        for component in path.split(|byte| *byte == b'/') {
            let kind = match component {
                b"" => CustodyFrameComponentFaultV1::Empty,
                b"." => CustodyFrameComponentFaultV1::Dot,
                b".." => CustodyFrameComponentFaultV1::DotDot,
                _ => continue,
            };
            return Err(PayloadFrameFaultV1::InvalidComponent {
                path: shown(),
                kind,
            });
        }
        let parent = path
            .iter()
            .rposition(|byte| *byte == b'/')
            .map(|at| &path[..at]);
        let action = match plane {
            PayloadPlaneV1::Worktree => {
                let top = path.split(|byte| *byte == b'/').next().unwrap_or(path);
                if top != GIT_CONNECTOR_V1 {
                    PlannedActionV1::Create
                } else if parent.is_some() {
                    return Err(PayloadFrameFaultV1::BeneathGitConnector { path: shown() });
                } else {
                    match kind {
                        FrameEntryKindV1::Directory { .. } => PlannedActionV1::SkipConnector,
                        FrameEntryKindV1::Regular { .. } => PlannedActionV1::RootGitfileToEvidence,
                        FrameEntryKindV1::Symlink { .. } => {
                            return Err(PayloadFrameFaultV1::GitConnectorSymlink)
                        }
                    }
                }
            }
            PayloadPlaneV1::Index => {
                if parent.is_some()
                    || !(path == INDEX_NAME_V1 || path.starts_with(SHARED_INDEX_PREFIX_V1))
                {
                    return Err(PayloadFrameFaultV1::IndexName { path: shown() });
                }
                if !matches!(kind, FrameEntryKindV1::Regular { .. }) {
                    return Err(PayloadFrameFaultV1::IndexKind { path: shown() });
                }
                PlannedActionV1::Create
            }
            PayloadPlaneV1::Evidence(_) => PlannedActionV1::Create,
        };
        let key = (plane, path.to_vec());
        if self.keys.contains_key(&key) {
            return Err(PayloadFrameFaultV1::Duplicate { path: shown() });
        }
        let planned = self.entries.entry(plane).or_default();
        let parent_at = match parent {
            None => None,
            Some(parent) => match self.keys.get(&(plane, parent.to_vec())) {
                Some(&at)
                    if matches!(
                        planned.get(at).map(|entry| &entry.kind),
                        Some(PlannedKindV1::Directory { .. })
                    ) =>
                {
                    Some(at)
                }
                _ => return Err(PayloadFrameFaultV1::ParentNotDirectory { path: shown() }),
            },
        };
        let depth = parent.map_or(0, |parent| parent.split(|byte| *byte == b'/').count());
        let kind = match kind {
            FrameEntryKindV1::Directory { mode } => PlannedKindV1::Directory {
                mode,
                childless: true,
            },
            FrameEntryKindV1::Regular { mode, length } => PlannedKindV1::Regular { mode, length },
            FrameEntryKindV1::Symlink { target } => PlannedKindV1::Symlink {
                external: is_external(depth, &target),
                target,
            },
        };
        let target = match &kind {
            PlannedKindV1::Symlink { target, .. } => target.len() as u64,
            _ => 0,
        };
        let retained = (path.len() as u64)
            .checked_mul(2)
            .and_then(|charge| charge.checked_add(target))
            .and_then(|charge| charge.checked_add(PAYLOAD_ENTRY_OVERHEAD_BYTES_V1))
            .and_then(|charge| self.retained.checked_add(charge))
            .filter(|retained| *retained <= self.bound)
            .ok_or(PayloadFrameFaultV1::PlanTableBound { bound: self.bound })?;
        self.retained = retained;
        if let Some(PlannedEntryV1 {
            kind: PlannedKindV1::Directory { childless, .. },
            ..
        }) = parent_at.and_then(|at| planned.get_mut(at))
        {
            *childless = false;
        }
        self.keys.insert(key, planned.len());
        planned.push(PlannedEntryV1 {
            path: path.to_vec(),
            kind,
            action,
        });
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Test seams
// ---------------------------------------------------------------------------------------------

/// Where a test hook runs.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum PayloadHookPointV1 {
    /// Admission's class rules passed, before its first rehash (the destination root).
    AdmissionEntry,
    /// The prevalidation pass rehashed one frame, before its decode reads a byte (the frame's staged
    /// name; the event names the class).
    PrevalidationRehashed,
    /// A decoded prevalidation entry is about to be judged (the entry's frame path; the event names the
    /// class).
    PrevalidationEntry,
    /// Every frame is planned, before prevalidation's gate (the destination root).
    BeforePrevalidationGate,
    /// The materialization pass rehashed one frame, before its third decode reads a byte (the frame's
    /// staged name; the event names the class). The materialization pass's own hook, separate from
    /// [`Self::PrevalidationRehashed`].
    MaterializationRehashed,
    /// A decoded materialization entry is about to be compared with its plan and created (the entry's
    /// frame path; the event names the class).
    MaterializationEntry,
    /// An entry is about to be created, create-new (the path it will have: its retained parent's path and
    /// its leaf; the event names the class).
    BeforeCreate,
    /// An entry was created and verified, before anything else is done (its path; the event names the
    /// class).
    EntryCreated,
    /// Every frame of a materialization step is written, before the step's gate (the destination root; the
    /// event names the step).
    BeforeStepGate,
}

#[cfg(test)]
pub(crate) struct PayloadHookEventV1<'a> {
    /// The frame's class, where the point concerns one frame.
    pub(crate) class: Option<CustodyCoverageClassV1>,
    /// The step, where the point concerns one step.
    pub(crate) step: Option<PayloadPlaneStepV1>,
    pub(crate) path: &'a Path,
}

#[cfg(test)]
type PayloadHookV1 = Box<dyn Fn(&PayloadHookEventV1<'_>)>;

#[cfg(test)]
type PlanEditV1 = Box<dyn Fn(&mut PayloadPlanV1)>;

#[cfg(test)]
thread_local! {
    static PAYLOAD_HOOKS: RefCell<BTreeMap<PayloadHookPointV1, PayloadHookV1>> =
        RefCell::new(BTreeMap::new());
    /// An edit materialization applies to the plan it was handed, so a plan that does not describe its
    /// frames is exercised.
    static PLAN_EDIT: RefCell<Option<PlanEditV1>> = const { RefCell::new(None) };
}

#[cfg(test)]
enum PayloadSeamV1 {
    Hook(PayloadHookPointV1),
    PlanEdit,
}

/// Uninstalls its one seam when dropped.
#[cfg(test)]
pub(crate) struct PayloadSeamGuardV1(PayloadSeamV1);

#[cfg(test)]
impl Drop for PayloadSeamGuardV1 {
    fn drop(&mut self) {
        match &self.0 {
            PayloadSeamV1::Hook(point) => PAYLOAD_HOOKS.with(|hooks| {
                hooks.borrow_mut().remove(point);
            }),
            PayloadSeamV1::PlanEdit => PLAN_EDIT.with(|edit| *edit.borrow_mut() = None),
        }
    }
}

#[cfg(test)]
pub(crate) fn install_payload_hook_for_test(
    point: PayloadHookPointV1,
    hook: impl Fn(&PayloadHookEventV1<'_>) + 'static,
) -> PayloadSeamGuardV1 {
    PAYLOAD_HOOKS.with(|hooks| hooks.borrow_mut().insert(point, Box::new(hook)));
    PayloadSeamGuardV1(PayloadSeamV1::Hook(point))
}

/// Makes materialization apply `edit` to the plan it was handed before it writes anything.
#[cfg(test)]
pub(crate) fn edit_plan_for_test(
    edit: impl Fn(&mut PayloadPlanV1) + 'static,
) -> PayloadSeamGuardV1 {
    PLAN_EDIT.with(|slot| *slot.borrow_mut() = Some(Box::new(edit)));
    PayloadSeamGuardV1(PayloadSeamV1::PlanEdit)
}

#[cfg(test)]
fn run_payload_hook(point: PayloadHookPointV1, class: Option<CustodyCoverageClassV1>, path: &Path) {
    run_payload_step_hook(point, class, None, path);
}

#[cfg(test)]
fn run_payload_step_hook(
    point: PayloadHookPointV1,
    class: Option<CustodyCoverageClassV1>,
    step: Option<PayloadPlaneStepV1>,
    path: &Path,
) {
    PAYLOAD_HOOKS.with(|hooks| {
        if let Some(hook) = hooks.borrow().get(&point) {
            hook(&PayloadHookEventV1 { class, step, path });
        }
    });
}

#[cfg(test)]
fn edited_plan(mut plan: PayloadPlanV1) -> PayloadPlanV1 {
    PLAN_EDIT.with(|edit| {
        if let Some(edit) = edit.borrow().as_ref() {
            edit(&mut plan);
        }
    });
    plan
}

// ---------------------------------------------------------------------------------------------
// The gate
// ---------------------------------------------------------------------------------------------

/// Phase G's metadata beneath `repository/.git`: every directory is still the entry its retained parent
/// holds, and every file the regular file its parent lists, of the identity and length it was made with.
fn metadata_chains(
    plane: &RestoredGitPlaneV1,
    pace: PaceV1<'_>,
) -> Result<(), GitBootstrapErrorV1> {
    let git = &plane.objects.pins.git;
    let directories = &plane.metadata.directories;
    for (relative, pin) in directories {
        pace("payload-metadata-chain")?;
        let (parent, leaf) = split_relative(relative);
        let above = pin_beneath(git, directories, parent)?;
        recheck_entry(
            above,
            leaf.as_bytes(),
            pin,
            metadata_name(relative).as_bytes(),
        )?;
    }
    for file in &plane.metadata.files {
        pace("payload-metadata-chain")?;
        let parent = pin_beneath(git, directories, split_relative(&file.relative).0)?;
        recheck_regular_entry(parent, &file.relative, (file.dev, file.ino, file.length))?;
    }
    Ok(())
}

/// The staging, HOME, XDG, and bootstrap chains: the same edges, in the same order, as phase G's retained
/// sweep, whose own line a 2B3b structural control pins in place. Every retained staging directory is
/// rechecked with its whole chain; HOME, XDG, and each bootstrap directory beneath its retained parent
/// (parents sort first). `pace` runs before every step. The destination root's identity is the gate's,
/// and comes after every created chain.
fn retained_chains(
    dest: &RestoreDestinationV1,
    boot: &VerifiedGitBootstrapV1,
    pace: PaceV1<'_>,
) -> Result<(), GitBootstrapErrorV1> {
    pace("payload-chain")?;
    dest.recheck_staging_chain(b"")?;
    for key in dest.staging.borrow().keys() {
        pace("payload-chain")?;
        dest.recheck_staging_chain(key)?;
    }
    let work = RESTORE_WORK_DIR_NAME_V1.as_bytes();
    for (name, pin) in [(HOME_NAME_V1, &boot.home), (XDG_NAME_V1, &boot.xdg)] {
        pace("payload-chain")?;
        recheck_entry(
            &dest.work,
            name.as_bytes(),
            pin,
            &joined(work, name.as_bytes()),
        )?;
    }
    let bootstrap = joined(work, BOOTSTRAP_NAME_V1.as_bytes());
    for (relative, pin) in &boot.directories {
        pace("payload-chain")?;
        if relative.is_empty() {
            recheck_entry(&dest.work, BOOTSTRAP_NAME_V1.as_bytes(), pin, &bootstrap)?;
            continue;
        }
        let (parent_relative, component) =
            relative.rsplit_once('/').unwrap_or(("", relative.as_str()));
        let parent = boot.directories.get(parent_relative).ok_or_else(|| {
            CustodyRestoreErrorV1::IdentityChanged(format!(
                "{relative}: its bootstrap parent is not retained"
            ))
        })?;
        recheck_entry(
            parent,
            component.as_bytes(),
            pin,
            &joined(&bootstrap, relative.as_bytes()),
        )?;
    }
    Ok(())
}

/// `parent`'s entry `leaf` must still be the file or symlink phase P created, as `(kind, dev, ino, length,
/// mode)`: observed by name without following it (a symlink's length is its target's, and a symlink, whose
/// mode nothing reads, is compared without one). A regular file must still be `0600`. Anything else, a
/// missing entry included, refuses as an identity change.
fn recheck_created(
    parent: &PinnedDirectoryV1,
    leaf: &[u8],
    created: (ChildKindV1, u64, u64, u64, Option<u32>),
    name: &[u8],
) -> Result<(), PayloadPlaneErrorV1> {
    let (kind, dev, ino, size, mode) = created;
    let observed = parent
        .child_metadata_no_follow(OsStr::from_bytes(leaf), "custody restore payload entry")
        .map_err(|error| CustodyRestoreErrorV1::Io(error.to_string()))?;
    match observed {
        Some(stat)
            if (stat.kind, stat.dev, stat.ino, stat.size) == (kind, dev, ino, size)
                && mode.is_none_or(|mode| stat.mode == mode) =>
        {
            Ok(())
        }
        _ => Err(CustodyRestoreErrorV1::IdentityChanged(format!(
            "{}: the entry is no longer the one phase P created, at its created mode",
            lossy(name)
        ))
        .into()),
    }
}

/// `parent`'s entry `leaf` must still be the directory phase P created as `pin`: observed by name without
/// following it, a directory with the pin's device and inode, and still `0700`, so every later write and
/// gate can search it. Anything else, a missing entry included, refuses as an identity change.
fn recheck_directory(
    parent: &PinnedDirectoryV1,
    leaf: &[u8],
    pin: &PinnedDirectoryV1,
    name: &[u8],
) -> Result<(), PayloadPlaneErrorV1> {
    let observed = parent
        .child_metadata_no_follow(OsStr::from_bytes(leaf), "custody restore payload directory")
        .map_err(|error| CustodyRestoreErrorV1::Io(error.to_string()))?;
    match observed {
        Some(stat)
            if stat.kind == ChildKindV1::Directory
                && Some(stat.dev) == pin.identity().dev
                && Some(stat.ino) == pin.identity().ino
                && stat.mode == CREATED_DIRECTORY_MODE_V1 =>
        {
            Ok(())
        }
        _ => Err(CustodyRestoreErrorV1::IdentityChanged(format!(
            "{}: the entry is no longer the directory phase P created, at 0700",
            lossy(name)
        ))
        .into()),
    }
}

/// Every entry of one created tree beneath `root`, in creation order (each parent before its children): a
/// directory must still be the entry its retained parent holds, with its pin's identity, at `0700`; a file
/// or symlink the entry phase P created (a file still `0600`). `pace` runs before every entry.
fn tree_chains(
    root: &PinnedDirectoryV1,
    tree: &CreatedTreeV1,
    at: TreeAtV1,
    pace: PaceV1<'_>,
) -> Result<(), PayloadPlaneErrorV1> {
    for entry in &tree.entries {
        pace("payload-output-chain")?;
        let (parent, leaf) = split_path(&entry.path);
        let name = at.name(&entry.path);
        let parent = tree.parent(root, parent, &name)?;
        match &entry.kind {
            CreatedKindV1::Directory => {
                let pin = tree.directories.get(&entry.path).ok_or_else(|| {
                    CustodyRestoreErrorV1::IdentityChanged(format!(
                        "{}: the created directory is not retained",
                        lossy(&name)
                    ))
                })?;
                recheck_directory(parent, leaf, pin, &name)?;
            }
            CreatedKindV1::Regular { .. } | CreatedKindV1::Symlink { .. } => {
                recheck_created(parent, leaf, entry.observed(), &name)?;
            }
        }
    }
    Ok(())
}

/// Everything materialization created so far, by name, parents first: the worktree beneath `repository/`,
/// the index files beneath `repository/.git`, and `evidence/`, each slot directory, and each slot's tree,
/// every directory still `0700` and every file still `0600`.
fn created_chains(
    plane: &RestoredGitPlaneV1,
    created: &PayloadCreatedV1,
    pace: PaceV1<'_>,
) -> Result<(), PayloadPlaneErrorV1> {
    let pins = &plane.objects.pins;
    tree_chains(
        &pins.repository,
        &created.worktree,
        TreeAtV1::Worktree,
        pace,
    )?;
    tree_chains(&pins.git, &created.index, TreeAtV1::Index, pace)?;
    let Some(evidence) = &created.evidence.root else {
        return Ok(());
    };
    pace("payload-output-chain")?;
    recheck_directory(
        &plane.capsule().destination.root,
        EVIDENCE_NAME_V1,
        evidence,
        EVIDENCE_NAME_V1,
    )?;
    for slot in &created.evidence.slots {
        pace("payload-output-chain")?;
        let at = TreeAtV1::Evidence(slot.slot);
        recheck_directory(
            evidence,
            slot.slot.directory_name(),
            &slot.pin,
            &at.prefix(),
        )?;
        tree_chains(&slot.pin, &slot.tree, at, pace)?;
    }
    Ok(())
}

/// The phase-P gate: the mount census over the destination first, then every retained chain (phase G's
/// object store and metadata, the bootstrap's admitted object files, and the staging, HOME, XDG, and
/// bootstrap directories), then everything materialization created so far (`created`; none while
/// planning), and the destination root's identity LAST. The one deadline is observed before the census,
/// before every chain and created entry, and immediately before the root's identity, which is the gate's
/// last fallible call. A replaced or retyped entry refuses; nothing is followed.
fn gate(
    plane: &RestoredGitPlaneV1,
    created: Option<&PayloadCreatedV1>,
    deadline: Instant,
) -> Result<(), PayloadPlaneErrorV1> {
    let observe = |step: &'static str| observe_deadline(deadline, step);
    let pace: PaceV1<'_> = &observe;
    let dest = &plane.capsule().destination;
    pace("payload-gate")?;
    refuse_mounts_within(&dest.root)?;
    output_chains(
        dest,
        PinViewV1::of_final(&plane.objects.pins),
        &plane.objects.files,
        pace,
    )?;
    metadata_chains(plane, pace)?;
    source_files_unchanged(&plane.bootstrap, pace)?;
    retained_chains(dest, &plane.bootstrap, pace)?;
    if let Some(created) = created {
        created_chains(plane, created, pace)?;
    }
    pace("payload-root")?;
    pinned_root_unchanged(&dest.root)
        .map_err(|detail| CustodyRestoreErrorV1::IdentityChanged(detail).into())
}

// ---------------------------------------------------------------------------------------------
// Admission
// ---------------------------------------------------------------------------------------------

/// D5 and the class-state rule, from the authenticated coverage and the staged roles. Pure: no
/// filesystem effect, and nothing is downgraded to Empty.
fn classify(
    verified: &VerifiedCapsuleV1,
) -> Result<[(CustodyCoverageClassV1, PayloadClassStateV1); 5], PayloadPlaneErrorV1> {
    let manifest = &verified.control.manifest;
    if let Some(row) = manifest.coverage().iter().find(|row| {
        row.state() == CustodyStateClassV1::Captured
            && !PAYLOAD_CLASSES_V1.contains(&row.class())
            && !GIT_PLANE_CLASSES_V1.contains(&row.class())
    }) {
        return Err(PayloadPlaneErrorV1::UnsupportedCapturedClass { class: row.class() });
    }
    let mut classes = PAYLOAD_CLASSES_V1.map(|class| (class, PayloadClassStateV1::Empty));
    for (class, state) in &mut classes {
        let payloads: Vec<usize> = verified
            .staged
            .iter()
            .enumerate()
            .filter(|(_, one)| one.role == CustodyCapsuleArtifactRoleV1::CoveragePayload(*class))
            .map(|(index, _)| index)
            .collect();
        let refuse = |detail: &'static str| PayloadPlaneErrorV1::UnsupportedCoverage {
            class: *class,
            detail,
        };
        *state = match (coverage_state(manifest, *class), payloads.as_slice()) {
            (Some(CustodyStateClassV1::Captured), [staged]) => {
                PayloadClassStateV1::Captured { staged: *staged }
            }
            (Some(CustodyStateClassV1::Captured), _) => {
                return Err(refuse(
                    "a Captured phase-P class needs exactly one staged payload",
                ))
            }
            (Some(CustodyStateClassV1::Empty), []) => PayloadClassStateV1::Empty,
            (Some(CustodyStateClassV1::Empty), _) => {
                return Err(refuse("an Empty phase-P class has a staged payload"))
            }
            _ => return Err(refuse("a phase-P class is neither Captured nor Empty")),
        };
    }
    Ok(classes)
}

/// The staged payload at `staged`, which admission identified.
fn staged_payload(
    plane: &mut RestoredGitPlaneV1,
    staged: usize,
) -> Result<&mut StagedPlaintextV1, PayloadPlaneErrorV1> {
    plane
        .bootstrap
        .verified
        .staged
        .get_mut(staged)
        .ok_or_else(|| {
            PayloadPlaneErrorV1::Restore(CustodyRestoreErrorV1::Io(
                "a phase-P class names no staged payload".into(),
            ))
        })
}

/// Admit phase P: the class rules, then a rehash of every phase-P payload from its retained descriptor,
/// then the first gate. Nothing is written.
fn admit(
    plane: &mut RestoredGitPlaneV1,
    deadline: Instant,
) -> Result<[(CustodyCoverageClassV1, PayloadClassStateV1); 5], PayloadPlaneErrorV1> {
    observe_deadline(deadline, "payload-entry")?;
    let classes = classify(plane.capsule())?;
    #[cfg(test)]
    run_payload_hook(
        PayloadHookPointV1::AdmissionEntry,
        None,
        plane.capsule().destination.root.canonical_path(),
    );
    for (_, state) in &classes {
        if let PayloadClassStateV1::Captured { staged } = state {
            reverify_staged_v1(staged_payload(plane, *staged)?, deadline)?;
        }
    }
    // The gate follows the (possibly long) rehash, so a drift during it is seen here.
    gate(plane, None, deadline)?;
    Ok(classes)
}

// ---------------------------------------------------------------------------------------------
// Prevalidation
// ---------------------------------------------------------------------------------------------

fn frame_invalid(class: CustodyCoverageClassV1) -> PayloadPlaneErrorV1 {
    PayloadPlaneErrorV1::Restore(CustodyRestoreErrorV1::FrameInvalid { class })
}

/// Prevalidate one phase-P frame: rehash it, then decode it completely, through its verified trailer,
/// into a null sink, feeding every entry to `table`. The first entry refusal is held until the whole frame
/// has decoded and the bytes the decoder consumed are bound to the verified plaintext. The descriptor
/// ends at offset 0.
fn prevalidate_frame(
    one: &mut StagedPlaintextV1,
    class: CustodyCoverageClassV1,
    staged: usize,
    generation: &str,
    budget: CustodyFrameBudgetV1,
    deadline: Instant,
    table: &mut PayloadPlanTableV1,
) -> Result<PlannedFrameV1, PayloadPlaneErrorV1> {
    let plane = PayloadPlaneV1::of(class).ok_or_else(|| frame_invalid(class))?;
    observe_deadline(deadline, "payload-frame")?;
    reverify_staged_v1(one, deadline)?;
    #[cfg(test)]
    run_payload_hook(
        PayloadHookPointV1::PrevalidationRehashed,
        Some(class),
        Path::new(OsStr::from_bytes(one.name.as_bytes())),
    );
    let header = CustodyFrameHeaderV1::new(class, generation).map_err(|_| frame_invalid(class))?;
    let expired = Cell::new(false);
    let refuse = || {
        if expired.get() {
            PayloadPlaneErrorV1::Timeout
        } else {
            frame_invalid(class)
        }
    };
    let mut fault: Option<PayloadFrameFaultV1> = None;
    let mut content_bytes = 0_u64;
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
            let (path, kind, content) = match entry {
                CustodyFrameEntryV1::Directory { path, mode } => {
                    (path, FrameEntryKindV1::Directory { mode }, None)
                }
                CustodyFrameEntryV1::Regular {
                    path,
                    mode,
                    length,
                    content,
                } => (
                    path,
                    FrameEntryKindV1::Regular { mode, length },
                    Some(content),
                ),
                CustodyFrameEntryV1::Symlink { path, target } => {
                    (path, FrameEntryKindV1::Symlink { target }, None)
                }
            };
            #[cfg(test)]
            run_payload_hook(
                PayloadHookPointV1::PrevalidationEntry,
                Some(class),
                Path::new(OsStr::from_bytes(path.as_bytes())),
            );
            if fault.is_none() {
                let length = match kind {
                    FrameEntryKindV1::Regular { length, .. } => length,
                    _ => 0,
                };
                match table.admit(plane, path.as_bytes(), kind) {
                    Ok(()) => content_bytes = content_bytes.saturating_add(length),
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
        return Err(PayloadPlaneErrorV1::StagedInputChanged {
            name: lossy(one.name.as_bytes()),
        });
    }
    if let Some(fault) = fault {
        return Err(PayloadPlaneErrorV1::PayloadFrameInvalid { class, fault });
    }
    observe_deadline(deadline, "payload-frame-decoded")?;
    one.file.seek(SeekFrom::Start(0)).map_err(|error| {
        CustodyRestoreErrorV1::Io(format!("{}: {error}", lossy(one.name.as_bytes())))
    })?;
    Ok(PlannedFrameV1 {
        class,
        plane,
        staged,
        length: one.length,
        sha256: one.sha256,
        entries: table.take_entries(plane),
        content_bytes,
    })
}

/// Prevalidate every Captured phase-P frame, in step order, into one table bounded by `table_bound`,
/// then run the gate. Nothing is written.
fn prevalidate(
    plane: &mut RestoredGitPlaneV1,
    classes: [(CustodyCoverageClassV1, PayloadClassStateV1); 5],
    frame_budget: CustodyFrameBudgetV1,
    deadline: Instant,
    table_bound: u64,
) -> Result<PayloadPlanV1, PayloadPlaneErrorV1> {
    let generation = plane.capsule().control.manifest.generation_id().to_owned();
    let mut table = PayloadPlanTableV1::new(table_bound);
    let mut frames = Vec::new();
    for (class, state) in classes {
        if let PayloadClassStateV1::Captured { staged } = state {
            frames.push(prevalidate_frame(
                staged_payload(plane, staged)?,
                class,
                staged,
                &generation,
                frame_budget,
                deadline,
                &mut table,
            )?);
        }
    }
    let root_git = frames
        .iter()
        .filter(|frame| frame.plane == PayloadPlaneV1::Worktree)
        .flat_map(|frame| &frame.entries)
        .find_map(|entry| match entry.action {
            PlannedActionV1::SkipConnector => Some(RootGitDispositionV1::ConnectorDirectorySkipped),
            PlannedActionV1::RootGitfileToEvidence => {
                Some(RootGitDispositionV1::RootGitfileToEvidence)
            }
            PlannedActionV1::Create => None,
        })
        .unwrap_or(RootGitDispositionV1::Absent);
    #[cfg(test)]
    run_payload_hook(
        PayloadHookPointV1::BeforePrevalidationGate,
        None,
        plane.capsule().destination.root.canonical_path(),
    );
    gate(plane, None, deadline)?;
    Ok(PayloadPlanV1 {
        classes,
        frames,
        root_git,
        table_bytes: table.retained(),
    })
}

// ---------------------------------------------------------------------------------------------
// The entry point
// ---------------------------------------------------------------------------------------------

/// Plan phase P over `plane`, the phase-G success value: admission (the eight-class union, the five
/// classes' states, a rehash of every phase-P payload, and the first gate), then prevalidation (every
/// Captured phase-P frame decoded whole into the bounded plan table, its consumed bytes bound to the
/// staged digest, and the gate again), all on the ONE caller-supplied absolute `deadline`.
///
/// Nothing is written and nothing is charged: the one ledger, every pin, and the destination are exactly
/// as phase G left them, whatever the outcome. On success every staged descriptor is at offset 0 and the
/// phase-G value is returned beside the plan; a refusal keeps it in its partial value.
pub(crate) fn plan_payload_plane_v1(
    plane: RestoredGitPlaneV1,
    frame_budget: CustodyFrameBudgetV1,
    deadline: Instant,
) -> Result<PlannedPayloadPlaneV1, Box<PayloadPlaneRestoreFailureV1>> {
    plan_payload_plane_bounded_v1(plane, frame_budget, deadline, PAYLOAD_PLAN_TABLE_BYTES_V1)
}

/// [`plan_payload_plane_v1`] with the plan table's byte bound as a parameter, so its refusal is
/// exercised at a small size.
pub(crate) fn plan_payload_plane_bounded_v1(
    mut plane: RestoredGitPlaneV1,
    frame_budget: CustodyFrameBudgetV1,
    deadline: Instant,
    table_bound: u64,
) -> Result<PlannedPayloadPlaneV1, Box<PayloadPlaneRestoreFailureV1>> {
    let refused = |cause, last_completed, plane: RestoredGitPlaneV1| {
        Box::new(PayloadPlaneRestoreFailureV1 {
            cause,
            last_completed,
            payload_materialization_began: false,
            ledger_used: plane.ledger_used(),
            partial: PayloadPlanePartialV1 {
                plane,
                plan: None,
                created: PayloadCreatedV1::default(),
            },
        })
    };
    let classes = match admit(&mut plane, deadline) {
        Ok(classes) => classes,
        Err(cause) => return Err(refused(cause, None, plane)),
    };
    match prevalidate(&mut plane, classes, frame_budget, deadline, table_bound) {
        Ok(plan) => Ok(PlannedPayloadPlaneV1 {
            step: PayloadPlaneStepV1::Prevalidation,
            plane,
            plan,
        }),
        Err(cause) => Err(refused(cause, Some(PayloadPlaneStepV1::Admission), plane)),
    }
}

// ---------------------------------------------------------------------------------------------
// Materialization (increment C2b): the worktree, the index, and the evidence
// ---------------------------------------------------------------------------------------------

use super::git::{stream_into, InFlightObjectFileV1, ObjectStoreChargeV1, SinkV1};
use super::{create_staging_directory, staging_refusal};
use crate::custody_export::{CustodyExportErrorV1, ENTRY_ALLOWANCE_BYTES_V1};
use crate::custody_frame::CustodyFrameContentReaderV1;
use crate::fs_custody::{ChildStatV1, FsCustodyError, SymlinkChildRefusalV1};
use std::path::PathBuf;

/// `repository/` and its Git directory, as phase G named them.
const REPOSITORY_NAME_V1: &[u8] = b"repository";
const GIT_DIRECTORY_NAME_V1: &[u8] = b"repository/.git";
/// The inert evidence plane beneath the destination root (D1), and the root gitfile's leaf in its slot
/// (D3).
const EVIDENCE_NAME_V1: &[u8] = b"evidence";
const ROOT_GITFILE_LEAF_V1: &[u8] = b"gitfile";
/// Every regular file is created `0600` and every directory `0700`, and each keeps that mode until the
/// terminal gate: a different mode at creation or at any gate refuses.
const CREATED_FILE_MODE_V1: u32 = 0o600;
const CREATED_DIRECTORY_MODE_V1: u32 = 0o700;
/// D2: evidence ends inert, whatever its frame says.
const EVIDENCE_FILE_MODE_V1: u32 = 0o400;
const EVIDENCE_DIRECTORY_MODE_V1: u32 = 0o500;
/// D4: the index files keep the mode phase G gives `.git` metadata.
const INDEX_FILE_MODE_V1: u32 = 0o600;

/// One directory beneath `evidence/` (D1): an evidence-only class, named by its snake-case wire name, or
/// the worktree's root gitfile (D3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EvidenceSlotV1 {
    InProgressGitOperations,
    GitConfigurationAndHooks,
    BridgeEvidence,
    WorktreeRootGitfile,
}

impl EvidenceSlotV1 {
    /// The slot an evidence-only class restores into; `None` for every other class.
    pub(crate) fn of_class(class: CustodyCoverageClassV1) -> Option<Self> {
        match class {
            CustodyCoverageClassV1::InProgressGitOperations => Some(Self::InProgressGitOperations),
            CustodyCoverageClassV1::GitConfigurationAndHooks => {
                Some(Self::GitConfigurationAndHooks)
            }
            CustodyCoverageClassV1::BridgeEvidence => Some(Self::BridgeEvidence),
            _ => None,
        }
    }

    /// The slot's directory name beneath `evidence/`.
    pub(crate) const fn directory_name(self) -> &'static [u8] {
        match self {
            Self::InProgressGitOperations => b"in_progress_git_operations",
            Self::GitConfigurationAndHooks => b"git_configuration_and_hooks",
            Self::BridgeEvidence => b"bridge_evidence",
            Self::WorktreeRootGitfile => b"worktree_root_gitfile",
        }
    }
}

/// Where a tree of created entries is rooted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TreeAtV1 {
    /// `repository/`, phase G's pin.
    Worktree,
    /// `repository/.git`, phase G's pin.
    Index,
    /// `evidence/<slot>/`, a pin this invocation created.
    Evidence(EvidenceSlotV1),
}

impl TreeAtV1 {
    /// The destination-relative name of the tree's root.
    pub(crate) fn prefix(self) -> Vec<u8> {
        match self {
            Self::Worktree => REPOSITORY_NAME_V1.to_vec(),
            Self::Index => GIT_DIRECTORY_NAME_V1.to_vec(),
            Self::Evidence(slot) => joined(EVIDENCE_NAME_V1, slot.directory_name()),
        }
    }

    /// The destination-relative name of the entry at `path` beneath the tree's root.
    pub(crate) fn name(self, path: &[u8]) -> Vec<u8> {
        joined(&self.prefix(), path)
    }
}

/// `path` split into its parent beneath its tree's root (`None` is the root itself) and its leaf.
fn split_path(path: &[u8]) -> (Option<&[u8]>, &[u8]) {
    match path.iter().rposition(|byte| *byte == b'/') {
        Some(at) => (Some(&path[..at]), &path[at + 1..]),
        None => (None, path),
    }
}

/// What one created entry is, as it was observed when it was made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CreatedKindV1 {
    Directory,
    /// The bytes written: their count, which the descriptor's `fstat` confirmed, and their SHA-256 as they
    /// were written.
    Regular {
        length: u64,
        sha256: [u8; 32],
    },
    /// The target, verbatim, as `readlinkat` read it back from the new link.
    Symlink {
        target: Vec<u8>,
    },
}

/// One entry this invocation created and verified: its identity as created, which the terminal gate
/// requires again, and its modes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CreatedEntryV1 {
    /// Its exact path beneath its tree's root: the frame path (`gitfile` for the root gitfile).
    pub(crate) path: Vec<u8>,
    pub(crate) kind: CreatedKindV1,
    /// The frame's mode; none for a symlink, whose frame record carries none.
    pub(crate) frame_mode: Option<u32>,
    /// The mode it was created with, and must keep until the terminal gate: `0600` for a file (its own
    /// `fstat` observed and required it), `0700` for a directory (the directory primitive requires it); none
    /// for a symlink.
    pub(crate) created_mode: Option<u32>,
    /// The mode the terminal gate is to apply: the frame's for the worktree (D4), `0600` for the index
    /// (D4), `0400`/`0500` for evidence (D2); none for a symlink.
    pub(crate) final_mode: Option<u32>,
    pub(crate) dev: u64,
    pub(crate) ino: u64,
}

impl CreatedEntryV1 {
    /// What a no-follow `fstatat` of its name must show: its kind, device, inode, length, and created mode
    /// (a symlink's length is its target's, and it is compared without a mode; a directory is compared by
    /// its pin instead).
    fn observed(&self) -> (ChildKindV1, u64, u64, u64, Option<u32>) {
        let (kind, size) = match &self.kind {
            CreatedKindV1::Directory => (ChildKindV1::Directory, 0),
            CreatedKindV1::Regular { length, .. } => (ChildKindV1::Regular, *length),
            CreatedKindV1::Symlink { target } => (ChildKindV1::Symlink, target.len() as u64),
        };
        (kind, self.dev, self.ino, size, self.created_mode)
    }
}

/// Every entry this invocation created beneath one retained root, in creation order (each parent before
/// its children), and the retained pin of each directory among them, by the same path.
#[derive(Debug, Default)]
pub(crate) struct CreatedTreeV1 {
    pub(crate) entries: Vec<CreatedEntryV1>,
    pub(crate) directories: BTreeMap<Vec<u8>, PinnedDirectoryV1>,
}

impl CreatedTreeV1 {
    /// The retained pin of `parent` (`None` is `root`), which must be a directory this tree created.
    fn parent<'t>(
        &'t self,
        root: &'t PinnedDirectoryV1,
        parent: Option<&[u8]>,
        name: &[u8],
    ) -> Result<&'t PinnedDirectoryV1, PayloadPlaneErrorV1> {
        match parent {
            None => Ok(root),
            Some(parent) => self.directories.get(parent).ok_or_else(|| {
                CustodyRestoreErrorV1::IdentityChanged(format!(
                    "{}: its parent is not a directory this restore created",
                    lossy(name)
                ))
                .into()
            }),
        }
    }
}

/// One slot directory beneath `evidence/`, its retained pin, and the entries restored beneath it.
#[derive(Debug)]
pub(crate) struct EvidenceDirectoryV1 {
    pub(crate) slot: EvidenceSlotV1,
    pub(crate) pin: PinnedDirectoryV1,
    pub(crate) tree: CreatedTreeV1,
}

/// The inert evidence plane as far as it exists: `evidence/` once an entry needed it, and each slot
/// directory in creation order. Both are created `0700`; the terminal gate ends them `0500` (D2).
#[derive(Debug, Default)]
pub(crate) struct CreatedEvidenceV1 {
    pub(crate) root: Option<PinnedDirectoryV1>,
    pub(crate) slots: Vec<EvidenceDirectoryV1>,
}

impl CreatedEvidenceV1 {
    pub(crate) fn slot(&self, slot: EvidenceSlotV1) -> Option<&EvidenceDirectoryV1> {
        self.slots.iter().find(|one| one.slot == slot)
    }
}

/// What materialization charged to the one ledger and has not given back: entry allowances and logical
/// bytes. The ledger moved by exactly `entries * ENTRY_ALLOWANCE_BYTES_V1 + bytes`.
pub(crate) type PayloadChargeV1 = ObjectStoreChargeV1;

/// The one file that was being created or written when a refusal came: it exists, with exactly `written`
/// bytes on disk, and they stay there (and stay charged).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InFlightPayloadFileV1 {
    /// Its destination-relative name, exact bytes.
    pub(crate) name: Vec<u8>,
    pub(crate) expected_length: u64,
    pub(crate) written: u64,
}

/// Everything materialization has created and charged so far. Each tree's entries are inventory rows with
/// the identity the terminal gate requires; each directory's pin is retained.
#[derive(Debug, Default)]
pub(crate) struct PayloadCreatedV1 {
    /// Beneath `repository/`.
    pub(crate) worktree: CreatedTreeV1,
    /// Beneath `repository/.git`: `index` and `sharedindex.*` only.
    pub(crate) index: CreatedTreeV1,
    pub(crate) evidence: CreatedEvidenceV1,
    pub(crate) in_flight: Option<InFlightPayloadFileV1>,
    pub(crate) charged: PayloadChargeV1,
}

impl PayloadCreatedV1 {
    /// The tree rooted `at`, and its root's retained pin (phase G's, or the slot directory's).
    fn tree<'t>(
        &'t self,
        plane: &'t RestoredGitPlaneV1,
        at: TreeAtV1,
    ) -> Result<(&'t CreatedTreeV1, &'t PinnedDirectoryV1), PayloadPlaneErrorV1> {
        match at {
            TreeAtV1::Worktree => Ok((&self.worktree, &plane.objects.pins.repository)),
            TreeAtV1::Index => Ok((&self.index, &plane.objects.pins.git)),
            TreeAtV1::Evidence(slot) => self
                .evidence
                .slot(slot)
                .map(|one| (&one.tree, &one.pin))
                .ok_or_else(|| unretained_slot(slot)),
        }
    }

    fn tree_mut(&mut self, at: TreeAtV1) -> Result<&mut CreatedTreeV1, PayloadPlaneErrorV1> {
        match at {
            TreeAtV1::Worktree => Ok(&mut self.worktree),
            TreeAtV1::Index => Ok(&mut self.index),
            TreeAtV1::Evidence(slot) => self
                .evidence
                .slots
                .iter_mut()
                .find(|one| one.slot == slot)
                .map(|one| &mut one.tree)
                .ok_or_else(|| unretained_slot(slot)),
        }
    }
}

fn unretained_slot(slot: EvidenceSlotV1) -> PayloadPlaneErrorV1 {
    CustodyRestoreErrorV1::IdentityChanged(format!(
        "{}: the slot directory is not retained",
        lossy(&TreeAtV1::Evidence(slot).prefix())
    ))
    .into()
}

/// Phase P's C2b success value: the phase-G value, unchanged (every staged descriptor back at offset 0),
/// the plan, and everything materialization created, with the identities, modes, and pins the terminal
/// gate requires. Every regular file is still `0600` and every directory `0700`.
#[derive(Debug)]
pub(crate) struct MaterializedPayloadPlaneV1 {
    /// The phase-P step this completes: [`PayloadPlaneStepV1::Evidence`].
    pub(crate) step: PayloadPlaneStepV1,
    pub(crate) plane: RestoredGitPlaneV1,
    pub(crate) plan: PayloadPlanV1,
    pub(crate) created: PayloadCreatedV1,
}

/// What one create attempt charged to the one ledger and has not given back, whether it may have created
/// an entry, and the file it left in flight: kept whatever the attempt returned.
#[derive(Debug, Default)]
struct AttemptV1 {
    charged: PayloadChargeV1,
    began: bool,
    in_flight: Option<InFlightPayloadFileV1>,
}

/// Where one entry is created: `leaf` beneath the retained `parent`, named `name` in the destination.
struct SlotV1<'s> {
    dest: &'s RestoreDestinationV1,
    parent: &'s PinnedDirectoryV1,
    leaf: &'s [u8],
    name: &'s [u8],
}

/// What one create made, as it was observed.
struct MadeV1 {
    kind: CreatedKindV1,
    created_mode: Option<u32>,
    dev: u64,
    ino: u64,
}

fn budget(error: CustodyExportErrorV1) -> PayloadPlaneErrorV1 {
    CustodyRestoreErrorV1::Budget(error.to_string()).into()
}

/// The device a created entry reports: a test seam may report another one for its destination-relative
/// name, as for every directory the restore creates.
fn created_dev(name: &[u8], observed: u64) -> u64 {
    #[cfg(test)]
    if let Some(dev) = super::CREATED_DEV.with(|devices| devices.borrow().get(name).copied()) {
        return dev;
    }
    #[cfg(not(test))]
    let _ = name;
    observed
}

/// The path `slot` will have; the test hook runs there, immediately before the create.
fn before_create(slot: &SlotV1<'_>) -> PathBuf {
    let path = slot
        .parent
        .canonical_path()
        .join(OsStr::from_bytes(slot.leaf));
    #[cfg(test)]
    run_payload_hook(PayloadHookPointV1::BeforeCreate, None, &path);
    path
}

/// Create the directory `slot`, create-new and `0700`, with its entry reserved first and given back only
/// when the create met an existing entry, and require it on the destination root's device (the restore's
/// one directory primitive).
fn create_directory(
    slot: &SlotV1<'_>,
    attempt: &mut AttemptV1,
) -> Result<PinnedDirectoryV1, PayloadPlaneErrorV1> {
    let _ = before_create(slot);
    let dest = slot.dest;
    match create_staging_directory(slot.parent, &dest.ledger, dest.dev, slot.leaf, slot.name) {
        Ok(pin) => {
            attempt.began = true;
            attempt.charged.entries += 1;
            Ok(pin)
        }
        // A collision and a refused reservation made nothing and charge nothing.
        Err(CustodyRestoreErrorV1::StagingCollision { .. }) => {
            Err(PayloadPlaneErrorV1::DestinationCollision {
                name: lossy(slot.name),
            })
        }
        Err(error @ CustodyRestoreErrorV1::Budget(_)) => Err(error.into()),
        // Any other refusal may follow a successful create, so its entry stays charged.
        Err(error) => {
            attempt.began = true;
            attempt.charged.entries += 1;
            Err(error.into())
        }
    }
}

/// Create the regular file `slot`, create-new, no-follow, and `0600`, and stream exactly `length` bytes
/// of `source` into it: the entry reserved first (given back only on a collision), the one deadline read as
/// the create returns (the file kept in flight, before anything else is done), the device and the `0600`
/// mode checked before the first byte, each chunk reserved before it is written (only its unwritten part
/// given back), the one deadline before every chunk, and the written file then required by its own
/// descriptor and by its name, still `0600`. The descriptor is not kept.
fn create_file(
    slot: &SlotV1<'_>,
    deadline: Instant,
    length: u64,
    source: &mut dyn io::Read,
    source_fault: &dyn Fn() -> GitBootstrapErrorV1,
    attempt: &mut AttemptV1,
) -> Result<MadeV1, PayloadPlaneErrorV1> {
    let dest = slot.dest;
    let shown = lossy(slot.name);
    let io_fault = |error: io::Error| -> PayloadPlaneErrorV1 {
        CustodyRestoreErrorV1::Io(format!("{shown}: {error}")).into()
    };
    dest.ledger
        .borrow_mut()
        .reserve_entries(1)
        .map_err(budget)?;
    attempt.charged.entries += 1;
    let hook_path = before_create(slot);
    let mut output = match slot
        .parent
        .create_new_regular_child(OsStr::from_bytes(slot.leaf), "custody restore payload file")
    {
        Ok(output) => output,
        Err(FsCustodyError::TargetExists(_)) => {
            // A create that met an existing entry made nothing, so its allowance goes back.
            dest.ledger.borrow_mut().release(ENTRY_ALLOWANCE_BYTES_V1);
            attempt.charged.entries -= 1;
            return Err(PayloadPlaneErrorV1::DestinationCollision { name: shown });
        }
        Err(error) => {
            // Any other refusal may follow a successful create, so it keeps the charge.
            attempt.began = true;
            return Err(staging_refusal(error, slot.name).into());
        }
    };
    attempt.began = true;
    attempt.in_flight = Some(InFlightPayloadFileV1 {
        name: slot.name.to_vec(),
        expected_length: length,
        written: 0,
    });
    // The create can block: the file is kept in flight, and the clock is read before anything else.
    observe_deadline(deadline, "payload-created")?;
    let created = ChildStatV1::from_metadata(&output.metadata().map_err(io_fault)?);
    if created_dev(slot.name, created.dev) != dest.dev {
        return Err(CustodyRestoreErrorV1::DeviceCrossing { name: shown }.into());
    }
    if created.mode != CREATED_FILE_MODE_V1 {
        return Err(CustodyRestoreErrorV1::IdentityChanged(format!(
            "{shown}: created with mode {:04o}, not {CREATED_FILE_MODE_V1:04o}",
            created.mode
        ))
        .into());
    }
    let mut flight = Some(InFlightObjectFileV1 {
        relative: shown.clone(),
        expected_length: length,
        written: 0,
    });
    let sink = SinkV1 {
        dest,
        deadline,
        relative: &shown,
        hook_path: &hook_path,
        length,
    };
    let streamed = stream_into(
        &sink,
        &mut attempt.charged,
        &mut flight,
        &mut output,
        source,
        source_fault,
    );
    if let (Some(mine), Some(written)) = (attempt.in_flight.as_mut(), flight) {
        mine.written = written.written;
    }
    let sha256 = streamed?;
    let held = ChildStatV1::from_metadata(&output.metadata().map_err(io_fault)?);
    if (held.kind, held.size, held.dev, held.ino, held.mode)
        != (
            ChildKindV1::Regular,
            length,
            dest.dev,
            created.ino,
            CREATED_FILE_MODE_V1,
        )
    {
        return Err(CustodyRestoreErrorV1::IdentityChanged(format!(
            "{shown}: the written file is not a 0600 regular file of the frame's length on the root's \
             device"
        ))
        .into());
    }
    let made = MadeV1 {
        kind: CreatedKindV1::Regular { length, sha256 },
        created_mode: Some(CREATED_FILE_MODE_V1),
        dev: held.dev,
        ino: held.ino,
    };
    recheck_created(
        slot.parent,
        slot.leaf,
        (
            ChildKindV1::Regular,
            made.dev,
            made.ino,
            length,
            made.created_mode,
        ),
        slot.name,
    )?;
    attempt.in_flight = None;
    Ok(made)
}

/// Create the symlink `slot` holding exactly `target`, through the symlink primitive (create-new, never
/// followed, verified by name), with its entry reserved first and given back only when the primitive
/// proves it created nothing (a collision, or a target the destination cannot hold), and require it on the
/// destination root's device.
fn create_symlink(
    slot: &SlotV1<'_>,
    target: &[u8],
    attempt: &mut AttemptV1,
) -> Result<MadeV1, PayloadPlaneErrorV1> {
    let dest = slot.dest;
    dest.ledger
        .borrow_mut()
        .reserve_entries(1)
        .map_err(budget)?;
    attempt.charged.entries += 1;
    let _ = before_create(slot);
    let made_nothing = |attempt: &mut AttemptV1| {
        dest.ledger.borrow_mut().release(ENTRY_ALLOWANCE_BYTES_V1);
        attempt.charged.entries -= 1;
    };
    let stat = match slot.parent.create_new_symlink_child(
        OsStr::from_bytes(slot.leaf),
        target,
        "custody restore payload symlink",
    ) {
        Ok(stat) => stat,
        Err(SymlinkChildRefusalV1::DestinationUnsupported { target_len, .. }) => {
            made_nothing(attempt);
            return Err(PayloadPlaneErrorV1::DestinationUnsupported {
                name: lossy(slot.name),
                target_len,
            });
        }
        Err(SymlinkChildRefusalV1::Custody(FsCustodyError::TargetExists(_))) => {
            made_nothing(attempt);
            return Err(PayloadPlaneErrorV1::DestinationCollision {
                name: lossy(slot.name),
            });
        }
        Err(SymlinkChildRefusalV1::Custody(error)) => {
            attempt.began = true;
            return Err(staging_refusal(error, slot.name).into());
        }
    };
    attempt.began = true;
    if created_dev(slot.name, stat.dev) != dest.dev {
        return Err(CustodyRestoreErrorV1::DeviceCrossing {
            name: lossy(slot.name),
        }
        .into());
    }
    Ok(MadeV1 {
        kind: CreatedKindV1::Symlink {
            target: target.to_vec(),
        },
        created_mode: None,
        dev: stat.dev,
        ino: stat.ino,
    })
}

/// The mode the terminal gate is to apply to an entry of `kind` rooted `at` whose frame mode is
/// `frame_mode` (D2, D4); none for a symlink.
fn final_mode(at: TreeAtV1, kind: &CreatedKindV1, frame_mode: Option<u32>) -> Option<u32> {
    match (at, kind) {
        (_, CreatedKindV1::Symlink { .. }) => None,
        (TreeAtV1::Worktree, _) => frame_mode,
        (TreeAtV1::Index, _) => Some(INDEX_FILE_MODE_V1),
        (TreeAtV1::Evidence(_), CreatedKindV1::Directory) => Some(EVIDENCE_DIRECTORY_MODE_V1),
        (TreeAtV1::Evidence(_), CreatedKindV1::Regular { .. }) => Some(EVIDENCE_FILE_MODE_V1),
    }
}

/// The step that writes a plane.
fn step_of(plane: PayloadPlaneV1) -> PayloadPlaneStepV1 {
    match plane {
        PayloadPlaneV1::Worktree => PayloadPlaneStepV1::Worktree,
        PayloadPlaneV1::Index => PayloadPlaneStepV1::Index,
        PayloadPlaneV1::Evidence(_) => PayloadPlaneStepV1::Evidence,
    }
}

/// The decoded entry is exactly the one the plan admitted at its position: the same path bytes, kind,
/// mode, length, and target.
fn is_planned(planned: &PlannedEntryV1, path: &[u8], kind: &FrameEntryKindV1) -> bool {
    planned.path == path
        && match (&planned.kind, kind) {
            (
                PlannedKindV1::Directory { mode, .. },
                FrameEntryKindV1::Directory { mode: found },
            ) => mode == found,
            (
                PlannedKindV1::Regular { mode, length },
                FrameEntryKindV1::Regular {
                    mode: found,
                    length: found_length,
                },
            ) => (mode, length) == (found, found_length),
            (
                PlannedKindV1::Symlink { target, .. },
                FrameEntryKindV1::Symlink { target: found },
            ) => target == found,
            _ => false,
        }
}

/// A refusal that ends a decode at once instead of being held until the stream is verified: the deadline
/// (nothing more is read after it), and the decoder's own failure (it is poisoned and cannot be drained).
/// Every other fault comes from what was DONE with a decoded entry, and is held.
fn ends_the_decode(error: &PayloadPlaneErrorV1) -> bool {
    matches!(
        error,
        PayloadPlaneErrorV1::Timeout
            | PayloadPlaneErrorV1::Restore(CustodyRestoreErrorV1::FrameInvalid { .. })
    )
}

/// One materialization: the phase-G value it writes beside, the one deadline, and what it has created,
/// charged, and completed so far.
struct WriteRunV1<'a> {
    plane: &'a RestoredGitPlaneV1,
    deadline: Instant,
    frame_budget: CustodyFrameBudgetV1,
    created: PayloadCreatedV1,
    began: bool,
    completed: Option<PayloadPlaneStepV1>,
}

impl WriteRunV1<'_> {
    /// Keep what one create attempt charged and left, whatever it returned.
    fn absorb(&mut self, attempt: AttemptV1) {
        self.created.charged.entries += attempt.charged.entries;
        self.created.charged.bytes += attempt.charged.bytes;
        self.began |= attempt.began;
        if attempt.in_flight.is_some() {
            self.created.in_flight = attempt.in_flight;
        }
    }

    /// `evidence/` and the slot directory beneath it, each created the first time an entry needs it, and
    /// each followed by the one deadline once its pin is kept, so no further create follows a slow one past
    /// the deadline.
    fn evidence_slot(&mut self, slot: EvidenceSlotV1) -> Result<(), PayloadPlaneErrorV1> {
        if self.created.evidence.slot(slot).is_some() {
            return Ok(());
        }
        let plane = self.plane;
        let dest = &plane.capsule().destination;
        if self.created.evidence.root.is_none() {
            let mut attempt = AttemptV1::default();
            let made = create_directory(
                &SlotV1 {
                    dest,
                    parent: &dest.root,
                    leaf: EVIDENCE_NAME_V1,
                    name: EVIDENCE_NAME_V1,
                },
                &mut attempt,
            );
            self.absorb(attempt);
            self.created.evidence.root = Some(made?);
            // The create can block: its pin is kept, and the clock is read before the next create.
            observe_deadline(self.deadline, "payload-created")?;
        }
        let name = TreeAtV1::Evidence(slot).prefix();
        let mut attempt = AttemptV1::default();
        let made = match &self.created.evidence.root {
            Some(root) => create_directory(
                &SlotV1 {
                    dest,
                    parent: root,
                    leaf: slot.directory_name(),
                    name: &name,
                },
                &mut attempt,
            ),
            None => Err(unretained_slot(slot)),
        };
        self.absorb(attempt);
        self.created.evidence.slots.push(EvidenceDirectoryV1 {
            slot,
            pin: made?,
            tree: CreatedTreeV1::default(),
        });
        observe_deadline(self.deadline, "payload-created")?;
        Ok(())
    }

    /// Create the entry at `path` beneath the tree rooted `at` (its parent created already) as `planned`
    /// says, from `content`, record it with the identity it was created with, and then read the one deadline:
    /// a slow create, write, or verification is never followed by another create past it.
    fn make_entry(
        &mut self,
        class: CustodyCoverageClassV1,
        at: TreeAtV1,
        path: &[u8],
        planned: &PlannedKindV1,
        content: Option<CustodyFrameContentReaderV1<'_>>,
        source_fault: &dyn Fn() -> GitBootstrapErrorV1,
    ) -> Result<(), PayloadPlaneErrorV1> {
        let plane = self.plane;
        let dest = &plane.capsule().destination;
        let (parent, leaf) = split_path(path);
        let name = at.name(path);
        let mut attempt = AttemptV1::default();
        #[cfg(test)]
        let created_path: PathBuf;
        let made = {
            let (tree, root) = self.created.tree(plane, at)?;
            let slot = SlotV1 {
                dest,
                parent: tree.parent(root, parent, &name)?,
                leaf,
                name: &name,
            };
            #[cfg(test)]
            {
                created_path = slot.parent.canonical_path().join(OsStr::from_bytes(leaf));
            }
            match (planned, content) {
                // The directory primitive verified the new directory's owner, its `0700` mode, and its
                // identity; it is recorded from its pin without another filesystem call.
                (PlannedKindV1::Directory { .. }, _) => create_directory(&slot, &mut attempt)
                    .and_then(|pin| {
                        let (Some(dev), Some(ino)) = (pin.identity().dev, pin.identity().ino)
                        else {
                            return Err(CustodyRestoreErrorV1::IdentityChanged(format!(
                                "{}: the created directory's identity is unavailable",
                                lossy(&name)
                            ))
                            .into());
                        };
                        let made = MadeV1 {
                            kind: CreatedKindV1::Directory,
                            created_mode: Some(CREATED_DIRECTORY_MODE_V1),
                            dev,
                            ino,
                        };
                        Ok((made, Some(pin)))
                    }),
                (PlannedKindV1::Regular { length, .. }, Some(mut content)) => create_file(
                    &slot,
                    self.deadline,
                    *length,
                    &mut content,
                    source_fault,
                    &mut attempt,
                )
                .map(|made| (made, None)),
                (PlannedKindV1::Symlink { target, .. }, _) => {
                    create_symlink(&slot, target, &mut attempt).map(|made| (made, None))
                }
                (PlannedKindV1::Regular { .. }, None) => Err(PayloadPlaneErrorV1::PlanMismatch {
                    class,
                    detail: format!("{}: a regular file without content", lossy(path)),
                }),
            }
        };
        self.absorb(attempt);
        let (made, pin) = made?;
        // A file's create was followed by the clock already; its write and verification are what this
        // reading follows. A directory or symlink is created and verified by one primitive call.
        let made_by = match planned {
            PlannedKindV1::Regular { .. } => "payload-verified",
            PlannedKindV1::Directory { .. } | PlannedKindV1::Symlink { .. } => "payload-created",
        };
        let frame_mode = match planned {
            PlannedKindV1::Directory { mode, .. } | PlannedKindV1::Regular { mode, .. } => {
                Some(*mode)
            }
            PlannedKindV1::Symlink { .. } => None,
        };
        let entry = CreatedEntryV1 {
            path: path.to_vec(),
            final_mode: final_mode(at, &made.kind, frame_mode),
            kind: made.kind,
            frame_mode,
            created_mode: made.created_mode,
            dev: made.dev,
            ino: made.ino,
        };
        let tree = self.created.tree_mut(at)?;
        if let Some(pin) = pin {
            tree.directories.insert(path.to_vec(), pin);
        }
        tree.entries.push(entry);
        #[cfg(test)]
        run_payload_hook(PayloadHookPointV1::EntryCreated, Some(class), &created_path);
        // The entry is recorded: the clock is read before the next filesystem call.
        observe_deadline(self.deadline, made_by)?;
        Ok(())
    }

    /// Restore one decoded entry the plan admitted: the connector is skipped (D3), the root gitfile goes to
    /// its evidence slot, and every other entry is created beneath its plane's root, after the one
    /// deadline (the decode can block) and, for evidence, after its slot exists.
    fn restore_entry(
        &mut self,
        frame: &PlannedFrameV1,
        planned: &PlannedEntryV1,
        content: Option<CustodyFrameContentReaderV1<'_>>,
        source_fault: &dyn Fn() -> GitBootstrapErrorV1,
    ) -> Result<(), PayloadPlaneErrorV1> {
        let class = frame.class;
        let mismatch = || PayloadPlaneErrorV1::PlanMismatch {
            class,
            detail: format!(
                "{}: an action its plane does not take",
                lossy(&planned.path)
            ),
        };
        let at = match (frame.plane, planned.action) {
            (_, PlannedActionV1::SkipConnector) => return Ok(()),
            (PayloadPlaneV1::Worktree, PlannedActionV1::Create) => TreeAtV1::Worktree,
            (PayloadPlaneV1::Index, PlannedActionV1::Create) => TreeAtV1::Index,
            (PayloadPlaneV1::Evidence(evidence), PlannedActionV1::Create) => {
                TreeAtV1::Evidence(EvidenceSlotV1::of_class(evidence).ok_or_else(mismatch)?)
            }
            (PayloadPlaneV1::Worktree, PlannedActionV1::RootGitfileToEvidence) => {
                TreeAtV1::Evidence(EvidenceSlotV1::WorktreeRootGitfile)
            }
            _ => return Err(mismatch()),
        };
        observe_deadline(self.deadline, "payload-write-item")?;
        let path: &[u8] = match planned.action {
            PlannedActionV1::RootGitfileToEvidence => ROOT_GITFILE_LEAF_V1,
            _ => &planned.path,
        };
        if let TreeAtV1::Evidence(slot) = at {
            self.evidence_slot(slot)?;
        }
        self.make_entry(class, at, path, &planned.kind, content, source_fault)
    }

    /// One planned frame, written: the clock, the staged payload the plan bound, its descriptor rehashed
    /// immediately before a THIRD complete decode, and every decoded entry, once it is the entry the plan
    /// admitted at its position, restored. The first refusal of what was done with an entry (or of an entry
    /// the plan did not admit) is HELD, and nothing after it is created, until the decoder has met the
    /// verified trailer and every byte it consumed is bound to the staged length and SHA-256; the deadline
    /// and the decoder's own failure end the decode at once. Every created byte stays.
    fn materialize_frame(&mut self, frame: &PlannedFrameV1) -> Result<(), PayloadPlaneErrorV1> {
        let (class, deadline) = (frame.class, self.deadline);
        let mismatch = |detail: String| PayloadPlaneErrorV1::PlanMismatch { class, detail };
        observe_deadline(deadline, "payload-write-frame")?;
        let plane = self.plane;
        let capsule = plane.capsule();
        let recorded = capsule
            .staged
            .get(frame.staged)
            .ok_or_else(|| mismatch("the plan names no staged payload".into()))?;
        if recorded.role != CustodyCapsuleArtifactRoleV1::CoveragePayload(class)
            || PayloadPlaneV1::of(class) != Some(frame.plane)
            || (recorded.length, recorded.sha256) != (frame.length, frame.sha256)
        {
            return Err(mismatch(
                "the plan does not bind the staged payload it names".into(),
            ));
        }
        // The retained descriptor is read through a duplicate of itself, which shares its open file
        // description: no staged name is ever opened again.
        let mut one = StagedPlaintextV1 {
            name: recorded.name.clone(),
            role: recorded.role.clone(),
            file: recorded.file.try_clone().map_err(|error| {
                CustodyRestoreErrorV1::Io(format!("{}: {error}", lossy(recorded.name.as_bytes())))
            })?,
            length: recorded.length,
            sha256: recorded.sha256,
        };
        reverify_staged_v1(&mut one, deadline)?;
        #[cfg(test)]
        run_payload_hook(
            PayloadHookPointV1::MaterializationRehashed,
            Some(class),
            Path::new(OsStr::from_bytes(one.name.as_bytes())),
        );
        let header = CustodyFrameHeaderV1::new(class, capsule.control.manifest.generation_id())
            .map_err(|_| frame_invalid(class))?;
        let expired = Cell::new(false);
        let refuse = || {
            if expired.get() {
                PayloadPlaneErrorV1::Timeout
            } else {
                frame_invalid(class)
            }
        };
        let source_fault = || {
            if expired.get() {
                GitBootstrapErrorV1::Git(CustodyGitError::Timeout)
            } else {
                GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::FrameInvalid { class })
            }
        };
        let mut held: Option<PayloadPlaneErrorV1> = None;
        let mut decoded = 0_usize;
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
                let (path, kind, content) = match entry {
                    CustodyFrameEntryV1::Directory { path, mode } => {
                        (path, FrameEntryKindV1::Directory { mode }, None)
                    }
                    CustodyFrameEntryV1::Regular {
                        path,
                        mode,
                        length,
                        content,
                    } => (
                        path,
                        FrameEntryKindV1::Regular { mode, length },
                        Some(content),
                    ),
                    CustodyFrameEntryV1::Symlink { path, target } => {
                        (path, FrameEntryKindV1::Symlink { target }, None)
                    }
                };
                #[cfg(test)]
                run_payload_hook(
                    PayloadHookPointV1::MaterializationEntry,
                    Some(class),
                    Path::new(OsStr::from_bytes(path.as_bytes())),
                );
                let planned = frame.entries.get(decoded);
                decoded = decoded.saturating_add(1);
                // Once a fault is held nothing more is created: the rest of the frame is only decoded, so
                // its trailer and its bytes are still verified (an unread remainder of the current content is
                // drained and verified by the decoder's next call).
                if held.is_some() {
                    continue;
                }
                let Some(planned) =
                    planned.filter(|planned| is_planned(planned, path.as_bytes(), &kind))
                else {
                    held = Some(mismatch(format!(
                        "{}: not the entry prevalidation admitted",
                        lossy(path.as_bytes())
                    )));
                    continue;
                };
                match self.restore_entry(frame, planned, content, &source_fault) {
                    Ok(()) => {}
                    Err(error) if ends_the_decode(&error) => return Err(error),
                    Err(error) => held = Some(error),
                }
            }
        }
        // The decoder met the verified trailer and the end of the stream. The bytes it consumed must be
        // exactly the verified plaintext: a frame replaced after its rehash is refused here, ahead of
        // anything it said.
        let (count, context) = (paced.count, paced.context);
        let mut decoded_sha256 = [0_u8; 32];
        decoded_sha256.copy_from_slice(context.finish().as_ref());
        if count != one.length || decoded_sha256 != one.sha256 {
            return Err(PayloadPlaneErrorV1::StagedInputChanged {
                name: lossy(one.name.as_bytes()),
            });
        }
        if decoded != frame.entries.len() {
            return Err(mismatch(format!(
                "{decoded} entries decoded where the plan admitted {}",
                frame.entries.len()
            )));
        }
        if let Some(error) = held {
            return Err(error);
        }
        observe_deadline(deadline, "payload-write-decoded")?;
        one.file.seek(SeekFrom::Start(0)).map_err(|error| {
            CustodyRestoreErrorV1::Io(format!("{}: {error}", lossy(one.name.as_bytes())))
        })?;
        Ok(())
    }

    /// The three materialization steps, in order, each ending in the gate over everything created so far:
    /// a step completes only once its frames are written and its gate passed.
    fn run(&mut self, plan: &PayloadPlanV1) -> Result<(), PayloadPlaneErrorV1> {
        for step in [
            PayloadPlaneStepV1::Worktree,
            PayloadPlaneStepV1::Index,
            PayloadPlaneStepV1::Evidence,
        ] {
            for frame in plan
                .frames
                .iter()
                .filter(|frame| step_of(frame.plane) == step)
            {
                self.materialize_frame(frame)?;
            }
            #[cfg(test)]
            run_payload_step_hook(
                PayloadHookPointV1::BeforeStepGate,
                None,
                Some(step),
                self.plane.capsule().destination.root.canonical_path(),
            );
            gate(self.plane, Some(&self.created), self.deadline)?;
            self.completed = Some(step);
        }
        Ok(())
    }
}

/// Plan phase P over `plane` ([`plan_payload_plane_v1`]: admission, prevalidation, and their gates) and
/// then write the plan: the worktree, the index, and the evidence, each step ending in the gate, all on the
/// ONE caller-supplied absolute `deadline` and the one ledger.
///
/// Every entry is create-new beneath a retained pin, `0600` for a file and `0700` for a directory, its
/// frame mode and its final mode recorded and never applied; symlinks are written verbatim and never
/// followed. On success every staged descriptor is at offset 0 and the value carries the phase-G value,
/// the plan, and every created entry's identity and pin. A refusal keeps all of them, with the last step
/// completed whole, whether any entry may have been created, the one ledger's figure, and the file in
/// flight; every created byte stays where it is, and a second invocation refuses at its first create.
pub(crate) fn materialize_payload_plane_v1(
    plane: RestoredGitPlaneV1,
    frame_budget: CustodyFrameBudgetV1,
    deadline: Instant,
) -> Result<MaterializedPayloadPlaneV1, Box<PayloadPlaneRestoreFailureV1>> {
    materialize_payload_plane_bounded_v1(plane, frame_budget, deadline, PAYLOAD_PLAN_TABLE_BYTES_V1)
}

/// [`materialize_payload_plane_v1`] with the plan table's byte bound as a parameter.
pub(crate) fn materialize_payload_plane_bounded_v1(
    plane: RestoredGitPlaneV1,
    frame_budget: CustodyFrameBudgetV1,
    deadline: Instant,
    table_bound: u64,
) -> Result<MaterializedPayloadPlaneV1, Box<PayloadPlaneRestoreFailureV1>> {
    let PlannedPayloadPlaneV1 { plane, plan, .. } =
        plan_payload_plane_bounded_v1(plane, frame_budget, deadline, table_bound)?;
    #[cfg(test)]
    let plan = edited_plan(plan);
    let mut run = WriteRunV1 {
        plane: &plane,
        deadline,
        frame_budget,
        created: PayloadCreatedV1::default(),
        began: false,
        completed: Some(PayloadPlaneStepV1::Prevalidation),
    };
    let outcome = run.run(&plan);
    let WriteRunV1 {
        created,
        began,
        completed,
        ..
    } = run;
    match outcome {
        Ok(()) => Ok(MaterializedPayloadPlaneV1 {
            step: PayloadPlaneStepV1::Evidence,
            plane,
            plan,
            created,
        }),
        Err(cause) => Err(Box::new(PayloadPlaneRestoreFailureV1 {
            cause,
            last_completed: completed,
            payload_materialization_began: began,
            ledger_used: plane.ledger_used(),
            partial: PayloadPlanePartialV1 {
                plane,
                plan: Some(plan),
                created,
            },
        })),
    }
}
