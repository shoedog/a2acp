//! Isolated local export of one validated sealable custody manifest (ADR-0041 slice 2B2).
//!
//! The exported capsule is local and inert. It creates no remote-custody, restoration,
//! deletion-eligibility, publication, or operator-adoption claim.
//!
//! The entry point [`export_capsule_v1`] is crate-private and has no production caller: the
//! capture capability can only be minted by a crate-private fixture constructor, and the only
//! sealer is the `#[cfg(test)]` fixture. That is deliberate — slice 2B2 owns the effect boundary,
//! and the wiring slice owns where the Git route pin and the quiescence decision come from.
//!
//! Every filesystem effect goes through the descriptor-relative primitives in
//! [`crate::fs_custody`], and every Git child goes through the closed runner in
//! [`crate::custody_git`]. This module neither reimplements nor extends either seam.

use crate::custody_capsule::{
    sealed, CustodyCapsuleArtifactRoleV1, CustodyCapsuleBindingV1, CustodyCapsuleErrorV1,
    CustodyCapsuleLayoutV1, CustodyCapsuleSealProofV1, CustodyEnvelopeChunkSinkV1,
    CustodyEnvelopeChunkSourceV1, CustodyEnvelopeChunkV1, CustodyEnvelopeContextV1,
    CustodyEnvelopeFormatV1, CustodyEnvelopeMetadataV1, CustodyEnvelopeSealReceiptV1,
    CustodyEnvelopeSealerV1, CustodyEnvelopeSinkValidatorV1, CustodyEnvelopeSourceDescriptorV1,
    CustodyEnvelopeSourceValidatorV1, CustodyEnvelopeStreamLimitsV1, CustodyRestorePolicyV1,
};
use crate::custody_git::{
    CustodyGitError, GitCommandV1, GitObjectFormatV1, GitObjectStoreRouteV1, GitRootNamesV1,
    GitRouteRequestV1, GitRunRequestV1, GitRunResultV1, GitRunnerV1, GitStdoutV1,
    GitStreamEvidenceV1,
};
use crate::custody_inventory::LosslessPathV1;
use crate::custody_seal::{
    CustodyCoverageClassV1, CustodyGitObjectFormatV1, CustodyGitObjectKindV1, CustodyManifestV1,
    CustodySealV1, CustodySealedArtifactV1,
};
use crate::execution_policy::Sha256HexV1;
use crate::fs_custody::{
    pinned_root_unchanged, CustodyPublicationV1, FsCustodyError, PinnedDirectoryV1,
    RegularChildRefV1,
};
use ring::digest;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::time::Instant;

// ---------------------------------------------------------------------------------------------
// V1 ceilings (§3). Fixed before any allocation or file creation; caller budgets may only be
// lower.
// ---------------------------------------------------------------------------------------------

const MAX_ARTIFACTS_V1: usize = 17;
const MAX_CHUNK_BYTES_V1: u64 = 1024 * 1024;
const MAX_CHUNKS_V1: u32 = 16_384;
const MAX_ARTIFACT_BYTES_V1: u64 = 10_737_418_240;
const MAX_SCRATCH_BYTES_V1: u64 = 10_737_418_240;
const MAX_CANONICAL_JSON_BYTES_V1: usize = 1024 * 1024;

/// The fixed per-entry allocation allowance charged against the scratch-wide ledger for every
/// file or directory the exporter (or a Git child it spawned) creates. The set of created
/// entries is fixed and small, so allocated-block overhead stays bounded.
const ENTRY_ALLOWANCE_BYTES_V1: u64 = 64 * 1024;

const CAPSULE_DIR_NAME: &str = "capsule";
const WORK_DIR_NAME: &str = "work";
const HOME_DIR_NAME: &str = "home";
const XDG_DIR_NAME: &str = "xdg";
const SOURCE_GIT_DIR_NAME: &str = "source-git";
const VERIFY_GIT_DIR_NAME: &str = "verify.git";
const PACK_FILE_NAME: &str = "objects.pack";
const SEAL_NAME: &str = "custody-seal.v1";

/// The exact file set `git init --bare --template=` leaves behind, enumerated so a post-exit
/// re-measure can refuse an unexpected Git-written file rather than merely totalling bytes.
const INIT_BARE_FILES_V1: [&str; 2] = ["HEAD", "config"];
/// `HEAD`, `config`, and the empty `objects/{info,pack}` and `refs/{heads,tags}` tree, plus the
/// git directory itself: nine created entries at the per-entry allowance.
const INIT_BARE_ENTRIES_V1: u64 = 9;
/// The logical bytes reserved for `HEAD` and `config` before each `git init`, apart from the entry
/// allowances. `HEAD` is one symbolic-ref line and `config` a handful of `core` and `extensions`
/// keys: 89 bytes for SHA-1 and 125 for SHA-256 on the container lane. The bound is enforced, not
/// assumed: the post-exit re-measure refuses any excess, then reconciles the reservation down to
/// the measured bytes.
const INIT_BARE_LOGICAL_BYTES_V1: u64 = 4 * 1024;
/// `index-pack` creates the pack, its version-2 index, and its reverse index.
const INDEX_PACK_ENTRIES_V1: u64 = 3;

const GIT_STDERR_LIMIT_V1: usize = 64 * 1024;
/// `cat-file --batch-check` and `cat-file --batch-all-objects` emit one bounded line per object,
/// and `rev-list` one per reachable object plus one per missing object.
const GIT_LINE_BYTES_V1: usize = 128;
const GIT_MIN_STDOUT_LIMIT_V1: usize = 16 * 1024;

/// Bounded walk limits for the post-exit git-directory re-measure.
const GIT_DIR_MAX_DEPTH_V1: usize = 6;
const GIT_DIR_MAX_ENTRIES_V1: usize = 256;

/// An `objects/info/alternates` file names a handful of store paths. It is read through a bound
/// one byte past this ceiling, so an oversized file is refused without ever being buffered whole.
const MAX_ALTERNATES_FILE_BYTES_V1: usize = 64 * 1024;

// ---------------------------------------------------------------------------------------------
// Caller budgets
// ---------------------------------------------------------------------------------------------

/// Caller budgets, validated to be at or below the fixed §3 ceilings and otherwise refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CustodyExportBudgetsV1 {
    pub(crate) max_artifacts: usize,
    pub(crate) max_chunk_bytes: u64,
    pub(crate) max_chunks: u32,
    pub(crate) max_artifact_bytes: u64,
    pub(crate) max_scratch_bytes: u64,
    pub(crate) max_canonical_json_bytes: usize,
}

impl CustodyExportBudgetsV1 {
    #[must_use]
    pub(crate) const fn v1_ceilings() -> Self {
        Self {
            max_artifacts: MAX_ARTIFACTS_V1,
            max_chunk_bytes: MAX_CHUNK_BYTES_V1,
            max_chunks: MAX_CHUNKS_V1,
            max_artifact_bytes: MAX_ARTIFACT_BYTES_V1,
            max_scratch_bytes: MAX_SCRATCH_BYTES_V1,
            max_canonical_json_bytes: MAX_CANONICAL_JSON_BYTES_V1,
        }
    }

    fn validate(self) -> Result<Self, CustodyExportErrorV1> {
        let refuse = |field: &'static str| Err(CustodyExportErrorV1::BudgetCeiling(field));
        if self.max_artifacts == 0 || self.max_artifacts > MAX_ARTIFACTS_V1 {
            return refuse("artifact count");
        }
        if self.max_chunk_bytes == 0 || self.max_chunk_bytes > MAX_CHUNK_BYTES_V1 {
            return refuse("chunk bytes");
        }
        if self.max_chunks == 0 || self.max_chunks > MAX_CHUNKS_V1 {
            return refuse("chunk count");
        }
        if self.max_artifact_bytes == 0 || self.max_artifact_bytes > MAX_ARTIFACT_BYTES_V1 {
            return refuse("per-artifact ciphertext");
        }
        if self.max_scratch_bytes == 0 || self.max_scratch_bytes > MAX_SCRATCH_BYTES_V1 {
            return refuse("scratch-wide ledger");
        }
        if self.max_canonical_json_bytes == 0
            || self.max_canonical_json_bytes > MAX_CANONICAL_JSON_BYTES_V1
        {
            return refuse("canonical JSON metadata");
        }
        Ok(self)
    }

    fn stream_limits(self) -> Result<CustodyEnvelopeStreamLimitsV1, CustodyExportErrorV1> {
        let capacity = self
            .max_chunk_bytes
            .checked_mul(u64::from(self.max_chunks))
            .ok_or(CustodyExportErrorV1::BudgetCeiling("chunk capacity"))?;
        CustodyEnvelopeStreamLimitsV1::new(
            self.max_artifact_bytes.min(capacity),
            self.max_chunk_bytes,
            self.max_chunks,
        )
        .map_err(CustodyExportErrorV1::Capsule)
    }
}

// ---------------------------------------------------------------------------------------------
// The scratch-wide ledger (§3)
// ---------------------------------------------------------------------------------------------

/// One ledger for every byte the exporter writes below the scratch root: capsule ciphertext,
/// staging names, the seal, the plaintext Git-pack staging file, and the verification database's
/// pack, index, and reverse index. Logical file bytes are counted with checked arithmetic, plus
/// a fixed per-entry allocation allowance for each created file or directory.
#[derive(Debug)]
struct ScratchLedgerV1 {
    limit: u64,
    used: u64,
}

impl ScratchLedgerV1 {
    const fn new(limit: u64) -> Self {
        Self { limit, used: 0 }
    }

    fn reserve(&mut self, bytes: u64) -> Result<(), CustodyExportErrorV1> {
        let next = self
            .used
            .checked_add(bytes)
            .ok_or_else(|| CustodyExportErrorV1::ScratchLedger("reservation overflowed".into()))?;
        if next > self.limit {
            return Err(CustodyExportErrorV1::ScratchLedger(format!(
                "{next} bytes exceeds the {} byte scratch ceiling",
                self.limit
            )));
        }
        self.used = next;
        Ok(())
    }

    fn reserve_entries(&mut self, entries: u64) -> Result<(), CustodyExportErrorV1> {
        let bytes = entries
            .checked_mul(ENTRY_ALLOWANCE_BYTES_V1)
            .ok_or_else(|| CustodyExportErrorV1::ScratchLedger("allowance overflowed".into()))?;
        self.reserve(bytes)
    }

    /// Reconcile a whole-stream reservation down to the length that was actually streamed.
    fn release(&mut self, bytes: u64) {
        self.used = self.used.saturating_sub(bytes);
    }

    const fn used(&self) -> u64 {
        self.used
    }
}

// ---------------------------------------------------------------------------------------------
// The generation-bound capture capability
// ---------------------------------------------------------------------------------------------

/// How the caller established that the source object population cannot move underneath the
/// export. A boolean quiescence assertion is deliberately not representable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CustodyQuiescenceDecisionV1 {
    /// A coherent filesystem snapshot of the source object stores.
    CoherentSnapshot,
    /// An exclusive managed-writer decision: the bridge is the only writer for this generation.
    ExclusiveManagedWriter,
}

/// One captured non-Git coverage stream, bound to exactly one coverage class, source generation,
/// declared length, and SHA-256, so a stream cannot be replayed under another artifact role.
#[derive(Clone, Debug)]
pub(crate) struct CustodyCapturedStreamV1 {
    class: CustodyCoverageClassV1,
    generation_id: String,
    length: u64,
    sha256: Sha256HexV1,
    bytes: Vec<u8>,
}

impl CustodyCapturedStreamV1 {
    #[cfg(test)]
    pub(crate) fn for_test_fixture(
        class: CustodyCoverageClassV1,
        generation_id: impl Into<String>,
        bytes: Vec<u8>,
    ) -> Self {
        let sha256 = Sha256HexV1::digest(&bytes);
        Self {
            class,
            generation_id: generation_id.into(),
            length: bytes.len() as u64,
            sha256,
            bytes,
        }
    }

    /// Exchange two streams' payload bytes while leaving each record's class, generation,
    /// declared length, and declared SHA-256 bound as captured — control 19's role replay.
    #[cfg(test)]
    pub(crate) fn exchange_payloads_for_test(left: &mut Self, right: &mut Self) {
        std::mem::swap(&mut left.bytes, &mut right.bytes);
    }
}

/// One pinned Git object store: a retained descriptor plus the content hash of its own
/// `objects/info/alternates`, so a rewritten alternates file is drift, not a silent retarget.
#[derive(Debug)]
pub(crate) struct PinnedObjectStoreV1 {
    pin: PinnedDirectoryV1,
    alternates_sha256: Option<[u8; 32]>,
}

impl PinnedObjectStoreV1 {
    fn open(path: &Path) -> Result<Self, CustodyExportErrorV1> {
        let pin = PinnedDirectoryV1::open(path, "custody export object store")?;
        let alternates_sha256 = read_alternates_digest(&pin)?.map(|(digest, _)| digest);
        Ok(Self {
            pin,
            alternates_sha256,
        })
    }

    fn recheck(&self) -> Result<(), CustodyExportErrorV1> {
        pinned_root_unchanged(&self.pin).map_err(CustodyExportErrorV1::IdentityDrift)?;
        let observed = read_alternates_digest(&self.pin)?.map(|(digest, _)| digest);
        if observed != self.alternates_sha256 {
            return Err(CustodyExportErrorV1::IdentityDrift(format!(
                "the alternates file of {} changed content",
                self.pin.canonical_path().display()
            )));
        }
        Ok(())
    }

    fn path(&self) -> &Path {
        self.pin.canonical_path()
    }
}

/// One store's `objects/info/alternates`: its content SHA-256 and its bounded contents.
type AlternatesFileV1 = ([u8; 32], Vec<u8>);

/// Read `objects/info/alternates` beneath a pinned store, returning its SHA-256 and its bounded
/// contents. `Ok(None)` means the store declares no alternates.
fn read_alternates_digest(
    store: &PinnedDirectoryV1,
) -> Result<Option<AlternatesFileV1>, CustodyExportErrorV1> {
    let info = match store.open_existing_child_directory(OsStr::new("info"), "custody alternates") {
        Ok(info) => info,
        Err(FsCustodyError::Io(_, error)) if error.raw_os_error() == Some(libc::ENOENT) => {
            return Ok(None)
        }
        Err(error) => return Err(CustodyExportErrorV1::Fs(error)),
    };
    let mut file = match info.open_regular_file(OsStr::new("alternates"), "custody alternates") {
        Ok(file) => file,
        Err(FsCustodyError::Io(_, error)) if error.raw_os_error() == Some(libc::ENOENT) => {
            return Ok(None)
        }
        Err(error) => return Err(CustodyExportErrorV1::Fs(error)),
    };
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_ALTERNATES_FILE_BYTES_V1 as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| CustodyExportErrorV1::Io(format!("alternates file: {error}")))?;
    if bytes.len() > MAX_ALTERNATES_FILE_BYTES_V1 {
        return Err(CustodyExportErrorV1::SourceState(
            "an alternates file exceeds its bounded size",
        ));
    }
    let digest = sha256_bytes(&bytes);
    Ok(Some((digest, bytes)))
}

/// A generation-bound, non-cloneable capture capability. It owns the retained source descriptors
/// and a fixed source/alternate/object population, so the exporter never re-derives any of them
/// from a caller path or a boolean.
#[derive(Debug)]
pub(crate) struct CustodyCaptureCapabilityV1 {
    decision: CustodyQuiescenceDecisionV1,
    unit_id: String,
    run_id: String,
    materialization_id: String,
    generation_id: String,
    object_format: CustodyGitObjectFormatV1,
    inventory: Vec<(CustodyGitObjectFormatV1, String, CustodyGitObjectKindV1)>,
    primary_store: PinnedObjectStoreV1,
    alternate_stores: Vec<PinnedObjectStoreV1>,
    /// The source repository root: a non-bare source's worktree, or a bare source's git directory.
    /// It is pinned apart from the git directory because a worktree holds source bytes outside
    /// `.git`, and no exporter write may land among them.
    source_repository: PinnedDirectoryV1,
    source_git_dir: PinnedDirectoryV1,
    shallow_or_grafted: bool,
    unresolved_promisor_boundary: bool,
    replacement_refs: Vec<String>,
    streams: Vec<CustodyCapturedStreamV1>,
}

