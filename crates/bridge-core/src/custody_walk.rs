//! The ADR-0041 descriptor-relative, no-follow custody walker, v1 (slice 2B2b2a).
//!
//! [`walk_tree_v1`] turns one pinned directory subtree into one 2B2b1 coverage frame. It is
//! class-agnostic: a caller-supplied [`WalkSelectionV1`] decides, entry by entry, what is emitted,
//! what is skipped, and what parks the walk. Every entry beneath the root is emitted exactly once,
//! skipped because the selection says so, or refuses the walk with a typed [`CustodyWalkErrorV1`].
//!
//! Every syscall is relative to a retained directory descriptor, and none follows a symlink.
//! Names are listed through a duplicate descriptor, stated with `AT_SYMLINK_NOFOLLOW`, and opened
//! with `O_NOFOLLOW`; a symlink's target is read with `readlinkat` and is never opened. Every
//! entry must be on the root's device. The walker never writes except into the caller's frame
//! encoder.
//!
//! **Guarantee boundary (task §1.1).** This is detection, not prevention. A change is detected if
//! it is visible in any emitted entry's `(kind, dev, ino, mode, size, mtime, ctime)`, or in any
//! directory's child-name set, at any point between the entry's first observation and the final
//! verification pass. Coherence of the capture rests on the capability's quiescence decision. A
//! writer that forges timestamps to hide a change is out of scope.
//!
//! **Traversal (§4.1).** Depth-first pre-order. Each directory's names are listed, sorted by byte
//! order, then stated and decided in that order, then processed in that order, a directory's
//! subtree before its next sibling. That is exactly the frame's component-wise canonical order.
//! The traversal is iterative, so depth costs heap rather than stack: one retained descriptor and
//! one name list per depth level, plus one shared path buffer.
//!
//! **Drift (§4.2–§4.6).**
//! - A regular file's opened descriptor must be the object that was listed, and its metadata must
//!   not change while its content is read.
//! - A symlink's target must be as long as the listed size, and a re-stat after the read must equal
//!   the listing.
//! - A child directory's opened descriptor must be the directory that was listed, mode included.
//! - After a directory's subtree, the directory is re-listed. It must hold exactly the same names,
//!   each kept child must keep its kind, device, and inode, and the directory's own device, inode,
//!   mtime, and ctime must be unchanged.
//! - Last, a stat-only pass re-walks the tree and must reproduce the inventory digest that the
//!   emitting pass folded over every emitted and skipped entry.
//!
//! **Budget (§2.1, §4.6).** One entry budget bounds every name the emitting pass lists. The
//! post-subtree re-listings and the final pass each charge their own counter capped at the same
//! budget, so the live name lists never hold more than two budgets' worth of names. Those two
//! counters can run out only if the tree grew after the emitting pass listed it, so running out is
//! drift, not a budget refusal.

use crate::custody_frame::{
    CustodyFrameEncoderV1, CustodyFrameErrorV1, CustodyFramePathV1, CustodyFrameSummaryV1,
};
use crate::custody_inventory::CustodyReasonCodeV1;
use crate::fs_custody::{
    ChildBytesV1, ChildKindV1, ChildStatV1, FsCustodyError, PinnedDirectoryV1,
};
use ring::digest;
use std::fmt;
use std::fs::File;
use std::io::{self, Write};
use std::ops::Deref;

const LABEL: &str = "custody walk";
/// The frame's symlink-target ceiling (2B2b1 §2.3).
const MAX_SYMLINK_TARGET_BYTES_V1: usize = 4095;

/// The inventory digest's domain prefix, NUL-terminated (§4.6).
const INVENTORY_DOMAIN_V1: &[u8] = b"a2a-walk-inventory-v1\0";
const RECORD_DIRECTORY_V1: u8 = 1;
const RECORD_REGULAR_V1: u8 = 2;
const RECORD_SYMLINK_V1: u8 = 3;
const RECORD_SKIPPED_V1: u8 = 4;
const NANOSECONDS_PER_SECOND: i128 = 1_000_000_000;

// ---------------------------------------------------------------------------------------------
// Selection
// ---------------------------------------------------------------------------------------------

/// A selection's refusal: a caller-typed park reason and the entry that raised it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WalkParkV1 {
    reason: CustodyReasonCodeV1,
    path: CustodyFramePathV1,
}

impl WalkParkV1 {
    pub(crate) fn new(reason: CustodyReasonCodeV1, path: CustodyFramePathV1) -> Self {
        Self { reason, path }
    }

    pub(crate) fn reason(&self) -> CustodyReasonCodeV1 {
        self.reason
    }

    pub(crate) fn path(&self) -> &CustodyFramePathV1 {
        &self.path
    }
}

/// What the walker does with one entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WalkDecisionV1 {
    /// Emit the entry, and recurse into a directory.
    Include,
    /// Emit the entry. A directory is emitted with no children and is never opened.
    IncludeEntryOnly,
    /// Emit nothing and do not recurse. The caller must account for the entry; the walker counts
    /// it in its receipt so the caller can prove that accounting.
    Skip,
    /// Refuse the walk with this caller-typed reason.
    Park(WalkParkV1),
}

