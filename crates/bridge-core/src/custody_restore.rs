//! The ADR-0041 capsule reader, restore phase V: verify and stage (slice 2B3a).
//!
//! [`verify_and_stage_v1`] pins a sealed local capsule and a new, empty, owner-private
//! destination. Before anything is materialized, it:
//! 1. verifies every ciphertext's exact length and SHA-256 against the seal (task 3);
//! 2. preflights the destination and creates its `.restore-work/plain/` staging tree (task 4);
//! 3. opens only the three control artifacts, and binds the manifest, the capsule index, and the
//!    restore policy through 2B1 (task 5);
//! 4. only then opens the Git pack and every coverage payload, and fully decode-verifies every
//!    coverage frame (task 6).
//!
//! **The three-tier timing contract.** The seal has no independently supplied expected digest in
//! 2B, and the controls are themselves encrypted artifacts, so tampering is refused in three tiers:
//! - **T1**, a malformed or out-of-bounds seal, and **T2**, a seal that disagrees with a
//!   ciphertext, refuse in step 1, before `.restore-work/` exists;
//! - **T3**, a seal that disagrees with the controls, refuses in step 3, with only the three
//!   control plaintexts staged and no pack or payload plaintext.
//!
//! **Effects.** The capsule is read-only input: every read goes through a retained no-follow
//! descriptor, and nothing in it is written. Every write is create-new beneath the retained
//! destination descriptor, under `.restore-work/` only. Nothing is ever overwritten, followed,
//! or deleted, not even on failure. No Git command runs, and nothing is decoded into the
//! destination tree.

use crate::custody_capsule::{
    sealed, CustodyCapsuleArtifactRoleRowV1, CustodyCapsuleArtifactRoleV1, CustodyCapsuleBindingV1,
    CustodyCapsuleErrorV1, CustodyCapsuleIndexV1, CustodyCapsuleSealProofV1,
    CustodyEnvelopeChunkSinkV1, CustodyEnvelopeChunkSourceV1, CustodyEnvelopeChunkV1,
    CustodyEnvelopeOpenRequestV1, CustodyEnvelopeOpenerV1, CustodyEnvelopeSinkValidatorV1,
    CustodyEnvelopeSourceDescriptorV1, CustodyEnvelopeSourceValidatorV1,
    CustodyEnvelopeStreamLimitsV1, CustodyEnvelopeStreamReceiptV1, CustodyRestorePolicyV1,
};
use crate::custody_export::{preflight_scratch_root, ScratchLedgerV1};
use crate::custody_frame::{
    CustodyFrameBudgetV1, CustodyFrameDecoderV1, CustodyFrameEntryV1, CustodyFrameHeaderV1,
};
use crate::custody_inventory::LosslessPathV1;
use crate::custody_mounts::{mount_point_within, mount_points_v1};
use crate::custody_seal::{CustodyCoverageClassV1, CustodyManifestV1, CustodySealV1};
use crate::execution_policy::Sha256HexV1;
use crate::fs_custody::{pinned_root_unchanged, ChildKindV1, FsCustodyError, PinnedDirectoryV1};
use ring::digest;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek as _, SeekFrom, Write as _};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::FileExt as _;
use std::path::Path;
use std::rc::Rc;

// ---------------------------------------------------------------------------------------------
// V1 names and bounds
// ---------------------------------------------------------------------------------------------

/// The published seal's name at the capsule root.
pub(crate) const RESTORE_SEAL_NAME_V1: &str = "custody-seal.v1";
/// The seal file and each control plaintext are read through this bound: the 2B1 canonical
/// metadata ceiling, 1 MiB. One byte more refuses.
pub(crate) const RESTORE_CONTROL_READ_BOUND_V1: u64 = 1024 * 1024;
/// One entry budget shared by every listing of one capsule.
pub(crate) const RESTORE_CAPSULE_ENTRY_BUDGET_V1: u64 = 4096;
/// The restore-wide ledger's ceiling: 10 GiB, as 2B2 §3's scratch ledger.
pub(crate) const RESTORE_BYTES_CEILING_V1: u64 = 10 * 1024 * 1024 * 1024;

/// The staging tree: `<dest>/.restore-work/plain/`.
const RESTORE_WORK_DIR_NAME_V1: &str = ".restore-work";
const RESTORE_PLAIN_DIR_NAME_V1: &str = "plain";

/// The ciphertext is read, and the plaintext accepted, in chunks of at most this size, and at most
/// this many chunks: the 2B1 envelope stream ceilings.
const RESTORE_CHUNK_BYTES_V1: u64 = 1024 * 1024;
const RESTORE_MAX_CHUNKS_V1: u32 = 16_384;

/// The three control artifacts, in the order they are staged and decoded.
const RESTORE_MANIFEST_NAME_V1: &str = "control/manifest.json.enc";
const RESTORE_INDEX_NAME_V1: &str = "control/capsule-index.json.enc";
const RESTORE_POLICY_NAME_V1: &str = "control/restore-policy.json.enc";

/// The capsule stream is hashed, and a staged frame decoded, through a fixed buffer of this size;
/// neither is ever buffered whole.
const HASH_BUFFER_BYTES_V1: usize = 64 * 1024;

// ---------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------

/// One variant per refusal. A logical name is rendered lossily for the message only.
#[derive(Debug, thiserror::Error)]
pub(crate) enum CustodyRestoreErrorV1 {
    // Task 3: the capsule, the seal, and the ciphertexts.
    #[error("the capsule seal cannot be read: {0}")]
    SealUnreadable(String),
    #[error("the capsule seal is invalid: {0}")]
    SealInvalid(String),
    #[error("the capsule holds an entry the seal does not name: {name}")]
    CapsuleEntryUnexpected { name: String },
    #[error("the capsule lacks an entry the seal names: {name}")]
    CapsuleEntryMissing { name: String },
    #[error("a ciphertext's length is not its sealed length: {name}")]
    CiphertextLength { name: String },
    #[error("a ciphertext's SHA-256 is not its sealed digest: {name}")]
    CiphertextDigest { name: String },
    #[error("a symlink was refused and not followed: {name}")]
    SymlinkRefused { name: String },
    // Task 4: the destination.
    #[error("the restore destination is refused: {0}")]
    DestinationInvalid(String),
    #[error("a pinned restore identity changed: {0}")]
    IdentityChanged(String),
    #[error("the restore ledger refused: {0}")]
    Budget(String),
    #[error("a created restore directory is on another device: {name}")]
    DeviceCrossing { name: String },
    #[error("a mount point lies inside the restore destination: {0}")]
    MountBoundary(String),
    // Task 5: opening and binding.
    #[error("the envelope opener refused: {0}")]
    Open(CustodyCapsuleErrorV1),
    #[error("the opener's receipt disagrees with the plaintext it wrote: {name}")]
    OpenerReceiptMismatch { name: String },
    #[error("the consumed ciphertext is not the verified ciphertext: {name}")]
    CiphertextChanged { name: String },
    #[error("a staging entry already exists: {name}")]
    StagingCollision { name: String },
    #[error("a control plaintext exceeds its read bound: {name}")]
    ControlOversize { name: String },
    #[error("a control plaintext does not decode: {name}")]
    ControlDecode { name: String },
    #[error("the 2B1 capsule binding refused: {0}")]
    Binding(CustodyCapsuleErrorV1),
    // Task 6: frames.
    #[error("a coverage frame does not decode and verify: {class:?}")]
    FrameInvalid { class: CustodyCoverageClassV1 },
    #[error("restore io: {0}")]
    Io(String),
}