/// Follow a primary object store's `objects/info/alternates` chain to a fixed point, pinning
/// every store by identity and by its alternates file's content.
fn pin_alternate_chain(
    primary: &PinnedObjectStoreV1,
) -> Result<Vec<PinnedObjectStoreV1>, CustodyExportErrorV1> {
    let mut pinned: Vec<PinnedObjectStoreV1> = Vec::new();
    let mut seen = BTreeSet::new();
    seen.insert(primary.path().to_path_buf());
    let mut frontier = vec![read_alternates_digest(&primary.pin)?];
    let mut bases = vec![primary.path().to_path_buf()];
    while let Some(entry) = frontier.pop() {
        let base = bases.pop().expect("one base per frontier entry");
        let Some((_, bytes)) = entry else { continue };
        for line in bytes.split(|byte| *byte == b'\n') {
            let line = trim_ascii(line);
            if line.is_empty() || line[0] == b'#' {
                continue;
            }
            let candidate = {
                use std::os::unix::ffi::OsStrExt as _;
                let raw = Path::new(OsStr::from_bytes(line));
                if raw.is_absolute() {
                    raw.to_path_buf()
                } else {
                    base.join(raw)
                }
            };
            let store = PinnedObjectStoreV1::open(&candidate)?;
            if !seen.insert(store.path().to_path_buf()) {
                continue;
            }
            if pinned.len() >= GIT_DIR_MAX_ENTRIES_V1 {
                return Err(CustodyExportErrorV1::SourceState(
                    "the alternate chain exceeds its bounded length",
                ));
            }
            frontier.push(read_alternates_digest(&store.pin)?);
            bases.push(store.path().to_path_buf());
            pinned.push(store);
        }
    }
    Ok(pinned)
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(start, |index| index + 1);
    &bytes[start..end]
}

impl CustodyCaptureCapabilityV1 {
    /// Mint a capability from a fixture quiescence decision.
    ///
    /// The production quiescence primitive does not exist yet, so this crate-private fixture
    /// constructor is the only mint. It still takes a decision value rather than a boolean, and
    /// it still pins the recursive alternate chain and the source repository root, so the shape
    /// the production mint must supply is fixed here rather than left to the wiring slice.
    #[cfg(test)]
    pub(crate) fn from_fixture_quiescence(
        decision: CustodyQuiescenceDecisionV1,
        manifest: &CustodyManifestV1,
        source_repository: &Path,
        source_git_dir: &Path,
        primary_store: &Path,
        streams: Vec<CustodyCapturedStreamV1>,
    ) -> Result<Self, CustodyExportErrorV1> {
        let primary = PinnedObjectStoreV1::open(primary_store)?;
        let alternate_stores = pin_alternate_chain(&primary)?;
        let object_format = manifest
            .original_objects()
            .first()
            .map_or(CustodyGitObjectFormatV1::Sha1, |object| object.format());
        Ok(Self {
            decision,
            unit_id: manifest.unit_id().to_owned(),
            run_id: manifest.run_id().to_owned(),
            materialization_id: manifest.materialization_id().to_owned(),
            generation_id: manifest.generation_id().to_owned(),
            object_format,
            inventory: manifest
                .original_objects()
                .iter()
                .map(|object| {
                    (
                        object.format(),
                        object.object_id().to_owned(),
                        object.kind(),
                    )
                })
                .collect(),
            primary_store: primary,
            alternate_stores,
            source_repository: PinnedDirectoryV1::open(
                source_repository,
                "custody export source repository",
            )?,
            source_git_dir: PinnedDirectoryV1::open(source_git_dir, "custody export source")?,
            shallow_or_grafted: false,
            unresolved_promisor_boundary: false,
            replacement_refs: Vec::new(),
            streams,
        })
    }

    #[cfg(test)]
    pub(crate) fn set_identity_for_test(&mut self, generation_id: impl Into<String>) {
        self.generation_id = generation_id.into();
    }

    #[cfg(test)]
    pub(crate) fn set_inventory_for_test(
        &mut self,
        inventory: Vec<(CustodyGitObjectFormatV1, String, CustodyGitObjectKindV1)>,
    ) {
        self.inventory = inventory;
    }

    #[cfg(test)]
    pub(crate) fn set_source_state_for_test(&mut self, shallow_or_grafted: bool, promisor: bool) {
        self.shallow_or_grafted = shallow_or_grafted;
        self.unresolved_promisor_boundary = promisor;
    }

    #[cfg(test)]
    pub(crate) fn streams_mut_for_test(&mut self) -> &mut Vec<CustodyCapturedStreamV1> {
        &mut self.streams
    }

    #[cfg(test)]
    pub(crate) fn source_git_dir_path_for_test(&self) -> &Path {
        self.source_git_dir.canonical_path()
    }

    #[cfg(test)]
    pub(crate) fn decision_for_test(&self) -> CustodyQuiescenceDecisionV1 {
        self.decision
    }

    /// Prove the capability describes the same generation and exact object inventory as the
    /// manifest, using read-only accessors rather than canonical-JSON re-parsing.
    fn bind_to_manifest(&self, manifest: &CustodyManifestV1) -> Result<(), CustodyExportErrorV1> {
        if self.unit_id != manifest.unit_id() {
            return Err(CustodyExportErrorV1::CapabilityBinding("unit id"));
        }
        if self.run_id != manifest.run_id() {
            return Err(CustodyExportErrorV1::CapabilityBinding("run id"));
        }
        if self.materialization_id != manifest.materialization_id() {
            return Err(CustodyExportErrorV1::CapabilityBinding(
                "materialization id",
            ));
        }
        if self.generation_id != manifest.generation_id() {
            return Err(CustodyExportErrorV1::CapabilityBinding("generation id"));
        }
        if manifest
            .original_objects()
            .iter()
            .any(|object| object.format() != self.object_format)
        {
            return Err(CustodyExportErrorV1::MixedObjectFormat);
        }
        let manifest_inventory: Vec<_> = manifest
            .original_objects()
            .iter()
            .map(|object| {
                (
                    object.format(),
                    object.object_id().to_owned(),
                    object.kind(),
                )
            })
            .collect();
        if self.inventory != manifest_inventory {
            return Err(CustodyExportErrorV1::CapabilityBinding("object inventory"));
        }
        Ok(())
    }

    fn refuse_unsupported_source_state(&self) -> Result<(), CustodyExportErrorV1> {
        if self.shallow_or_grafted {
            return Err(CustodyExportErrorV1::SourceState(
                "shallow or grafted source state was observed",
            ));
        }
        if self.unresolved_promisor_boundary {
            return Err(CustodyExportErrorV1::SourceState(
                "an unresolved promisor boundary was observed",
            ));
        }
        Ok(())
    }

    fn recheck(&self) -> Result<(), CustodyExportErrorV1> {
        self.primary_store.recheck()?;
        for alternate in &self.alternate_stores {
            alternate.recheck()?;
        }
        pinned_root_unchanged(&self.source_git_dir).map_err(CustodyExportErrorV1::IdentityDrift)?;
        pinned_root_unchanged(&self.source_repository).map_err(CustodyExportErrorV1::IdentityDrift)
    }

    fn object_store_route(&self) -> Result<GitObjectStoreRouteV1, CustodyExportErrorV1> {
        GitObjectStoreRouteV1::new(
            self.primary_store.path().to_path_buf(),
            self.alternate_stores
                .iter()
                .map(|store| store.path().to_path_buf())
                .collect(),
        )
        .map_err(CustodyExportErrorV1::Git)
    }

    /// The source paths no scratch root may be, contain, or lie inside: the repository root, its
    /// git directory, the primary object store, and every pinned alternate store.
    fn protected_paths(&self) -> Vec<PathBuf> {
        let mut paths = vec![self.source_repository.canonical_path().to_path_buf()];
        paths.push(self.source_git_dir.canonical_path().to_path_buf());
        paths.push(self.primary_store.path().to_path_buf());
        paths.extend(
            self.alternate_stores
                .iter()
                .map(|store| store.path().to_path_buf()),
        );
        paths
    }

    /// The captured stream bound to exactly this coverage class, generation, declared length,
    /// and SHA-256. A stream whose payload was replayed under another role refuses here.
    fn stream_for(
        &self,
        class: CustodyCoverageClassV1,
    ) -> Result<&CustodyCapturedStreamV1, CustodyExportErrorV1> {
        let stream = self
            .streams
            .iter()
            .find(|stream| stream.class == class)
            .ok_or(CustodyExportErrorV1::CapabilityBinding(
                "no captured stream for a coverage class",
            ))?;
        if stream.generation_id != self.generation_id {
            return Err(CustodyExportErrorV1::CapabilityBinding(
                "a captured stream names another generation",
            ));
        }
        if stream.bytes.len() as u64 != stream.length {
            return Err(CustodyExportErrorV1::CapabilityBinding(
                "a captured stream does not have its declared length",
            ));
        }
        if Sha256HexV1::digest(&stream.bytes) != stream.sha256 {
            return Err(CustodyExportErrorV1::CapabilityBinding(
                "a captured stream does not have its declared SHA-256",
            ));
        }
        Ok(stream)
    }

    fn git_object_format(&self) -> GitObjectFormatV1 {
        match self.object_format {
            CustodyGitObjectFormatV1::Sha1 => GitObjectFormatV1::Sha1,
            CustodyGitObjectFormatV1::Sha256 => GitObjectFormatV1::Sha256,
        }
    }