/// Decides each entry beneath the root, given its root-relative frame path and its no-follow
/// listing observation.
///
/// `decide` must be a pure function of `(path, stat)`. The walker calls it only after sorting a
/// directory's names, so its decisions, and the first reported `Park`, do not depend on `readdir`
/// order. The final verification pass calls it again for every entry.
pub(crate) trait WalkSelectionV1 {
    fn decide(&self, path: &CustodyFramePathV1, stat: &ChildStatV1) -> WalkDecisionV1;
}

// ---------------------------------------------------------------------------------------------
// Refusals and receipt
// ---------------------------------------------------------------------------------------------

/// Which check observed a concurrent change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WalkDriftV1 {
    /// The root descriptor's device or inode differs between the start and the end of the walk.
    RootIdentity,
    /// A listed name was absent when it was stated, opened, or read.
    Vanished,
    /// Opening or reading a listed entry failed, and the entry is no longer the listed object.
    Replaced,
    /// A regular file's opened descriptor is not the object that was listed.
    OpenedIdentity,
    /// A regular file's metadata changed while its content was read.
    DuringRead,
    /// A symlink's target read is not the listed symlink's: another length, or another object
    /// after the read.
    SymlinkTarget,
    /// A child directory's opened descriptor is not the directory that was listed.
    DescentIdentity,
    /// A child directory's mode changed between listing and descent.
    DescentMode,
    /// A directory's child-name set changed during its subtree.
    ChildNames,
    /// A kept child's kind, device, or inode changed during its directory's subtree.
    ChildIdentity,
    /// A directory's own device, inode, mtime, or ctime changed during its subtree.
    DirectoryMetadata,
    /// The final stat-only pass observed a different inventory.
    FinalVerification,
}

impl fmt::Display for WalkDriftV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::RootIdentity => "the root's device or inode changed",
            Self::Vanished => "a listed entry vanished",
            Self::Replaced => "a listed entry was replaced before it was read",
            Self::OpenedIdentity => "an opened file is not the listed file",
            Self::DuringRead => "a file changed while it was read",
            Self::SymlinkTarget => "a symlink changed while its target was read",
            Self::DescentIdentity => "an opened directory is not the listed directory",
            Self::DescentMode => "a directory's mode changed before descent",
            Self::ChildNames => "a directory's names changed",
            Self::ChildIdentity => "a kept child's kind, device, or inode changed",
            Self::DirectoryMetadata => "a directory's own metadata changed",
            Self::FinalVerification => "the final verification pass observed a different tree",
        })
    }
}

/// A walk refusal. After any refusal no receipt exists, and the caller must discard its sink: the
/// encoder was consumed and may have written a partial frame.
///
/// A message names its cause only; the paths it carries are for the caller's accounting, not for
/// display.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum CustodyWalkErrorV1 {
    /// A frame refusal: a path, mode, symlink-target, entry-limit, or byte-budget refusal.
    #[error("custody walk frame refusal: {0}")]
    Frame(CustodyFrameErrorV1),
    #[error("custody walk entry budget exceeded")]
    EntryLimit,
    #[error("custody walk entry is not a directory, regular file, or symlink")]
    UnsupportedEntry {
        path: CustodyFramePathV1,
        kind: ChildKindV1,
    },
    #[error("custody walk entry is on another device than the root")]
    MountBoundary { path: CustodyFramePathV1 },
    /// `path` is `None` for the root directory, which is never a frame entry.
    #[error("custody walk source changed: {detail}")]
    SourceDrift {
        path: Option<CustodyFramePathV1>,
        detail: WalkDriftV1,
    },
    #[error("custody walk parked by its selection: {}", .0.reason().as_code())]
    Park(WalkParkV1),
    #[error("custody walk io failed: {0}")]
    Io(io::ErrorKind),
}

impl From<CustodyFrameErrorV1> for CustodyWalkErrorV1 {
    fn from(error: CustodyFrameErrorV1) -> Self {
        Self::Frame(error)
    }
}

type WalkResult<T> = Result<T, CustodyWalkErrorV1>;

fn drift(path: Option<CustodyFramePathV1>, detail: WalkDriftV1) -> CustodyWalkErrorV1 {
    CustodyWalkErrorV1::SourceDrift { path, detail }
}

/// A primitive's refusal, other than a listing budget, as the walk's I/O refusal.
fn io_refusal(error: &FsCustodyError) -> CustodyWalkErrorV1 {
    CustodyWalkErrorV1::Io(match error {
        FsCustodyError::Io(_, error) => error.kind(),
        FsCustodyError::InvalidChildName(_) => io::ErrorKind::InvalidData,
        FsCustodyError::Unsupported(_) => io::ErrorKind::Unsupported,
        _ => io::ErrorKind::Other,
    })
}