fn lossy(name: &[u8]) -> String {
    String::from_utf8_lossy(name).into_owned()
}

/// `parent/child`, or `child` at the root.
fn joined(parent: &[u8], child: &[u8]) -> Vec<u8> {
    let mut key = parent.to_vec();
    if !key.is_empty() {
        key.push(b'/');
    }
    key.extend_from_slice(child);
    key
}

/// A no-follow open that met a symlink fails `ELOOP`; that is a refused link, never an I/O fault.
fn refused_link(error: &FsCustodyError) -> bool {
    matches!(error, FsCustodyError::Io(_, source) if source.raw_os_error() == Some(libc::ELOOP))
}

// ---------------------------------------------------------------------------------------------
// The bounded reader (tasks 3 and 5)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BoundedReadErrorV1 {
    /// `bound + 1` bytes were read.
    Oversize,
    Io(io::ErrorKind),
}

/// Reads all of `reader` if it holds at most `bound` bytes. It never requests a byte past
/// `bound + 1` and never allocates more than `bound + 1` bytes, so an oversize input is refused
/// at the bound, whatever its size.
pub(crate) fn read_bounded_v1(
    reader: &mut dyn Read,
    bound: u64,
) -> Result<Vec<u8>, BoundedReadErrorV1> {
    let limit = bound
        .checked_add(1)
        .and_then(|limit| usize::try_from(limit).ok())
        .ok_or(BoundedReadErrorV1::Io(io::ErrorKind::InvalidInput))?;
    let mut buffer = vec![0_u8; limit];
    let mut filled = 0_usize;
    while filled < limit {
        match reader.read(&mut buffer[filled..]) {
            Ok(0) => {
                buffer.truncate(filled);
                return Ok(buffer);
            }
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(BoundedReadErrorV1::Io(error.kind())),
        }
    }
    Err(BoundedReadErrorV1::Oversize)
}

// ---------------------------------------------------------------------------------------------
// Test seams
// ---------------------------------------------------------------------------------------------

/// Where a test hook runs. Each hook receives the path it concerns.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RestoreHookPointV1 {
    /// Every ciphertext is verified and retained (the capsule root).
    AfterCiphertextVerification,
    /// The destination is preflighted and censused, before its pin recheck and first create (the
    /// destination root).
    BeforeFirstCreate,
    /// A staged file's parents exist and are rechecked, before its create-new (the file's path).
    BeforeLeafCreate,
}

#[cfg(test)]
type RestoreHookV1 = Box<dyn Fn(&Path)>;

#[cfg(test)]
thread_local! {
    static HOOKS: RefCell<BTreeMap<RestoreHookPointV1, RestoreHookV1>> =
        RefCell::new(BTreeMap::new());
    /// A device reported for a created directory, by its destination-relative name.
    static CREATED_DEV: RefCell<BTreeMap<Vec<u8>, u64>> = const { RefCell::new(BTreeMap::new()) };
    /// A smaller ciphertext chunk, so a small capsule's artifacts span many chunks.
    static CHUNK_BYTES: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
enum RestoreSeamV1 {
    Hook(RestoreHookPointV1),
    CreatedDev(Vec<u8>),
    ChunkBytes,
}

/// Uninstalls its one seam when dropped.
#[cfg(test)]
pub(crate) struct RestoreSeamGuardV1(RestoreSeamV1);

#[cfg(test)]
impl Drop for RestoreSeamGuardV1 {
    fn drop(&mut self) {
        match &self.0 {
            RestoreSeamV1::Hook(point) => {
                HOOKS.with(|hooks| hooks.borrow_mut().remove(point));
            }
            RestoreSeamV1::CreatedDev(name) => {
                CREATED_DEV.with(|devices| devices.borrow_mut().remove(name));
            }
            RestoreSeamV1::ChunkBytes => CHUNK_BYTES.with(|chunk| chunk.set(None)),
        }
    }
}

#[cfg(test)]
pub(crate) fn install_restore_hook_for_test(
    point: RestoreHookPointV1,
    hook: impl Fn(&Path) + 'static,
) -> RestoreSeamGuardV1 {
    HOOKS.with(|hooks| hooks.borrow_mut().insert(point, Box::new(hook)));
    RestoreSeamGuardV1(RestoreSeamV1::Hook(point))
}

/// Reports `dev` for the directory the restore creates at `name` (relative to the destination),
/// in place of the device its retained descriptor reports.
#[cfg(test)]
pub(crate) fn override_created_dev_for_test(name: &str, dev: u64) -> RestoreSeamGuardV1 {
    let name = name.as_bytes().to_vec();
    CREATED_DEV.with(|devices| devices.borrow_mut().insert(name.clone(), dev));
    RestoreSeamGuardV1(RestoreSeamV1::CreatedDev(name))
}

/// Reads every ciphertext in chunks of `bytes` instead of [`RESTORE_CHUNK_BYTES_V1`].
#[cfg(test)]
pub(crate) fn override_chunk_bytes_for_test(bytes: u64) -> RestoreSeamGuardV1 {
    CHUNK_BYTES.with(|chunk| chunk.set(Some(bytes)));
    RestoreSeamGuardV1(RestoreSeamV1::ChunkBytes)
}

/// Review focus 6's seam: runs once every ciphertext is verified and its descriptor retained.
#[cfg(test)]
pub(crate) fn after_ciphertext_verification_for_test(
    hook: impl Fn(&Path) + 'static,
) -> RestoreSeamGuardV1 {
    install_restore_hook_for_test(RestoreHookPointV1::AfterCiphertextVerification, hook)
}

#[cfg(test)]
fn run_hook(point: RestoreHookPointV1, path: &Path) {
    HOOKS.with(|hooks| {
        if let Some(hook) = hooks.borrow().get(&point) {
            hook(path);
        }
    });
}

// ---------------------------------------------------------------------------------------------
// Task 3: pin the capsule, read the seal, and verify every ciphertext
// ---------------------------------------------------------------------------------------------

/// A pinned capsule whose every ciphertext equals its seal row.
#[derive(Debug)]
pub(crate) struct PinnedCapsuleV1 {
    root: PinnedDirectoryV1,
    seal: CustodyCapsuleSealProofV1,
    /// One per seal artifact, in the seal's (canonical, name-sorted) order.
    ciphertexts: Vec<VerifiedCiphertextV1>,
}

impl PinnedCapsuleV1 {
    pub(crate) fn seal(&self) -> &CustodyCapsuleSealProofV1 {
        &self.seal
    }

    fn ciphertext(&self, name: &LosslessPathV1) -> Option<&VerifiedCiphertextV1> {
        self.ciphertexts
            .iter()
            .find(|ciphertext| ciphertext.name.as_bytes() == name.as_bytes())
    }
}

/// One verified ciphertext: its retained no-follow descriptor, rewound, and its sealed length.
#[derive(Debug)]
pub(crate) struct VerifiedCiphertextV1 {
    pub(crate) name: LosslessPathV1,
    pub(crate) file: File,
    pub(crate) length: u64,
}

/// What an exterior entry must be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExpectedKindV1 {
    Directory,
    Regular,
}