    fn raw_hash_width(&self) -> u64 {
        match self.object_format {
            CustodyGitObjectFormatV1::Sha1 => 20,
            CustodyGitObjectFormatV1::Sha256 => 32,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Fault injection (§6): a crate-private fault-point enum plus an ordinal.
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ExportFaultPointV1 {
    StagingCreate,
    StagingFileSync,
    StagingIdentityRecheck,
    ArtifactRename,
    ArtifactDirectorySync,
    GitSpawn,
    GitStdinClose,
    GitStdoutComplete,
    GitPostExitRecheck,
    IndexPack,
    VerifyPack,
    InventoryComparison,
    RevListClosure,
    Fsck,
    BindingConstruction,
    SealRename,
    PostSealDirectorySync,
}

#[cfg(test)]
impl ExportFaultPointV1 {
    /// Every named boundary in §6.
    pub(crate) const ALL: [Self; 17] = [
        Self::StagingCreate,
        Self::StagingFileSync,
        Self::StagingIdentityRecheck,
        Self::ArtifactRename,
        Self::ArtifactDirectorySync,
        Self::GitSpawn,
        Self::GitStdinClose,
        Self::GitStdoutComplete,
        Self::GitPostExitRecheck,
        Self::IndexPack,
        Self::VerifyPack,
        Self::InventoryComparison,
        Self::RevListClosure,
        Self::Fsck,
        Self::BindingConstruction,
        Self::SealRename,
        Self::PostSealDirectorySync,
    ];

    /// Whether a fault here lands strictly before the seal rename, the single commit point.
    ///
    /// The seal rename's `Before` side is pre-commit: nothing has been renamed yet. Its `After`
    /// side and both sides of the post-seal directory sync are after the commit point, so a fault
    /// there must surface as `PublishedDurabilityUnconfirmed` with the seal present (control 13),
    /// never as an incomplete outcome (control 12).
    pub(crate) const fn is_pre_commit(self, position: ExportFaultPositionV1) -> bool {
        match self {
            Self::SealRename => matches!(position, ExportFaultPositionV1::Before),
            Self::PostSealDirectorySync => false,
            _ => true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ExportFaultPositionV1 {
    Before,
    After,
}

/// Which ciphertext chunk of one artifact a chunk fault fires on. §6 samples the first, second,
/// and final chunk; `Final` names the last chunk by its flag, so the sample does not depend on
/// precomputing an artifact's envelope length.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExportChunkSelectorV1 {
    Ordinal(u32),
    Final,
}

#[cfg(test)]
thread_local! {
    static ARMED_FAULT: RefCell<Option<(ExportFaultPointV1, ExportFaultPositionV1, usize)>> =
        const { RefCell::new(None) };
    static ARMED_CHUNK_FAULT: RefCell<Option<(Vec<u8>, ExportChunkSelectorV1)>> =
        const { RefCell::new(None) };
}

#[cfg(test)]
pub(crate) struct ExportFaultGuardV1;

#[cfg(test)]
impl Drop for ExportFaultGuardV1 {
    fn drop(&mut self) {
        ARMED_FAULT.with(|slot| *slot.borrow_mut() = None);
        ARMED_CHUNK_FAULT.with(|slot| *slot.borrow_mut() = None);
    }
}

/// Arm the `ordinal`-th (1-based) arrival at `point`/`position`.
#[cfg(test)]
pub(crate) fn arm_export_fault_for_test(
    point: ExportFaultPointV1,
    position: ExportFaultPositionV1,
    ordinal: usize,
) -> ExportFaultGuardV1 {
    assert!(ordinal > 0, "fault ordinals are 1-based");
    ARMED_FAULT.with(|slot| *slot.borrow_mut() = Some((point, position, ordinal)));
    ExportFaultGuardV1
}

/// Arm the sink write of one artifact's selected ciphertext chunk (ordinals are 0-based, as the
/// envelope numbers them). §6 samples the first, second, and final chunk rather than exhausting
/// the matrix.
#[cfg(test)]
pub(crate) fn arm_export_chunk_fault_for_test(
    artifact: &[u8],
    selector: ExportChunkSelectorV1,
) -> ExportFaultGuardV1 {
    ARMED_CHUNK_FAULT.with(|slot| *slot.borrow_mut() = Some((artifact.to_vec(), selector)));
    ExportFaultGuardV1
}

#[cfg(test)]
fn fault(
    point: ExportFaultPointV1,
    position: ExportFaultPositionV1,
) -> Result<(), CustodyExportErrorV1> {
    let fire = ARMED_FAULT.with(|slot| {
        let mut slot = slot.borrow_mut();
        match slot.as_mut() {
            Some((armed_point, armed_position, remaining))
                if *armed_point == point && *armed_position == position =>
            {
                *remaining -= 1;
                *remaining == 0
            }
            _ => false,
        }
    });
    if fire {
        ARMED_FAULT.with(|slot| *slot.borrow_mut() = None);
        return Err(CustodyExportErrorV1::InjectedFault { point, position });
    }
    Ok(())
}

#[cfg(not(test))]
#[inline]
const fn fault(
    _point: ExportFaultPointV1,
    _position: ExportFaultPositionV1,
) -> Result<(), CustodyExportErrorV1> {
    Ok(())
}

#[cfg(test)]
fn chunk_fault(
    artifact: &[u8],
    chunk: &CustodyEnvelopeChunkV1,
) -> Result<(), CustodyExportErrorV1> {
    let fire = ARMED_CHUNK_FAULT.with(|slot| {
        slot.borrow().as_ref().is_some_and(|(name, selector)| {
            name == artifact
                && match selector {
                    ExportChunkSelectorV1::Ordinal(ordinal) => *ordinal == chunk.ordinal(),
                    ExportChunkSelectorV1::Final => chunk.final_chunk(),
                }
        })
    });
    if fire {
        ARMED_CHUNK_FAULT.with(|slot| *slot.borrow_mut() = None);
        return Err(CustodyExportErrorV1::InjectedChunkFault {
            ordinal: chunk.ordinal(),
        });
    }
    Ok(())
}

#[cfg(not(test))]
#[inline]
const fn chunk_fault(
    _artifact: &[u8],
    _chunk: &CustodyEnvelopeChunkV1,
) -> Result<(), CustodyExportErrorV1> {
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Exporter-local `#[cfg(test)]` seams (§7). Every one of these is a fixture affordance that
// bypasses an OUTER layer so a control's mutation is the only thing between the fixture and a
// wrong success; none of them is a guard in its own right.
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ExportBypassV1 {
    /// Isolate the post-exit callback (controls 5b, 9b).
    pub(crate) pre_spawn_callback: bool,
    /// Isolate the pre-spawn callback (controls 5a, 9a).
    pub(crate) post_exit_callback: bool,
    /// Let control 21 reach the disjointness preflight with a non-empty scratch root.
    pub(crate) scratch_emptiness: bool,
    /// Control 20b: install a pre-indexed pack in place of §5 step 1, so step 4 is the only
    /// strict-object guard left.
    pub(crate) preindexed_pack: bool,
    /// Control 3: skip §5 step 3's closure comparison. A root dropped from the pack is also a
    /// `?`-missing root to `rev-list`, so without this step 2's equality is not the only guard.
    pub(crate) closure_step: bool,
    /// Control 19: skip §6's post-seal plaintext identity comparison. A replayed stream is also a
    /// plaintext-digest mismatch there, so without this the capability binding is not the only
    /// guard.
    pub(crate) plaintext_identity_comparison: bool,
    /// Control 32: skip the §6 seal barrier. An in-place staging overwrite is also a barrier
    /// mismatch, so without this the content remeasurement is not the only guard.
    pub(crate) seal_barrier: bool,
}

#[cfg(test)]
thread_local! {
    static BYPASS: RefCell<ExportBypassV1> = const { RefCell::new(ExportBypassV1 {
        pre_spawn_callback: false,
        post_exit_callback: false,
        scratch_emptiness: false,
        preindexed_pack: false,
        closure_step: false,
        plaintext_identity_comparison: false,
        seal_barrier: false,
    }) };
    static GIT_GUARD_BYPASS: RefCell<crate::custody_git::GitGuardBypassV1> =
        RefCell::new(crate::custody_git::GitGuardBypassV1::default());
    static INDEX_PACK_STDIN_SUBSTITUTE: RefCell<Option<OsString>> = const { RefCell::new(None) };
    static SEAL_PUBLICATION_OVERRIDE: RefCell<Option<SealPublicationOverrideV1>> =
        const { RefCell::new(None) };
    static PACK_INPUT_OVERRIDE: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
    static PLAINTEXT_SUBSTITUTE: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
    static CAPSULE_RENAME_FAULT: RefCell<Option<(usize, crate::fs_custody::PublicationRenameFaultV1)>> =
        const { RefCell::new(None) };
    static LAST_PACK_ALLOWANCE: std::cell::Cell<(u64, u64)> = const { std::cell::Cell::new((0, 0)) };
    static HOOKS: RefCell<ExportHookTableV1> = RefCell::new(BTreeMap::new());
}

#[cfg(test)]
type ExportHookTableV1 = BTreeMap<ExportHookPointV1, Box<dyn Fn(&Path)>>;

#[cfg(test)]
pub(crate) struct ExportSeamGuardV1;

#[cfg(test)]
impl Drop for ExportSeamGuardV1 {
    fn drop(&mut self) {
        BYPASS.with(|slot| *slot.borrow_mut() = ExportBypassV1::default());
        GIT_GUARD_BYPASS
            .with(|slot| *slot.borrow_mut() = crate::custody_git::GitGuardBypassV1::default());
        INDEX_PACK_STDIN_SUBSTITUTE.with(|slot| *slot.borrow_mut() = None);
        SEAL_PUBLICATION_OVERRIDE.with(|slot| *slot.borrow_mut() = None);
        PACK_INPUT_OVERRIDE.with(|slot| *slot.borrow_mut() = None);
        PLAINTEXT_SUBSTITUTE.with(|slot| *slot.borrow_mut() = None);
        CAPSULE_RENAME_FAULT.with(|slot| *slot.borrow_mut() = None);
        HOOKS.with(|slot| slot.borrow_mut().clear());
    }
}

/// Controls 3 and 6: hand `pack-objects` an object list that differs from the manifest inventory
/// by exactly one object, so the staged pack is legitimately produced but does not match the
/// manifest. §5 step 2's inventory equality is then the only guard left.
#[cfg(test)]
pub(crate) fn override_pack_input_for_test(lines: Vec<u8>) -> ExportSeamGuardV1 {
    PACK_INPUT_OVERRIDE.with(|slot| *slot.borrow_mut() = Some(lines));
    ExportSeamGuardV1
}

/// Control 16: substitute the plaintext bytes handed to the `seal()` call of the artifact whose
/// name contains `marker`, while leaving the exporter's expected plaintext identity as planned.
///
/// The substitute is the planned plaintext with every byte inverted, so it has exactly the planned
/// length and differs in every byte: a length comparison cannot catch it and the exact-total
/// wrapper's SHA-256 is the only guard left. This bypasses the OUTER layer — the plan's own
/// correctness — exactly as §7 requires of a fixture affordance.
#[cfg(test)]
pub(crate) fn substitute_plaintext_for_test(marker: &[u8]) -> ExportSeamGuardV1 {
    PLAINTEXT_SUBSTITUTE.with(|slot| *slot.borrow_mut() = Some(marker.to_vec()));
    ExportSeamGuardV1
}

/// Control 14: arm `fs_custody`'s existing publication-rename fault on the `capsule/` pin, which
/// only the exporter can name. The seal rename is the first rename performed on that pin.
#[cfg(test)]
pub(crate) fn arm_capsule_rename_fault_for_test(
    nth: usize,
    shape: crate::fs_custody::PublicationRenameFaultV1,
) -> ExportSeamGuardV1 {
    CAPSULE_RENAME_FAULT.with(|slot| *slot.borrow_mut() = Some((nth, shape)));
    ExportSeamGuardV1
}

/// Control 35: the pack-output allowance `A` the exporter reserved, and the ledger use before
/// that reservation, so the control can construct the exact `B` and `B + 1` boundary.
#[cfg(test)]
pub(crate) fn last_pack_allowance_for_test() -> (u64, u64) {
    LAST_PACK_ALLOWANCE.with(std::cell::Cell::get)
}

#[cfg(test)]
pub(crate) fn set_export_bypass_for_test(bypass: ExportBypassV1) -> ExportSeamGuardV1 {
    BYPASS.with(|slot| *slot.borrow_mut() = bypass);
    ExportSeamGuardV1
}

#[cfg(test)]
fn bypassed(select: impl FnOnce(&ExportBypassV1) -> bool) -> bool {
    BYPASS.with(|slot| select(&slot.borrow()))
}

#[cfg(test)]
pub(crate) fn set_git_guard_bypass_for_test(
    bypass: crate::custody_git::GitGuardBypassV1,
) -> ExportSeamGuardV1 {
    GIT_GUARD_BYPASS.with(|slot| *slot.borrow_mut() = bypass);
    ExportSeamGuardV1
}

/// Control 17: hand §5 step 1 a byte-distinct pack with the same inventory while the exporter
/// still seals the pack it recorded.
#[cfg(test)]
pub(crate) fn substitute_index_pack_stdin_for_test(name: &str) -> ExportSeamGuardV1 {
    INDEX_PACK_STDIN_SUBSTITUTE.with(|slot| *slot.borrow_mut() = Some(OsString::from(name)));
    ExportSeamGuardV1
}

/// Control 14b: hand the commit-point classifier a publication outcome that `fs_custody` will
/// not produce on demand, so the classifier arm is the only thing under test.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SealPublicationOverrideV1 {
    RenameUnverified,
    TargetIdentityUnverified,
    ParentSyncAmbiguous,
}

#[cfg(test)]
pub(crate) fn override_seal_publication_for_test(
    outcome: SealPublicationOverrideV1,
) -> ExportSeamGuardV1 {
    SEAL_PUBLICATION_OVERRIDE.with(|slot| *slot.borrow_mut() = Some(outcome));
    ExportSeamGuardV1
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ExportHookPointV1 {
    /// Runs inside the pre-spawn callback, before its checks: controls 5b and 9b mutate here.
    PreSpawnCallback,
    /// Runs inside the post-exit callback, before its checks.
    PostExitCallback,
    /// Controls 1 and 2: seed a loose object into `verify.git` before §5 step 1.
    BeforeIndexPack,
    /// Controls 1 and 2: remove the seed after step 1 and before step 2.
    AfterIndexPack,
    /// Control 32: overwrite the staging file in place after its sink finish and sync.
    AfterStagingSync,
    /// Control 10: plant a symlink or alias at the reserved logical name before its rename.
    BeforeArtifactRename,
    /// Control 33: overwrite a published artifact in place before the seal barrier.
    AfterArtifactPublish,
}

#[cfg(test)]
pub(crate) fn install_export_hook_for_test(
    point: ExportHookPointV1,
    hook: impl Fn(&Path) + 'static,
) -> ExportSeamGuardV1 {
    HOOKS.with(|slot| slot.borrow_mut().insert(point, Box::new(hook)));
    ExportSeamGuardV1
}

#[cfg(test)]
fn run_hook(point: ExportHookPointV1, path: &Path) {
    let hook = HOOKS.with(|slot| slot.borrow_mut().remove(&point));
    if let Some(hook) = hook {
        hook(path);
        HOOKS.with(|slot| slot.borrow_mut().insert(point, hook));
    }
}

#[cfg(test)]
thread_local! {
    static DERIVE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static PACK_OBJECTS_SPAWNS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Control 11: prove `CustodyCapsuleLayoutV1::derive`, which allocates the full canonical
/// manifest encoding, is never entered after an over-limit preflight.
#[cfg(test)]
pub(crate) fn reset_export_counters_for_test() {
    DERIVE_CALLS.with(|slot| slot.set(0));
    PACK_OBJECTS_SPAWNS.with(|slot| slot.set(0));
}

#[cfg(test)]
pub(crate) fn derive_calls_for_test() -> usize {
    DERIVE_CALLS.with(std::cell::Cell::get)
}

/// Control 34: `pack-objects` is spawned exactly once per export.
#[cfg(test)]
pub(crate) fn pack_objects_spawns_for_test() -> usize {
    PACK_OBJECTS_SPAWNS.with(std::cell::Cell::get)
}

fn derive_layout(
    manifest: &CustodyManifestV1,
) -> Result<CustodyCapsuleLayoutV1, CustodyExportErrorV1> {
    #[cfg(test)]
    DERIVE_CALLS.with(|slot| slot.set(slot.get() + 1));
    CustodyCapsuleLayoutV1::derive(manifest).map_err(CustodyExportErrorV1::Capsule)
}

// ---------------------------------------------------------------------------------------------
// Errors and outcomes
// ---------------------------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub(crate) enum CustodyExportErrorV1 {
    #[error("caller budget exceeds the v1 {0} ceiling")]
    BudgetCeiling(&'static str),
    #[error("scratch ledger refused: {0}")]
    ScratchLedger(String),
    #[error("the canonical manifest encoding exceeds its ceiling")]
    CanonicalManifestTooLarge,
    #[error("the capture capability does not describe this manifest: {0}")]
    CapabilityBinding(&'static str),
    #[error("manifest object formats are mixed or differ from the capability's")]
    MixedObjectFormat,
    #[error("source repository state refuses export: {0}")]
    SourceState(&'static str),
    #[error("scratch root preflight refused: {0}")]
    ScratchPreflight(String),
    #[error("custody identity drifted: {0}")]
    IdentityDrift(String),
    #[error(transparent)]
    Git(CustodyGitError),
    #[error("Git child {command} exited {status:?}: {detail}")]
    GitChild {
        command: &'static str,
        status: Option<i32>,
        detail: String,
    },
    #[error("a strict Git object check refused: {0}")]
    StrictObjectCheck(String),
    #[error("strict indexing refused the recorded pack: {0}")]
    StrictPack(String),
    #[error("a manifest object is absent or has another kind: {0}")]
    ObjectPresence(String),
    #[error("the pack inventory does not equal the manifest inventory: {0}")]
    InventoryMismatch(String),
    #[error("the pack object closure is incomplete: {0}")]
    ClosureIncomplete(String),
    #[error("the verification input is not the recorded pack")]
    VerificationInputMismatch,
    #[error("the sealer receipt does not equal the expected receipt")]
    ReceiptMismatch,
    #[error("plaintext identity mismatch for {0}")]
    PlaintextIdentity(String),
    #[error("the sealer did not consume the exact plaintext of {0}")]
    PlaintextNotConsumed(String),
    #[error("a staged artifact changed content after its sink finished: {0}")]
    ContentRemeasurement(String),
    #[error("the seal barrier refused: {0}")]
    SealBarrier(String),
    #[error("custody capsule record refused: {0}")]
    Capsule(CustodyCapsuleErrorV1),
    #[error("custody seal record refused: {0}")]
    Seal(crate::custody_seal::CustodySealErrorV1),
    #[error(transparent)]
    Fs(#[from] FsCustodyError),
    #[error("publication refused: {0}")]
    Publication(String),
    #[error("an unexpected file appeared under a Git directory: {0}")]
    UnexpectedGitFile(String),
    #[error("Git produced output the exporter cannot parse: {0}")]
    GitOutput(String),
    #[error("io: {0}")]
    Io(String),
    #[error("injected fault at {point:?} ({position:?})")]
    InjectedFault {
        point: ExportFaultPointV1,
        position: ExportFaultPositionV1,
    },
    #[error("injected chunk fault at ordinal {ordinal}")]
    InjectedChunkFault { ordinal: u32 },
}

/// The record of one Git child: the exact argv, the Git version the runner admitted, the closed
/// environment keys, child status, and bounded stream evidence. Exit status alone is never
/// evidence.
#[derive(Clone, Debug)]
pub(crate) struct GitRunRecordV1 {
    pub(crate) label: &'static str,
    pub(crate) version: (u32, u32, u32),
    pub(crate) argv: Vec<OsString>,
    pub(crate) environment: BTreeMap<String, String>,
    pub(crate) exit_status: Option<i32>,
    pub(crate) stdin: GitStreamEvidenceV1,
    pub(crate) stdout: GitStreamEvidenceV1,
    pub(crate) stderr: GitStreamEvidenceV1,
}

#[derive(Clone, Debug)]
pub(crate) struct CustodyExportEvidenceV1 {
    pub(crate) git_version: (u32, u32, u32),
    pub(crate) object_format: CustodyGitObjectFormatV1,
    pub(crate) verified_pack_length: u64,
    pub(crate) verified_pack_sha256: [u8; 32],
    pub(crate) pack_hash: String,
    pub(crate) scratch_bytes_used: u64,
    pub(crate) runs: Vec<GitRunRecordV1>,
}

#[derive(Debug)]
pub(crate) struct CustodyExportSealedV1 {
    pub(crate) binding: CustodyCapsuleBindingV1,
    pub(crate) seal: CustodySealV1,
    pub(crate) evidence: CustodyExportEvidenceV1,
}

/// The §6 commit-point lattice. Only [`Self::Sealed`] attests a complete local capsule; every
/// other arm names the seal and claims neither a published seal nor an absent one.
#[must_use = "an export outcome must be classified: only `Sealed` is a complete local capsule"]
#[derive(Debug)]
pub(crate) enum CustodyExportOutcomeV1 {
    Sealed(Box<CustodyExportSealedV1>),
    /// `CustodyPublicationV1::RenameOutcomeUnverified`: neither the staged source nor the target
    /// proves whether the seal rename happened.
    SealPublicationUnverified {
        seal_name: String,
        detail: String,
    },
    /// `CustodyPublicationV1::TargetIdentityUnverified`: the rename landed and the parent synced,
    /// but the target could not be re-opened as the published object.
    SealPublicationTargetUnverified {
        seal_name: String,
        detail: String,
    },
    /// A fault at or after the parent sync of the committed seal rename.
    PublishedDurabilityUnconfirmed {
        seal_name: String,
        detail: String,
    },
}

// ---------------------------------------------------------------------------------------------
// The receipt comparison view (§6)
// ---------------------------------------------------------------------------------------------

/// The exporter-owned whole-receipt comparison view, built through the receipt's public
/// accessors. Control 15 perturbs this field by field, including a ciphertext length whose
/// SHA-256 is unchanged — a combination no genuine receipt can carry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReceiptFieldsV1 {
    pub(crate) artifact_name: Vec<u8>,
    pub(crate) manifest_digest: String,
    pub(crate) capsule_format: String,
    pub(crate) sealing_tool: String,
    pub(crate) sealing_tool_version: String,
    pub(crate) recipients: Vec<String>,
    pub(crate) ciphertext_length: u64,
    pub(crate) ciphertext_sha256: String,
}

impl ReceiptFieldsV1 {
    pub(crate) fn from_receipt(receipt: &CustodyEnvelopeSealReceiptV1) -> Self {
        Self {
            artifact_name: receipt.artifact_name().as_bytes().to_vec(),
            manifest_digest: receipt.manifest_digest().as_str().to_owned(),
            capsule_format: receipt.format().capsule_format().to_owned(),
            sealing_tool: receipt.format().sealing_tool().to_owned(),
            sealing_tool_version: receipt.format().sealing_tool_version().to_owned(),
            recipients: receipt.recipients().to_vec(),
            ciphertext_length: receipt.ciphertext_length(),
            ciphertext_sha256: receipt.ciphertext_sha256().as_str().to_owned(),
        }
    }
}

/// Whole-receipt equality over every field of the view. Written as an explicit conjunction so a
/// mutation that drops any single field is a compiling mutation control 15 can discriminate.
pub(crate) fn receipt_fields_equal(left: &ReceiptFieldsV1, right: &ReceiptFieldsV1) -> bool {
    left.artifact_name == right.artifact_name
        && left.manifest_digest == right.manifest_digest
        && left.capsule_format == right.capsule_format
        && left.sealing_tool == right.sealing_tool
        && left.sealing_tool_version == right.sealing_tool_version
        && left.recipients == right.recipients
        && left.ciphertext_length == right.ciphertext_length
        && left.ciphertext_sha256 == right.ciphertext_sha256
}

// ---------------------------------------------------------------------------------------------
// Plaintext sources and the exact-total consumption wrapper (§6)
// ---------------------------------------------------------------------------------------------

#[derive(Debug)]
enum PlaintextReaderV1 {
    Bytes { bytes: Vec<u8>, offset: usize },
    File(File),
}

/// A bounded plaintext chunk iterator. Chunk sizes come from the validated caller budget, never
/// from an unvalidated length.
#[derive(Debug)]
struct PlaintextChunksV1 {
    reader: PlaintextReaderV1,
    max_chunk_bytes: u64,
    total: u64,
    emitted: u64,
    ordinal: u32,
    done: bool,
}

impl PlaintextChunksV1 {
    fn next_chunk(&mut self) -> Result<Option<CustodyEnvelopeChunkV1>, CustodyExportErrorV1> {
        if self.done {
            return Ok(None);
        }
        let remaining = self.total - self.emitted;
        let take = remaining.min(self.max_chunk_bytes);
        let mut bytes = vec![
            0_u8;
            usize::try_from(take).map_err(|_| {
                CustodyExportErrorV1::Io("a chunk length does not fit this platform".into())
            })?
        ];
        match &mut self.reader {
            PlaintextReaderV1::Bytes {
                bytes: source,
                offset,
            } => {
                let window = offset
                    .checked_add(bytes.len())
                    .and_then(|end| source.get(*offset..end))
                    .ok_or_else(|| {
                        CustodyExportErrorV1::Io(
                            "a planned plaintext is shorter than its declared length".into(),
                        )
                    })?;
                bytes.copy_from_slice(window);
                *offset += window.len();
            }
            PlaintextReaderV1::File(file) => {
                file.read_exact(&mut bytes).map_err(|error| {
                    CustodyExportErrorV1::Io(format!("plaintext read: {error}"))
                })?;
            }
        }
        self.emitted += take;
        let final_chunk = self.emitted == self.total;
        let chunk = CustodyEnvelopeChunkV1::new(self.ordinal, bytes, final_chunk)
            .map_err(CustodyExportErrorV1::Capsule)?;
        self.ordinal += 1;
        self.done = final_chunk;
        Ok(Some(chunk))
    }
}

/// The exporter-owned exact-total source wrapper every `seal()` call reads its plaintext
/// through. A sealer that consumes only a prefix, or nothing, fails `finish` or the plaintext
/// identity comparison, and the export refuses before any exterior seal.
struct ExactTotalPlaintextSourceV1<'a> {
    descriptor: CustodyEnvelopeSourceDescriptorV1,
    chunks: PlaintextChunksV1,
    validator: Option<CustodyEnvelopeSourceValidatorV1>,
    failure: &'a RefCell<Option<CustodyExportErrorV1>>,
}

impl sealed::Sealed for ExactTotalPlaintextSourceV1<'_> {}

impl CustodyEnvelopeChunkSourceV1 for ExactTotalPlaintextSourceV1<'_> {
    fn descriptor(&self) -> &CustodyEnvelopeSourceDescriptorV1 {
        &self.descriptor
    }

    fn next_chunk(&mut self) -> Result<Option<CustodyEnvelopeChunkV1>, CustodyCapsuleErrorV1> {
        let chunk = match self.chunks.next_chunk() {
            Ok(chunk) => chunk,
            Err(error) => return Err(self.record(error)),
        };
        if let Some(chunk) = &chunk {
            let validator = self
                .validator
                .as_mut()
                .ok_or(CustodyCapsuleErrorV1::InvalidInput)?;
            validator.accept_chunk(chunk)?;
        }
        Ok(chunk)
    }
}

impl ExactTotalPlaintextSourceV1<'_> {
    fn record(&self, error: CustodyExportErrorV1) -> CustodyCapsuleErrorV1 {
        *self.failure.borrow_mut() = Some(error);
        CustodyCapsuleErrorV1::InvalidInput
    }

    fn finish(&mut self) -> Result<(u64, Sha256HexV1), CustodyExportErrorV1> {
        let validator = self.validator.take().ok_or_else(|| {
            CustodyExportErrorV1::Io("the source wrapper was already finished".into())
        })?;
        let receipt = validator.finish().map_err(CustodyExportErrorV1::Capsule)?;
        Ok((receipt.total_bytes(), receipt.sha256().clone()))
    }
}

/// The destination sink. It owns its own [`CustodyEnvelopeSinkValidatorV1`] and admits a chunk to
/// that validator only after the chunk's `write_all` has succeeded, so the completion describes
/// exactly the bytes this destination durably accepted.
struct CapsuleDestinationSinkV1<'a> {
    file: &'a mut File,
    limits: CustodyEnvelopeStreamLimitsV1,
    validator: Option<CustodyEnvelopeSinkValidatorV1>,
    ledger: &'a RefCell<ScratchLedgerV1>,
    failure: &'a RefCell<Option<CustodyExportErrorV1>>,
    artifact_name: Vec<u8>,
    written: u64,
    chunks: u32,
    max_artifact_bytes: u64,
}

impl sealed::Sealed for CapsuleDestinationSinkV1<'_> {}

impl CustodyEnvelopeChunkSinkV1 for CapsuleDestinationSinkV1<'_> {
    fn limits(&self) -> CustodyEnvelopeStreamLimitsV1 {
        self.limits
    }

    fn write_chunk(&mut self, chunk: CustodyEnvelopeChunkV1) -> Result<(), CustodyCapsuleErrorV1> {
        if let Err(error) = chunk_fault(&self.artifact_name, &chunk) {
            return Err(self.record(error));
        }
        let length = chunk.bytes().len() as u64;
        // The caller budget's chunk-byte and chunk-count ceilings are enforced before the chunk is
        // charged or written, so an over-limit chunk never reaches the file or the ledger. The
        // 2B1 validator below re-checks both after the write; it is the second layer, not the
        // first.
        if length > self.limits.max_chunk_bytes() {
            return Err(self.record(CustodyExportErrorV1::BudgetCeiling("chunk bytes")));
        }
        if self.chunks >= self.limits.max_chunks() {
            return Err(self.record(CustodyExportErrorV1::BudgetCeiling("chunk count")));
        }
        let next = match self.written.checked_add(length) {
            Some(next) if next <= self.max_artifact_bytes => next,
            _ => {
                return Err(self.record(CustodyExportErrorV1::ScratchLedger(
                    "an artifact exceeded its per-artifact ciphertext ceiling".into(),
                )))
            }
        };
        if let Err(error) = self.ledger.borrow_mut().reserve(length) {
            return Err(self.record(error));
        }
        if let Err(error) = self.file.write_all(chunk.bytes()) {
            return Err(self.record(CustodyExportErrorV1::Io(format!(
                "ciphertext write: {error}"
            ))));
        }
        let validator = self
            .validator
            .as_mut()
            .ok_or(CustodyCapsuleErrorV1::InvalidInput)?;
        validator.accept_chunk(&chunk)?;
        self.written = next;
        self.chunks += 1;
        Ok(())
    }
}

impl CapsuleDestinationSinkV1<'_> {
    fn record(&self, error: CustodyExportErrorV1) -> CustodyCapsuleErrorV1 {
        *self.failure.borrow_mut() = Some(error);
        CustodyCapsuleErrorV1::InvalidInput
    }
}

// ---------------------------------------------------------------------------------------------
// The exporter
// ---------------------------------------------------------------------------------------------

pub(crate) struct CustodyExportRequestV1<'a> {
    pub(crate) manifest: &'a CustodyManifestV1,
    pub(crate) envelope_format: &'a CustodyEnvelopeFormatV1,
    pub(crate) recipients: &'a [String],
    pub(crate) budgets: CustodyExportBudgetsV1,
    pub(crate) sealer: &'a dyn CustodyEnvelopeSealerV1,
    pub(crate) capability: CustodyCaptureCapabilityV1,
    pub(crate) git_route: GitRouteRequestV1,
    pub(crate) scratch_root: &'a Path,
    pub(crate) deadline: Instant,
}

struct ExportContextV1<'a> {
    runner: &'a GitRunnerV1,
    work: &'a PinnedDirectoryV1,
    scratch: &'a PinnedDirectoryV1,
    capability: &'a CustodyCaptureCapabilityV1,
}

/// Export one validated sealable manifest's derived 2B1 capsule layout into a new owner-private
/// scratch root, and prove its Git pack is an exact, self-contained object closure from a fresh
/// isolated object database.
///
/// Every failure is typed, leaves the source unchanged, and reports an incomplete local outcome.
pub(crate) fn export_capsule_v1(
    request: CustodyExportRequestV1<'_>,
) -> Result<CustodyExportOutcomeV1, CustodyExportErrorV1> {
    let CustodyExportRequestV1 {
        manifest,
        envelope_format,
        recipients,
        budgets,
        sealer,
        capability,
        git_route,
        scratch_root,
        deadline,
    } = request;

    let budgets = budgets.validate()?;

    // The canonical-encoding ceiling is checked with a bounded streaming serializer FIRST,
    // because `CustodyCapsuleLayoutV1::derive` calls `manifest.content_digest()`, which allocates
    // the full encoding. Control 11's call counter proves the ordering.
    preflight_canonical_manifest(manifest, budgets.max_canonical_json_bytes)?;

    capability.bind_to_manifest(manifest)?;
    capability.refuse_unsupported_source_state()?;

    let layout = derive_layout(manifest)?;
    if layout.artifact_count() > budgets.max_artifacts {
        return Err(CustodyExportErrorV1::BudgetCeiling("artifact count"));
    }

    let scratch = preflight_scratch_root(scratch_root, &capability)?;
    let ledger = RefCell::new(ScratchLedgerV1::new(budgets.max_scratch_bytes));

    ledger.borrow_mut().reserve_entries(2)?;
    let capsule = scratch
        .create_new_child_directory(OsStr::new(CAPSULE_DIR_NAME), "custody export capsule root")?;
    let work = scratch
        .create_new_child_directory(OsStr::new(WORK_DIR_NAME), "custody export work root")?;
    #[cfg(test)]
    if let Some((nth, shape)) = CAPSULE_RENAME_FAULT.with(|slot| *slot.borrow()) {
        capsule.fail_publication_rename_on_nth_call_for_test(nth, shape);
    }

    ledger.borrow_mut().reserve_entries(2)?;
    let _home =
        work.create_new_child_directory(OsStr::new(HOME_DIR_NAME), "custody export HOME")?;
    let _xdg = work.create_new_child_directory(OsStr::new(XDG_DIR_NAME), "custody export XDG")?;

    let init_names = git_root_names(SOURCE_GIT_DIR_NAME)?;
    let runner = GitRunnerV1::admit(git_route, &work, &init_names, deadline)
        .map_err(CustodyExportErrorV1::Git)?;

    let context = ExportContextV1 {
        runner: &runner,
        work: &work,
        scratch: &scratch,
        capability: &capability,
    };

    let mut runs = Vec::new();
    let object_format = capability.git_object_format();

    // §4.1: the synthesized source git directory is created by `InitBare`, which takes no
    // GIT_DIR, and the exporter writes nothing else into it (control 30). Each git directory
    // carries the budget its post-exit re-measure (§3) is checked against.
    let mut source_budget = GitDirectoryBudgetV1::for_init(SOURCE_GIT_DIR_NAME);
    let mut verify_budget = GitDirectoryBudgetV1::for_init(VERIFY_GIT_DIR_NAME);
    init_bare_git_directory(
        &context,
        &ledger,
        &mut runs,
        &mut source_budget,
        object_format,
        deadline,
    )?;
    init_bare_git_directory(
        &context,
        &ledger,
        &mut runs,
        &mut verify_budget,
        object_format,
        deadline,
    )?;

    let source_names = git_root_names(SOURCE_GIT_DIR_NAME)?;
    let verify_names = git_root_names(VERIFY_GIT_DIR_NAME)?;

    let object_lines = inventory_stdin(&capability);

    // §4.2 step 1: prove every manifest object exists with the declared kind.
    prove_object_presence(
        &context,
        &source_names,
        &mut source_budget,
        &ledger,
        &mut runs,
        &object_lines,
        deadline,
    )?;

    // §4.2 steps 2-3: one pack-objects run, streamed into one create-new file.
    let pack = produce_pack(
        &context,
        &source_names,
        &mut source_budget,
        &ledger,
        &mut runs,
        &object_lines,
        budgets,
        deadline,
    )?;

    // §5: the isolated closure proof.
    let pack_hash = prove_isolated_closure(
        &context,
        &verify_names,
        &mut verify_budget,
        &ledger,
        &mut runs,
        &pack,
        object_format,
        deadline,
    )?;

    // §6: publication.
    let restore_policy = CustodyRestorePolicyV1::inert();
    let plan = build_artifact_plan(
        manifest,
        &layout,
        &restore_policy,
        &capability,
        &pack,
        budgets,
    )?;

    let mut published: Vec<PublishedArtifactV1> = Vec::new();
    let mut receipts = Vec::new();
    let manifest_digest = manifest
        .content_digest()
        .map_err(CustodyExportErrorV1::Seal)?;
    let mut directories: BTreeMap<Vec<u8>, PinnedDirectoryV1> = BTreeMap::new();

    for (ordinal, artifact) in plan.into_iter().enumerate() {
        let sealed = seal_one_artifact(SealOneArtifactV1 {
            capsule: &capsule,
            directories: &mut directories,
            pack: &pack,
            ledger: &ledger,
            sealer,
            envelope_format,
            recipients,
            manifest_digest: &manifest_digest,
            budgets,
            ordinal,
            artifact,
        })?;
        receipts.push(sealed.receipt.clone());
        published.push(sealed);
    }

    let seal_proof = CustodyCapsuleSealProofV1::from_receipts(receipts)
        .map_err(CustodyExportErrorV1::Capsule)?;

    fault(
        ExportFaultPointV1::BindingConstruction,
        ExportFaultPositionV1::Before,
    )?;
    let binding =
        CustodyCapsuleBindingV1::new(manifest, layout.index(), &restore_policy, &seal_proof)
            .map_err(CustodyExportErrorV1::Capsule)?;
    fault(
        ExportFaultPointV1::BindingConstruction,
        ExportFaultPositionV1::After,
    )?;

    let seal = seal_proof.seal().clone();
    let outcome = publish_seal(PublishSealV1 {
        capsule: &capsule,
        ledger: &ledger,
        seal: &seal,
        budgets,
        barrier: SealBarrierV1 {
            scratch: &scratch,
            capsule: &capsule,
            directories: &directories,
            published: &published,
        },
    })?;
    let seal_name = SEAL_NAME.to_owned();
    Ok(match outcome {
        SealPublicationV1::Durable => {
            let git_version = runs
                .first()
                .map_or(GitRunnerV1::MINIMUM_VERSION, |run| run.version);
            CustodyExportOutcomeV1::Sealed(Box::new(CustodyExportSealedV1 {
                binding,
                seal,
                evidence: CustodyExportEvidenceV1 {
                    git_version,
                    object_format: capability.object_format,
                    verified_pack_length: pack.length,
                    verified_pack_sha256: pack.sha256,
                    pack_hash,
                    scratch_bytes_used: ledger.borrow().used(),
                    runs,
                },
            }))
        }
        SealPublicationV1::RenameUnverified(detail) => {
            CustodyExportOutcomeV1::SealPublicationUnverified { seal_name, detail }
        }
        SealPublicationV1::TargetIdentityUnverified(detail) => {
            CustodyExportOutcomeV1::SealPublicationTargetUnverified { seal_name, detail }
        }
        SealPublicationV1::DurabilityUnconfirmed(detail) => {
            CustodyExportOutcomeV1::PublishedDurabilityUnconfirmed { seal_name, detail }
        }
    })
}

/// Serialize the manifest through a bounded counting writer that refuses past the ceiling
/// without ever holding the whole encoding.
fn preflight_canonical_manifest(
    manifest: &CustodyManifestV1,
    limit: usize,
) -> Result<(), CustodyExportErrorV1> {
    struct BoundedCountingWriterV1 {
        limit: usize,
        written: usize,
    }
    impl std::io::Write for BoundedCountingWriterV1 {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.written = self.written.saturating_add(buffer.len());
            if self.written > self.limit {
                return Err(std::io::Error::other(
                    "canonical encoding exceeds its ceiling",
                ));
            }
            Ok(buffer.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let mut writer = BoundedCountingWriterV1 { limit, written: 0 };
    serde_json::to_writer(&mut writer, manifest)
        .map_err(|_| CustodyExportErrorV1::CanonicalManifestTooLarge)
}

/// §2 preflight: the scratch root must be owner-private and empty, and must not be, contain, or
/// lie inside the source repository, its git directory, or any pinned alternate store. The
/// comparison runs in both directions over canonical paths, and over the directory identity of
/// every ancestor captured here.
fn preflight_scratch_root(
    scratch_root: &Path,
    capability: &CustodyCaptureCapabilityV1,
) -> Result<PinnedDirectoryV1, CustodyExportErrorV1> {
    let canonical = scratch_root.canonicalize().map_err(|error| {
        CustodyExportErrorV1::ScratchPreflight(format!(
            "cannot canonicalize the scratch root: {error}"
        ))
    })?;

    refuse_source_overlap(&canonical, capability)?;

    let pin = PinnedDirectoryV1::open(&canonical, "custody export scratch root")?;

    #[cfg(test)]
    if bypassed(|bypass| bypass.scratch_emptiness) {
        return Ok(pin);
    }

    let metadata = std::fs::symlink_metadata(&canonical).map_err(|error| {
        CustodyExportErrorV1::ScratchPreflight(format!("cannot inspect the scratch root: {error}"))
    })?;
    {
        use std::os::unix::fs::MetadataExt as _;
        // SAFETY: `geteuid` has no preconditions and reads this process's effective uid.
        let effective_uid = unsafe { libc::geteuid() };
        if metadata.uid() != effective_uid || metadata.mode() & 0o077 != 0 {
            return Err(CustodyExportErrorV1::ScratchPreflight(
                "the scratch root is not owner-private".into(),
            ));
        }
    }
    let entries = std::fs::read_dir(&canonical).map_err(|error| {
        CustodyExportErrorV1::ScratchPreflight(format!(
            "cannot enumerate the scratch root: {error}"
        ))
    })?;
    if entries.count() != 0 {
        return Err(CustodyExportErrorV1::ScratchPreflight(
            "the scratch root is not empty".into(),
        ));
    }
    Ok(pin)
}

/// §2 disjointness: no exporter write may land in the source. The scratch root must not be,
/// contain, or lie inside the source repository root (a non-bare source's worktree), the source
/// git directory, the primary object store, or any pinned alternate store. Canonical paths are
/// compared in both directions, and so are directory identities: every ancestor of the scratch
/// root (itself included) against each protected path, and every ancestor of each protected path
/// (itself included) against the scratch root, so an alias that path comparison cannot see is
/// still refused.
fn refuse_source_overlap(
    canonical: &Path,
    capability: &CustodyCaptureCapabilityV1,
) -> Result<(), CustodyExportErrorV1> {
    let refuse = |relation: &str, store: &Path| {
        Err(CustodyExportErrorV1::ScratchPreflight(format!(
            "the scratch root {} {relation} the protected source path {}",
            canonical.display(),
            store.display()
        )))
    };
    let identity = |path: &Path| {
        crate::fs_custody::directory_dev_ino(path).map_err(CustodyExportErrorV1::ScratchPreflight)
    };
    let scratch_identity = identity(canonical)?;
    for protected in capability.protected_paths() {
        if canonical == protected
            || canonical.starts_with(&protected)
            || protected.starts_with(canonical)
        {
            return refuse("is, contains, or lies inside", &protected);
        }
        let protected_identity = identity(&protected)?;
        for ancestor in canonical.ancestors() {
            if identity(ancestor)? == protected_identity {
                return refuse("is or lies inside", &protected);
            }
        }
        for ancestor in protected.ancestors() {
            if identity(ancestor)? == scratch_identity {
                return refuse("is or contains", &protected);
            }
        }
    }
    Ok(())
}

fn git_root_names(git_dir: &str) -> Result<GitRootNamesV1, CustodyExportErrorV1> {
    GitRootNamesV1::new(HOME_DIR_NAME, XDG_DIR_NAME, git_dir).map_err(CustodyExportErrorV1::Git)
}

fn inventory_stdin(capability: &CustodyCaptureCapabilityV1) -> Vec<u8> {
    let mut ids: Vec<&str> = capability
        .inventory
        .iter()
        .map(|(_, object_id, _)| object_id.as_str())
        .collect();
    ids.sort_unstable();
    let mut bytes = Vec::new();
    for id in ids {
        bytes.extend_from_slice(id.as_bytes());
        bytes.push(b'\n');
    }
    bytes
}

fn stdout_limit_for_objects(count: usize) -> usize {
    count
        .saturating_mul(GIT_LINE_BYTES_V1)
        .saturating_add(GIT_MIN_STDOUT_LIMIT_V1)
}

/// A widest `verify-pack -v` delta row, less its two hex object names:
/// `<oid> <type> <size> <size-in-pack> <offset> <depth> <base-oid>\n` holds the `%-6s` type, three
/// `uintmax_t` decimals of at most 20 digits, a `%u` depth of at most 10, six separators, and a
/// newline.
const VERIFY_PACK_ROW_BYTES_V1: usize = 6 + 3 * 20 + 10 + 6 + 1;
/// A widest chain-length histogram line, `chain length = %d: %lu objects\n`. There is at most one
/// per object.
const VERIFY_PACK_HISTOGRAM_BYTES_V1: usize = 15 + 10 + 2 + 20 + 9;

/// The `verify-pack -v` stdout bound for `objects` objects. Unlike the other listings, a delta row
/// carries TWO object names, so the bound depends on the object format: `2·W + 83` bytes for the
/// widest row at hex width `W`, plus one histogram line, per object. The `non delta` and final
/// `<pack>: ok` lines fit the fixed floor. The runner fixes `LC_ALL=C`, so these are Git's
/// untranslated formats. Every step is checked.
fn verify_pack_stdout_limit(
    objects: usize,
    object_format: GitObjectFormatV1,
) -> Result<usize, CustodyExportErrorV1> {
    let hex_width = match object_format {
        GitObjectFormatV1::Sha1 => 40,
        GitObjectFormatV1::Sha256 => 64,
    };
    let per_object = 2 * hex_width + VERIFY_PACK_ROW_BYTES_V1 + VERIFY_PACK_HISTOGRAM_BYTES_V1;
    objects
        .checked_mul(per_object)
        .and_then(|bytes| bytes.checked_add(GIT_MIN_STDOUT_LIMIT_V1))
        .ok_or_else(|| {
            CustodyExportErrorV1::Io(
                "the verify-pack output bound does not fit this platform".into(),
            )
        })
}

// ---------------------------------------------------------------------------------------------
// Git children
// ---------------------------------------------------------------------------------------------

fn run_git(
    context: &ExportContextV1<'_>,
    names: &GitRootNamesV1,
    label: &'static str,
    request: GitRunRequestV1,
    runs: &mut Vec<GitRunRecordV1>,
) -> Result<GitRunResultV1, CustodyExportErrorV1> {
    #[cfg(test)]
    let request = {
        let mut request = request;
        request.guard_bypass = GIT_GUARD_BYPASS.with(|slot| *slot.borrow());
        if matches!(request.command, GitCommandV1::PackObjectsStdout) {
            PACK_OBJECTS_SPAWNS.with(|slot| slot.set(slot.get() + 1));
        }
        request
    };

    fault(ExportFaultPointV1::GitSpawn, ExportFaultPositionV1::Before)?;
    let callback_error: RefCell<Option<CustodyExportErrorV1>> = RefCell::new(None);
    let result = context.runner.run(
        context.work,
        names,
        request,
        || match pre_spawn_checks(context) {
            Ok(()) => Ok(()),
            Err(error) => {
                *callback_error.borrow_mut() = Some(error);
                Err(CustodyGitError::RouteRefusal(
                    "the custody pre-spawn recheck refused".into(),
                ))
            }
        },
        || match post_exit_checks(context) {
            Ok(()) => Ok(()),
            Err(error) => {
                *callback_error.borrow_mut() = Some(error);
                Err(CustodyGitError::RouteRefusal(
                    "the custody post-exit recheck refused".into(),
                ))
            }
        },
    );
    let callback_error = callback_error.into_inner();
    let result = match result {
        Ok(result) => {
            if let Some(error) = callback_error {
                return Err(error);
            }
            result
        }
        Err(error @ CustodyGitError::BinaryDrift(_)) => {
            return Err(CustodyExportErrorV1::Git(error))
        }
        Err(error) => {
            return Err(callback_error.unwrap_or(CustodyExportErrorV1::Git(error)));
        }
    };
    fault(ExportFaultPointV1::GitSpawn, ExportFaultPositionV1::After)?;
    fault(
        ExportFaultPointV1::GitStdinClose,
        ExportFaultPositionV1::Before,
    )?;
    fault(
        ExportFaultPointV1::GitStdinClose,
        ExportFaultPositionV1::After,
    )?;
    fault(
        ExportFaultPointV1::GitStdoutComplete,
        ExportFaultPositionV1::Before,
    )?;
    fault(
        ExportFaultPointV1::GitStdoutComplete,
        ExportFaultPositionV1::After,
    )?;

    runs.push(GitRunRecordV1 {
        label,
        version: result.evidence.version,
        argv: result.evidence.argv.clone(),
        environment: result.evidence.environment.clone(),
        exit_status: result.evidence.exit_status,
        stdin: result.evidence.stdin.clone(),
        stdout: result.evidence.stdout.clone(),
        stderr: result.evidence.stderr.clone(),
    });
    Ok(result)
}

fn pre_spawn_checks(context: &ExportContextV1<'_>) -> Result<(), CustodyExportErrorV1> {
    #[cfg(test)]
    run_hook(
        ExportHookPointV1::PreSpawnCallback,
        context.work.canonical_path(),
    );
    #[cfg(test)]
    if bypassed(|bypass| bypass.pre_spawn_callback) {
        return Ok(());
    }
    identity_checks(context)
}

fn post_exit_checks(context: &ExportContextV1<'_>) -> Result<(), CustodyExportErrorV1> {
    #[cfg(test)]
    run_hook(
        ExportHookPointV1::PostExitCallback,
        context.work.canonical_path(),
    );
    fault(
        ExportFaultPointV1::GitPostExitRecheck,
        ExportFaultPositionV1::Before,
    )?;
    #[cfg(test)]
    if bypassed(|bypass| bypass.post_exit_callback) {
        return Ok(());
    }
    identity_checks(context)?;
    fault(
        ExportFaultPointV1::GitPostExitRecheck,
        ExportFaultPositionV1::After,
    )
}

fn identity_checks(context: &ExportContextV1<'_>) -> Result<(), CustodyExportErrorV1> {
    pinned_root_unchanged(context.scratch).map_err(CustodyExportErrorV1::IdentityDrift)?;
    pinned_root_unchanged(context.work).map_err(CustodyExportErrorV1::IdentityDrift)?;
    context.capability.recheck()
}

/// The generic exit-status check. It deliberately does not classify strict-object messages:
/// §5 step 1 (`classify_index_pack`) and step 4 (`check_fsck_output`) each own their classifier,
/// so each is a separable guard that controls 20a and 20b can discriminate.
fn require_success(
    result: &GitRunResultV1,
    command: &'static str,
) -> Result<(), CustodyExportErrorV1> {
    if result.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&result.stderr).trim().to_owned();
    Err(CustodyExportErrorV1::GitChild {
        command,
        status: result.evidence.exit_status,
        detail: bounded(&detail),
    })
}

/// §5 step 1's own refusal classifier. A strict fsck-class rejection of an object carries a Git
/// message id and is a dedicated `StrictObjectCheck` (control 20a); any other refusal of the
/// recorded pack — truncation, corruption, a broken link — is a typed `StrictPack` (control 7).
fn classify_index_pack(result: &GitRunResultV1) -> Result<(), CustodyExportErrorV1> {
    if result.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&result.stderr).trim().to_owned();
    if let Some(message_id) = strict_object_message_id(&detail) {
        return Err(CustodyExportErrorV1::StrictObjectCheck(message_id));
    }
    Err(CustodyExportErrorV1::StrictPack(bounded(&detail)))
}

fn bounded(text: &str) -> String {
    text.chars().take(512).collect()
}

/// A strict fsck-class rejection of a legitimate historical object carries a Git message id such
/// as `zeroPaddedFilemode`. Both `index-pack --strict` (§5 step 1) and `fsck --strict` (step 4)
/// report one, and each is classified as a dedicated `StrictObjectCheck` refusal rather than a
/// generic ambiguity.
fn strict_object_message_id(text: &str) -> Option<String> {
    for line in text.lines().take(64) {
        // Scan every bounded line rather than only the first: `index-pack --strict` and
        // `fsck --strict` both interleave their object report with other output, and a
        // first-line-only classifier would demote a genuine strict rejection to a generic child
        // failure. A line that does not carry a message id is skipped, never terminal.
        let Some(body) = line
            .strip_prefix("error: object ")
            .or_else(|| line.strip_prefix("error in tree "))
            .or_else(|| line.strip_prefix("error in commit "))
            .or_else(|| line.strip_prefix("error in blob "))
            .or_else(|| line.strip_prefix("error in tag "))
            .or_else(|| line.strip_prefix("warning: object "))
        else {
            continue;
        };
        let Some((_, rest)) = body.split_once(": ") else {
            continue;
        };
        let Some((message_id, _)) = rest.split_once(':') else {
            continue;
        };
        if !message_id.is_empty()
            && message_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Some(bounded(message_id));
        }
    }
    None
}