/// The walk's receipt: the finished frame's summary, the entries the selection skipped, and the
/// inventory digest that the final verification pass reproduced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WalkReceiptV1 {
    summary: CustodyFrameSummaryV1,
    skipped_entries: u64,
    inventory_sha256: [u8; 32],
}

impl WalkReceiptV1 {
    pub(crate) fn summary(&self) -> CustodyFrameSummaryV1 {
        self.summary
    }

    pub(crate) fn skipped_entries(&self) -> u64 {
        self.skipped_entries
    }

    pub(crate) fn inventory_sha256(&self) -> [u8; 32] {
        self.inventory_sha256
    }
}

// ---------------------------------------------------------------------------------------------
// The inventory digest (§4.6)
// ---------------------------------------------------------------------------------------------

/// The running SHA-256 over every emitted and skipped entry. Every field is fixed width or
/// length-prefixed, so the encoding is injective:
///
/// ```text
/// digest input = "a2a-walk-inventory-v1" %x00 *record
/// record       = tag:u8 path-length:u16 path dev:u64 ino:u64 mode:u32 size:u64
///                mtime-sec:i64 mtime-nsec:u32 ctime-sec:i64 ctime-nsec:u32
/// ```
///
/// Integers are little-endian. The tag is 1 for a directory, 2 for a regular file, 3 for a
/// symlink, and 4 for a skipped entry of any kind.
struct InventoryV1 {
    context: digest::Context,
}

impl InventoryV1 {
    fn new() -> Self {
        let mut context = digest::Context::new(&digest::SHA256);
        context.update(INVENTORY_DOMAIN_V1);
        Self { context }
    }

    fn fold(&mut self, tag: u8, path: &CustodyFramePathV1, stat: &ChildStatV1) {
        let path = path.as_bytes();
        // A frame path is at most 4096 bytes, so its length always fits.
        let length = u16::try_from(path.len()).unwrap_or(u16::MAX);
        let (mtime_sec, mtime_nsec) = split_nanoseconds(stat.mtime_ns);
        let (ctime_sec, ctime_nsec) = split_nanoseconds(stat.ctime_ns);
        self.context.update(&[tag]);
        self.context.update(&length.to_le_bytes());
        self.context.update(path);
        self.context.update(&stat.dev.to_le_bytes());
        self.context.update(&stat.ino.to_le_bytes());
        self.context.update(&stat.mode.to_le_bytes());
        self.context.update(&stat.size.to_le_bytes());
        self.context.update(&mtime_sec.to_le_bytes());
        self.context.update(&mtime_nsec.to_le_bytes());
        self.context.update(&ctime_sec.to_le_bytes());
        self.context.update(&ctime_nsec.to_le_bytes());
    }

    fn finish(self) -> [u8; 32] {
        let mut out = [0_u8; 32];
        out.copy_from_slice(self.context.finish().as_ref());
        out
    }
}

/// Seconds and a nanosecond remainder in `0..1_000_000_000`. The nanosecond count was built from
/// an `i64` second and an in-range nanosecond, so the seconds fit an `i64`.
fn split_nanoseconds(nanoseconds: i128) -> (i64, u32) {
    let seconds = nanoseconds.div_euclid(NANOSECONDS_PER_SECOND);
    let remainder = nanoseconds.rem_euclid(NANOSECONDS_PER_SECOND);
    (seconds as i64, remainder as u32)
}

// ---------------------------------------------------------------------------------------------
// Observation points (named for the test seam)
// ---------------------------------------------------------------------------------------------

/// Which pass made an observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PassV1 {
    /// The pass that reads content and drives the encoder.
    Emit,
    /// The final stat-only pass.
    Verify,
}

/// Where a metadata observation was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObservationV1 {
    /// The root descriptor, at the start or the end of the walk.
    Root,
    /// An entry's no-follow listing stat.
    Listing,
    /// A regular file's opened descriptor, before its content is read.
    Opened,
    /// A regular file's opened descriptor, after its content was read.
    AfterRead,
    /// A symlink's no-follow stat, after its target was read.
    TargetRead,
    /// A child directory's opened descriptor.
    Descent,
    /// A directory's own descriptor, before it is listed.
    DirectoryBefore,
    /// A directory's own descriptor, after its subtree.
    DirectoryAfter,
    /// An entry's no-follow stat after its directory's subtree, or after an open failed.
    Recheck,
}

/// Every metadata observation passes through here, so the test seam can stand in for a mount, a
/// coarse-timestamp filesystem, or a changed field. Outside tests it is the identity.
fn observed(
    pass: PassV1,
    observation: ObservationV1,
    path: Option<&CustodyFramePathV1>,
    stat: ChildStatV1,
) -> ChildStatV1 {
    #[cfg(test)]
    let stat = {
        let mut stat = stat;
        seam::stat(pass, observation, path, &mut stat);
        stat
    };
    #[cfg(not(test))]
    let _ = (pass, observation, path);
    stat
}

fn file_metadata(file: &File) -> Result<ChildStatV1, FsCustodyError> {
    file.metadata()
        .map(|metadata| ChildStatV1::from_metadata(&metadata))
        .map_err(|error| FsCustodyError::Io(LABEL.to_owned(), error))
}