/// The exact exterior the seal names: every directory, by its relative key (`""` is the root),
/// and exactly its children. The root also holds the seal itself.
type ExteriorV1 = BTreeMap<Vec<u8>, BTreeMap<Vec<u8>, ExpectedKindV1>>;

fn expected_exterior(seal: &CustodySealV1) -> Result<ExteriorV1, CustodyRestoreErrorV1> {
    let mut exterior = ExteriorV1::new();
    exterior.entry(Vec::new()).or_default().insert(
        RESTORE_SEAL_NAME_V1.as_bytes().to_vec(),
        ExpectedKindV1::Regular,
    );
    for artifact in seal.artifacts() {
        let components: Vec<&[u8]> = artifact
            .name()
            .as_bytes()
            .split(|byte| *byte == b'/')
            .collect();
        let mut key = Vec::new();
        for (index, component) in components.iter().enumerate() {
            let kind = if index + 1 == components.len() {
                ExpectedKindV1::Regular
            } else {
                ExpectedKindV1::Directory
            };
            let previous = exterior
                .entry(key.clone())
                .or_default()
                .insert(component.to_vec(), kind);
            if previous.is_some_and(|previous| previous != kind || kind == ExpectedKindV1::Regular)
            {
                return Err(CustodyRestoreErrorV1::SealInvalid(format!(
                    "the sealed name {} collides with another capsule entry",
                    lossy(artifact.name().as_bytes())
                )));
            }
            key = joined(&key, component);
        }
    }
    Ok(exterior)
}

/// Task 3: pin the capsule root, read and validate the seal through the bounded reader, require
/// the capsule's exterior to be exactly the seal's artifacts plus the seal, and verify every
/// ciphertext's length and SHA-256 through its retained descriptor. Nothing is opened, and
/// nothing is written.
pub(crate) fn pin_and_verify_capsule_v1(
    capsule_root: &Path,
) -> Result<PinnedCapsuleV1, CustodyRestoreErrorV1> {
    let io = |error: FsCustodyError| CustodyRestoreErrorV1::Io(error.to_string());
    let root = PinnedDirectoryV1::open(capsule_root, "custody restore capsule root").map_err(io)?;
    let mut remaining = RESTORE_CAPSULE_ENTRY_BUDGET_V1;

    // Step 2: the seal, through the bounded reader, then the canonical decode and the seal proof.
    let seal_name = OsStr::new(RESTORE_SEAL_NAME_V1);
    match root
        .child_metadata_no_follow(seal_name, "custody restore seal")
        .map_err(io)?
    {
        None => {
            return Err(CustodyRestoreErrorV1::CapsuleEntryMissing {
                name: RESTORE_SEAL_NAME_V1.to_owned(),
            })
        }
        Some(stat) if stat.kind == ChildKindV1::Symlink => {
            return Err(CustodyRestoreErrorV1::SymlinkRefused {
                name: RESTORE_SEAL_NAME_V1.to_owned(),
            })
        }
        Some(_) => {}
    }
    let mut seal_file = root
        .open_regular_file(seal_name, "custody restore seal")
        .map_err(|error| {
            if refused_link(&error) {
                CustodyRestoreErrorV1::SymlinkRefused {
                    name: RESTORE_SEAL_NAME_V1.to_owned(),
                }
            } else {
                CustodyRestoreErrorV1::SealUnreadable(error.to_string())
            }
        })?;
    let seal_bytes =
        read_bounded_v1(&mut seal_file, RESTORE_CONTROL_READ_BOUND_V1).map_err(|error| {
            CustodyRestoreErrorV1::SealUnreadable(match error {
                BoundedReadErrorV1::Oversize => {
                    format!("the seal exceeds its {RESTORE_CONTROL_READ_BOUND_V1}-byte read bound")
                }
                BoundedReadErrorV1::Io(kind) => format!("the seal read failed: {kind}"),
            })
        })?;
    let seal = CustodySealV1::decode_canonical(&seal_bytes)
        .map_err(|error| CustodyRestoreErrorV1::SealInvalid(error.to_string()))?;
    let seal = CustodyCapsuleSealProofV1::from_published_seal_v1(seal)
        .map_err(|error| CustodyRestoreErrorV1::SealInvalid(error.to_string()))?;

    // Step 3: the exterior is exactly the seal's names, reached component by component through
    // no-follow opens.
    let exterior = expected_exterior(seal.seal())?;
    let mut directories: BTreeMap<Vec<u8>, PinnedDirectoryV1> = BTreeMap::new();
    let mut files: BTreeMap<Vec<u8>, File> = BTreeMap::new();
    // Keys sort parents first, so every directory is pinned before it is listed.
    for (key, children) in &exterior {
        let mut pinned = Vec::new();
        let directory = if key.is_empty() {
            &root
        } else {
            directories.get(key).ok_or_else(|| {
                CustodyRestoreErrorV1::Io(format!("{}: an unpinned capsule directory", lossy(key)))
            })?
        };
        let listed = directory
            .list_child_names(&mut remaining, "custody restore capsule listing")
            .map_err(io)?;
        if let Some(extra) = listed
            .iter()
            .find(|name| !children.contains_key(name.as_bytes()))
        {
            return Err(CustodyRestoreErrorV1::CapsuleEntryUnexpected {
                name: lossy(&joined(key, extra.as_bytes())),
            });
        }
        for (child, kind) in children {
            let name = joined(key, child);
            let missing = || CustodyRestoreErrorV1::CapsuleEntryMissing { name: lossy(&name) };
            if !listed
                .iter()
                .any(|listed| listed.as_bytes() == child.as_slice())
            {
                return Err(missing());
            }
            let stat = directory
                .child_metadata_no_follow(OsStr::from_bytes(child), "custody restore capsule entry")
                .map_err(io)?
                .ok_or_else(missing)?;
            let refuse_link = |error: FsCustodyError| {
                if refused_link(&error) {
                    CustodyRestoreErrorV1::SymlinkRefused { name: lossy(&name) }
                } else {
                    io(error)
                }
            };
            match (stat.kind, kind) {
                (ChildKindV1::Symlink, _) => {
                    return Err(CustodyRestoreErrorV1::SymlinkRefused { name: lossy(&name) })
                }
                (ChildKindV1::Directory, ExpectedKindV1::Directory) => {
                    let pin = directory
                        .open_existing_child_directory(
                            OsStr::from_bytes(child),
                            "custody restore capsule directory",
                        )
                        .map_err(refuse_link)?;
                    pinned.push((name, pin));
                }
                (ChildKindV1::Regular, ExpectedKindV1::Regular) => {
                    // The seal itself was opened and read in step 2.
                    if key.is_empty() && child.as_slice() == RESTORE_SEAL_NAME_V1.as_bytes() {
                        continue;
                    }
                    let file = directory
                        .open_regular_file(OsStr::from_bytes(child), "custody restore ciphertext")
                        .map_err(refuse_link)?;
                    files.insert(name, file);
                }
                _ => {
                    return Err(CustodyRestoreErrorV1::CapsuleEntryUnexpected {
                        name: lossy(&name),
                    })
                }
            }
        }
        directories.extend(pinned);
    }

    // Step 4: every ciphertext's length and SHA-256, from its retained descriptor, before any is
    // opened.
    let mut ciphertexts = Vec::with_capacity(seal.seal().artifacts().len());
    for artifact in seal.seal().artifacts() {
        let name = artifact.name().as_bytes();
        let mut file = files
            .remove(name)
            .ok_or_else(|| CustodyRestoreErrorV1::CapsuleEntryMissing { name: lossy(name) })?;
        let (length, sha256) = hash_prefix(&mut file, artifact.byte_length())
            .map_err(|error| CustodyRestoreErrorV1::Io(format!("{}: {error}", lossy(name))))?;
        if length != artifact.byte_length() {
            return Err(CustodyRestoreErrorV1::CiphertextLength { name: lossy(name) });
        }
        if sha256_hex(&sha256) != *artifact.sha256() {
            return Err(CustodyRestoreErrorV1::CiphertextDigest { name: lossy(name) });
        }
        file.seek(SeekFrom::Start(0))
            .map_err(|error| CustodyRestoreErrorV1::Io(format!("{}: {error}", lossy(name))))?;
        ciphertexts.push(VerifiedCiphertextV1 {
            name: artifact.name().clone(),
            file,
            length,
        });
    }
    #[cfg(test)]
    run_hook(
        RestoreHookPointV1::AfterCiphertextVerification,
        root.canonical_path(),
    );
    Ok(PinnedCapsuleV1 {
        root,
        seal,
        ciphertexts,
    })
}