fn init_bare_git_directory(
    context: &ExportContextV1<'_>,
    ledger: &RefCell<ScratchLedgerV1>,
    runs: &mut Vec<GitRunRecordV1>,
    budget: &mut GitDirectoryBudgetV1,
    object_format: GitObjectFormatV1,
    deadline: Instant,
) -> Result<(), CustodyExportErrorV1> {
    // The nine entry allowances and, apart from them, the logical-byte bound for `HEAD` and
    // `config`; the post-exit re-measure reconciles the latter to the bytes Git wrote.
    ledger.borrow_mut().reserve_entries(budget.entries)?;
    ledger.borrow_mut().reserve(budget.logical_bytes)?;

    let names = git_root_names(budget.directory)?;
    let request = GitRunRequestV1::new(
        GitCommandV1::InitBare {
            dir: budget.directory.to_owned(),
            object_format,
        },
        Vec::new(),
        GIT_MIN_STDOUT_LIMIT_V1,
        GIT_STDERR_LIMIT_V1,
        deadline,
    );
    let result = run_git(context, &names, "init --bare", request, runs)?;
    remeasure_git_directory(context, budget, ledger)?;
    require_success(&result, "init --bare")
}

/// What one git directory under `work/` may hold once a child exits: its enumerated files, the
/// logical bytes the ledger charges for them, and the entries it charges at the per-entry
/// allowance. §3 requires the re-measure after EVERY child, including the read-only ones, which
/// add nothing to either figure.
#[derive(Debug)]
struct GitDirectoryBudgetV1 {
    directory: &'static str,
    files: BTreeSet<String>,
    /// The logical file bytes this directory may hold. Each writing child adds its proven bound
    /// before it runs; each re-measure reconciles the figure, and the ledger, down to the bytes
    /// measured.
    logical_bytes: u64,
    /// Created entries, charged at the per-entry allowance. They are kept apart from
    /// `logical_bytes`, so an allowance can never absorb file bytes the ledger did not charge.
    entries: u64,
}