// ---------------------------------------------------------------------------------------------
// The walk
// ---------------------------------------------------------------------------------------------

/// Walks the pinned subtree under `selection` into `encoder`, then verifies it and finishes the
/// frame (§3, §4).
///
/// `entry_budget` bounds the names the emitting pass lists, skipped entries included; exceeding it
/// refuses [`CustodyWalkErrorV1::EntryLimit`]. The walker takes the encoder by value and finishes
/// it only after the final verification pass succeeds; a caller keeps its sink by passing a
/// borrowed writer as `W`.
pub(crate) fn walk_tree_v1<W: Write>(
    root: &PinnedDirectoryV1,
    selection: &dyn WalkSelectionV1,
    entry_budget: u64,
    mut encoder: CustodyFrameEncoderV1<W>,
) -> WalkResult<WalkReceiptV1> {
    let start = root
        .current_metadata(LABEL)
        .map_err(|error| io_refusal(&error))?;
    let start = observed(PassV1::Emit, ObservationV1::Root, None, start);

    let mut emit = WalkV1::new(selection, PassV1::Emit, start.dev, entry_budget);
    emit.run(root, Some(&mut encoder))?;
    let skipped_entries = emit.skipped_entries;
    let inventory_sha256 = emit.inventory.finish();

    #[cfg(test)]
    seam::point(PassV1::Emit, &seam::PointV1::Verifying);

    let mut verify = WalkV1::new(selection, PassV1::Verify, start.dev, entry_budget);
    verify.run::<W>(root, None)?;
    if verify.inventory.finish() != inventory_sha256 {
        return Err(drift(None, WalkDriftV1::FinalVerification));
    }

    let end = root
        .current_metadata(LABEL)
        .map_err(|error| io_refusal(&error))?;
    let end = observed(PassV1::Verify, ObservationV1::Root, None, end);
    if end.dev != start.dev || end.ino != start.ino {
        return Err(drift(None, WalkDriftV1::RootIdentity));
    }

    let summary = encoder.finish()?;
    Ok(WalkReceiptV1 {
        summary,
        skipped_entries,
        inventory_sha256,
    })
}

/// The selection's decision for a kept or skipped entry; `Park` never gets this far.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeptV1 {
    Include,
    EntryOnly,
    Skip,
}

/// One stated and decided child. The path is rebuilt from the shared buffer when needed, so a
/// wide directory holds names, not full paths.
struct EntryV1 {
    name: ChildBytesV1,
    stat: ChildStatV1,
    decision: KeptV1,
}

enum DirectoryHandleV1<'r> {
    Root(&'r PinnedDirectoryV1),
    Child(PinnedDirectoryV1),
}

impl Deref for DirectoryHandleV1<'_> {
    type Target = PinnedDirectoryV1;

    fn deref(&self) -> &PinnedDirectoryV1 {
        match self {
            Self::Root(root) => root,
            Self::Child(child) => child,
        }
    }
}

/// One directory on the traversal stack.
struct DirectoryFrameV1<'r> {
    directory: DirectoryHandleV1<'r>,
    /// This directory's path length in the shared path buffer; 0 for the root.
    path_length: usize,
    /// The directory's own metadata, observed before it was listed (§4.4).
    before: ChildStatV1,
    /// Every child, sorted by name bytes.
    entries: Vec<EntryV1>,
    next: usize,
}

/// One pass over the tree. The emitting pass has an encoder; the final pass has none and reads no
/// content.
struct WalkV1<'s> {
    selection: &'s dyn WalkSelectionV1,
    pass: PassV1,
    root_dev: u64,
    /// The pass's listing counter: the global entry budget in the emitting pass.
    listing_remaining: u64,
    /// The post-subtree re-listing counter (§4.4), capped at the same budget.
    relisting_remaining: u64,
    inventory: InventoryV1,
    skipped_entries: u64,
    /// The path of the entry most recently named, shared by every depth level.
    path: Vec<u8>,
    #[cfg(test)]
    live_names: u64,
}

impl<'s> WalkV1<'s> {
    fn new(
        selection: &'s dyn WalkSelectionV1,
        pass: PassV1,
        root_dev: u64,
        entry_budget: u64,
    ) -> Self {
        Self {
            selection,
            pass,
            root_dev,
            listing_remaining: entry_budget,
            relisting_remaining: entry_budget,
            inventory: InventoryV1::new(),
            skipped_entries: 0,
            path: Vec::new(),
            #[cfg(test)]
            live_names: 0,
        }
    }

    fn run<W: Write>(
        &mut self,
        root: &PinnedDirectoryV1,
        mut encoder: Option<&mut CustodyFrameEncoderV1<W>>,
    ) -> WalkResult<()> {
        let mut stack = vec![self.enter(DirectoryHandleV1::Root(root), 0)?];
        while let Some(frame) = stack.last_mut() {
            let index = frame.next;
            if index == frame.entries.len() {
                if let Some(frame) = stack.pop() {
                    self.leave(frame)?;
                }
                continue;
            }
            frame.next += 1;
            let frame = &*frame;
            let child = self.process(
                &frame.directory,
                frame.path_length,
                &frame.entries[index],
                encoder.as_deref_mut(),
            )?;
            if let Some(child) = child {
                stack.push(child);
            }
        }
        Ok(())
    }