// ---------------------------------------------------------------------------------------------
// Task 4: destination preflight, the work tree, and the ledger
// ---------------------------------------------------------------------------------------------

/// The restore-wide ledger's limit, validated against [`RESTORE_BYTES_CEILING_V1`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CustodyRestoreBudgetV1 {
    max_restore_bytes: u64,
}

impl CustodyRestoreBudgetV1 {
    /// Refuses 0 and anything above [`RESTORE_BYTES_CEILING_V1`].
    pub(crate) fn new(max_restore_bytes: u64) -> Result<Self, CustodyRestoreErrorV1> {
        if max_restore_bytes == 0 || max_restore_bytes > RESTORE_BYTES_CEILING_V1 {
            return Err(CustodyRestoreErrorV1::Budget(format!(
                "a restore budget of {max_restore_bytes} bytes is outside 1..={RESTORE_BYTES_CEILING_V1}"
            )));
        }
        Ok(Self { max_restore_bytes })
    }

    /// Exactly [`RESTORE_BYTES_CEILING_V1`].
    pub(crate) const fn ceiling() -> Self {
        Self {
            max_restore_bytes: RESTORE_BYTES_CEILING_V1,
        }
    }

    pub(crate) const fn max_restore_bytes(&self) -> u64 {
        self.max_restore_bytes
    }
}

/// A preflighted destination and its staging tree. Every directory the restore creates is pinned
/// by the descriptor it was created through, and on the destination root's device.
#[derive(Debug)]
pub(crate) struct RestoreDestinationV1 {
    root: PinnedDirectoryV1,
    work: PinnedDirectoryV1,
    plain: PinnedDirectoryV1,
    /// The one restore-wide ledger: every staged byte and entry is reserved before it is written.
    ledger: RefCell<ScratchLedgerV1>,
    /// The destination root's `dev`, recorded at preflight.
    dev: u64,
    /// Every staging directory created beneath `plain/`, by its `plain/`-relative key. A later
    /// artifact reuses only this retained pin, and never re-opens the directory by name.
    staging: RefCell<BTreeMap<Vec<u8>, Rc<PinnedDirectoryV1>>>,
}

impl RestoreDestinationV1 {
    /// The mount census again: no mount point may lie strictly inside the destination.
    pub(crate) fn recheck_containment(&self) -> Result<(), CustodyRestoreErrorV1> {
        refuse_mounts_within(&self.root)
    }

    /// The retained pin of the staging directory at `key` (`plain/` itself for an empty key),
    /// creating each missing component create-new. Before each create, the chain of retained pins
    /// down to its parent is rechecked.
    fn staging_directory(
        &self,
        components: &[&[u8]],
    ) -> Result<Option<Rc<PinnedDirectoryV1>>, CustodyRestoreErrorV1> {
        let mut key = Vec::new();
        let mut parent: Option<Rc<PinnedDirectoryV1>> = None;
        for component in components {
            let child_key = joined(&key, component);
            let retained = self.staging.borrow().get(&child_key).cloned();
            let pin = match retained {
                Some(pin) => pin,
                None => {
                    self.recheck_staging_chain(&key)?;
                    let parent_pin = parent.as_deref().unwrap_or(&self.plain);
                    let created = create_staging_directory(
                        parent_pin,
                        &self.ledger,
                        self.dev,
                        component,
                        &plain_relative(&child_key),
                    )?;
                    let created = Rc::new(created);
                    self.staging
                        .borrow_mut()
                        .insert(child_key.clone(), Rc::clone(&created));
                    created
                }
            };
            parent = Some(pin);
            key = child_key;
        }
        Ok(parent)
    }