impl GitDirectoryBudgetV1 {
    fn for_init(directory: &'static str) -> Self {
        Self {
            directory,
            files: INIT_BARE_FILES_V1
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
            logical_bytes: INIT_BARE_LOGICAL_BYTES_V1,
            entries: INIT_BARE_ENTRIES_V1,
        }
    }

    /// Admit the pack, index, and reverse index `index-pack` writes, at its proven logical bound
    /// plus their three entries.
    fn admit_index_pack(
        &mut self,
        pack_hash: &str,
        logical_bytes: u64,
    ) -> Result<(), CustodyExportErrorV1> {
        let overflow =
            || CustodyExportErrorV1::ScratchLedger("a git directory budget overflowed".into());
        self.logical_bytes = self
            .logical_bytes
            .checked_add(logical_bytes)
            .ok_or_else(overflow)?;
        self.entries = self
            .entries
            .checked_add(INDEX_PACK_ENTRIES_V1)
            .ok_or_else(overflow)?;
        for extension in ["pack", "idx", "rev"] {
            self.files
                .insert(format!("objects/pack/pack-{pack_hash}.{extension}"));
        }
        Ok(())
    }
}

/// §3's proven upper bound on the logical bytes of one `index-pack --stdin` into an empty
/// verification database. The temporary and final pack count once at the staged pack length,
/// because the temporary pack is renamed rather than copied. For `objects` objects and a raw hash
/// width `hash_width`, the version-2 index is at most `1072 + N·(H + 8) + 8·N + 2·H` bytes and the
/// reverse index at most `12 + 4·N + 2·H`; no bitmap or `.keep` file is requested. The three files'
/// entries are charged separately, as `INDEX_PACK_ENTRIES_V1`. Every step is checked.
fn index_pack_logical_bound(
    pack_length: u64,
    objects: u64,
    hash_width: u64,
) -> Result<u64, CustodyExportErrorV1> {
    let overflow = || CustodyExportErrorV1::ScratchLedger("the index-pack bound overflowed".into());
    let trailer = hash_width.checked_mul(2).ok_or_else(overflow)?;
    let index = hash_width
        .checked_add(8)
        .and_then(|row| objects.checked_mul(row))
        .and_then(|rows| rows.checked_add(objects.checked_mul(8)?))
        .and_then(|rows| rows.checked_add(1072))
        .and_then(|bytes| bytes.checked_add(trailer))
        .ok_or_else(overflow)?;
    let reverse = objects
        .checked_mul(4)
        .and_then(|rows| rows.checked_add(12))
        .and_then(|bytes| bytes.checked_add(trailer))
        .ok_or_else(overflow)?;
    pack_length
        .checked_add(index)
        .and_then(|total| total.checked_add(reverse))
        .ok_or_else(overflow)
}