    fn observe(
        &self,
        observation: ObservationV1,
        path: Option<&CustodyFramePathV1>,
        stat: Result<ChildStatV1, FsCustodyError>,
    ) -> WalkResult<ChildStatV1> {
        let stat = stat.map_err(|error| io_refusal(&error))?;
        Ok(observed(self.pass, observation, path, stat))
    }

    /// Writes the path of `name` beneath the directory whose path is the buffer's first
    /// `directory_length` bytes, and validates it as a frame path.
    fn entry_path(
        &mut self,
        directory_length: usize,
        name: &ChildBytesV1,
    ) -> WalkResult<CustodyFramePathV1> {
        self.path.truncate(directory_length);
        if directory_length != 0 {
            self.path.push(b'/');
        }
        self.path.extend_from_slice(name.as_bytes());
        Ok(CustodyFramePathV1::from_bytes(&self.path)?)
    }

    /// The path of the directory whose path is the buffer's first `length` bytes; `None` for the
    /// root. Only descendants extend the buffer, so the prefix is intact while the directory is on
    /// the stack.
    fn directory_path(&self, length: usize) -> Option<CustodyFramePathV1> {
        if length == 0 {
            return None;
        }
        CustodyFramePathV1::from_bytes(&self.path[..length]).ok()
    }