    /// Requires `.restore-work/`, `plain/`, and every retained staging directory down to `key` to
    /// still be the entry its parent holds: a directory with the retained `dev` and `ino`. Each
    /// retained `dev` was compared with the root's when the directory was created.
    fn recheck_staging_chain(&self, key: &[u8]) -> Result<(), CustodyRestoreErrorV1> {
        recheck_entry(
            &self.root,
            RESTORE_WORK_DIR_NAME_V1.as_bytes(),
            &self.work,
            RESTORE_WORK_DIR_NAME_V1.as_bytes(),
        )?;
        recheck_entry(
            &self.work,
            RESTORE_PLAIN_DIR_NAME_V1.as_bytes(),
            &self.plain,
            &plain_relative(b""),
        )?;
        if key.is_empty() {
            return Ok(());
        }
        let staging = self.staging.borrow();
        let mut parent: &PinnedDirectoryV1 = &self.plain;
        let mut prefix = Vec::new();
        for component in key.split(|byte| *byte == b'/') {
            prefix = joined(&prefix, component);
            let pin = staging.get(&prefix).ok_or_else(|| {
                CustodyRestoreErrorV1::IdentityChanged(format!(
                    "{}: a staging directory is not retained",
                    lossy(&plain_relative(&prefix))
                ))
            })?;
            recheck_entry(parent, component, pin, &plain_relative(&prefix))?;
            parent = pin;
        }
        Ok(())
    }
}

/// `parent`'s entry `component` must still be `pin`: a directory with its `dev` and `ino`.
fn recheck_entry(
    parent: &PinnedDirectoryV1,
    component: &[u8],
    pin: &PinnedDirectoryV1,
    name: &[u8],
) -> Result<(), CustodyRestoreErrorV1> {
    let changed =
        |detail: &str| CustodyRestoreErrorV1::IdentityChanged(format!("{}: {detail}", lossy(name)));
    let entry = parent
        .child_metadata_no_follow(
            OsStr::from_bytes(component),
            "custody restore staging recheck",
        )
        .map_err(|error| CustodyRestoreErrorV1::Io(error.to_string()))?
        .ok_or_else(|| changed("the retained staging directory's entry is gone"))?;
    if entry.kind != ChildKindV1::Directory
        || Some(entry.dev) != pin.identity().dev
        || Some(entry.ino) != pin.identity().ino
    {
        return Err(changed(
            "the entry is no longer the retained staging directory",
        ));
    }
    Ok(())
}

/// `.restore-work/plain/<key>`, the destination-relative name of a staging entry.
fn plain_relative(key: &[u8]) -> Vec<u8> {
    let plain = joined(
        RESTORE_WORK_DIR_NAME_V1.as_bytes(),
        RESTORE_PLAIN_DIR_NAME_V1.as_bytes(),
    );
    if key.is_empty() {
        plain
    } else {
        joined(&plain, key)
    }
}

/// Task 4: preflight `destination_root` as 2B2 preflights a scratch root (owner-private, empty,
/// and disjoint from the pinned capsule in both directions, by path and by retained identity),
/// take the mount census, recheck the destination pin, and create `.restore-work/plain/`
/// create-new, charging the ledger for each entry before it is created.
pub(crate) fn prepare_destination_v1(
    destination_root: &Path,
    capsule: &PinnedCapsuleV1,
    budget: CustodyRestoreBudgetV1,
) -> Result<RestoreDestinationV1, CustodyRestoreErrorV1> {
    let root = preflight_scratch_root(destination_root, &[&capsule.root])
        .map_err(|error| CustodyRestoreErrorV1::DestinationInvalid(error.to_string()))?;
    let dev = root.identity().dev.ok_or_else(|| {
        CustodyRestoreErrorV1::DestinationInvalid(
            "the destination's retained device is unavailable".into(),
        )
    })?;
    refuse_mounts_within(&root)?;

    #[cfg(test)]
    run_hook(RestoreHookPointV1::BeforeFirstCreate, root.canonical_path());
    pinned_root_unchanged(&root).map_err(CustodyRestoreErrorV1::IdentityChanged)?;

    let ledger = RefCell::new(ScratchLedgerV1::new(budget.max_restore_bytes()));
    let work = create_staging_directory(
        &root,
        &ledger,
        dev,
        RESTORE_WORK_DIR_NAME_V1.as_bytes(),
        RESTORE_WORK_DIR_NAME_V1.as_bytes(),
    )?;
    let plain_name = joined(
        RESTORE_WORK_DIR_NAME_V1.as_bytes(),
        RESTORE_PLAIN_DIR_NAME_V1.as_bytes(),
    );
    let plain = create_staging_directory(
        &work,
        &ledger,
        dev,
        RESTORE_PLAIN_DIR_NAME_V1.as_bytes(),
        &plain_name,
    )?;
    Ok(RestoreDestinationV1 {
        root,
        work,
        plain,
        ledger,
        dev,
        staging: RefCell::new(BTreeMap::new()),
    })
}

/// `MountBoundary` if any mount point lies strictly inside `root`'s canonical path. A census that
/// cannot list the mount points also refuses: the restore cannot prove none hides there.
fn refuse_mounts_within(root: &PinnedDirectoryV1) -> Result<(), CustodyRestoreErrorV1> {
    let mount_points = mount_points_v1().map_err(|error| {
        CustodyRestoreErrorV1::MountBoundary(format!("the mount census failed: {error}"))
    })?;
    let root = root.canonical_path().as_os_str().as_bytes();
    match mount_points
        .iter()
        .find(|mount_point| mount_point_within(mount_point, root))
    {
        Some(mount_point) => Err(CustodyRestoreErrorV1::MountBoundary(format!(
            "the mount point {} lies inside the destination {}",
            lossy(mount_point),
            lossy(root)
        ))),
        None => Ok(()),
    }
}

/// Creates the directory `leaf` beneath `parent` create-new, after reserving its entry, and
/// requires it to be on the destination root's device. `name` is its destination-relative name.
fn create_staging_directory(
    parent: &PinnedDirectoryV1,
    ledger: &RefCell<ScratchLedgerV1>,
    dev: u64,
    leaf: &[u8],
    name: &[u8],
) -> Result<PinnedDirectoryV1, CustodyRestoreErrorV1> {
    ledger
        .borrow_mut()
        .reserve_entries(1)
        .map_err(|error| CustodyRestoreErrorV1::Budget(error.to_string()))?;
    let created = parent
        .create_new_child_directory(OsStr::from_bytes(leaf), "custody restore staging directory")
        .map_err(|error| staging_refusal(error, name))?;
    let observed = created
        .current_metadata("custody restore staging directory")
        .map_err(|error| CustodyRestoreErrorV1::Io(error.to_string()))?
        .dev;
    #[cfg(test)]
    let observed = CREATED_DEV
        .with(|devices| devices.borrow().get(name).copied())
        .unwrap_or(observed);
    if observed != dev {
        return Err(CustodyRestoreErrorV1::DeviceCrossing { name: lossy(name) });
    }
    Ok(created)
}

/// A create-new that met an existing entry (a planted file, directory, or symlink) is a collision;
/// the existing object is left untouched.
fn staging_refusal(error: FsCustodyError, name: &[u8]) -> CustodyRestoreErrorV1 {
    match error {
        FsCustodyError::TargetExists(_) => {
            CustodyRestoreErrorV1::StagingCollision { name: lossy(name) }
        }
        FsCustodyError::IdentityChanged(detail) => CustodyRestoreErrorV1::IdentityChanged(detail),
        other => CustodyRestoreErrorV1::Io(other.to_string()),
    }
}