/// Re-measure one git directory after a child exits, hold its logical bytes to their own bound,
/// and reconcile that reservation, in the budget and in the ledger, down to the measured bytes.
/// The ledger then charges exactly the logical bytes Git wrote there, as §3 requires.
fn remeasure_git_directory(
    context: &ExportContextV1<'_>,
    budget: &mut GitDirectoryBudgetV1,
    ledger: &RefCell<ScratchLedgerV1>,
) -> Result<(), CustodyExportErrorV1> {
    let git_dir = context.work.open_existing_child_directory(
        OsStr::new(budget.directory),
        "custody export git directory",
    )?;
    let measured = measure_git_directory(&git_dir)?;
    verify_git_writes(
        &measured,
        &budget.files,
        budget.logical_bytes,
        budget.directory,
    )?;
    let unused = budget.logical_bytes.saturating_sub(measured.logical_bytes);
    ledger.borrow_mut().release(unused);
    budget.logical_bytes = measured.logical_bytes;
    Ok(())
}

#[derive(Debug, Default)]
struct GitDirectoryMeasurementV1 {
    files: BTreeSet<String>,
    logical_bytes: u64,
}

/// Bounded recursive re-measure of every file a Git child may have written under one git
/// directory, so an unexpected file or an over-reservation total is a typed budget refusal.
fn measure_git_directory(
    git_dir: &PinnedDirectoryV1,
) -> Result<GitDirectoryMeasurementV1, CustodyExportErrorV1> {
    let mut measurement = GitDirectoryMeasurementV1::default();
    let mut frontier = vec![(
        git_dir.canonical_path().to_path_buf(),
        String::new(),
        0_usize,
    )];
    let mut visited = 0_usize;
    while let Some((path, prefix, depth)) = frontier.pop() {
        if depth > GIT_DIR_MAX_DEPTH_V1 {
            return Err(CustodyExportErrorV1::UnexpectedGitFile(format!(
                "{} nests deeper than the bounded walk",
                path.display()
            )));
        }
        let entries = std::fs::read_dir(&path).map_err(|error| {
            CustodyExportErrorV1::Io(format!("cannot enumerate {}: {error}", path.display()))
        })?;
        for entry in entries {
            visited += 1;
            if visited > GIT_DIR_MAX_ENTRIES_V1 {
                return Err(CustodyExportErrorV1::UnexpectedGitFile(
                    "a Git directory exceeds the bounded entry count".into(),
                ));
            }
            let entry = entry
                .map_err(|error| CustodyExportErrorV1::Io(format!("directory entry: {error}")))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let relative = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let metadata = entry.metadata().map_err(|error| {
                CustodyExportErrorV1::Io(format!("cannot stat {relative}: {error}"))
            })?;
            if metadata.is_dir() {
                frontier.push((entry.path(), relative, depth + 1));
            } else {
                measurement.logical_bytes = measurement
                    .logical_bytes
                    .checked_add(metadata.len())
                    .ok_or_else(|| {
                    CustodyExportErrorV1::ScratchLedger("measured bytes overflowed".into())
                })?;
                measurement.files.insert(relative);
            }
        }
    }
    Ok(measurement)
}

fn verify_git_writes(
    measured: &GitDirectoryMeasurementV1,
    expected: &BTreeSet<String>,
    reservation: u64,
    directory: &str,
) -> Result<(), CustodyExportErrorV1> {
    if let Some(unexpected) = measured.files.difference(expected).next() {
        return Err(CustodyExportErrorV1::UnexpectedGitFile(format!(
            "{directory}/{unexpected}"
        )));
    }
    if measured.logical_bytes > reservation {
        return Err(CustodyExportErrorV1::ScratchLedger(format!(
            "{directory} holds {} bytes above its {reservation} byte reservation",
            measured.logical_bytes
        )));
    }
    Ok(())
}

fn prove_object_presence(
    context: &ExportContextV1<'_>,
    names: &GitRootNamesV1,
    budget: &mut GitDirectoryBudgetV1,
    ledger: &RefCell<ScratchLedgerV1>,
    runs: &mut Vec<GitRunRecordV1>,
    object_lines: &[u8],
    deadline: Instant,
) -> Result<(), CustodyExportErrorV1> {
    let mut request = GitRunRequestV1::new(
        GitCommandV1::CatFileBatchCheck,
        object_lines.to_vec(),
        stdout_limit_for_objects(context.capability.inventory.len()),
        GIT_STDERR_LIMIT_V1,
        deadline,
    );
    request.object_store = Some(context.capability.object_store_route()?);
    let result = run_git(context, names, "cat-file --batch-check", request, runs)?;
    remeasure_git_directory(context, budget, ledger)?;
    require_success(&result, "cat-file --batch-check")?;

    let stdout = result
        .captured_stdout()
        .ok_or_else(|| CustodyExportErrorV1::GitOutput("cat-file stdout was streamed".into()))?;
    let mut observed = BTreeMap::new();
    for line in String::from_utf8_lossy(stdout).lines() {
        let mut fields = line.split_whitespace();
        let (Some(object_id), Some(kind)) = (fields.next(), fields.next()) else {
            return Err(CustodyExportErrorV1::GitOutput(bounded(line)));
        };
        observed.insert(object_id.to_owned(), kind.to_owned());
    }
    for (_, object_id, kind) in &context.capability.inventory {
        match observed.get(object_id.as_str()) {
            Some(observed_kind) if observed_kind == git_kind_name(*kind) => {}
            Some(observed_kind) => {
                return Err(CustodyExportErrorV1::ObjectPresence(format!(
                    "{object_id} is {observed_kind}, not {}",
                    git_kind_name(*kind)
                )))
            }
            None => {
                return Err(CustodyExportErrorV1::ObjectPresence(format!(
                    "{object_id} is absent"
                )))
            }
        }
    }
    Ok(())
}

fn git_kind_name(kind: CustodyGitObjectKindV1) -> &'static str {
    match kind {
        CustodyGitObjectKindV1::Commit => "commit",
        CustodyGitObjectKindV1::Tree => "tree",
        CustodyGitObjectKindV1::Blob => "blob",
        CustodyGitObjectKindV1::Tag => "tag",
        CustodyGitObjectKindV1::Unknown => "unknown",
    }
}

/// The verified-pack identity: the staged `work/objects.pack`, retained by descriptor, plus the
/// length and SHA-256 recorded from the single `pack-objects` stream. §5 verifies these bytes and
/// §6 seals them; neither re-opens the file by name.
#[derive(Debug)]
struct VerifiedPackV1 {
    file: File,
    length: u64,
    sha256: [u8; 32],
}

impl VerifiedPackV1 {
    /// A duplicate of the retained descriptor, positioned at offset 0. Duplicates share one file
    /// offset, so every reader rewinds before it starts.
    fn rewound(&self, label: &str) -> Result<File, CustodyExportErrorV1> {
        let mut file = self
            .file
            .try_clone()
            .map_err(|error| CustodyExportErrorV1::Io(format!("{label}: {error}")))?;
        file.seek(SeekFrom::Start(0))
            .map_err(|error| CustodyExportErrorV1::Io(format!("{label}: {error}")))?;
        Ok(file)
    }
}

/// §4.2: `pack-objects` runs exactly once, its stdout is streamed into one create-new file, and
/// the recorded length and SHA-256 are the verified-pack identity for both §5 and §6.
#[allow(clippy::too_many_arguments)]
fn produce_pack(
    context: &ExportContextV1<'_>,
    names: &GitRootNamesV1,
    budget: &mut GitDirectoryBudgetV1,
    ledger: &RefCell<ScratchLedgerV1>,
    runs: &mut Vec<GitRunRecordV1>,
    object_lines: &[u8],
    budgets: CustodyExportBudgetsV1,
    deadline: Instant,
) -> Result<VerifiedPackV1, CustodyExportErrorV1> {
    // The runner owns the stdout reads and file writes and exposes only a whole-stream
    // `stdout_limit`, so the exporter reserves one complete pack-output allowance `A` before the
    // spawn and reconciles it down to the streamed length afterwards.
    ledger.borrow_mut().reserve_entries(1)?;
    let (used_before, headroom) = {
        let ledger = ledger.borrow();
        (ledger.used(), ledger.limit.saturating_sub(ledger.used()))
    };
    let allowance = headroom.min(budgets.max_artifact_bytes);
    ledger.borrow_mut().reserve(allowance)?;
    let allowance_limit = usize::try_from(allowance).unwrap_or(usize::MAX);
    #[cfg(test)]
    LAST_PACK_ALLOWANCE.with(|slot| slot.set((used_before, allowance)));
    #[cfg(not(test))]
    let _ = used_before;

    #[cfg(test)]
    let object_lines = PACK_INPUT_OVERRIDE
        .with(|slot| slot.borrow().clone())
        .unwrap_or_else(|| object_lines.to_vec());
    #[cfg(not(test))]
    let object_lines = object_lines.to_vec();

    let mut request = GitRunRequestV1::new(
        GitCommandV1::PackObjectsStdout,
        object_lines,
        allowance_limit,
        GIT_STDERR_LIMIT_V1,
        deadline,
    );
    request.object_store = Some(context.capability.object_store_route()?);
    request
        .stream_stdout_to_new_child(context.work, PACK_FILE_NAME)
        .map_err(CustodyExportErrorV1::Git)?;
    let result = run_git(context, names, "pack-objects --stdout", request, runs)?;
    remeasure_git_directory(context, budget, ledger)?;
    require_success(&result, "pack-objects --stdout")?;

    let GitStdoutV1::Streamed(evidence) = &result.stdout else {
        return Err(CustodyExportErrorV1::GitOutput(
            "the pack stdout was captured rather than streamed".into(),
        ));
    };
    let length = evidence.length as u64;
    ledger
        .borrow_mut()
        .release(allowance.saturating_sub(length));

    // The staged file is opened once, synced once, and never rewritten; this descriptor is the
    // one §5 streams to `index-pack` and §6 seals from.
    let mut file = context
        .work
        .open_regular_file(OsStr::new(PACK_FILE_NAME), "custody export staged pack")?;
    file.sync_all()
        .map_err(|error| CustodyExportErrorV1::Io(format!("pack sync: {error}")))?;
    let (measured_length, measured_sha256) = hash_from_start(&mut file)?;
    if measured_length != length || measured_sha256 != evidence.sha256 {
        return Err(CustodyExportErrorV1::ContentRemeasurement(
            "the staged pack does not match the streamed evidence".into(),
        ));
    }
    Ok(VerifiedPackV1 {
        file,
        length,
        sha256: evidence.sha256,
    })
}

/// §5: the isolated closure proof. Returns the pack hash `index-pack` reported.
#[allow(clippy::too_many_arguments)]
fn prove_isolated_closure(
    context: &ExportContextV1<'_>,
    names: &GitRootNamesV1,
    budget: &mut GitDirectoryBudgetV1,
    ledger: &RefCell<ScratchLedgerV1>,
    runs: &mut Vec<GitRunRecordV1>,
    pack: &VerifiedPackV1,
    object_format: GitObjectFormatV1,
    deadline: Instant,
) -> Result<String, CustodyExportErrorV1> {
    let logical_bound = index_pack_logical_bound(
        pack.length,
        context.capability.inventory.len() as u64,
        context.capability.raw_hash_width(),
    )?;
    ledger.borrow_mut().reserve_entries(INDEX_PACK_ENTRIES_V1)?;
    ledger.borrow_mut().reserve(logical_bound)?;

    #[cfg(test)]
    run_hook(
        ExportHookPointV1::BeforeIndexPack,
        context.work.canonical_path(),
    );

    // Step 1: strict indexing of the recorded pack, streamed from its retained descriptor.
    fault(ExportFaultPointV1::IndexPack, ExportFaultPositionV1::Before)?;
    let pack_hash = index_pack_strict(context, names, runs, pack, deadline)?;
    fault(ExportFaultPointV1::IndexPack, ExportFaultPositionV1::After)?;

    #[cfg(test)]
    run_hook(
        ExportHookPointV1::AfterIndexPack,
        context.work.canonical_path(),
    );

    budget.admit_index_pack(&pack_hash, logical_bound)?;
    remeasure_git_directory(context, budget, ledger)?;

    fault(
        ExportFaultPointV1::VerifyPack,
        ExportFaultPositionV1::Before,
    )?;
    let verify_pack_limit =
        verify_pack_stdout_limit(context.capability.inventory.len(), object_format)?;
    let request = GitRunRequestV1::new(
        GitCommandV1::VerifyPack {
            git_dir: VERIFY_GIT_DIR_NAME.to_owned(),
            pack_hash: pack_hash.clone(),
            object_format,
        },
        Vec::new(),
        verify_pack_limit,
        GIT_STDERR_LIMIT_V1,
        deadline,
    );
    let result = run_git(context, names, "verify-pack", request, runs)?;
    remeasure_git_directory(context, budget, ledger)?;
    require_success(&result, "verify-pack")?;
    let stdout = result
        .captured_stdout()
        .ok_or_else(|| CustodyExportErrorV1::GitOutput("verify-pack stdout was streamed".into()))?;
    if !String::from_utf8_lossy(stdout)
        .lines()
        .any(|line| line.ends_with(": ok"))
    {
        return Err(CustodyExportErrorV1::GitOutput(
            "verify-pack did not report an integrity result".into(),
        ));
    }
    fault(ExportFaultPointV1::VerifyPack, ExportFaultPositionV1::After)?;

    // Step 2: exact inventory equality.
    fault(
        ExportFaultPointV1::InventoryComparison,
        ExportFaultPositionV1::Before,
    )?;
    let request = GitRunRequestV1::new(
        GitCommandV1::CatFileAllObjects,
        Vec::new(),
        stdout_limit_for_objects(context.capability.inventory.len()),
        GIT_STDERR_LIMIT_V1,
        deadline,
    );
    let result = run_git(
        context,
        names,
        "cat-file --batch-all-objects",
        request,
        runs,
    )?;
    remeasure_git_directory(context, budget, ledger)?;
    require_success(&result, "cat-file --batch-all-objects")?;
    let stdout = result.captured_stdout().ok_or_else(|| {
        CustodyExportErrorV1::GitOutput("cat-file --batch-all-objects stdout was streamed".into())
    })?;
    compare_inventory(context, stdout)?;
    fault(
        ExportFaultPointV1::InventoryComparison,
        ExportFaultPositionV1::After,
    )?;

    // Step 3: closure from EVERY inventory object, not only commit/tag roots.
    fault(
        ExportFaultPointV1::RevListClosure,
        ExportFaultPositionV1::Before,
    )?;
    let object_lines = inventory_stdin(context.capability);
    let request = GitRunRequestV1::new(
        GitCommandV1::RevListMissingPrint,
        object_lines,
        stdout_limit_for_objects(context.capability.inventory.len().saturating_mul(2)),
        GIT_STDERR_LIMIT_V1,
        deadline,
    );
    let result = run_git(context, names, "rev-list --missing=print", request, runs)?;
    remeasure_git_directory(context, budget, ledger)?;
    require_success(&result, "rev-list --missing=print")?;
    let stdout = result
        .captured_stdout()
        .ok_or_else(|| CustodyExportErrorV1::GitOutput("rev-list stdout was streamed".into()))?;
    #[cfg(test)]
    let closure_bypassed = bypassed(|bypass| bypass.closure_step);
    #[cfg(not(test))]
    let closure_bypassed = false;
    if !closure_bypassed {
        compare_closure(context, stdout)?;
    }
    fault(
        ExportFaultPointV1::RevListClosure,
        ExportFaultPositionV1::After,
    )?;

    // Step 4: object syntax.
    fault(ExportFaultPointV1::Fsck, ExportFaultPositionV1::Before)?;
    let request = GitRunRequestV1::new(
        GitCommandV1::FsckStrict,
        Vec::new(),
        stdout_limit_for_objects(context.capability.inventory.len()),
        GIT_STDERR_LIMIT_V1,
        deadline,
    );
    let result = run_git(context, names, "fsck --strict", request, runs)?;
    remeasure_git_directory(context, budget, ledger)?;
    // Step 4's own output check runs BEFORE the generic exit-status check. `fsck --strict` reports
    // a strict-object rejection on stderr and exits non-zero, so letting `require_success` answer
    // first would demote it to a generic child failure. Keeping step 4's classifier here and step
    // 1's in `classify_index_pack` keeps the two guards separable: control 20a discriminates the
    // step-1 classifier and control 20b discriminates this step, and neither masks the other.
    check_fsck_output(&result)?;
    require_success(&result, "fsck --strict")?;
    fault(ExportFaultPointV1::Fsck, ExportFaultPositionV1::After).map(|()| pack_hash)
}