    /// Lists, sorts, states, and decides one directory's children (§4.1).
    fn enter<'r>(
        &mut self,
        directory: DirectoryHandleV1<'r>,
        path_length: usize,
    ) -> WalkResult<DirectoryFrameV1<'r>> {
        let directory_path = self.directory_path(path_length);
        let before = self.observe(
            ObservationV1::DirectoryBefore,
            directory_path.as_ref(),
            directory.current_metadata(LABEL),
        )?;
        let mut names = directory
            .list_child_names(&mut self.listing_remaining, LABEL)
            .map_err(|error| match error {
                FsCustodyError::EnumerationLimitExceeded { .. } => match self.pass {
                    PassV1::Emit => CustodyWalkErrorV1::EntryLimit,
                    // The emitting pass listed this tree within the same budget, so running out
                    // here means the tree grew.
                    PassV1::Verify => drift(directory_path.clone(), WalkDriftV1::FinalVerification),
                },
                error => io_refusal(&error),
            })?;
        #[cfg(test)]
        seam::listed(self.pass, directory_path.as_ref(), &mut names);
        names.sort();
        #[cfg(test)]
        self.hold_names(names.len());

        let mut entries = Vec::with_capacity(names.len());
        for name in names {
            let path = self.entry_path(path_length, &name)?;
            let stat = match directory.child_metadata_no_follow(name.as_os_str(), LABEL) {
                Ok(Some(stat)) => observed(self.pass, ObservationV1::Listing, Some(&path), stat),
                Ok(None) => return Err(drift(Some(path), WalkDriftV1::Vanished)),
                Err(error) => return Err(io_refusal(&error)),
            };
            if stat.dev != self.root_dev {
                return Err(CustodyWalkErrorV1::MountBoundary { path });
            }
            let decision = match self.selection.decide(&path, &stat) {
                WalkDecisionV1::Include => KeptV1::Include,
                WalkDecisionV1::IncludeEntryOnly => KeptV1::EntryOnly,
                WalkDecisionV1::Skip => KeptV1::Skip,
                WalkDecisionV1::Park(park) => return Err(CustodyWalkErrorV1::Park(park)),
            };
            entries.push(EntryV1 {
                name,
                stat,
                decision,
            });
        }
        Ok(DirectoryFrameV1 {
            directory,
            path_length,
            before,
            entries,
            next: 0,
        })
    }

    /// Emits (or, in the final pass, only folds) one child. Returns the child directory's frame
    /// when the walk descends into it.
    fn process<'r, W: Write>(
        &mut self,
        directory: &PinnedDirectoryV1,
        path_length: usize,
        entry: &EntryV1,
        encoder: Option<&mut CustodyFrameEncoderV1<W>>,
    ) -> WalkResult<Option<DirectoryFrameV1<'r>>> {
        let path = self.entry_path(path_length, &entry.name)?;
        let stat = &entry.stat;
        if entry.decision == KeptV1::Skip {
            self.inventory.fold(RECORD_SKIPPED_V1, &path, stat);
            self.skipped_entries += 1;
            return Ok(None);
        }
        match stat.kind {
            ChildKindV1::Special(_) => Err(CustodyWalkErrorV1::UnsupportedEntry {
                path,
                kind: stat.kind,
            }),
            ChildKindV1::Symlink => {
                if let Some(encoder) = encoder {
                    let target = self.read_symlink(directory, entry, &path)?;
                    encoder.symlink(&path, &target)?;
                }
                self.inventory.fold(RECORD_SYMLINK_V1, &path, stat);
                Ok(None)
            }
            ChildKindV1::Regular => {
                if let Some(encoder) = encoder {
                    self.capture_regular(directory, entry, &path, encoder)?;
                }
                self.inventory.fold(RECORD_REGULAR_V1, &path, stat);
                Ok(None)
            }
            ChildKindV1::Directory => {
                if entry.decision == KeptV1::EntryOnly {
                    if let Some(encoder) = encoder {
                        encoder.directory(&path, stat.mode)?;
                    }
                    self.inventory.fold(RECORD_DIRECTORY_V1, &path, stat);
                    return Ok(None);
                }
                #[cfg(test)]
                seam::point(self.pass, &seam::PointV1::BeforeOpen(path.clone()));
                let child = directory
                    .open_existing_child_directory(entry.name.as_os_str(), LABEL)
                    .map_err(|error| self.explain(directory, entry, &path, &error))?;
                #[cfg(test)]
                seam::opened(self.pass, &path);
                let opened = self.observe(
                    ObservationV1::Descent,
                    Some(&path),
                    child.current_metadata(LABEL),
                )?;
                // Mode first: a chmod also moves ctime, but only on a filesystem whose timestamp
                // granularity separates it from the listing.
                if opened.mode != stat.mode {
                    return Err(drift(Some(path), WalkDriftV1::DescentMode));
                }
                if opened.kind != stat.kind
                    || opened.dev != stat.dev
                    || opened.ino != stat.ino
                    || opened.mtime_ns != stat.mtime_ns
                    || opened.ctime_ns != stat.ctime_ns
                {
                    return Err(drift(Some(path), WalkDriftV1::DescentIdentity));
                }
                if let Some(encoder) = encoder {
                    encoder.directory(&path, opened.mode)?;
                }
                self.inventory.fold(RECORD_DIRECTORY_V1, &path, stat);
                let child_length = self.path.len();
                self.enter(DirectoryHandleV1::Child(child), child_length)
                    .map(Some)
            }
        }
    }

    /// Streams one regular file's content (§4.2): open no-follow, prove the opened descriptor is
    /// the listed object, stream exactly its size, and prove nothing changed while it was read.
    fn capture_regular<W: Write>(
        &self,
        directory: &PinnedDirectoryV1,
        entry: &EntryV1,
        path: &CustodyFramePathV1,
        encoder: &mut CustodyFrameEncoderV1<W>,
    ) -> WalkResult<()> {
        #[cfg(test)]
        seam::point(self.pass, &seam::PointV1::BeforeOpen(path.clone()));
        let mut file = directory
            .open_regular_file(entry.name.as_os_str(), LABEL)
            .map_err(|error| self.explain(directory, entry, path, &error))?;
        #[cfg(test)]
        seam::opened(self.pass, path);
        let opened = self.observe(ObservationV1::Opened, Some(path), file_metadata(&file))?;
        if opened != entry.stat {
            return Err(drift(Some(path.clone()), WalkDriftV1::OpenedIdentity));
        }

        #[cfg(test)]
        let mut content = seam::ReadingV1::new(&mut file, self.pass, path);
        #[cfg(not(test))]
        let mut content = &mut file;
        let streamed = encoder.regular_file(path, opened.mode, opened.size, &mut content);

        // A change during the read explains a length refusal too, so it is checked first.
        let after = self.observe(ObservationV1::AfterRead, Some(path), file_metadata(&file))?;
        if after != opened {
            return Err(drift(Some(path.clone()), WalkDriftV1::DuringRead));
        }
        streamed?;
        Ok(())
    }

    /// Reads one symlink's target and binds it to the listed entry (§4.2). A symlink has no
    /// descriptor to pin, so the read is bracketed instead. The target's length must be the listed
    /// size, which is a symlink's target length, and a no-follow re-stat after the read must equal
    /// the listing. A same-length target swapped in and back out within one timestamp granule is
    /// the §1.1 hostile racer.
    fn read_symlink(
        &self,
        directory: &PinnedDirectoryV1,
        entry: &EntryV1,
        path: &CustodyFramePathV1,
    ) -> WalkResult<Vec<u8>> {
        #[cfg(test)]
        seam::point(self.pass, &seam::PointV1::BeforeOpen(path.clone()));
        let target = directory
            .read_child_symlink(entry.name.as_os_str(), MAX_SYMLINK_TARGET_BYTES_V1, LABEL)
            .map_err(|error| match error {
                FsCustodyError::Unsupported(_) => {
                    CustodyWalkErrorV1::Frame(CustodyFrameErrorV1::InvalidSymlinkTarget)
                }
                error => self.explain(directory, entry, path, &error),
            })?;
        #[cfg(test)]
        seam::point(self.pass, &seam::PointV1::TargetRead(path.clone()));
        if target.len() as u64 != entry.stat.size {
            return Err(drift(Some(path.clone()), WalkDriftV1::SymlinkTarget));
        }
        let after = match directory.child_metadata_no_follow(entry.name.as_os_str(), LABEL) {
            Ok(Some(after)) => observed(self.pass, ObservationV1::TargetRead, Some(path), after),
            Ok(None) => return Err(drift(Some(path.clone()), WalkDriftV1::Vanished)),
            Err(error) => return Err(io_refusal(&error)),
        };
        if after != entry.stat {
            return Err(drift(Some(path.clone()), WalkDriftV1::SymlinkTarget));
        }
        Ok(target)
    }

    /// Classifies an open or `readlinkat` refusal. It is drift when the entry is now absent or is
    /// no longer the listed object, and otherwise the I/O refusal it reports.
    fn explain(
        &self,
        directory: &PinnedDirectoryV1,
        entry: &EntryV1,
        path: &CustodyFramePathV1,
        error: &FsCustodyError,
    ) -> CustodyWalkErrorV1 {
        match directory.child_metadata_no_follow(entry.name.as_os_str(), LABEL) {
            Ok(None) => drift(Some(path.clone()), WalkDriftV1::Vanished),
            Ok(Some(now))
                if observed(self.pass, ObservationV1::Recheck, Some(path), now) != entry.stat =>
            {
                drift(Some(path.clone()), WalkDriftV1::Replaced)
            }
            _ => io_refusal(error),
        }
    }

    /// Closes one directory. The emitting pass first runs the post-subtree checks (§4.4).
    fn leave(&mut self, frame: DirectoryFrameV1<'_>) -> WalkResult<()> {
        if self.pass == PassV1::Emit {
            self.verify_subtree(&frame)?;
        }
        #[cfg(test)]
        self.release_names(frame.entries.len());
        Ok(())
    }

    /// Re-lists a directory whose subtree is complete: the same names, each kept child with the
    /// same kind, device, and inode, and the directory's own metadata unchanged (§4.4).
    fn verify_subtree(&mut self, frame: &DirectoryFrameV1<'_>) -> WalkResult<()> {
        let directory_path = self.directory_path(frame.path_length);
        #[cfg(test)]
        seam::point(self.pass, &seam::PointV1::Subtree(directory_path.clone()));

        let mut relisted = frame
            .directory
            .list_child_names(&mut self.relisting_remaining, LABEL)
            .map_err(|error| match error {
                // Every directory re-listed before this one matched its first listing, and the
                // first listings fit the same budget, so running out here means this one grew.
                FsCustodyError::EnumerationLimitExceeded { .. } => {
                    drift(directory_path.clone(), WalkDriftV1::ChildNames)
                }
                error => io_refusal(&error),
            })?;
        relisted.sort();
        #[cfg(test)]
        {
            self.hold_names(relisted.len());
            self.release_names(relisted.len());
        }
        let same_names = relisted.len() == frame.entries.len()
            && relisted
                .iter()
                .zip(&frame.entries)
                .all(|(name, entry)| *name == entry.name);
        if !same_names {
            return Err(drift(directory_path, WalkDriftV1::ChildNames));
        }
        drop(relisted);

        for entry in &frame.entries {
            if entry.decision == KeptV1::Skip {
                continue;
            }
            let path = self.entry_path(frame.path_length, &entry.name)?;
            let now = match frame
                .directory
                .child_metadata_no_follow(entry.name.as_os_str(), LABEL)
            {
                Ok(Some(now)) => observed(self.pass, ObservationV1::Recheck, Some(&path), now),
                Ok(None) => return Err(drift(Some(path), WalkDriftV1::Vanished)),
                Err(error) => return Err(io_refusal(&error)),
            };
            if now.kind != entry.stat.kind || now.dev != entry.stat.dev || now.ino != entry.stat.ino
            {
                return Err(drift(Some(path), WalkDriftV1::ChildIdentity));
            }
        }

        let after = self.observe(
            ObservationV1::DirectoryAfter,
            directory_path.as_ref(),
            frame.directory.current_metadata(LABEL),
        )?;
        let before = frame.before;
        if after.dev != before.dev
            || after.ino != before.ino
            || after.mtime_ns != before.mtime_ns
            || after.ctime_ns != before.ctime_ns
        {
            return Err(drift(directory_path, WalkDriftV1::DirectoryMetadata));
        }
        Ok(())
    }

    #[cfg(test)]
    fn hold_names(&mut self, count: usize) {
        self.live_names += count as u64;
        seam::live_names(self.live_names);
    }

    #[cfg(test)]
    fn release_names(&mut self, count: usize) {
        self.live_names -= count as u64;
    }
}

