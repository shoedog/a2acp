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
//! **Effects.** None. No entry is created, nothing is charged to the one ledger, no Git child runs, and
//! no staged name is reopened: every payload byte is read through its retained descriptor, which is back
//! at offset 0 on success. A refusal reports the one ledger's figure, which this increment never moves.

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
use crate::fs_custody::pinned_root_unchanged;
use ring::digest;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::io::{self, BufReader, Read as _, Seek as _, SeekFrom};
use std::time::Instant;
#[cfg(test)]
use std::{cell::RefCell, ffi::OsStr, os::unix::ffi::OsStrExt as _, path::Path};

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

/// The steps of phase P, in order. Progress advances only when a whole step succeeds. Increment C2a
/// reaches [`Self::Prevalidation`]; the rest belong to the later increments and are enumerated here so
/// one progress type spans the slice.
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

/// Every value a refused phase P still holds. Nothing is ever removed: dropping these closes only
/// descriptors. Increment C2a creates nothing, so this is the phase-G value alone.
#[derive(Debug)]
pub(crate) struct PayloadPlanePartialV1 {
    /// The phase-G value, with its capsule, its pins, its one ledger, and every staged descriptor.
    pub(crate) plane: RestoredGitPlaneV1,
}

/// A refused or failed phase P, with how far it got. `last_completed` is the last step that completed
/// whole, and `payload_materialization_began` is whether any payload entry may have been created (never,
/// before the materialization steps).
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
}

#[cfg(test)]
pub(crate) struct PayloadHookEventV1<'a> {
    /// The frame's class, where the point concerns one frame.
    pub(crate) class: Option<CustodyCoverageClassV1>,
    pub(crate) path: &'a Path,
}

#[cfg(test)]
type PayloadHookV1 = Box<dyn Fn(&PayloadHookEventV1<'_>)>;

#[cfg(test)]
thread_local! {
    static PAYLOAD_HOOKS: RefCell<BTreeMap<PayloadHookPointV1, PayloadHookV1>> =
        RefCell::new(BTreeMap::new());
}

/// Uninstalls its one hook when dropped.
#[cfg(test)]
pub(crate) struct PayloadSeamGuardV1(PayloadHookPointV1);

#[cfg(test)]
impl Drop for PayloadSeamGuardV1 {
    fn drop(&mut self) {
        PAYLOAD_HOOKS.with(|hooks| {
            hooks.borrow_mut().remove(&self.0);
        });
    }
}

#[cfg(test)]
pub(crate) fn install_payload_hook_for_test(
    point: PayloadHookPointV1,
    hook: impl Fn(&PayloadHookEventV1<'_>) + 'static,
) -> PayloadSeamGuardV1 {
    PAYLOAD_HOOKS.with(|hooks| hooks.borrow_mut().insert(point, Box::new(hook)));
    PayloadSeamGuardV1(point)
}

#[cfg(test)]
fn run_payload_hook(point: PayloadHookPointV1, class: Option<CustodyCoverageClassV1>, path: &Path) {
    PAYLOAD_HOOKS.with(|hooks| {
        if let Some(hook) = hooks.borrow().get(&point) {
            hook(&PayloadHookEventV1 { class, path });
        }
    });
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

/// The staging, HOME, XDG, and bootstrap chains, then the destination root's identity LAST: the same
/// edges, in the same order, as phase G's retained sweep, whose own line a 2B3b structural control pins
/// in place. Every retained staging directory is rechecked with its whole chain; HOME, XDG, and each
/// bootstrap directory beneath its retained parent (parents sort first). `pace` runs before every step,
/// and its last call is the last statement ahead of the root's identity.
fn retained_sweep(
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
    pace("payload-root")?;
    pinned_root_unchanged(&dest.root)
        .map_err(|detail| CustodyRestoreErrorV1::IdentityChanged(detail).into())
}

/// The phase-P gate: the mount census over the destination first, then every retained chain (phase G's
/// object store and metadata, the bootstrap's admitted object files, and the staging, HOME, XDG, and
/// bootstrap directories), and the destination root's identity LAST. The one deadline is observed before
/// the census, before every chain, and immediately before the root's identity, which is the gate's last
/// fallible call. A replaced or retyped entry refuses; nothing is followed.
fn gate(plane: &RestoredGitPlaneV1, deadline: Instant) -> Result<(), PayloadPlaneErrorV1> {
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
    retained_sweep(dest, &plane.bootstrap, pace)?;
    Ok(())
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
    gate(plane, deadline)?;
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
    gate(plane, deadline)?;
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
            partial: PayloadPlanePartialV1 { plane },
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