// ---------------------------------------------------------------------------------------------
// Task 5: open the control artifacts and bind through 2B1
// ---------------------------------------------------------------------------------------------

/// One staged plaintext: its retained descriptor, rewound, and its measured length and SHA-256.
#[derive(Debug)]
pub(crate) struct StagedPlaintextV1 {
    pub(crate) name: LosslessPathV1,
    pub(crate) role: CustodyCapsuleArtifactRoleV1,
    pub(crate) file: File,
    pub(crate) length: u64,
    pub(crate) sha256: [u8; 32],
}

/// The decoded controls and their 2B1 binding: three-way digest agreement, the exact artifact
/// mapping, and the closed inert policy.
#[derive(Debug)]
pub(crate) struct BoundControlV1 {
    pub(crate) manifest: CustodyManifestV1,
    pub(crate) index: CustodyCapsuleIndexV1,
    pub(crate) policy: CustodyRestorePolicyV1,
    pub(crate) binding: CustodyCapsuleBindingV1,
}

fn chunk_bytes() -> u64 {
    #[cfg(test)]
    if let Some(bytes) = CHUNK_BYTES.with(std::cell::Cell::get) {
        return bytes;
    }
    RESTORE_CHUNK_BYTES_V1
}

fn stream_limits() -> Result<CustodyEnvelopeStreamLimitsV1, CustodyRestoreErrorV1> {
    let chunk = chunk_bytes();
    let capacity = chunk.saturating_mul(u64::from(RESTORE_MAX_CHUNKS_V1));
    CustodyEnvelopeStreamLimitsV1::new(
        capacity.min(RESTORE_BYTES_CEILING_V1),
        chunk,
        RESTORE_MAX_CHUNKS_V1,
    )
    .map_err(CustodyRestoreErrorV1::Open)
}

/// The role a reserved 2B1 capsule name carries, if any.
fn reserved_role(name: &LosslessPathV1) -> Option<CustodyCapsuleArtifactRoleV1> {
    let mut roles = vec![
        CustodyCapsuleArtifactRoleV1::Manifest,
        CustodyCapsuleArtifactRoleV1::CapsuleIndex,
        CustodyCapsuleArtifactRoleV1::RestorePolicy,
        CustodyCapsuleArtifactRoleV1::GitObjectPack,
    ];
    roles.extend(
        CustodyCoverageClassV1::ALL
            .into_iter()
            .filter(|class| *class != CustodyCoverageClassV1::ObjectDatabase)
            .map(CustodyCapsuleArtifactRoleV1::CoveragePayload),
    );
    roles
        .into_iter()
        .find(|role| CustodyCapsuleArtifactRoleRowV1::new(name.clone(), role.clone()).is_ok())
}

/// The retained, verified ciphertext as the opener's chunk source. It reads the descriptor at
/// explicit offsets from 0, in bounded chunks, and feeds every chunk it serves to a source
/// validator declared at the sealed length, so the bytes the opener consumes are bound to the
/// seal: this is the exporter's `ExactTotalPlaintextSourceV1` pattern, applied to ciphertext.
struct VerifiedCiphertextSourceV1<'a> {
    file: &'a File,
    descriptor: CustodyEnvelopeSourceDescriptorV1,
    validator: Option<CustodyEnvelopeSourceValidatorV1>,
    chunk_bytes: u64,
    emitted: u64,
    ordinal: u32,
    name: &'a [u8],
    failure: &'a RefCell<Option<CustodyRestoreErrorV1>>,
}

impl sealed::Sealed for VerifiedCiphertextSourceV1<'_> {}

impl CustodyEnvelopeChunkSourceV1 for VerifiedCiphertextSourceV1<'_> {
    fn descriptor(&self) -> &CustodyEnvelopeSourceDescriptorV1 {
        &self.descriptor
    }

    fn next_chunk(&mut self) -> Result<Option<CustodyEnvelopeChunkV1>, CustodyCapsuleErrorV1> {
        let total = self.descriptor.total_bytes();
        if self.emitted == total {
            return Ok(None);
        }
        let take = (total - self.emitted).min(self.chunk_bytes);
        let mut bytes =
            vec![0_u8; usize::try_from(take).map_err(|_| CustodyCapsuleErrorV1::InvalidInput)?];
        if self.file.read_exact_at(&mut bytes, self.emitted).is_err() {
            // The retained ciphertext no longer holds its verified length.
            *self.failure.borrow_mut() = Some(CustodyRestoreErrorV1::CiphertextChanged {
                name: lossy(self.name),
            });
            return Err(CustodyCapsuleErrorV1::InvalidInput);
        }
        self.emitted += take;
        let chunk = CustodyEnvelopeChunkV1::new(self.ordinal, bytes, self.emitted == total)?;
        self.ordinal += 1;
        self.validator
            .as_mut()
            .ok_or(CustodyCapsuleErrorV1::InvalidInput)?
            .accept_chunk(&chunk)?;
        Ok(Some(chunk))
    }
}

impl VerifiedCiphertextSourceV1<'_> {
    /// The consumed stream's receipt. It refuses unless the opener consumed exactly the declared
    /// total.
    fn finish(&mut self) -> Result<CustodyEnvelopeStreamReceiptV1, CustodyCapsuleErrorV1> {
        self.validator
            .take()
            .ok_or(CustodyCapsuleErrorV1::InvalidInput)?
            .finish()
    }
}

/// The staged plaintext file as the opener's sink. It reserves each chunk in the ledger before
/// writing it, and measures its own length and SHA-256 over exactly the bytes it wrote.
struct StagingPlaintextSinkV1<'a> {
    file: &'a mut File,
    limits: CustodyEnvelopeStreamLimitsV1,
    validator: Option<CustodyEnvelopeSinkValidatorV1>,
    ledger: &'a RefCell<ScratchLedgerV1>,
    digest: digest::Context,
    written: u64,
    failure: &'a RefCell<Option<CustodyRestoreErrorV1>>,
}

impl sealed::Sealed for StagingPlaintextSinkV1<'_> {}

impl CustodyEnvelopeChunkSinkV1 for StagingPlaintextSinkV1<'_> {
    fn limits(&self) -> CustodyEnvelopeStreamLimitsV1 {
        self.limits
    }

    fn write_chunk(&mut self, chunk: CustodyEnvelopeChunkV1) -> Result<(), CustodyCapsuleErrorV1> {
        let length = chunk.bytes().len() as u64;
        if let Err(error) = self.ledger.borrow_mut().reserve(length) {
            *self.failure.borrow_mut() = Some(CustodyRestoreErrorV1::Budget(error.to_string()));
            return Err(CustodyCapsuleErrorV1::InvalidInput);
        }
        self.validator
            .as_mut()
            .ok_or(CustodyCapsuleErrorV1::InvalidInput)?
            .accept_chunk(&chunk)?;
        if let Err(error) = self.file.write_all(chunk.bytes()) {
            *self.failure.borrow_mut() = Some(CustodyRestoreErrorV1::Io(format!(
                "staged plaintext write: {error}"
            )));
            return Err(CustodyCapsuleErrorV1::InvalidInput);
        }
        self.digest.update(chunk.bytes());
        self.written += length;
        Ok(())
    }
}