fn index_pack_strict(
    context: &ExportContextV1<'_>,
    names: &GitRootNamesV1,
    runs: &mut Vec<GitRunRecordV1>,
    pack: &VerifiedPackV1,
    deadline: Instant,
) -> Result<String, CustodyExportErrorV1> {
    #[cfg(test)]
    if bypassed(|bypass| bypass.preindexed_pack) {
        // Control 20b: a pre-indexed pack plus `.idx` is already installed in `verify.git`, so
        // step 4's `fsck` is the only strict-object guard left for the same tree.
        return read_installed_pack_hash(context);
    }

    // The recorded pack, streamed from its retained descriptor and bounded by its RECORDED length:
    // a file that has since grown is refused by `from_file` before any spawn.
    #[cfg(test)]
    let stdin = match INDEX_PACK_STDIN_SUBSTITUTE.with(|slot| slot.borrow().clone()) {
        Some(name) => context
            .work
            .open_regular_file(&name, "custody export verification input")?,
        None => pack.rewound("custody export verification input")?,
    };
    #[cfg(not(test))]
    let stdin = pack.rewound("custody export verification input")?;
    let request = GitRunRequestV1::from_file(
        GitCommandV1::IndexPackStrictStdin,
        stdin,
        pack.length,
        GIT_MIN_STDOUT_LIMIT_V1,
        GIT_STDERR_LIMIT_V1,
        deadline,
    )
    .map_err(CustodyExportErrorV1::Git)?;
    let result = run_git(context, names, "index-pack --strict --stdin", request, runs)?;
    classify_index_pack(&result)?;

    // The bytes that were verified must be the bytes that were recorded. 2B2a attests exactly
    // what the child's stdin pipe accepted, so no exporter-owned hashing writer is needed.
    if result.evidence.stdin.length as u64 != pack.length
        || result.evidence.stdin.sha256 != pack.sha256
    {
        return Err(CustodyExportErrorV1::VerificationInputMismatch);
    }

    let stdout = result
        .captured_stdout()
        .ok_or_else(|| CustodyExportErrorV1::GitOutput("index-pack stdout was streamed".into()))?;
    parse_pack_hash(stdout, context.capability.git_object_format())
}

#[cfg(test)]
fn read_installed_pack_hash(context: &ExportContextV1<'_>) -> Result<String, CustodyExportErrorV1> {
    let pack_dir = context
        .work
        .canonical_path()
        .join(VERIFY_GIT_DIR_NAME)
        .join("objects/pack");
    let entries = std::fs::read_dir(&pack_dir)
        .map_err(|error| CustodyExportErrorV1::Io(format!("pre-indexed pack: {error}")))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| CustodyExportErrorV1::Io(format!("pre-indexed pack: {error}")))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(hash) = name
            .strip_prefix("pack-")
            .and_then(|n| n.strip_suffix(".idx"))
        {
            return Ok(hash.to_owned());
        }
    }
    Err(CustodyExportErrorV1::GitOutput(
        "no pre-indexed pack was installed".into(),
    ))
}

fn parse_pack_hash(
    stdout: &[u8],
    object_format: GitObjectFormatV1,
) -> Result<String, CustodyExportErrorV1> {
    let text = String::from_utf8_lossy(stdout);
    let line = text
        .lines()
        .next()
        .ok_or_else(|| CustodyExportErrorV1::GitOutput("index-pack printed nothing".into()))?;
    let hash = line
        .strip_prefix("pack\t")
        .ok_or_else(|| CustodyExportErrorV1::GitOutput(bounded(line)))?
        .trim();
    let width = match object_format {
        GitObjectFormatV1::Sha1 => 40,
        GitObjectFormatV1::Sha256 => 64,
    };
    if hash.len() != width
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(CustodyExportErrorV1::GitOutput(bounded(hash)));
    }
    Ok(hash.to_owned())
}

fn compare_inventory(
    context: &ExportContextV1<'_>,
    stdout: &[u8],
) -> Result<(), CustodyExportErrorV1> {
    let mut observed: BTreeSet<(String, String)> = BTreeSet::new();
    let mut rows = 0_usize;
    for line in String::from_utf8_lossy(stdout).lines() {
        rows += 1;
        let mut fields = line.split_whitespace();
        let (Some(object_id), Some(kind), None) = (fields.next(), fields.next(), fields.next())
        else {
            return Err(CustodyExportErrorV1::GitOutput(bounded(line)));
        };
        if !observed.insert((object_id.to_owned(), kind.to_owned())) {
            return Err(CustodyExportErrorV1::InventoryMismatch(format!(
                "{object_id} is listed twice"
            )));
        }
    }
    let expected: BTreeSet<(String, String)> = context
        .capability
        .inventory
        .iter()
        .map(|(_, object_id, kind)| (object_id.clone(), git_kind_name(*kind).to_owned()))
        .collect();
    if rows != observed.len() {
        return Err(CustodyExportErrorV1::InventoryMismatch(
            "a duplicate row was listed".into(),
        ));
    }
    if let Some((object_id, _)) = expected.difference(&observed).next() {
        return Err(CustodyExportErrorV1::InventoryMismatch(format!(
            "{object_id} is missing from the pack"
        )));
    }
    if let Some((object_id, _)) = observed.difference(&expected).next() {
        return Err(CustodyExportErrorV1::InventoryMismatch(format!(
            "{object_id} is an extra packed object"
        )));
    }
    Ok(())
}

fn compare_closure(
    context: &ExportContextV1<'_>,
    stdout: &[u8],
) -> Result<(), CustodyExportErrorV1> {
    let mut reached: BTreeSet<String> = BTreeSet::new();
    for line in String::from_utf8_lossy(stdout).lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(missing) = line.strip_prefix('?') {
            return Err(CustodyExportErrorV1::ClosureIncomplete(format!(
                "{} is reachable from the inventory but absent",
                bounded(missing)
            )));
        }
        reached.insert(line.split_whitespace().next().unwrap_or(line).to_owned());
    }
    let expected: BTreeSet<String> = context
        .capability
        .inventory
        .iter()
        .map(|(_, object_id, _)| object_id.clone())
        .collect();
    if reached != expected {
        return Err(CustodyExportErrorV1::ClosureIncomplete(
            "the reachable set does not equal the manifest inventory".into(),
        ));
    }
    Ok(())
}

/// `fsck --strict --full --no-reflogs --no-dangling --no-progress` writes nothing to stdout for a
/// clean database. On stderr it emits exactly one informational line for the refless verification
/// database §5 mandates. Any other line refuses.
const FSCK_ALLOWED_NOTICE_V1: &str = "notice: No default references";

fn check_fsck_output(result: &GitRunResultV1) -> Result<(), CustodyExportErrorV1> {
    let stdout = result
        .captured_stdout()
        .ok_or_else(|| CustodyExportErrorV1::GitOutput("fsck stdout was streamed".into()))?;
    for line in String::from_utf8_lossy(stdout).lines() {
        if !line.trim().is_empty() {
            return Err(CustodyExportErrorV1::GitOutput(bounded(line)));
        }
    }
    let stderr = String::from_utf8_lossy(&result.stderr);
    for line in stderr.lines() {
        let line = line.trim();
        if line.is_empty() || line == FSCK_ALLOWED_NOTICE_V1 {
            continue;
        }
        if let Some(message_id) = strict_object_message_id(line) {
            return Err(CustodyExportErrorV1::StrictObjectCheck(message_id));
        }
        return Err(CustodyExportErrorV1::GitOutput(bounded(line)));
    }
    Ok(())
}

fn hash_from_start(file: &mut File) -> Result<(u64, [u8; 32]), CustodyExportErrorV1> {
    file.seek(SeekFrom::Start(0))
        .map_err(|error| CustodyExportErrorV1::Io(format!("positioned read: {error}")))?;
    let mut context = digest::Context::new(&digest::SHA256);
    let mut buffer = [0_u8; 32 * 1024];
    let mut length = 0_u64;
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| CustodyExportErrorV1::Io(format!("positioned read: {error}")))?;
        if count == 0 {
            let mut sha256 = [0_u8; 32];
            sha256.copy_from_slice(context.finish().as_ref());
            return Ok((length, sha256));
        }
        length += count as u64;
        context.update(&buffer[..count]);
    }
}

fn sha256_bytes(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0_u8; 32];
    out.copy_from_slice(digest::digest(&digest::SHA256, bytes).as_ref());
    out
}

fn sha256_hex(bytes: [u8; 32]) -> Sha256HexV1 {
    let mut value = String::with_capacity(64);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut value, "{byte:02x}");
    }
    Sha256HexV1::parse(value).expect("a SHA-256 digest renders as 64 lower-hex bytes")
}

// ---------------------------------------------------------------------------------------------
// Artifact planning and publication
// ---------------------------------------------------------------------------------------------

enum PlannedPlaintextV1 {
    Bytes(Vec<u8>),
    Pack,
}

struct ArtifactPlanV1 {
    name: LosslessPathV1,
    plaintext: PlannedPlaintextV1,
    expected_length: u64,
    expected_sha256: Sha256HexV1,
}

fn build_artifact_plan(
    manifest: &CustodyManifestV1,
    layout: &CustodyCapsuleLayoutV1,
    restore_policy: &CustodyRestorePolicyV1,
    capability: &CustodyCaptureCapabilityV1,
    pack: &VerifiedPackV1,
    budgets: CustodyExportBudgetsV1,
) -> Result<Vec<ArtifactPlanV1>, CustodyExportErrorV1> {
    let manifest_bytes = manifest
        .encode_canonical()
        .map_err(CustodyExportErrorV1::Seal)?;
    let index_bytes = layout
        .index()
        .encode_canonical()
        .map_err(CustodyExportErrorV1::Capsule)?;
    let policy_bytes = restore_policy
        .encode_canonical()
        .map_err(CustodyExportErrorV1::Capsule)?;

    let mut plan = Vec::new();
    for row in layout.index().artifacts() {
        let (plaintext, length, sha256) = match row.role() {
            CustodyCapsuleArtifactRoleV1::Manifest => bytes_plan(&manifest_bytes),
            CustodyCapsuleArtifactRoleV1::CapsuleIndex => bytes_plan(&index_bytes),
            CustodyCapsuleArtifactRoleV1::RestorePolicy => bytes_plan(&policy_bytes),
            CustodyCapsuleArtifactRoleV1::GitObjectPack => (
                PlannedPlaintextV1::Pack,
                pack.length,
                sha256_hex(pack.sha256),
            ),
            CustodyCapsuleArtifactRoleV1::CoveragePayload(class) => {
                let stream = capability.stream_for(*class)?;
                (
                    PlannedPlaintextV1::Bytes(stream.bytes.clone()),
                    stream.length,
                    stream.sha256.clone(),
                )
            }
        };
        check_plaintext_budget(length, budgets)?;
        #[cfg(test)]
        let plaintext = substituted_plaintext(row.name().as_bytes(), plaintext, pack)?;
        plan.push(ArtifactPlanV1 {
            name: row.name().clone(),
            plaintext,
            expected_length: length,
            expected_sha256: sha256,
        });
    }
    Ok(plan)
}

/// The armed control-16 substitute for this artifact: the planned plaintext with every byte
/// inverted, or the plan unchanged when no substitute is armed for this name.
#[cfg(test)]
fn substituted_plaintext(
    name: &[u8],
    plaintext: PlannedPlaintextV1,
    pack: &VerifiedPackV1,
) -> Result<PlannedPlaintextV1, CustodyExportErrorV1> {
    let armed = PLAINTEXT_SUBSTITUTE.with(|slot| {
        slot.borrow().as_ref().is_some_and(|marker| {
            name.windows(marker.len())
                .any(|window| window == marker.as_slice())
        })
    });
    if !armed {
        return Ok(plaintext);
    }
    let mut bytes = match plaintext {
        PlannedPlaintextV1::Bytes(bytes) => bytes,
        PlannedPlaintextV1::Pack => {
            let mut bytes = Vec::new();
            pack.rewound("control 16 substitute")?
                .read_to_end(&mut bytes)
                .map_err(|error| CustodyExportErrorV1::Io(format!("control 16: {error}")))?;
            bytes
        }
    };
    for byte in &mut bytes {
        *byte = !*byte;
    }
    Ok(PlannedPlaintextV1::Bytes(bytes))
}

/// §3 per-artifact ceilings for one planned plaintext, checked before any staging file exists: at
/// most the per-artifact byte ceiling, and at most the chunk-count ceiling at the budget's chunk
/// size (an empty plaintext is one empty final chunk).
fn check_plaintext_budget(
    length: u64,
    budgets: CustodyExportBudgetsV1,
) -> Result<(), CustodyExportErrorV1> {
    if length > budgets.max_artifact_bytes {
        return Err(CustodyExportErrorV1::BudgetCeiling(
            "per-artifact ciphertext",
        ));
    }
    let chunks = length.div_ceil(budgets.max_chunk_bytes).max(1);
    if chunks > u64::from(budgets.max_chunks) {
        return Err(CustodyExportErrorV1::BudgetCeiling("chunk count"));
    }
    Ok(())
}

fn bytes_plan(bytes: &[u8]) -> (PlannedPlaintextV1, u64, Sha256HexV1) {
    (
        PlannedPlaintextV1::Bytes(bytes.to_vec()),
        bytes.len() as u64,
        Sha256HexV1::digest(bytes),
    )
}

struct PublishedArtifactV1 {
    name: LosslessPathV1,
    parent_key: Vec<u8>,
    leaf: OsString,
    file: File,
    receipt: CustodyEnvelopeSealReceiptV1,
    artifact: CustodySealedArtifactV1,
}

struct SealOneArtifactV1<'a> {
    capsule: &'a PinnedDirectoryV1,
    directories: &'a mut BTreeMap<Vec<u8>, PinnedDirectoryV1>,
    pack: &'a VerifiedPackV1,
    ledger: &'a RefCell<ScratchLedgerV1>,
    sealer: &'a dyn CustodyEnvelopeSealerV1,
    envelope_format: &'a CustodyEnvelopeFormatV1,
    recipients: &'a [String],
    manifest_digest: &'a Sha256HexV1,
    budgets: CustodyExportBudgetsV1,
    ordinal: usize,
    artifact: ArtifactPlanV1,
}