// ---------------------------------------------------------------------------------------------
// Test seam
// ---------------------------------------------------------------------------------------------

/// The walker's test-only seam. It lets a control change one observation (standing in for a mount
/// or a coarse-timestamp filesystem), change the tree between two steps, reverse `readdir` order,
/// and read back every open and the peak count of live listed names.
#[cfg(test)]
pub(crate) mod seam {
    use super::{ChildBytesV1, ChildStatV1, CustodyFramePathV1, ObservationV1, PassV1};
    use std::cell::RefCell;
    use std::fs::File;
    use std::io::{self, Read};

    /// A point between two walker steps at which a control may change the tree.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(crate) enum PointV1 {
        /// A directory's names were listed; none has been stated yet.
        Listed(Option<CustodyFramePathV1>),
        /// A regular file or a child directory is about to be opened, or a symlink's target read.
        BeforeOpen(CustodyFramePathV1),
        /// A regular file or a child directory was opened and is not yet checked.
        Opened(CustodyFramePathV1),
        /// A symlink's target was read; its re-stat has not run yet.
        TargetRead(CustodyFramePathV1),
        /// A regular file's first content bytes were read.
        Reading(CustodyFramePathV1),
        /// A directory's subtree is complete, and it is about to be re-listed.
        Subtree(Option<CustodyFramePathV1>),
        /// The emitting pass is complete, and the final pass is about to start.
        Verifying,
    }

    pub(crate) type StatHookV1 =
        Box<dyn FnMut(PassV1, ObservationV1, Option<&CustodyFramePathV1>, &mut ChildStatV1)>;
    pub(crate) type PointHookV1 = Box<dyn FnMut(PassV1, &PointV1)>;

    #[derive(Default)]
    pub(crate) struct WalkSeamsV1 {
        pub(crate) stat: Option<StatHookV1>,
        pub(crate) point: Option<PointHookV1>,
        pub(crate) reverse_listings: bool,
    }

    /// What the walker reported through the seam.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub(crate) struct WalkRecordV1 {
        /// Every open, in order: each is one descriptor-relative `openat`.
        pub(crate) opened: Vec<(PassV1, CustodyFramePathV1)>,
        pub(crate) peak_live_names: u64,
    }

    struct StateV1 {
        seams: WalkSeamsV1,
        record: WalkRecordV1,
    }

    thread_local! {
        static STATE: RefCell<Option<StateV1>> = const { RefCell::new(None) };
    }

    /// Uninstalls the seam when dropped.
    pub(crate) struct InstalledV1(());

    impl InstalledV1 {
        pub(crate) fn record(&self) -> WalkRecordV1 {
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

    pub(crate) fn install(seams: WalkSeamsV1) -> InstalledV1 {
        STATE.with(|state| {
            *state.borrow_mut() = Some(StateV1 {
                seams,
                record: WalkRecordV1::default(),
            });
        });
        InstalledV1(())
    }

    fn with_state(update: impl FnOnce(&mut StateV1)) {
        STATE.with(|state| {
            if let Some(state) = state.borrow_mut().as_mut() {
                update(state);
            }
        });
    }

    /// Runs a hook with the state released, so the hook may do anything but reinstall the seam.
    pub(super) fn stat(
        pass: PassV1,
        observation: ObservationV1,
        path: Option<&CustodyFramePathV1>,
        stat: &mut ChildStatV1,
    ) {
        let mut hook = None;
        with_state(|state| hook = state.seams.stat.take());
        if let Some(mut hook) = hook {
            hook(pass, observation, path, stat);
            with_state(|state| state.seams.stat = Some(hook));
        }
    }

    pub(super) fn point(pass: PassV1, point: &PointV1) {
        let mut hook = None;
        with_state(|state| hook = state.seams.point.take());
        if let Some(mut hook) = hook {
            hook(pass, point);
            with_state(|state| state.seams.point = Some(hook));
        }
    }

    pub(super) fn listed(
        pass: PassV1,
        directory: Option<&CustodyFramePathV1>,
        names: &mut [ChildBytesV1],
    ) {
        let mut reverse = false;
        with_state(|state| reverse = state.seams.reverse_listings);
        if reverse {
            names.reverse();
        }
        point(pass, &PointV1::Listed(directory.cloned()));
    }

    pub(super) fn opened(pass: PassV1, path: &CustodyFramePathV1) {
        with_state(|state| state.record.opened.push((pass, path.clone())));
        point(pass, &PointV1::Opened(path.clone()));
    }

    pub(super) fn live_names(live: u64) {
        with_state(|state| {
            state.record.peak_live_names = state.record.peak_live_names.max(live);
        });
    }

    /// A regular file's content reader that fires [`PointV1::Reading`] once, after the first read
    /// that returns bytes.
    pub(crate) struct ReadingV1<'f> {
        file: &'f mut File,
        pass: PassV1,
        pending: Option<CustodyFramePathV1>,
    }

    impl<'f> ReadingV1<'f> {
        pub(super) fn new(file: &'f mut File, pass: PassV1, path: &CustodyFramePathV1) -> Self {
            Self {
                file,
                pass,
                pending: Some(path.clone()),
            }
        }
    }

    impl Read for ReadingV1<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let read = self.file.read(buffer)?;
            if read != 0 {
                if let Some(path) = self.pending.take() {
                    point(self.pass, &PointV1::Reading(path));
                }
            }
            Ok(read)
        }
    }
}

#[cfg(test)]
#[path = "custody_walk_tests.rs"]
mod tests;