/// Task 5: open one sealed artifact into `plain/<name>`, created new, with the consumed
/// ciphertext bound to the seal and the opener's receipt checked against the sink's own
/// measurement. Nothing is deleted on failure.
fn stage_one_v1(
    dest: &RestoreDestinationV1,
    capsule: &PinnedCapsuleV1,
    name: &LosslessPathV1,
    opener: &dyn CustodyEnvelopeOpenerV1,
) -> Result<StagedPlaintextV1, CustodyRestoreErrorV1> {
    let label = || lossy(name.as_bytes());
    let role = reserved_role(name).ok_or(CustodyRestoreErrorV1::Binding(
        CustodyCapsuleErrorV1::UnmappedArtifact,
    ))?;
    let ciphertext = capsule
        .ciphertext(name)
        .ok_or_else(|| CustodyRestoreErrorV1::CapsuleEntryMissing { name: label() })?;
    let request = CustodyEnvelopeOpenRequestV1::from_seal_artifact(&capsule.seal, name.clone())
        .map_err(CustodyRestoreErrorV1::Open)?;

    // The staged file mirrors the logical name beneath `plain/`.
    let components: Vec<&[u8]> = name.as_bytes().split(|byte| *byte == b'/').collect();
    let (leaf, parents) = components
        .split_last()
        .ok_or_else(|| CustodyRestoreErrorV1::Io("an artifact name has no leaf".into()))?;
    let parent = dest.staging_directory(parents)?;
    let parent_key = parents.join(&b'/');
    dest.recheck_staging_chain(&parent_key)?;
    let parent_pin = parent.as_deref().unwrap_or(&dest.plain);
    #[cfg(test)]
    run_hook(
        RestoreHookPointV1::BeforeLeafCreate,
        &parent_pin.canonical_path().join(OsStr::from_bytes(leaf)),
    );
    dest.ledger
        .borrow_mut()
        .reserve_entries(1)
        .map_err(|error| CustodyRestoreErrorV1::Budget(error.to_string()))?;
    let staged_name = plain_relative(name.as_bytes());
    let mut file = parent_pin
        .create_new_regular_child(OsStr::from_bytes(leaf), "custody restore staged plaintext")
        .map_err(|error| staging_refusal(error, &staged_name))?;

    let limits = stream_limits()?;
    let failure: RefCell<Option<CustodyRestoreErrorV1>> = RefCell::new(None);
    let descriptor = || {
        CustodyEnvelopeSourceDescriptorV1::new(request.ciphertext_length(), limits)
            .map_err(CustodyRestoreErrorV1::Open)
    };
    let mut source = VerifiedCiphertextSourceV1 {
        file: &ciphertext.file,
        descriptor: descriptor()?,
        validator: Some(CustodyEnvelopeSourceValidatorV1::new(descriptor()?)),
        chunk_bytes: limits.max_chunk_bytes(),
        emitted: 0,
        ordinal: 0,
        name: name.as_bytes(),
        failure: &failure,
    };
    let mut sink = StagingPlaintextSinkV1 {
        file: &mut file,
        limits,
        validator: Some(CustodyEnvelopeSinkValidatorV1::new(limits)),
        ledger: &dest.ledger,
        digest: digest::Context::new(&digest::SHA256),
        written: 0,
        failure: &failure,
    };

    let opened = opener.open(&request, &mut source, &mut sink);
    let consumed = source.finish();
    let completed = sink
        .validator
        .take()
        .ok_or(CustodyCapsuleErrorV1::InvalidInput)
        .and_then(CustodyEnvelopeSinkValidatorV1::finish);
    let written = sink.written;
    let mut measured = [0_u8; 32];
    measured.copy_from_slice(sink.digest.finish().as_ref());

    let receipt = match opened {
        Ok(receipt) => receipt,
        Err(error) => {
            return Err(failure
                .into_inner()
                .unwrap_or(CustodyRestoreErrorV1::Open(error)))
        }
    };
    // The consumed ciphertext: exactly the sealed length, with the sealed digest.
    let changed = || CustodyRestoreErrorV1::CiphertextChanged { name: label() };
    let consumed = consumed.map_err(|_| changed())?;
    if consumed.total_bytes() != request.ciphertext_length()
        || consumed.sha256() != request.ciphertext_sha256()
    {
        return Err(changed());
    }
    // The opener's receipt against the sink's own measurement of what it wrote.
    let mismatch = || CustodyRestoreErrorV1::OpenerReceiptMismatch { name: label() };
    completed.map_err(|_| mismatch())?;
    if receipt.plaintext_length() != written || *receipt.plaintext_sha256() != sha256_hex(&measured)
    {
        return Err(mismatch());
    }

    file.sync_all()
        .map_err(|error| CustodyRestoreErrorV1::Io(format!("staged plaintext sync: {error}")))?;
    file.seek(SeekFrom::Start(0))
        .map_err(|error| CustodyRestoreErrorV1::Io(format!("staged plaintext rewind: {error}")))?;
    Ok(StagedPlaintextV1 {
        name: name.clone(),
        role,
        file,
        length: written,
        sha256: measured,
    })
}

/// Task 5: stage exactly the three controls, read each back through the bounded reader, decode
/// them, and bind them to the seal through 2B1. No pack or payload is opened.
fn bind_control_v1(
    dest: &RestoreDestinationV1,
    capsule: &PinnedCapsuleV1,
    opener: &dyn CustodyEnvelopeOpenerV1,
) -> Result<(BoundControlV1, Vec<StagedPlaintextV1>), CustodyRestoreErrorV1> {
    let mut staged = Vec::with_capacity(3);
    let mut read_back = |name: &str| -> Result<Vec<u8>, CustodyRestoreErrorV1> {
        let name = LosslessPathV1::from_bytes(name.as_bytes().to_vec());
        if capsule.ciphertext(&name).is_none() {
            return Err(CustodyRestoreErrorV1::Binding(
                CustodyCapsuleErrorV1::MissingArtifact,
            ));
        }
        let mut one = stage_one_v1(dest, capsule, &name, opener)?;
        let bytes =
            read_bounded_v1(&mut one.file, RESTORE_CONTROL_READ_BOUND_V1).map_err(|error| {
                match error {
                    BoundedReadErrorV1::Oversize => CustodyRestoreErrorV1::ControlOversize {
                        name: lossy(name.as_bytes()),
                    },
                    BoundedReadErrorV1::Io(kind) => {
                        CustodyRestoreErrorV1::Io(format!("control read-back: {kind}"))
                    }
                }
            })?;
        one.file
            .seek(SeekFrom::Start(0))
            .map_err(|error| CustodyRestoreErrorV1::Io(format!("control rewind: {error}")))?;
        staged.push(one);
        Ok(bytes)
    };
    let decode = |name: &str| CustodyRestoreErrorV1::ControlDecode {
        name: name.to_owned(),
    };

    let manifest = CustodyManifestV1::decode_canonical(&read_back(RESTORE_MANIFEST_NAME_V1)?)
        .map_err(|_| decode(RESTORE_MANIFEST_NAME_V1))?;
    let index = CustodyCapsuleIndexV1::decode_canonical(&read_back(RESTORE_INDEX_NAME_V1)?)
        .map_err(|_| decode(RESTORE_INDEX_NAME_V1))?;
    let policy = CustodyRestorePolicyV1::decode_canonical(&read_back(RESTORE_POLICY_NAME_V1)?)
        .map_err(|_| decode(RESTORE_POLICY_NAME_V1))?;
    let binding = CustodyCapsuleBindingV1::new(&manifest, &index, &policy, &capsule.seal)
        .map_err(CustodyRestoreErrorV1::Binding)?;
    Ok((
        BoundControlV1 {
            manifest,
            index,
            policy,
            binding,
        },
        staged,
    ))
}