/// Seal one artifact into a unique create-new staging name below `capsule/`, prove the sealer's
/// receipt against an exporter-built expected receipt, remeasure the staged content, and publish
/// the staging file without replacement to its reserved logical name.
fn seal_one_artifact(
    request: SealOneArtifactV1<'_>,
) -> Result<PublishedArtifactV1, CustodyExportErrorV1> {
    let SealOneArtifactV1 {
        capsule,
        directories,
        pack,
        ledger,
        sealer,
        envelope_format,
        recipients,
        manifest_digest,
        budgets,
        ordinal,
        artifact,
    } = request;

    let (parent_key, leaf) =
        resolve_artifact_parent(capsule, directories, artifact.name.as_bytes(), ledger)?;
    let parent = directories
        .get(&parent_key)
        .ok_or_else(|| CustodyExportErrorV1::Io("a capsule directory vanished".into()))?;

    let staging = OsString::from(format!(".a2a-staging-{ordinal}"));
    fault(
        ExportFaultPointV1::StagingCreate,
        ExportFaultPositionV1::Before,
    )?;
    ledger.borrow_mut().reserve_entries(1)?;
    let mut file = parent.create_new_regular_child(&staging, "custody export staging artifact")?;
    fault(
        ExportFaultPointV1::StagingCreate,
        ExportFaultPositionV1::After,
    )?;

    let context = CustodyEnvelopeContextV1::new(
        artifact.name.clone(),
        manifest_digest.clone(),
        envelope_format.clone(),
        recipients.to_vec(),
    )
    .map_err(CustodyExportErrorV1::Capsule)?;
    let metadata =
        CustodyEnvelopeMetadataV1::new(BTreeMap::new()).map_err(CustodyExportErrorV1::Capsule)?;

    let limits = budgets.stream_limits()?;
    let descriptor = CustodyEnvelopeSourceDescriptorV1::new(artifact.expected_length, limits)
        .map_err(CustodyExportErrorV1::Capsule)?;
    let reader = match artifact.plaintext {
        PlannedPlaintextV1::Bytes(bytes) => PlaintextReaderV1::Bytes { bytes, offset: 0 },
        // The pack is sealed from the retained descriptor §5 verified, never re-opened by name.
        PlannedPlaintextV1::Pack => {
            PlaintextReaderV1::File(pack.rewound("custody export pack plaintext")?)
        }
    };

    let failure: RefCell<Option<CustodyExportErrorV1>> = RefCell::new(None);
    let receipt = {
        let mut source = ExactTotalPlaintextSourceV1 {
            descriptor,
            chunks: PlaintextChunksV1 {
                reader,
                max_chunk_bytes: budgets.max_chunk_bytes,
                total: artifact.expected_length,
                emitted: 0,
                ordinal: 0,
                done: false,
            },
            validator: Some(CustodyEnvelopeSourceValidatorV1::new(
                CustodyEnvelopeSourceDescriptorV1::new(artifact.expected_length, limits)
                    .map_err(CustodyExportErrorV1::Capsule)?,
            )),
            failure: &failure,
        };
        let mut sink = CapsuleDestinationSinkV1 {
            file: &mut file,
            limits,
            validator: Some(CustodyEnvelopeSinkValidatorV1::new(limits)),
            ledger,
            failure: &failure,
            artifact_name: artifact.name.as_bytes().to_vec(),
            written: 0,
            chunks: 0,
            max_artifact_bytes: budgets.max_artifact_bytes,
        };

        let sealed = sealer.seal(&context, &mut source, &metadata, &mut sink);

        // Full plaintext consumption, for EVERY artifact: the wrapper is finished after `seal()`
        // returns and its identity must equal the expected plaintext identity.
        let consumed = source.finish();
        let ciphertext = sink
            .validator
            .take()
            .ok_or_else(|| CustodyExportErrorV1::Io("the sink was already finished".into()))?
            .finish();

        let sealed = match sealed {
            Ok(receipt) => receipt,
            Err(error) => {
                return Err(failure
                    .into_inner()
                    .unwrap_or(CustodyExportErrorV1::Capsule(error)))
            }
        };
        let artifact_label = String::from_utf8_lossy(artifact.name.as_bytes()).into_owned();
        let (length, sha256) = consumed
            .map_err(|_| CustodyExportErrorV1::PlaintextNotConsumed(artifact_label.clone()))?;
        #[cfg(test)]
        let compare_identity = !bypassed(|bypass| bypass.plaintext_identity_comparison);
        #[cfg(not(test))]
        let compare_identity = true;
        if compare_identity
            && (length != artifact.expected_length || sha256 != artifact.expected_sha256)
        {
            return Err(CustodyExportErrorV1::PlaintextIdentity(artifact_label));
        }

        // The file is synced and identity-rechecked, and only then is the destination validator's
        // completion turned into the expected receipt.
        fault(
            ExportFaultPointV1::StagingFileSync,
            ExportFaultPositionV1::Before,
        )?;
        file.sync_all()
            .map_err(|error| CustodyExportErrorV1::Io(format!("staging sync: {error}")))?;
        fault(
            ExportFaultPointV1::StagingFileSync,
            ExportFaultPositionV1::After,
        )?;

        fault(
            ExportFaultPointV1::StagingIdentityRecheck,
            ExportFaultPositionV1::Before,
        )?;
        let staged_identity =
            crate::fs_custody::regular_file_identity(&file, "custody export staging identity")?;
        let reopened = parent.open_regular_file(&staging, "custody export staging identity")?;
        let reopened_identity =
            crate::fs_custody::regular_file_identity(&reopened, "custody export staging identity")?;
        if staged_identity.dev != reopened_identity.dev
            || staged_identity.ino != reopened_identity.ino
        {
            return Err(CustodyExportErrorV1::IdentityDrift(
                "the staging name no longer resolves to the staged object".into(),
            ));
        }
        fault(
            ExportFaultPointV1::StagingIdentityRecheck,
            ExportFaultPositionV1::After,
        )?;

        let ciphertext = ciphertext.map_err(CustodyExportErrorV1::Capsule)?;
        let expected = CustodyEnvelopeSealReceiptV1::new(&context, ciphertext)
            .map_err(CustodyExportErrorV1::Capsule)?;
        if !receipt_fields_equal(
            &ReceiptFieldsV1::from_receipt(&sealed),
            &ReceiptFieldsV1::from_receipt(&expected),
        ) {
            return Err(CustodyExportErrorV1::ReceiptMismatch);
        }
        expected
    };

    #[cfg(test)]
    run_hook(
        ExportHookPointV1::AfterStagingSync,
        &parent.canonical_path().join(&staging),
    );

    // Content remeasurement: a positioned read from offset 0 of the retained descriptor must
    // still equal the destination receipt, so an in-place overwrite cannot reach the seal.
    let (measured_length, measured_sha256) = hash_from_start(&mut file)?;
    if measured_length != receipt.ciphertext_length()
        || sha256_hex(measured_sha256) != *receipt.ciphertext_sha256()
    {
        return Err(CustodyExportErrorV1::ContentRemeasurement(
            String::from_utf8_lossy(artifact.name.as_bytes()).into_owned(),
        ));
    }

    #[cfg(test)]
    run_hook(
        ExportHookPointV1::BeforeArtifactRename,
        &parent.canonical_path().join(&leaf),
    );

    fault(
        ExportFaultPointV1::ArtifactRename,
        ExportFaultPositionV1::Before,
    )?;
    let publication = parent.publish_new_regular_child(
        RegularChildRefV1::new(&staging, &file),
        &leaf,
        "custody export artifact publication",
    )?;
    if let Some(detail) = publication.ambiguity() {
        return Err(CustodyExportErrorV1::Publication(detail.to_owned()));
    }
    fault(
        ExportFaultPointV1::ArtifactRename,
        ExportFaultPositionV1::After,
    )?;

    fault(
        ExportFaultPointV1::ArtifactDirectorySync,
        ExportFaultPositionV1::Before,
    )?;
    capsule.sync("custody export capsule sync")?;
    fault(
        ExportFaultPointV1::ArtifactDirectorySync,
        ExportFaultPositionV1::After,
    )?;

    #[cfg(test)]
    run_hook(
        ExportHookPointV1::AfterArtifactPublish,
        &parent.canonical_path().join(&leaf),
    );

    let sealed_artifact = receipt
        .to_sealed_artifact()
        .map_err(CustodyExportErrorV1::Capsule)?;
    Ok(PublishedArtifactV1 {
        name: artifact.name,
        parent_key,
        leaf,
        file,
        receipt,
        artifact: sealed_artifact,
    })
}

/// Walk a validated exterior logical name's components below the retained `capsule/` descriptor.
/// The name itself is never passed to a path API.
fn resolve_artifact_parent(
    capsule: &PinnedDirectoryV1,
    directories: &mut BTreeMap<Vec<u8>, PinnedDirectoryV1>,
    name: &[u8],
    ledger: &RefCell<ScratchLedgerV1>,
) -> Result<(Vec<u8>, OsString), CustodyExportErrorV1> {
    use std::os::unix::ffi::OsStrExt as _;

    let mut components: Vec<&[u8]> = name.split(|byte| *byte == b'/').collect();
    let leaf = components.pop().ok_or_else(|| {
        CustodyExportErrorV1::Publication("an artifact name has no final component".into())
    })?;
    if leaf.is_empty() || components.iter().any(|part| part.is_empty()) {
        return Err(CustodyExportErrorV1::Publication(
            "an artifact name has an empty component".into(),
        ));
    }

    let mut key: Vec<u8> = Vec::new();
    for component in components {
        if !key.is_empty() {
            key.push(b'/');
        }
        key.extend_from_slice(component);
        if directories.contains_key(&key) {
            continue;
        }
        let parent = if key.len() == component.len() {
            capsule
        } else {
            let parent_key = key[..key.len() - component.len() - 1].to_vec();
            directories.get(&parent_key).ok_or_else(|| {
                CustodyExportErrorV1::Publication("a capsule parent directory is missing".into())
            })?
        };
        ledger.borrow_mut().reserve_entries(1)?;
        let created = parent.create_new_child_directory(
            OsStr::from_bytes(component),
            "custody export capsule directory",
        )?;
        directories.insert(key.clone(), created);
    }
    Ok((key, OsStr::from_bytes(leaf).to_os_string()))
}

/// Everything the §6 seal barrier rechecks: the destination directories and every published
/// artifact.
struct SealBarrierV1<'a> {
    scratch: &'a PinnedDirectoryV1,
    capsule: &'a PinnedDirectoryV1,
    directories: &'a BTreeMap<Vec<u8>, PinnedDirectoryV1>,
    published: &'a [PublishedArtifactV1],
}

/// §6 seal barrier, run inside the seal publication's last-chance hook, strictly before the
/// rename becomes visible.
///
/// Destination identities first (§2 requires them rechecked across every effect boundary): the
/// scratch root, `capsule/`, and every capsule directory must still be the pinned object at its
/// name, so a retargeted component cannot receive a seal over artifacts that now live elsewhere.
/// Then every published artifact is re-opened descriptor-relatively (no-follow), synced, and
/// re-hashed, and must equal its `CustodySealedArtifactV1` length and digest. Re-reads do not
/// write, so they consume no ledger.
fn run_seal_barrier(barrier: &SealBarrierV1<'_>) -> Result<(), CustodyExportErrorV1> {
    #[cfg(test)]
    if bypassed(|bypass| bypass.seal_barrier) {
        return Ok(());
    }
    for directory in [barrier.scratch, barrier.capsule]
        .into_iter()
        .chain(barrier.directories.values())
    {
        pinned_root_unchanged(directory).map_err(CustodyExportErrorV1::SealBarrier)?;
    }
    for artifact in barrier.published {
        artifact.file.sync_all().map_err(|error| {
            CustodyExportErrorV1::SealBarrier(format!("published artifact sync: {error}"))
        })?;
        let parent = if artifact.parent_key.is_empty() {
            barrier.capsule
        } else {
            barrier
                .directories
                .get(&artifact.parent_key)
                .ok_or_else(|| {
                    CustodyExportErrorV1::SealBarrier("a capsule directory is missing".into())
                })?
        };
        let mut reopened =
            parent.open_regular_file(&artifact.leaf, "custody export seal barrier")?;
        let (length, sha256) = hash_from_start(&mut reopened)?;
        if length != artifact.artifact.byte_length()
            || sha256_hex(sha256) != *artifact.artifact.sha256()
        {
            return Err(CustodyExportErrorV1::SealBarrier(
                String::from_utf8_lossy(artifact.name.as_bytes()).into_owned(),
            ));
        }
    }
    Ok(())
}

enum SealPublicationV1 {
    Durable,
    RenameUnverified(String),
    TargetIdentityUnverified(String),
    DurabilityUnconfirmed(String),
}

struct PublishSealV1<'a> {
    capsule: &'a PinnedDirectoryV1,
    ledger: &'a RefCell<ScratchLedgerV1>,
    seal: &'a CustodySealV1,
    budgets: CustodyExportBudgetsV1,
    barrier: SealBarrierV1<'a>,
}

/// The commit point: the no-replace rename of `custody-seal.v1` into `capsule/`.
///
/// `Err` means no seal exists. Once the rename has been attempted, every answer is an
/// [`SealPublicationV1`] outcome instead, because the seal may now exist.
fn publish_seal(request: PublishSealV1<'_>) -> Result<SealPublicationV1, CustodyExportErrorV1> {
    let PublishSealV1 {
        capsule,
        ledger,
        seal,
        budgets,
        barrier,
    } = request;

    let bytes = seal
        .encode_canonical()
        .map_err(CustodyExportErrorV1::Seal)?;
    if bytes.len() > budgets.max_canonical_json_bytes {
        return Err(CustodyExportErrorV1::BudgetCeiling(
            "canonical JSON metadata",
        ));
    }
    ledger.borrow_mut().reserve_entries(1)?;
    ledger.borrow_mut().reserve(bytes.len() as u64)?;

    let staging = OsString::from(".a2a-staging-seal");
    let mut file = capsule.create_new_regular_child(&staging, "custody export seal staging")?;
    file.write_all(&bytes)
        .map_err(|error| CustodyExportErrorV1::Io(format!("seal write: {error}")))?;
    file.sync_all()
        .map_err(|error| CustodyExportErrorV1::Io(format!("seal sync: {error}")))?;
    let (length, sha256) = hash_from_start(&mut file)?;
    if length != bytes.len() as u64 || sha256 != sha256_bytes(&bytes) {
        return Err(CustodyExportErrorV1::ContentRemeasurement(
            SEAL_NAME.to_owned(),
        ));
    }

    fault(
        ExportFaultPointV1::SealRename,
        ExportFaultPositionV1::Before,
    )?;
    // The seal barrier runs in `fs_custody`'s last-chance hook: after its own pre-checks and
    // strictly before the rename, so no exporter step sits between the barrier and the commit
    // point. A barrier refusal is carried out typed; the hook itself can only speak
    // `FsCustodyError`.
    let barrier_refusal: RefCell<Option<CustodyExportErrorV1>> = RefCell::new(None);
    let publication = capsule.publish_new_regular_child_with_before_rename(
        RegularChildRefV1::new(&staging, &file),
        OsStr::new(SEAL_NAME),
        "custody export seal publication",
        || {
            run_seal_barrier(&barrier).map_err(|refusal| {
                let detail = refusal.to_string();
                *barrier_refusal.borrow_mut() = Some(refusal);
                FsCustodyError::IdentityChanged(detail)
            })
        },
    );
    let publication = match publication {
        Ok(publication) => publication,
        Err(error) => {
            return Err(barrier_refusal
                .into_inner()
                .unwrap_or(CustodyExportErrorV1::Fs(error)))
        }
    };

    #[cfg(test)]
    let publication = match SEAL_PUBLICATION_OVERRIDE.with(|slot| *slot.borrow()) {
        Some(SealPublicationOverrideV1::RenameUnverified) => {
            CustodyPublicationV1::RenameOutcomeUnverified(
                "overridden for the commit-point lattice control".into(),
            )
        }
        Some(SealPublicationOverrideV1::TargetIdentityUnverified) => {
            CustodyPublicationV1::TargetIdentityUnverified(
                "overridden for the commit-point lattice control".into(),
            )
        }
        Some(SealPublicationOverrideV1::ParentSyncAmbiguous) => {
            CustodyPublicationV1::ParentSyncAmbiguous(
                "overridden for the commit-point lattice control".into(),
            )
        }
        None => publication,
    };

    match publication {
        CustodyPublicationV1::RenameOutcomeUnverified(detail) => {
            return Ok(SealPublicationV1::RenameUnverified(detail))
        }
        CustodyPublicationV1::TargetIdentityUnverified(detail) => {
            return Ok(SealPublicationV1::TargetIdentityUnverified(detail))
        }
        CustodyPublicationV1::ParentSyncAmbiguous(detail) => {
            return Ok(SealPublicationV1::DurabilityUnconfirmed(detail))
        }
        CustodyPublicationV1::Durable { .. } => {}
    }

    // Everything from here on is an OUTCOME, never an `Err`: the seal exists.
    if let Err(error) = fault(ExportFaultPointV1::SealRename, ExportFaultPositionV1::After) {
        return Ok(SealPublicationV1::DurabilityUnconfirmed(error.to_string()));
    }
    if let Err(error) = fault(
        ExportFaultPointV1::PostSealDirectorySync,
        ExportFaultPositionV1::Before,
    ) {
        return Ok(SealPublicationV1::DurabilityUnconfirmed(error.to_string()));
    }
    if let Err(error) = capsule.sync("custody export post-seal sync") {
        return Ok(SealPublicationV1::DurabilityUnconfirmed(error.to_string()));
    }
    if let Err(error) = fault(
        ExportFaultPointV1::PostSealDirectorySync,
        ExportFaultPositionV1::After,
    ) {
        return Ok(SealPublicationV1::DurabilityUnconfirmed(error.to_string()));
    }
    Ok(SealPublicationV1::Durable)
}

#[cfg(test)]
#[path = "custody_export_tests.rs"]
mod tests;