// ---------------------------------------------------------------------------------------------
// Task 6: stage the pack and the payloads, then decode-verify every frame
// ---------------------------------------------------------------------------------------------

/// Phase V's output: the prepared destination, the bound controls, and a retained, rewound
/// descriptor of every staged plaintext.
#[derive(Debug)]
pub(crate) struct VerifiedCapsuleV1 {
    pub(crate) destination: RestoreDestinationV1,
    pub(crate) control: BoundControlV1,
    /// The controls, the pack, and the payloads, in exactly `control.index`'s artifact order.
    pub(crate) staged: Vec<StagedPlaintextV1>,
}

/// Restore phase V (design §3.1): pin and verify the capsule (task 3), prepare the destination
/// (task 4), and bind the controls (task 5). A T1 or T2 refusal therefore happens before
/// `.restore-work/` exists, and a T3 refusal with only the control plaintexts staged. Only then is
/// every remaining artifact staged, in index order, and every coverage frame fully decoded against
/// its class and the manifest's generation. The Git pack is staged only; 2B3b indexes it.
pub(crate) fn verify_and_stage_v1(
    capsule_root: &Path,
    destination_root: &Path,
    budget: CustodyRestoreBudgetV1,
    frame_budget: CustodyFrameBudgetV1,
    opener: &dyn CustodyEnvelopeOpenerV1,
) -> Result<VerifiedCapsuleV1, CustodyRestoreErrorV1> {
    let capsule = pin_and_verify_capsule_v1(capsule_root)?;
    let destination = prepare_destination_v1(destination_root, &capsule, budget)?;
    let (control, mut staged) = bind_control_v1(&destination, &capsule, opener)?;

    for row in control.index.artifacts() {
        if staged.iter().any(|one| one.name == *row.name()) {
            continue;
        }
        let one = stage_one_v1(&destination, &capsule, row.name(), opener)?;
        if one.role != *row.role() {
            return Err(CustodyRestoreErrorV1::Binding(
                CustodyCapsuleErrorV1::DerivedLayoutMismatch,
            ));
        }
        staged.push(one);
    }
    // The controls were staged in decode order; consumers get the index's canonical order, and
    // that order is a checked invariant.
    let position = |name: &LosslessPathV1| {
        control
            .index
            .artifacts()
            .iter()
            .position(|row| row.name() == name)
    };
    staged.sort_by_key(|one| position(&one.name));
    if !staged.iter().map(|one| &one.name).eq(control
        .index
        .artifacts()
        .iter()
        .map(|row| row.name()))
    {
        return Err(CustodyRestoreErrorV1::Io(
            "the staged plaintexts are not exactly the index's artifacts in its order".into(),
        ));
    }

    let generation = control.manifest.generation_id();
    for one in &mut staged {
        if let CustodyCapsuleArtifactRoleV1::CoveragePayload(class) = one.role {
            verify_frame(&mut one.file, class, generation, frame_budget)?;
        }
    }
    destination.recheck_containment()?;
    Ok(VerifiedCapsuleV1 {
        destination,
        control,
        staged,
    })
}

/// Decodes the whole staged frame with the expected header `(class, generation)` into a null
/// sink, draining every entry's content, until the decoder's `Ok(None)`: so the trailer's digest,
/// count, and total are verified. The descriptor is rewound afterwards.
fn verify_frame(
    file: &mut File,
    class: CustodyCoverageClassV1,
    generation: &str,
    budget: CustodyFrameBudgetV1,
) -> Result<(), CustodyRestoreErrorV1> {
    let invalid = |_| CustodyRestoreErrorV1::FrameInvalid { class };
    let rewind = |file: &mut File| {
        file.seek(SeekFrom::Start(0))
            .map(|_| ())
            .map_err(|error| CustodyRestoreErrorV1::Io(format!("staged frame rewind: {error}")))
    };
    rewind(file)?;
    let header = CustodyFrameHeaderV1::new(class, generation).map_err(invalid)?;
    {
        let reader = BufReader::with_capacity(HASH_BUFFER_BYTES_V1, &*file);
        let mut decoder = CustodyFrameDecoderV1::new(reader, &header, budget).map_err(invalid)?;
        while let Some(entry) = decoder.next_entry().map_err(invalid)? {
            if let CustodyFrameEntryV1::Regular { mut content, .. } = entry {
                io::copy(&mut content, &mut io::sink())
                    .map_err(|_| CustodyRestoreErrorV1::FrameInvalid { class })?;
            }
        }
    }
    rewind(file)
}

/// The length and SHA-256 of `file` from offset 0, reading at most `expected + 1` bytes: a longer
/// file is reported one byte long, never read whole.
fn hash_prefix(file: &mut File, expected: u64) -> io::Result<(u64, [u8; 32])> {
    file.seek(SeekFrom::Start(0))?;
    let mut limited = Read::by_ref(file).take(expected.saturating_add(1));
    let mut context = digest::Context::new(&digest::SHA256);
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES_V1];
    let mut length = 0_u64;
    loop {
        let read = match limited.read(&mut buffer) {
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if read == 0 {
            let mut sha256 = [0_u8; 32];
            sha256.copy_from_slice(context.finish().as_ref());
            return Ok((length, sha256));
        }
        length += read as u64;
        context.update(&buffer[..read]);
    }
}

fn sha256_hex(bytes: &[u8; 32]) -> Sha256HexV1 {
    let mut value = String::with_capacity(64);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut value, "{byte:02x}");
    }
    Sha256HexV1::parse(value).expect("a SHA-256 digest renders as 64 lower-hex bytes")
}

#[cfg(test)]
#[path = "custody_restore_tests.rs"]
mod tests;
