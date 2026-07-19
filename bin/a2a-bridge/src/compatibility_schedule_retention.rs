//! Independently authorized cold evidence publication, verification, and hot-cache eviction.
//!
//! R3d5 remains the production root and FileProvider adapter owner. This module accepts only
//! injected retained roots and probes.

use std::ffi::OsStr;
use std::io::Write as _;
use std::os::unix::fs::MetadataExt as _;

use crate::compatibility_schedule_authority::{
    validate_sealed_storage_consent, validate_storage_consent, FileAuthorityJournal,
    StorageConsentRequestV1,
};
use crate::compatibility_schedule_evidence::{
    acquire_evidence_read_lease, try_acquire_evidence_gc_lease, ColdCopyLifecycleV1,
    ColdCopyRecordV1, EvidenceHotStoreV1, EvidenceStateModelV1, FileEvidenceJournal,
    FileProviderMaterializationV1, FileProviderObjectStateV1, FileProviderObservationV1,
    FileProviderSynchronizationV1, IndexedEvidenceV1, StorageIntegrityHoldV1,
};
use crate::compatibility_schedule_schema::{
    ColdStorageBindingV1, OptionalRelativeEvidencePathV1, RelativeEvidencePathV1, StorageConsentV1,
};
use crate::compatibility_schedule_state::{AuthorityStateCapability, EvidenceStateCapability};
use crate::{local_file, BoxError};

const COLD_ROOT_LITERAL: &str = "~/Documents/a2a-bridge/evidence-archive";
const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;
const COLD_CAP_BYTES: u64 = 25 * 1024 * 1024 * 1024;
const MAX_COLD_ROOT_ENTRIES: usize = 2_048;

#[derive(Clone)]
pub(super) struct ColdEvidenceStoreV1 {
    root: local_file::PinnedDirectory,
}

impl ColdEvidenceStoreV1 {
    pub(super) fn open_existing(root: &local_file::PinnedDirectory) -> Result<Self, BoxError> {
        validate_private_directory(root, "cold evidence root")?;
        Ok(Self { root: root.clone() })
    }

    pub(super) fn root_sha256(&self) -> &str {
        self.root.object_sha256()
    }
}

fn validate_private_directory(
    directory: &local_file::PinnedDirectory,
    label: &str,
) -> Result<(), BoxError> {
    let metadata = directory.file_handle().metadata()?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o777 != PRIVATE_DIRECTORY_MODE
        || !directory.current_path_matches()
    {
        return Err(format!(
            "schedule retention: {label} is not a current owner-owned mode-0700 directory"
        )
        .into());
    }
    Ok(())
}

fn validate_private_file(metadata: &std::fs::Metadata, label: &str) -> Result<(), BoxError> {
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o777 != PRIVATE_FILE_MODE
    {
        return Err(format!(
            "schedule retention: {label} is not an owner-owned single-link mode-0600 file"
        )
        .into());
    }
    Ok(())
}

fn validate_private_child(
    metadata: local_file::ChildMetadataSnapshot,
    label: &str,
) -> Result<(), BoxError> {
    if !metadata.is_regular()
        || metadata.link_count() != 1
        || metadata.owner_uid() != unsafe { libc::geteuid() }
        || metadata.permission_mode() != PRIVATE_FILE_MODE
    {
        return Err(format!(
            "schedule retention: {label} is not an owner-owned single-link mode-0600 file"
        )
        .into());
    }
    Ok(())
}

fn reserve_cold_capacity(current_bytes: u64, reserved_bytes: u64) -> Result<u64, BoxError> {
    let total = current_bytes
        .checked_add(reserved_bytes)
        .ok_or("schedule retention: cold capacity arithmetic overflow")?;
    if total > COLD_CAP_BYTES {
        return Err("schedule retention: cold evidence cap would be exceeded".into());
    }
    Ok(total)
}

fn cold_usage_bytes(cold: &ColdEvidenceStoreV1) -> Result<u64, BoxError> {
    validate_private_directory(&cold.root, "cold evidence root")?;
    let mut total = 0_u64;
    let mut count = 0_usize;
    for entry in std::fs::read_dir(cold.root.acp_session_cwd())? {
        let entry = entry?;
        count = count
            .checked_add(1)
            .ok_or("schedule retention: cold entry count overflow")?;
        if count > MAX_COLD_ROOT_ENTRIES {
            return Err("schedule retention: cold root inventory exceeds its bound".into());
        }
        let metadata = cold
            .root
            .child_metadata_no_follow(&entry.file_name(), "cold evidence inventory entry")?
            .ok_or("schedule retention: cold inventory entry disappeared during inspection")?;
        validate_private_child(metadata, "cold evidence inventory entry")?;
        total = total
            .checked_add(metadata.length())
            .ok_or("schedule retention: cold byte inventory overflow")?;
        if total > COLD_CAP_BYTES {
            return Err("schedule retention: cold evidence root already exceeds its cap".into());
        }
    }
    if !cold.root.current_path_matches() {
        return Err("schedule retention: cold root changed during inventory".into());
    }
    Ok(total)
}

fn cold_copy_bytes(copy: &ColdCopyRecordV1) -> Result<u64, BoxError> {
    copy.archive_bytes
        .checked_add(copy.manifest_bytes)
        .ok_or_else(|| "schedule retention: cold-copy bytes overflow".into())
}

fn validate_cold_capacity(
    cold: &ColdEvidenceStoreV1,
    state: &EvidenceStateModelV1,
) -> Result<(), BoxError> {
    let materialized = cold_usage_bytes(cold)?;
    let mut published = 0_u64;
    let mut pending = 0_u64;
    let mut pending_materialized = 0_u64;
    for copy in state.cold_copies.values() {
        match copy.lifecycle {
            ColdCopyLifecycleV1::Published { .. } => {
                published = published
                    .checked_add(cold_copy_bytes(copy)?)
                    .ok_or("schedule retention: published cold bytes overflow")?;
            }
            ColdCopyLifecycleV1::Admitted => {
                pending = pending
                    .checked_add(cold_copy_bytes(copy)?)
                    .ok_or("schedule retention: pending cold bytes overflow")?;
                let present = match inspect_cold_copy_residue(cold, copy)? {
                    ColdResidueDispositionV1::None | ColdResidueDispositionV1::Ambiguous => 0,
                    ColdResidueDispositionV1::ArchivePartialOnly => copy.archive_bytes,
                    ColdResidueDispositionV1::ManifestPartialOnly => copy.manifest_bytes,
                    ColdResidueDispositionV1::BothPartials
                    | ColdResidueDispositionV1::ArchivePublishedManifestPartial
                    | ColdResidueDispositionV1::Published => cold_copy_bytes(copy)?,
                };
                pending_materialized = pending_materialized
                    .checked_add(present)
                    .ok_or("schedule retention: pending materialized bytes overflow")?;
            }
            ColdCopyLifecycleV1::Abandoned { .. } => {}
        }
    }
    let other_materialized = materialized
        .checked_sub(pending_materialized)
        .ok_or("schedule retention: pending materialization exceeds cold inventory")?;
    // Published bytes are expected in the materialized inventory. Any additional bytes remain
    // counted, while each pending admission reserves its entire final footprint exactly once.
    reserve_cold_capacity(other_materialized.max(published), pending)?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FileProviderProbeRequestV1 {
    pub(super) cold_root_sha256: String,
    pub(super) file_provider_domain_id: String,
    pub(super) object_path: OptionalRelativeEvidencePathV1,
    pub(super) observed_at_ms: i64,
}

pub(super) trait FileProviderStateProbeV1 {
    fn probe(
        &mut self,
        request: &FileProviderProbeRequestV1,
    ) -> Result<FileProviderObservationV1, BoxError>;

    fn materialize(
        &mut self,
        request: &FileProviderProbeRequestV1,
    ) -> Result<FileProviderObservationV1, BoxError>;
}

fn probe_request(
    copy: Option<&ColdCopyRecordV1>,
    cold: &ColdEvidenceStoreV1,
    domain_id: &str,
    object_path: OptionalRelativeEvidencePathV1,
    observed_at_ms: i64,
) -> Result<FileProviderProbeRequestV1, BoxError> {
    if observed_at_ms <= 0 {
        return Err("schedule retention: FileProvider probe time must be positive".into());
    }
    if let Some(copy) = copy {
        if copy.cold_root_sha256 != cold.root_sha256() || copy.file_provider_domain_id != domain_id
        {
            return Err("schedule retention: copy/root/domain probe binding mismatch".into());
        }
    }
    Ok(FileProviderProbeRequestV1 {
        cold_root_sha256: cold.root_sha256().into(),
        file_provider_domain_id: domain_id.into(),
        object_path,
        observed_at_ms,
    })
}

fn validate_observation(
    request: &FileProviderProbeRequestV1,
    observation: &FileProviderObservationV1,
) -> Result<(), BoxError> {
    observation.validate()?;
    if observation.cold_root_sha256 != request.cold_root_sha256
        || observation.file_provider_domain_id != request.file_provider_domain_id
        || observation.object_path != request.object_path
        || observation.observed_at_ms != request.observed_at_ms
    {
        return Err("schedule retention: FileProvider observation binding mismatch".into());
    }
    Ok(())
}

fn probe_root_ready<P: FileProviderStateProbeV1 + ?Sized>(
    cold: &ColdEvidenceStoreV1,
    probe: &mut P,
    domain_id: &str,
    now_ms: i64,
) -> Result<FileProviderObservationV1, BoxError> {
    validate_private_directory(&cold.root, "cold evidence root")?;
    let request = probe_request(
        None,
        cold,
        domain_id,
        OptionalRelativeEvidencePathV1::Absent,
        now_ms,
    )?;
    let observation = probe.probe(&request)?;
    validate_observation(&request, &observation)?;
    if !matches!(
        observation.state,
        FileProviderObjectStateV1::Known {
            materialization: FileProviderMaterializationV1::Materialized,
            ..
        }
    ) {
        return Err("schedule retention: cold root is unavailable, unknown, or offloaded".into());
    }
    Ok(observation)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ColdCopyAdmissionRequestV1 {
    pub(super) evidence_id: String,
    pub(super) consent_id: String,
    pub(super) operator: String,
    pub(super) environment_owner: String,
    pub(super) deadline_ms: i64,
}

fn cold_copy_id(evidence_id: &str, consent_sha256: &str, admitted_at_ms: i64) -> String {
    let material = format!("{evidence_id}\n{consent_sha256}\n{admitted_at_ms}\n");
    format!("cold-copy:{}", local_file::sha256_hex(material.as_bytes()))
}

fn cold_paths(
    evidence_id: &str,
    copy_id: &str,
) -> (RelativeEvidencePathV1, RelativeEvidencePathV1) {
    let object = local_file::sha256_hex(format!("{evidence_id}\n{copy_id}\n").as_bytes());
    (
        RelativeEvidencePathV1 {
            components: vec![format!("{object}.tar.gz")],
        },
        RelativeEvidencePathV1 {
            components: vec![format!("{object}.manifest.json")],
        },
    )
}

pub(super) fn admit_cold_copy<
    C: EvidenceStateCapability + AuthorityStateCapability + ?Sized,
    P: FileProviderStateProbeV1 + ?Sized,
>(
    capability: &C,
    journal: &mut FileEvidenceJournal<'_>,
    state: &mut EvidenceStateModelV1,
    cold: &ColdEvidenceStoreV1,
    probe: &mut P,
    request: &ColdCopyAdmissionRequestV1,
    recorded_at_ms: i64,
) -> Result<ColdCopyRecordV1, BoxError> {
    state.validate()?;
    let entry = state
        .entries
        .get(&request.evidence_id)
        .ok_or("schedule retention: cold-copy evidence does not exist")?;
    if request.deadline_ms <= recorded_at_ms {
        return Err("schedule retention: cold-copy deadline has elapsed or has no window".into());
    }
    let authority = FileAuthorityJournal::open_existing(capability)?;
    let consent = validate_storage_consent(
        &authority.snapshot.state,
        &request.consent_id,
        &StorageConsentRequestV1 {
            operator: request.operator.clone(),
            environment_owner: request.environment_owner.clone(),
            evidence_class: entry.evidence_class,
            cold_root: COLD_ROOT_LITERAL.into(),
            file_provider_domain_id: authority
                .snapshot
                .state
                .storage_consents
                .get(&request.consent_id)
                .ok_or("schedule retention: storage consent disappeared")?
                .file_provider_domain_id
                .clone(),
            now_ms: recorded_at_ms,
            terminal_deadline_ms: request.deadline_ms,
        },
    )?;
    probe_root_ready(
        cold,
        probe,
        &consent.file_provider_domain_id,
        recorded_at_ms,
    )?;
    let copy_id = cold_copy_id(
        &request.evidence_id,
        &consent.consent_sha256,
        recorded_at_ms,
    );
    let (archive_path, manifest_path) = cold_paths(&request.evidence_id, &copy_id);
    let copy = ColdCopyRecordV1 {
        copy_id,
        evidence_id: request.evidence_id.clone(),
        archive_sha256: entry.full_evidence_sha256.clone(),
        archive_bytes: entry.archive_bytes,
        manifest_sha256: entry.manifest_sha256.clone(),
        manifest_bytes: entry.manifest_bytes,
        consent_id: consent.consent_id.clone(),
        consent_sha256: consent.consent_sha256.clone(),
        consent_revocation_generation: consent.revocation_generation,
        cold_root_sha256: cold.root_sha256().into(),
        file_provider_domain_id: consent.file_provider_domain_id.clone(),
        archive_path,
        manifest_path,
        deadline_ms: request.deadline_ms,
        admitted_at_ms: recorded_at_ms,
        lifecycle: ColdCopyLifecycleV1::Admitted,
    };
    let mut candidate = state.clone();
    candidate.admit_cold_copy(
        ColdStorageBindingV1::OwnerIcloud {
            consent_id: consent.consent_id.clone(),
            consent_sha256: consent.consent_sha256.clone(),
            root_sha256: cold.root_sha256().into(),
            file_provider_domain_id: consent.file_provider_domain_id.clone(),
        },
        copy.clone(),
    )?;
    validate_cold_capacity(cold, &candidate)?;
    journal.append(&candidate, recorded_at_ms)?;
    *state = candidate;
    Ok(copy)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ColdPublicationFailpointV1 {
    None,
    AfterArchivePartial,
    AfterManifestPartial,
    AfterArchivePublication,
    AfterFinalPublication,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ColdResidueDispositionV1 {
    None,
    ArchivePartialOnly,
    ManifestPartialOnly,
    BothPartials,
    ArchivePublishedManifestPartial,
    Published,
    Ambiguous,
}

#[derive(Clone, Debug)]
pub(super) struct ColdPublicationResultV1 {
    pub(super) snapshot_sha256: String,
    pub(super) archive_path: RelativeEvidencePathV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChildDispositionV1 {
    Absent,
    PrivateRegular,
    Other,
}

fn child_disposition(
    directory: &local_file::PinnedDirectory,
    name: &str,
) -> Result<ChildDispositionV1, BoxError> {
    match directory.child_metadata_no_follow(OsStr::new(name), "cold evidence child")? {
        Some(metadata) => {
            if metadata.is_regular()
                && metadata.link_count() == 1
                && metadata.owner_uid() == unsafe { libc::geteuid() }
                && metadata.permission_mode() == PRIVATE_FILE_MODE
            {
                Ok(ChildDispositionV1::PrivateRegular)
            } else {
                Ok(ChildDispositionV1::Other)
            }
        }
        None => Ok(ChildDispositionV1::Absent),
    }
}

fn single_component<'a>(
    path: &'a RelativeEvidencePathV1,
    label: &str,
) -> Result<&'a str, BoxError> {
    if path.components.len() != 1 {
        return Err(format!("schedule retention: {label} is not a single cold-root child").into());
    }
    Ok(&path.components[0])
}

fn partial_name(final_name: &str) -> String {
    format!("{final_name}.partial")
}

pub(super) fn inspect_cold_copy_residue(
    cold: &ColdEvidenceStoreV1,
    copy: &ColdCopyRecordV1,
) -> Result<ColdResidueDispositionV1, BoxError> {
    validate_private_directory(&cold.root, "cold evidence root")?;
    if copy.cold_root_sha256 != cold.root_sha256() {
        return Err("schedule retention: cold-copy/root identity mismatch".into());
    }
    let archive = single_component(&copy.archive_path, "cold archive path")?;
    let manifest = single_component(&copy.manifest_path, "cold manifest path")?;
    let states = [
        child_disposition(&cold.root, &partial_name(archive))?,
        child_disposition(&cold.root, &partial_name(manifest))?,
        child_disposition(&cold.root, archive)?,
        child_disposition(&cold.root, manifest)?,
    ];
    if states.contains(&ChildDispositionV1::Other) {
        return Ok(ColdResidueDispositionV1::Ambiguous);
    }
    Ok(match states {
        [ChildDispositionV1::Absent, ChildDispositionV1::Absent, ChildDispositionV1::Absent, ChildDispositionV1::Absent] => {
            ColdResidueDispositionV1::None
        }
        [ChildDispositionV1::PrivateRegular, ChildDispositionV1::Absent, ChildDispositionV1::Absent, ChildDispositionV1::Absent] => {
            ColdResidueDispositionV1::ArchivePartialOnly
        }
        [ChildDispositionV1::Absent, ChildDispositionV1::PrivateRegular, ChildDispositionV1::Absent, ChildDispositionV1::Absent] => {
            ColdResidueDispositionV1::ManifestPartialOnly
        }
        [ChildDispositionV1::PrivateRegular, ChildDispositionV1::PrivateRegular, ChildDispositionV1::Absent, ChildDispositionV1::Absent] => {
            ColdResidueDispositionV1::BothPartials
        }
        [ChildDispositionV1::Absent, ChildDispositionV1::PrivateRegular, ChildDispositionV1::PrivateRegular, ChildDispositionV1::Absent] => {
            ColdResidueDispositionV1::ArchivePublishedManifestPartial
        }
        [ChildDispositionV1::Absent, ChildDispositionV1::Absent, ChildDispositionV1::PrivateRegular, ChildDispositionV1::PrivateRegular] => {
            ColdResidueDispositionV1::Published
        }
        _ => ColdResidueDispositionV1::Ambiguous,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PayloadBytesV1 {
    archive: Vec<u8>,
    manifest: Vec<u8>,
}

fn read_exact_payload_file(
    directory: &local_file::PinnedDirectory,
    name: &str,
    expected_bytes: u64,
    expected_sha256: &str,
    label: &str,
) -> Result<Vec<u8>, BoxError> {
    let file = directory.open_regular_file(OsStr::new(name), label)?;
    validate_private_file(&file.metadata()?, label)?;
    let snapshot = local_file::read_open_regular_file_bounded(&file, label, expected_bytes)?;
    if snapshot.bytes.len() as u64 != expected_bytes || snapshot.sha256 != expected_sha256 {
        return Err(format!("schedule retention: {label} length or hash mismatch").into());
    }
    Ok(snapshot.bytes)
}

fn load_hot_payload(
    hot: &EvidenceHotStoreV1,
    entry: &IndexedEvidenceV1,
) -> Result<PayloadBytesV1, BoxError> {
    if !entry.hot_present
        || entry.hot_path.components.len() != 2
        || entry.hot_path.components[0] != "sealed"
        || entry.hot_path.components[1] != local_file::sha256_hex(entry.evidence_id.as_bytes())
    {
        return Err("schedule retention: indexed hot payload path is invalid".into());
    }
    let payload = hot.sealed_directory().open_child_directory(
        OsStr::new(&entry.hot_path.components[1]),
        "indexed hot evidence payload",
    )?;
    validate_private_directory(&payload, "indexed hot evidence payload")?;
    let archive = read_exact_payload_file(
        &payload,
        "evidence.tar.gz",
        entry.archive_bytes,
        &entry.full_evidence_sha256,
        "indexed hot evidence archive",
    )?;
    let manifest = read_exact_payload_file(
        &payload,
        "manifest.json",
        entry.manifest_bytes,
        &entry.manifest_sha256,
        "indexed hot evidence manifest",
    )?;
    if !payload.current_path_matches() || !hot.sealed_directory().current_path_matches() {
        return Err("schedule retention: hot payload path changed during verification".into());
    }
    Ok(PayloadBytesV1 { archive, manifest })
}

fn load_cold_payload(
    cold: &ColdEvidenceStoreV1,
    copy: &ColdCopyRecordV1,
) -> Result<PayloadBytesV1, BoxError> {
    validate_private_directory(&cold.root, "cold evidence root")?;
    let archive_name = single_component(&copy.archive_path, "cold archive path")?;
    let manifest_name = single_component(&copy.manifest_path, "cold manifest path")?;
    let archive = read_exact_payload_file(
        &cold.root,
        archive_name,
        copy.archive_bytes,
        &copy.archive_sha256,
        "cold evidence archive",
    )?;
    let manifest = read_exact_payload_file(
        &cold.root,
        manifest_name,
        copy.manifest_bytes,
        &copy.manifest_sha256,
        "cold evidence manifest",
    )?;
    if !cold.root.current_path_matches() {
        return Err("schedule retention: cold evidence root changed during verification".into());
    }
    Ok(PayloadBytesV1 { archive, manifest })
}

fn write_or_verify_partial(
    cold: &ColdEvidenceStoreV1,
    name: &str,
    bytes: &[u8],
    sha256: &str,
) -> Result<(), BoxError> {
    match child_disposition(&cold.root, name)? {
        ChildDispositionV1::Absent => {
            let mut file = cold.root.create_new_file(
                OsStr::new(name),
                PRIVATE_FILE_MODE,
                "cold evidence partial",
            )?;
            file.write_all(bytes)?;
            file.sync_all()?;
            cold.root.sync()?;
        }
        ChildDispositionV1::PrivateRegular => {}
        ChildDispositionV1::Other => {
            return Err("schedule retention: cold partial is not a private regular file".into())
        }
    }
    let observed = read_exact_payload_file(
        &cold.root,
        name,
        bytes.len() as u64,
        sha256,
        "cold evidence partial",
    )?;
    if observed != bytes {
        return Err("schedule retention: cold partial bytes diverged".into());
    }
    Ok(())
}

fn publish_partial(
    cold: &ColdEvidenceStoreV1,
    partial: &str,
    final_name: &str,
) -> Result<(), BoxError> {
    let file = cold
        .root
        .open_regular_file(OsStr::new(partial), "cold evidence partial publication")?;
    validate_private_file(&file.metadata()?, "cold evidence partial publication")?;
    cold.root.publish_new_regular_child(
        local_file::RegularChildRef::new(OsStr::new(partial), &file),
        OsStr::new(final_name),
        "cold evidence final publication",
    )
}

fn validate_admitted_consent(
    consent: &StorageConsentV1,
    copy: &ColdCopyRecordV1,
    entry: &IndexedEvidenceV1,
    now_ms: i64,
) -> Result<(), BoxError> {
    validate_sealed_storage_consent(consent)?;
    if copy.lifecycle != ColdCopyLifecycleV1::Admitted
        || now_ms < copy.admitted_at_ms
        || now_ms > copy.deadline_ms
        || consent.consent_id != copy.consent_id
        || consent.consent_sha256 != copy.consent_sha256
        || consent.revocation_generation != copy.consent_revocation_generation
        || consent.file_provider_domain_id != copy.file_provider_domain_id
        || consent.cold_root != COLD_ROOT_LITERAL
        || consent.not_before_ms > copy.admitted_at_ms
        || consent.expires_at_ms < copy.deadline_ms
        || !consent.evidence_classes.contains(&entry.evidence_class)
    {
        return Err("schedule retention: admitted storage-consent snapshot is invalid".into());
    }
    Ok(())
}

fn probe_object<P: FileProviderStateProbeV1 + ?Sized>(
    cold: &ColdEvidenceStoreV1,
    probe: &mut P,
    copy: &ColdCopyRecordV1,
    path: &RelativeEvidencePathV1,
    now_ms: i64,
) -> Result<FileProviderObservationV1, BoxError> {
    let request = probe_request(
        Some(copy),
        cold,
        &copy.file_provider_domain_id,
        OptionalRelativeEvidencePathV1::RelativePath {
            value: path.clone(),
        },
        now_ms,
    )?;
    let observation = probe.probe(&request)?;
    validate_observation(&request, &observation)?;
    Ok(observation)
}

pub(super) fn publish_admitted_cold_copy<
    C: EvidenceStateCapability + ?Sized,
    P: FileProviderStateProbeV1 + ?Sized,
>(
    _capability: &C,
    journal: &mut FileEvidenceJournal<'_>,
    state: &mut EvidenceStateModelV1,
    hot: &EvidenceHotStoreV1,
    cold: &ColdEvidenceStoreV1,
    probe: &mut P,
    consent: &StorageConsentV1,
    copy_id: &str,
    recorded_at_ms: i64,
    failpoint: ColdPublicationFailpointV1,
) -> Result<ColdPublicationResultV1, BoxError> {
    state.validate()?;
    if state.hot_root_sha256 != hot.root_sha256() {
        return Err("schedule retention: evidence state/hot-root binding mismatch".into());
    }
    let copy = state
        .cold_copies
        .get(copy_id)
        .ok_or("schedule retention: cold-copy admission does not exist")?
        .clone();
    let entry = state
        .entries
        .get(&copy.evidence_id)
        .ok_or("schedule retention: cold-copy target disappeared")?
        .clone();
    validate_admitted_consent(consent, &copy, &entry, recorded_at_ms)?;
    validate_cold_capacity(cold, state)?;
    probe_root_ready(cold, probe, &copy.file_provider_domain_id, recorded_at_ms)?;
    let hot_bytes = load_hot_payload(hot, &entry)?;
    if hot_bytes.archive.len() as u64 != copy.archive_bytes
        || hot_bytes.manifest.len() as u64 != copy.manifest_bytes
        || local_file::sha256_hex(&hot_bytes.archive) != copy.archive_sha256
        || local_file::sha256_hex(&hot_bytes.manifest) != copy.manifest_sha256
    {
        return Err("schedule retention: hot payload diverges from cold admission".into());
    }

    let archive_name = single_component(&copy.archive_path, "cold archive path")?;
    let manifest_name = single_component(&copy.manifest_path, "cold manifest path")?;
    let archive_partial = partial_name(archive_name);
    let manifest_partial = partial_name(manifest_name);
    let disposition = inspect_cold_copy_residue(cold, &copy)?;
    match disposition {
        ColdResidueDispositionV1::None => {
            write_or_verify_partial(
                cold,
                &archive_partial,
                &hot_bytes.archive,
                &copy.archive_sha256,
            )?;
            if failpoint == ColdPublicationFailpointV1::AfterArchivePartial {
                return Err("schedule retention: injected crash after cold archive partial".into());
            }
            write_or_verify_partial(
                cold,
                &manifest_partial,
                &hot_bytes.manifest,
                &copy.manifest_sha256,
            )?;
        }
        ColdResidueDispositionV1::ArchivePartialOnly => {
            write_or_verify_partial(
                cold,
                &archive_partial,
                &hot_bytes.archive,
                &copy.archive_sha256,
            )?;
            write_or_verify_partial(
                cold,
                &manifest_partial,
                &hot_bytes.manifest,
                &copy.manifest_sha256,
            )?;
        }
        ColdResidueDispositionV1::ManifestPartialOnly => {
            write_or_verify_partial(
                cold,
                &archive_partial,
                &hot_bytes.archive,
                &copy.archive_sha256,
            )?;
            write_or_verify_partial(
                cold,
                &manifest_partial,
                &hot_bytes.manifest,
                &copy.manifest_sha256,
            )?;
        }
        ColdResidueDispositionV1::BothPartials => {
            write_or_verify_partial(
                cold,
                &archive_partial,
                &hot_bytes.archive,
                &copy.archive_sha256,
            )?;
            write_or_verify_partial(
                cold,
                &manifest_partial,
                &hot_bytes.manifest,
                &copy.manifest_sha256,
            )?;
        }
        ColdResidueDispositionV1::ArchivePublishedManifestPartial => {
            read_exact_payload_file(
                &cold.root,
                archive_name,
                copy.archive_bytes,
                &copy.archive_sha256,
                "published cold evidence archive",
            )?;
            write_or_verify_partial(
                cold,
                &manifest_partial,
                &hot_bytes.manifest,
                &copy.manifest_sha256,
            )?;
        }
        ColdResidueDispositionV1::Published => {}
        ColdResidueDispositionV1::Ambiguous => {
            return Err("schedule retention: cold publication residue is ambiguous".into())
        }
    }
    if failpoint == ColdPublicationFailpointV1::AfterManifestPartial
        && disposition != ColdResidueDispositionV1::Published
    {
        return Err("schedule retention: injected crash after cold manifest partial".into());
    }

    let rechecked = load_hot_payload(hot, &entry)?;
    if rechecked != hot_bytes {
        return Err("schedule retention: hot source changed before cold publication".into());
    }
    validate_admitted_consent(consent, &copy, &entry, recorded_at_ms)?;
    probe_root_ready(cold, probe, &copy.file_provider_domain_id, recorded_at_ms)?;

    match disposition {
        ColdResidueDispositionV1::Published => {}
        ColdResidueDispositionV1::ArchivePublishedManifestPartial => {
            publish_partial(cold, &manifest_partial, manifest_name)?;
        }
        _ => {
            publish_partial(cold, &archive_partial, archive_name)?;
            if failpoint == ColdPublicationFailpointV1::AfterArchivePublication {
                return Err(
                    "schedule retention: injected crash after cold archive publication".into(),
                );
            }
            publish_partial(cold, &manifest_partial, manifest_name)?;
        }
    }
    let cold_bytes = load_cold_payload(cold, &copy)?;
    if cold_bytes != hot_bytes {
        return Err("schedule retention: published cold payload bytes diverged".into());
    }
    if failpoint == ColdPublicationFailpointV1::AfterFinalPublication {
        return Err("schedule retention: injected crash after cold final publication".into());
    }
    let archive_observation = probe_object(cold, probe, &copy, &copy.archive_path, recorded_at_ms)?;
    let manifest_observation =
        probe_object(cold, probe, &copy, &copy.manifest_path, recorded_at_ms)?;
    let mut candidate = state.clone();
    candidate.publish_cold_copy(
        copy_id,
        archive_observation,
        manifest_observation,
        recorded_at_ms,
    )?;
    let (_snapshot, snapshot_sha256) = journal.append(&candidate, recorded_at_ms)?;
    *state = candidate;
    Ok(ColdPublicationResultV1 {
        snapshot_sha256,
        archive_path: copy.archive_path,
    })
}

fn cleanup_exact_partial(
    cold: &ColdEvidenceStoreV1,
    name: &str,
    expected_bytes: u64,
    expected_sha256: &str,
) -> Result<(), BoxError> {
    let file = cold
        .root
        .open_regular_file(OsStr::new(name), "abandoned cold partial")?;
    validate_private_file(&file.metadata()?, "abandoned cold partial")?;
    let snapshot = local_file::read_open_regular_file_bounded(
        &file,
        "abandoned cold partial",
        expected_bytes,
    )?;
    if snapshot.bytes.len() as u64 != expected_bytes || snapshot.sha256 != expected_sha256 {
        return Err("schedule retention: abandoned cold partial length or hash mismatch".into());
    }
    cold.root.remove_regular_child(
        local_file::RegularChildRef::new(OsStr::new(name), &file),
        "abandoned cold partial cleanup",
    )?;
    cold.root.sync()?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ColdAbandonmentResultV1 {
    pub(super) snapshot_sha256: String,
    pub(super) residue: ColdResidueDispositionV1,
    pub(super) cleanup_required: bool,
}

pub(super) fn abandon_cold_copy<C: EvidenceStateCapability + ?Sized>(
    capability: &C,
    journal: &mut FileEvidenceJournal<'_>,
    state: &mut EvidenceStateModelV1,
    cold: &ColdEvidenceStoreV1,
    copy_id: &str,
    reason_code: &str,
    recorded_at_ms: i64,
) -> Result<ColdAbandonmentResultV1, BoxError> {
    state.validate()?;
    let copy = state
        .cold_copies
        .get(copy_id)
        .ok_or("schedule retention: cold-copy abandonment target does not exist")?
        .clone();
    if copy.lifecycle != ColdCopyLifecycleV1::Admitted || recorded_at_ms <= copy.deadline_ms {
        return Err(
            "schedule retention: only an expired admitted cold copy can be abandoned".into(),
        );
    }
    let residue = inspect_cold_copy_residue(cold, &copy)?;
    let mut candidate = state.clone();
    candidate.abandon_cold_copy(copy_id, reason_code, recorded_at_ms)?;
    let (_snapshot, snapshot_sha256) = journal.append(&candidate, recorded_at_ms)?;
    *state = candidate;

    let cleanup = cleanup_abandoned_cold_copy(capability, state, cold, copy_id);
    Ok(ColdAbandonmentResultV1 {
        snapshot_sha256,
        residue,
        cleanup_required: cleanup.is_err(),
    })
}

pub(super) fn cleanup_abandoned_cold_copy<C: EvidenceStateCapability + ?Sized>(
    _capability: &C,
    state: &EvidenceStateModelV1,
    cold: &ColdEvidenceStoreV1,
    copy_id: &str,
) -> Result<ColdResidueDispositionV1, BoxError> {
    state.validate()?;
    let copy = state
        .cold_copies
        .get(copy_id)
        .ok_or("schedule retention: abandoned cold-copy cleanup target does not exist")?;
    if !matches!(copy.lifecycle, ColdCopyLifecycleV1::Abandoned { .. }) {
        return Err("schedule retention: cold-copy cleanup target is not abandoned".into());
    }
    let archive_name = single_component(&copy.archive_path, "cold archive path")?;
    let manifest_name = single_component(&copy.manifest_path, "cold manifest path")?;
    let archive_partial = partial_name(archive_name);
    let manifest_partial = partial_name(manifest_name);
    let residue = inspect_cold_copy_residue(cold, copy)?;
    match residue {
        ColdResidueDispositionV1::None => {}
        ColdResidueDispositionV1::ArchivePartialOnly => cleanup_exact_partial(
            cold,
            &archive_partial,
            copy.archive_bytes,
            &copy.archive_sha256,
        )?,
        ColdResidueDispositionV1::ManifestPartialOnly => cleanup_exact_partial(
            cold,
            &manifest_partial,
            copy.manifest_bytes,
            &copy.manifest_sha256,
        )?,
        ColdResidueDispositionV1::BothPartials => {
            cleanup_exact_partial(
                cold,
                &archive_partial,
                copy.archive_bytes,
                &copy.archive_sha256,
            )?;
            cleanup_exact_partial(
                cold,
                &manifest_partial,
                copy.manifest_bytes,
                &copy.manifest_sha256,
            )?;
        }
        ColdResidueDispositionV1::ArchivePublishedManifestPartial => {
            cleanup_exact_partial(
                cold,
                &manifest_partial,
                copy.manifest_bytes,
                &copy.manifest_sha256,
            )?;
            return Err("schedule retention: abandoned cold final requires owner cleanup".into());
        }
        // Final cloud objects may already have synchronized. Their removal is a distinct explicit
        // owner action; ambiguity is likewise retained for inspection.
        ColdResidueDispositionV1::Published | ColdResidueDispositionV1::Ambiguous => {
            return Err("schedule retention: abandoned cold final requires owner cleanup".into());
        }
    }
    Ok(residue)
}

fn require_materialized_synchronized(
    observation: &FileProviderObservationV1,
) -> Result<(), BoxError> {
    if !matches!(
        observation.state,
        FileProviderObjectStateV1::Known {
            materialization: FileProviderMaterializationV1::Materialized,
            synchronization: FileProviderSynchronizationV1::Synchronized,
        }
    ) {
        return Err(
            "schedule retention: cold object is not known materialized and synchronized".into(),
        );
    }
    Ok(())
}

fn validate_current_eviction_consent<C: AuthorityStateCapability + ?Sized>(
    capability: &C,
    copy: &ColdCopyRecordV1,
    entry: &IndexedEvidenceV1,
    now_ms: i64,
) -> Result<(), BoxError> {
    let authority = FileAuthorityJournal::open_existing(capability)?;
    let consent = authority
        .snapshot
        .state
        .storage_consents
        .get(&copy.consent_id)
        .ok_or("schedule retention: hot eviction consent no longer exists")?;
    let selected = validate_storage_consent(
        &authority.snapshot.state,
        &copy.consent_id,
        &StorageConsentRequestV1 {
            operator: consent.operator.clone(),
            environment_owner: consent.environment_owner.clone(),
            evidence_class: entry.evidence_class,
            cold_root: COLD_ROOT_LITERAL.into(),
            file_provider_domain_id: copy.file_provider_domain_id.clone(),
            now_ms,
            terminal_deadline_ms: now_ms,
        },
    )?;
    if selected.consent_sha256 != copy.consent_sha256
        || selected.revocation_generation != copy.consent_revocation_generation
    {
        return Err("schedule retention: hot eviction consent differs from copy admission".into());
    }
    Ok(())
}

fn verified_optional_hot_file(
    payload: &local_file::PinnedDirectory,
    name: &str,
    expected_bytes: u64,
    expected_sha256: &str,
) -> Result<Option<std::fs::File>, BoxError> {
    let Some(metadata) =
        payload.child_metadata_no_follow(OsStr::new(name), "hot evidence cleanup payload child")?
    else {
        return Ok(None);
    };
    validate_private_child(metadata, "hot evidence cleanup payload child")?;
    let file = payload.open_regular_file(OsStr::new(name), "hot evidence cleanup payload child")?;
    validate_private_file(&file.metadata()?, "hot evidence cleanup payload child")?;
    let snapshot = local_file::read_open_regular_file_bounded(
        &file,
        "hot evidence cleanup payload child",
        expected_bytes,
    )?;
    if snapshot.bytes.len() as u64 != expected_bytes || snapshot.sha256 != expected_sha256 {
        return Err("schedule retention: hot cleanup payload length or hash mismatch".into());
    }
    Ok(Some(file))
}

fn cleanup_hot_payload(
    hot: &EvidenceHotStoreV1,
    entry: &IndexedEvidenceV1,
    failpoint: HotEvictionFailpointV1,
) -> Result<(), BoxError> {
    if entry.hot_path.components.len() != 2
        || entry.hot_path.components[0] != "sealed"
        || entry.hot_path.components[1] != local_file::sha256_hex(entry.evidence_id.as_bytes())
    {
        return Err("schedule retention: indexed hot cleanup path is invalid".into());
    }
    let object_name = &entry.hot_path.components[1];
    let Some(payload) = hot
        .sealed_directory()
        .open_child_directory_optional(OsStr::new(object_name), "hot evidence eviction payload")?
    else {
        return Ok(());
    };
    validate_private_directory(&payload, "hot evidence eviction payload")?;
    let mut names = std::fs::read_dir(payload.acp_session_cwd())?
        .map(|entry| {
            entry?
                .file_name()
                .into_string()
                .map_err(|_| std::io::Error::other("hot cleanup entry is not UTF-8"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    if names
        .iter()
        .any(|name| name != "evidence.tar.gz" && name != "manifest.json")
    {
        return Err("schedule retention: hot cleanup payload has an unexpected child".into());
    }
    let archive = verified_optional_hot_file(
        &payload,
        "evidence.tar.gz",
        entry.archive_bytes,
        &entry.full_evidence_sha256,
    )?;
    let manifest = verified_optional_hot_file(
        &payload,
        "manifest.json",
        entry.manifest_bytes,
        &entry.manifest_sha256,
    )?;
    if let Some(file) = archive {
        payload.remove_regular_child(
            local_file::RegularChildRef::new(OsStr::new("evidence.tar.gz"), &file),
            "hot evidence archive eviction",
        )?;
        payload.sync()?;
        if failpoint == HotEvictionFailpointV1::AfterArchiveCleanup {
            return Err("schedule retention: injected crash after hot archive cleanup".into());
        }
    }
    if let Some(file) = manifest {
        payload.remove_regular_child(
            local_file::RegularChildRef::new(OsStr::new("manifest.json"), &file),
            "hot evidence manifest eviction",
        )?;
    }
    payload.sync()?;
    hot.sealed_directory().remove_child(
        OsStr::new(object_name),
        true,
        "hot evidence payload eviction",
    )?;
    hot.sealed_directory().sync()?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HotEvictionFailpointV1 {
    None,
    AfterIndexPublication,
    AfterArchiveCleanup,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct HotEvictionResultV1 {
    pub(super) snapshot_sha256: String,
    pub(super) cleanup_required: bool,
}

pub(super) fn evict_hot_evidence<
    C: EvidenceStateCapability + AuthorityStateCapability + ?Sized,
    P: FileProviderStateProbeV1 + ?Sized,
>(
    capability: &C,
    journal: &mut FileEvidenceJournal<'_>,
    state: &mut EvidenceStateModelV1,
    hot: &EvidenceHotStoreV1,
    cold: &ColdEvidenceStoreV1,
    probe: &mut P,
    evidence_id: &str,
    recorded_at_ms: i64,
    failpoint: HotEvictionFailpointV1,
) -> Result<HotEvictionResultV1, BoxError> {
    state.validate()?;
    if state.hot_root_sha256 != hot.root_sha256() {
        return Err("schedule retention: hot-eviction root binding mismatch".into());
    }
    if !state.storage_integrity_holds.is_empty() {
        return Err("schedule retention: storage-integrity hold blocks hot eviction".into());
    }
    let entry = state
        .entries
        .get(evidence_id)
        .ok_or("schedule retention: hot-eviction evidence does not exist")?
        .clone();
    if !entry.hot_present
        || recorded_at_ms < entry.hot_retain_until_ms
        || state.has_active_pin(evidence_id)
    {
        return Err("schedule retention: hot evidence is not eviction-eligible".into());
    }
    let copy = state
        .cold_copies
        .values()
        .find(|copy| {
            copy.evidence_id == evidence_id
                && matches!(copy.lifecycle, ColdCopyLifecycleV1::Published { .. })
        })
        .ok_or("schedule retention: hot eviction has no published cold copy")?
        .clone();
    validate_current_eviction_consent(capability, &copy, &entry, recorded_at_ms)?;
    let _lease = try_acquire_evidence_gc_lease(capability, evidence_id)?;

    let archive_observation = probe_object(cold, probe, &copy, &copy.archive_path, recorded_at_ms)?;
    let manifest_observation =
        probe_object(cold, probe, &copy, &copy.manifest_path, recorded_at_ms)?;
    require_materialized_synchronized(&archive_observation)?;
    require_materialized_synchronized(&manifest_observation)?;
    load_cold_payload(cold, &copy)?;
    let final_archive_observation =
        probe_object(cold, probe, &copy, &copy.archive_path, recorded_at_ms)?;
    let final_manifest_observation =
        probe_object(cold, probe, &copy, &copy.manifest_path, recorded_at_ms)?;
    require_materialized_synchronized(&final_archive_observation)?;
    require_materialized_synchronized(&final_manifest_observation)?;
    if archive_observation != final_archive_observation
        || manifest_observation != final_manifest_observation
    {
        return Err("schedule retention: FileProvider state changed during hot eviction".into());
    }

    load_hot_payload(hot, &entry)?;
    let mut candidate = state.clone();
    candidate.update_published_cold_copy(
        &copy.copy_id,
        final_archive_observation,
        final_manifest_observation,
        Some(recorded_at_ms),
    )?;
    candidate.mark_hot_evidence_absent(evidence_id)?;
    let (_snapshot, snapshot_sha256) = journal.append(&candidate, recorded_at_ms)?;
    *state = candidate;
    if failpoint == HotEvictionFailpointV1::AfterIndexPublication {
        return Err("schedule retention: injected crash after hot-eviction index".into());
    }
    let cleanup_required = cleanup_hot_payload(hot, &entry, failpoint).is_err();
    Ok(HotEvictionResultV1 {
        snapshot_sha256,
        cleanup_required,
    })
}

pub(super) fn cleanup_evicted_hot_evidence<C: EvidenceStateCapability + ?Sized>(
    capability: &C,
    state: &EvidenceStateModelV1,
    hot: &EvidenceHotStoreV1,
    evidence_id: &str,
) -> Result<(), BoxError> {
    state.validate()?;
    if state.hot_root_sha256 != hot.root_sha256() {
        return Err("schedule retention: hot-cleanup root binding mismatch".into());
    }
    let entry = state
        .entries
        .get(evidence_id)
        .ok_or("schedule retention: hot-cleanup evidence does not exist")?;
    if entry.hot_present {
        return Err("schedule retention: indexed hot evidence remains present".into());
    }
    let _lease = try_acquire_evidence_gc_lease(capability, evidence_id)?;
    cleanup_hot_payload(hot, entry, HotEvictionFailpointV1::None)
}

const MAX_ROTATING_VERIFICATION_BATCH: usize = 64;

pub(super) fn plan_rotating_cold_verifications(
    state: &EvidenceStateModelV1,
    max_items: usize,
) -> Result<Vec<String>, BoxError> {
    state.validate()?;
    if max_items == 0 || max_items > MAX_ROTATING_VERIFICATION_BATCH {
        return Err("schedule retention: rotating verification batch is out of bounds".into());
    }
    let mut candidates = state
        .cold_copies
        .values()
        .filter_map(|copy| {
            let ColdCopyLifecycleV1::Published {
                last_content_verified_at_ms,
                ..
            } = copy.lifecycle
            else {
                return None;
            };
            state
                .entries
                .contains_key(&copy.evidence_id)
                .then(|| (last_content_verified_at_ms, copy.evidence_id.clone()))
        })
        .collect::<Vec<_>>();
    candidates.sort();
    Ok(candidates
        .into_iter()
        .take(max_items)
        .map(|(_, evidence_id)| evidence_id)
        .collect())
}

fn materialize_for_verification<P: FileProviderStateProbeV1 + ?Sized>(
    cold: &ColdEvidenceStoreV1,
    probe: &mut P,
    copy: &ColdCopyRecordV1,
    path: &RelativeEvidencePathV1,
    now_ms: i64,
) -> Result<FileProviderObservationV1, BoxError> {
    let request = probe_request(
        Some(copy),
        cold,
        &copy.file_provider_domain_id,
        OptionalRelativeEvidencePathV1::RelativePath {
            value: path.clone(),
        },
        now_ms,
    )?;
    let observed = probe.probe(&request)?;
    validate_observation(&request, &observed)?;
    match observed.state {
        FileProviderObjectStateV1::Known {
            materialization: FileProviderMaterializationV1::Materialized,
            synchronization: FileProviderSynchronizationV1::Synchronized,
        } => Ok(observed),
        FileProviderObjectStateV1::Known {
            materialization: FileProviderMaterializationV1::Offloaded,
            synchronization: FileProviderSynchronizationV1::Synchronized,
        } => {
            let materialized = probe.materialize(&request)?;
            validate_observation(&request, &materialized)?;
            require_materialized_synchronized(&materialized)?;
            Ok(materialized)
        }
        _ => Err("schedule retention: cold verification provider state is blocking".into()),
    }
}

fn integrity_hold_id(evidence_id: &str) -> String {
    format!(
        "cold-integrity:{}",
        local_file::sha256_hex(evidence_id.as_bytes())
    )
}

fn persist_integrity_hold(
    journal: &mut FileEvidenceJournal<'_>,
    state: &mut EvidenceStateModelV1,
    evidence_id: &str,
    reason_code: &str,
    recorded_at_ms: i64,
) -> Result<(String, Option<String>), BoxError> {
    if let Some(existing) = state
        .storage_integrity_holds
        .values()
        .find(|hold| hold.evidence_id == evidence_id)
    {
        return Ok((existing.hold_id.clone(), None));
    }
    let hold = StorageIntegrityHoldV1 {
        hold_id: integrity_hold_id(evidence_id),
        evidence_id: evidence_id.into(),
        reason_code: reason_code.into(),
        detected_at_ms: recorded_at_ms,
    };
    let mut candidate = state.clone();
    candidate.add_storage_integrity_hold(hold.clone())?;
    let (_snapshot, snapshot_sha256) = journal.append(&candidate, recorded_at_ms)?;
    *state = candidate;
    Ok((hold.hold_id, Some(snapshot_sha256)))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ColdVerificationOutcomeV1 {
    Verified {
        snapshot_sha256: String,
    },
    IntegrityBlocked {
        hold_id: String,
        snapshot_sha256: Option<String>,
    },
}

pub(super) fn verify_cold_evidence<
    C: EvidenceStateCapability + ?Sized,
    P: FileProviderStateProbeV1 + ?Sized,
>(
    capability: &C,
    journal: &mut FileEvidenceJournal<'_>,
    state: &mut EvidenceStateModelV1,
    cold: &ColdEvidenceStoreV1,
    probe: &mut P,
    evidence_id: &str,
    recorded_at_ms: i64,
) -> Result<ColdVerificationOutcomeV1, BoxError> {
    state.validate()?;
    let copy = state
        .cold_copies
        .values()
        .find(|copy| {
            copy.evidence_id == evidence_id
                && matches!(copy.lifecycle, ColdCopyLifecycleV1::Published { .. })
        })
        .ok_or("schedule retention: cold verification target is not published")?
        .clone();
    let _lease = acquire_evidence_read_lease(capability, evidence_id)?;
    let verified =
        (|| -> Result<(FileProviderObservationV1, FileProviderObservationV1), BoxError> {
            let archive_observation = materialize_for_verification(
                cold,
                probe,
                &copy,
                &copy.archive_path,
                recorded_at_ms,
            )?;
            let manifest_observation = materialize_for_verification(
                cold,
                probe,
                &copy,
                &copy.manifest_path,
                recorded_at_ms,
            )?;
            load_cold_payload(cold, &copy)?;
            let final_archive =
                probe_object(cold, probe, &copy, &copy.archive_path, recorded_at_ms)?;
            let final_manifest =
                probe_object(cold, probe, &copy, &copy.manifest_path, recorded_at_ms)?;
            require_materialized_synchronized(&final_archive)?;
            require_materialized_synchronized(&final_manifest)?;
            if final_archive != archive_observation || final_manifest != manifest_observation {
                return Err(
                    "schedule retention: FileProvider state changed during verification".into(),
                );
            }
            Ok((final_archive, final_manifest))
        })();

    let (archive_observation, manifest_observation) = match verified {
        Ok(value) => value,
        Err(_error) => {
            let (hold_id, snapshot_sha256) = persist_integrity_hold(
                journal,
                state,
                evidence_id,
                "cold_verification_failed",
                recorded_at_ms,
            )?;
            return Ok(ColdVerificationOutcomeV1::IntegrityBlocked {
                hold_id,
                snapshot_sha256,
            });
        }
    };
    let mut candidate = state.clone();
    candidate.update_published_cold_copy(
        &copy.copy_id,
        archive_observation,
        manifest_observation,
        Some(recorded_at_ms),
    )?;
    let (_snapshot, snapshot_sha256) = journal.append(&candidate, recorded_at_ms)?;
    *state = candidate;
    Ok(ColdVerificationOutcomeV1::Verified { snapshot_sha256 })
}

fn observation_is_integrity_blocking(observation: &FileProviderObservationV1) -> bool {
    matches!(
        observation.state,
        FileProviderObjectStateV1::Unavailable { .. } | FileProviderObjectStateV1::Unknown { .. }
    )
}

fn add_integrity_hold_if_absent(
    state: &mut EvidenceStateModelV1,
    evidence_id: &str,
    reason_code: &str,
    recorded_at_ms: i64,
) -> Result<(), BoxError> {
    if state
        .storage_integrity_holds
        .values()
        .any(|hold| hold.evidence_id == evidence_id)
    {
        return Ok(());
    }
    state.add_storage_integrity_hold(StorageIntegrityHoldV1 {
        hold_id: integrity_hold_id(evidence_id),
        evidence_id: evidence_id.into(),
        reason_code: reason_code.into(),
        detected_at_ms: recorded_at_ms,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ColdMetadataReconciliationV1 {
    pub(super) observed_objects: usize,
    pub(super) blocked_evidence_ids: Vec<String>,
    pub(super) snapshot_sha256: String,
}

pub(super) fn reconcile_cold_metadata<
    C: EvidenceStateCapability + ?Sized,
    P: FileProviderStateProbeV1 + ?Sized,
>(
    _capability: &C,
    journal: &mut FileEvidenceJournal<'_>,
    state: &mut EvidenceStateModelV1,
    cold: &ColdEvidenceStoreV1,
    probe: &mut P,
    recorded_at_ms: i64,
) -> Result<ColdMetadataReconciliationV1, BoxError> {
    state.validate()?;
    let mut copies = state
        .cold_copies
        .values()
        .filter(|copy| matches!(copy.lifecycle, ColdCopyLifecycleV1::Published { .. }))
        .cloned()
        .collect::<Vec<_>>();
    copies.sort_by(|left, right| left.evidence_id.cmp(&right.evidence_id));
    if copies.is_empty() {
        return Err("schedule retention: metadata reconciliation has no published copies".into());
    }
    let mut candidate = state.clone();
    let mut observed_objects = 0_usize;
    let mut blocked_evidence_ids = Vec::new();
    for copy in copies {
        let observations = (|| {
            let archive = probe_object(cold, probe, &copy, &copy.archive_path, recorded_at_ms)?;
            let manifest = probe_object(cold, probe, &copy, &copy.manifest_path, recorded_at_ms)?;
            Ok::<_, BoxError>((archive, manifest))
        })();
        match observations {
            Ok((archive, manifest)) => {
                observed_objects = observed_objects
                    .checked_add(2)
                    .ok_or("schedule retention: metadata observation count overflow")?;
                let blocking = observation_is_integrity_blocking(&archive)
                    || observation_is_integrity_blocking(&manifest);
                candidate.update_published_cold_copy(&copy.copy_id, archive, manifest, None)?;
                if blocking {
                    add_integrity_hold_if_absent(
                        &mut candidate,
                        &copy.evidence_id,
                        "file_provider_state_unknown",
                        recorded_at_ms,
                    )?;
                    blocked_evidence_ids.push(copy.evidence_id);
                }
            }
            Err(_) => {
                add_integrity_hold_if_absent(
                    &mut candidate,
                    &copy.evidence_id,
                    "file_provider_probe_failed",
                    recorded_at_ms,
                )?;
                blocked_evidence_ids.push(copy.evidence_id);
            }
        }
    }
    blocked_evidence_ids.sort();
    blocked_evidence_ids.dedup();
    let (_snapshot, snapshot_sha256) = journal.append(&candidate, recorded_at_ms)?;
    *state = candidate;
    Ok(ColdMetadataReconciliationV1 {
        observed_objects,
        blocked_evidence_ids,
        snapshot_sha256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compatibility_schedule::ReplicationModeV1;
    use crate::compatibility_schedule_authority::{seal_storage_consent, FileAuthorityJournal};
    use crate::compatibility_schedule_evidence::{
        acquire_evidence_read_lease, decide_retention, ColdCopyLifecycleV1, EvidenceHotStoreV1,
        EvidenceRetentionRequestV1, EvidenceStateModelV1, FileEvidenceJournal,
        FileProviderMaterializationV1, FileProviderObjectStateV1, FileProviderObservationV1,
        FileProviderSynchronizationV1, IndexedEvidenceV1,
    };
    use crate::compatibility_schedule_schema::{
        ColdStorageBindingV1, EvidenceClassV1, OptionalRelativeEvidencePathV1, OptionalSha256V1,
        RelativeEvidencePathV1, StorageConsentV1,
    };
    use crate::compatibility_schedule_state::SchedulerStateRoot;
    use crate::local_file;
    use serde::Serialize;
    use std::collections::{BTreeMap, BTreeSet};
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::Path;

    const BASE: i64 = 20_000_000_000;

    fn digest(ch: char) -> String {
        ch.to_string().repeat(64)
    }

    fn private_root() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        root
    }

    fn pin(path: &Path, label: &str) -> local_file::PinnedDirectory {
        let snapshot = local_file::snapshot_directory(path, label).unwrap();
        local_file::PinnedDirectory::open(path, &snapshot.canonical_cwd, &snapshot.identity, label)
            .unwrap()
    }

    fn write_private(path: &Path, bytes: &[u8]) {
        std::fs::write(path, bytes).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[derive(Serialize)]
    struct TestCompactRecordV1 {
        schema_version: u16,
        evidence_id: String,
        evidence_class: EvidenceClassV1,
        terminal_at_ms: i64,
        affected_case_ids: Vec<String>,
        sidecar_sha256: String,
        aggregate_sha256: OptionalSha256V1,
        archive_sha256: String,
        manifest_sha256: String,
    }

    fn add_hot_evidence(hot_root: &Path, state: &mut EvidenceStateModelV1, evidence_id: &str) {
        let archive = format!("archive:{evidence_id}\n").into_bytes();
        let manifest = format!("manifest:{evidence_id}\n").into_bytes();
        let archive_sha256 = local_file::sha256_hex(&archive);
        let manifest_sha256 = local_file::sha256_hex(&manifest);
        let object_name = local_file::sha256_hex(evidence_id.as_bytes());
        let payload = hot_root.join("sealed").join(&object_name);
        std::fs::create_dir(&payload).unwrap();
        std::fs::set_permissions(&payload, std::fs::Permissions::from_mode(0o700)).unwrap();
        write_private(&payload.join("evidence.tar.gz"), &archive);
        write_private(&payload.join("manifest.json"), &manifest);

        let terminal_at_ms = 1_000_000;
        let retention = decide_retention(&EvidenceRetentionRequestV1 {
            evidence_class: EvidenceClassV1::RoutineGreen,
            terminal_at_ms,
            case_minimum_days: 30,
            release_retain_until_ms: None,
            pinned: false,
        })
        .unwrap();
        let mut compact_record = serde_json::to_vec(&TestCompactRecordV1 {
            schema_version: 1,
            evidence_id: evidence_id.into(),
            evidence_class: EvidenceClassV1::RoutineGreen,
            terminal_at_ms,
            affected_case_ids: vec!["case-1".into()],
            sidecar_sha256: digest('c'),
            aggregate_sha256: OptionalSha256V1::Absent,
            archive_sha256: archive_sha256.clone(),
            manifest_sha256: manifest_sha256.clone(),
        })
        .unwrap();
        compact_record.push(b'\n');
        let compact_record = String::from_utf8(compact_record).unwrap();
        state
            .insert_entry(IndexedEvidenceV1 {
                evidence_id: evidence_id.into(),
                evidence_class: EvidenceClassV1::RoutineGreen,
                full_evidence_sha256: archive_sha256,
                manifest_sha256,
                compact_record_sha256: local_file::sha256_hex(compact_record.as_bytes()),
                archive_bytes: archive.len() as u64,
                manifest_bytes: manifest.len() as u64,
                compact_record_bytes: compact_record.len() as u64,
                compact_record,
                hot_path: RelativeEvidencePathV1 {
                    components: vec!["sealed".into(), object_name],
                },
                cold_path: OptionalRelativeEvidencePathV1::Absent,
                terminal_at_ms,
                case_minimum_days: 30,
                full_retain_until_ms: retention.full_retain_until_ms,
                compact_retain_until_ms: retention.compact_retain_until_ms,
                hot_retain_until_ms: retention.hot_retain_until_ms,
                hot_present: true,
            })
            .unwrap();
    }

    fn consent() -> StorageConsentV1 {
        seal_storage_consent(StorageConsentV1 {
            schema_version: 1,
            consent_id: "consent-1".into(),
            consent_sha256: digest('0'),
            operator: "Wesley Jinks".into(),
            environment_owner: "wesley-macbook".into(),
            evidence_classes: vec![EvidenceClassV1::RoutineGreen],
            cold_root: "~/Documents/a2a-bridge/evidence-archive".into(),
            replication_mode: ReplicationModeV1::OwnerIcloud,
            file_provider_domain_id: "icloud-domain-1".into(),
            not_before_ms: BASE - 100,
            expires_at_ms: BASE + 10_000,
            revocation_generation: 1,
        })
        .unwrap()
    }

    struct Fixture {
        _scheduler_root: tempfile::TempDir,
        hot_root: tempfile::TempDir,
        cold_root: tempfile::TempDir,
        scheduler: SchedulerStateRoot,
        hot: EvidenceHotStoreV1,
        cold: ColdEvidenceStoreV1,
        consent: StorageConsentV1,
    }

    impl Fixture {
        fn new() -> Self {
            let scheduler_root = private_root();
            let scheduler = SchedulerStateRoot::initialize_for_test(scheduler_root.path()).unwrap();
            let authority = scheduler
                .try_authority_mutation("test/r3d3c-authority-setup")
                .unwrap();
            let mut opened = FileAuthorityJournal::initialize(&authority, BASE - 3).unwrap();
            let consent = consent();
            let mut authority_state = opened.snapshot.state.clone();
            authority_state
                .install_storage_consent(consent.clone())
                .unwrap();
            opened.journal.append(&authority_state, BASE - 2).unwrap();
            drop(opened);
            drop(authority);

            let hot_root = private_root();
            for name in ["state", "scratch", "sealed"] {
                std::fs::create_dir(hot_root.path().join(name)).unwrap();
                std::fs::set_permissions(
                    hot_root.path().join(name),
                    std::fs::Permissions::from_mode(0o700),
                )
                .unwrap();
            }
            let hot_pin = pin(hot_root.path(), "test hot root");
            let hot = EvidenceHotStoreV1::open_existing(&hot_pin).unwrap();
            let mut state =
                EvidenceStateModelV1::new(hot.root_sha256().into(), ColdStorageBindingV1::Absent)
                    .unwrap();
            add_hot_evidence(hot_root.path(), &mut state, "evidence-1");
            add_hot_evidence(hot_root.path(), &mut state, "evidence-2");
            let owner = scheduler
                .try_owner_admission("test/r3d3c-evidence-setup")
                .unwrap();
            let opened = FileEvidenceJournal::initialize(&owner, &state, BASE).unwrap();
            drop(opened);
            drop(owner);

            let cold_root = private_root();
            let cold_pin = pin(cold_root.path(), "test cold root");
            let cold = ColdEvidenceStoreV1::open_existing(&cold_pin).unwrap();
            Self {
                _scheduler_root: scheduler_root,
                hot_root,
                cold_root,
                scheduler,
                hot,
                cold,
                consent,
            }
        }

        fn request(evidence_id: &str) -> ColdCopyAdmissionRequestV1 {
            ColdCopyAdmissionRequestV1 {
                evidence_id: evidence_id.into(),
                consent_id: "consent-1".into(),
                operator: "Wesley Jinks".into(),
                environment_owner: "wesley-macbook".into(),
                deadline_ms: BASE + 100,
            }
        }

        fn revoke_consent(&self, at_ms: i64) {
            let authority = self
                .scheduler
                .try_authority_mutation("test/r3d3c-revoke")
                .unwrap();
            let mut opened = FileAuthorityJournal::open_existing(&authority).unwrap();
            let mut state = opened.snapshot.state.clone();
            state.revoke_storage_consent("consent-1").unwrap();
            opened.journal.append(&state, at_ms).unwrap();
        }
    }

    #[derive(Clone)]
    struct FakeProvider {
        cold_root_sha256: String,
        domain_id: String,
        materialization: FileProviderMaterializationV1,
        synchronization: FileProviderSynchronizationV1,
        overrides: BTreeMap<String, FileProviderObjectStateV1>,
        materialized_paths: BTreeSet<String>,
        materialize_calls: usize,
    }

    impl FakeProvider {
        fn new(cold: &ColdEvidenceStoreV1) -> Self {
            Self {
                cold_root_sha256: cold.root_sha256().into(),
                domain_id: "icloud-domain-1".into(),
                materialization: FileProviderMaterializationV1::Materialized,
                synchronization: FileProviderSynchronizationV1::NotUploaded,
                overrides: BTreeMap::new(),
                materialized_paths: BTreeSet::new(),
                materialize_calls: 0,
            }
        }

        fn observation(&self, request: &FileProviderProbeRequestV1) -> FileProviderObservationV1 {
            let key = match &request.object_path {
                OptionalRelativeEvidencePathV1::Absent => "<root>".into(),
                OptionalRelativeEvidencePathV1::RelativePath { value } => {
                    value.components.join("/")
                }
            };
            let state = if self.materialized_paths.contains(&key) {
                FileProviderObjectStateV1::Known {
                    materialization: FileProviderMaterializationV1::Materialized,
                    synchronization: self.synchronization,
                }
            } else {
                self.overrides
                    .get(&key)
                    .cloned()
                    .unwrap_or(FileProviderObjectStateV1::Known {
                        materialization: self.materialization,
                        synchronization: self.synchronization,
                    })
            };
            FileProviderObservationV1 {
                cold_root_sha256: self.cold_root_sha256.clone(),
                file_provider_domain_id: self.domain_id.clone(),
                object_path: request.object_path.clone(),
                state,
                observed_at_ms: request.observed_at_ms,
            }
        }
    }

    impl FileProviderStateProbeV1 for FakeProvider {
        fn probe(
            &mut self,
            request: &FileProviderProbeRequestV1,
        ) -> Result<FileProviderObservationV1, crate::BoxError> {
            Ok(self.observation(request))
        }

        fn materialize(
            &mut self,
            request: &FileProviderProbeRequestV1,
        ) -> Result<FileProviderObservationV1, crate::BoxError> {
            self.materialize_calls += 1;
            let key = match &request.object_path {
                OptionalRelativeEvidencePathV1::Absent => "<root>".into(),
                OptionalRelativeEvidencePathV1::RelativePath { value } => {
                    value.components.join("/")
                }
            };
            self.materialized_paths.insert(key);
            Ok(self.observation(request))
        }
    }

    fn admit(
        fixture: &Fixture,
        provider: &mut FakeProvider,
        evidence_id: &str,
        at_ms: i64,
    ) -> crate::compatibility_schedule_evidence::ColdCopyRecordV1 {
        let combined = fixture
            .scheduler
            .try_owner_admission("test/r3d3c-admit-owner")
            .unwrap()
            .try_authority_state("test/r3d3c-admit-authority")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&combined).unwrap();
        let mut state = opened.snapshot.state.clone();
        admit_cold_copy(
            &combined,
            &mut opened.journal,
            &mut state,
            &fixture.cold,
            provider,
            &Fixture::request(evidence_id),
            at_ms,
        )
        .unwrap()
    }

    #[test]
    fn cold_admission_is_independent_current_and_precedes_every_cold_byte() {
        let fixture = Fixture::new();
        let mut provider = FakeProvider::new(&fixture.cold);
        let combined = fixture
            .scheduler
            .try_owner_admission("test/r3d3c-zero-window-owner")
            .unwrap()
            .try_authority_state("test/r3d3c-zero-window-authority")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&combined).unwrap();
        let mut state = opened.snapshot.state.clone();
        let mut zero_window = Fixture::request("evidence-1");
        zero_window.deadline_ms = BASE + 1;
        assert!(admit_cold_copy(
            &combined,
            &mut opened.journal,
            &mut state,
            &fixture.cold,
            &mut provider,
            &zero_window,
            BASE + 1,
        )
        .is_err());
        drop(opened);
        drop(combined);

        provider.domain_id = "wrong-domain".into();
        let combined = fixture
            .scheduler
            .try_owner_admission("test/r3d3c-wrong-domain-owner")
            .unwrap()
            .try_authority_state("test/r3d3c-wrong-domain-authority")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&combined).unwrap();
        let mut state = opened.snapshot.state.clone();
        assert!(admit_cold_copy(
            &combined,
            &mut opened.journal,
            &mut state,
            &fixture.cold,
            &mut provider,
            &Fixture::request("evidence-1"),
            BASE + 1,
        )
        .is_err());
        assert!(state.cold_copies.is_empty());
        drop(opened);
        drop(combined);

        provider.domain_id = "icloud-domain-1".into();
        let admission = admit(&fixture, &mut provider, "evidence-1", BASE + 1);
        assert_eq!(admission.lifecycle, ColdCopyLifecycleV1::Admitted);
        assert!(std::fs::read_dir(fixture.cold_root.path())
            .unwrap()
            .next()
            .is_none());

        fixture.revoke_consent(BASE + 2);
        let combined = fixture
            .scheduler
            .try_owner_admission("test/r3d3c-revoked-owner")
            .unwrap()
            .try_authority_state("test/r3d3c-revoked-authority")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&combined).unwrap();
        let mut state = opened.snapshot.state.clone();
        assert!(admit_cold_copy(
            &combined,
            &mut opened.journal,
            &mut state,
            &fixture.cold,
            &mut provider,
            &Fixture::request("evidence-2"),
            BASE + 3,
        )
        .is_err());
        assert!(std::fs::read_dir(fixture.cold_root.path())
            .unwrap()
            .next()
            .is_none());
    }

    #[test]
    fn admitted_copy_recovers_partial_after_revocation_and_indexes_last() {
        let fixture = Fixture::new();
        let mut provider = FakeProvider::new(&fixture.cold);
        let admission = admit(&fixture, &mut provider, "evidence-1", BASE + 1);
        fixture.revoke_consent(BASE + 2);

        let owner = fixture
            .scheduler
            .try_owner_admission("test/r3d3c-partial")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let mut state = opened.snapshot.state.clone();
        assert!(publish_admitted_cold_copy(
            &owner,
            &mut opened.journal,
            &mut state,
            &fixture.hot,
            &fixture.cold,
            &mut provider,
            &fixture.consent,
            &admission.copy_id,
            BASE + 3,
            ColdPublicationFailpointV1::AfterArchivePartial,
        )
        .is_err());
        assert_eq!(
            inspect_cold_copy_residue(&fixture.cold, &admission).unwrap(),
            ColdResidueDispositionV1::ArchivePartialOnly
        );
        assert!(state.entries["evidence-1"].hot_present);
        assert!(matches!(
            state.entries["evidence-1"].cold_path,
            OptionalRelativeEvidencePathV1::Absent
        ));
        drop(opened);
        drop(owner);

        let owner = fixture
            .scheduler
            .try_owner_admission("test/r3d3c-recover")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let mut state = opened.snapshot.state.clone();
        publish_admitted_cold_copy(
            &owner,
            &mut opened.journal,
            &mut state,
            &fixture.hot,
            &fixture.cold,
            &mut provider,
            &fixture.consent,
            &admission.copy_id,
            BASE + 4,
            ColdPublicationFailpointV1::None,
        )
        .unwrap();
        assert_eq!(
            inspect_cold_copy_residue(&fixture.cold, &admission).unwrap(),
            ColdResidueDispositionV1::Published
        );
        assert!(matches!(
            state.cold_copies[&admission.copy_id].lifecycle,
            ColdCopyLifecycleV1::Published { .. }
        ));
        assert!(state.entries["evidence-1"].hot_present);
        assert!(matches!(
            state.entries["evidence-1"].cold_path,
            OptionalRelativeEvidencePathV1::RelativePath { .. }
        ));
    }

    #[test]
    fn cold_publication_refuses_tamper_and_collision_and_recovers_final_before_index() {
        let fixture = Fixture::new();
        let mut provider = FakeProvider::new(&fixture.cold);
        let admission = admit(&fixture, &mut provider, "evidence-1", BASE + 1);
        let hot_archive = fixture
            .hot_root
            .path()
            .join("sealed")
            .join(local_file::sha256_hex(b"evidence-1"))
            .join("evidence.tar.gz");
        std::fs::write(&hot_archive, b"tampered").unwrap();
        let owner = fixture
            .scheduler
            .try_owner_admission("test/r3d3c-tamper")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let mut state = opened.snapshot.state.clone();
        assert!(publish_admitted_cold_copy(
            &owner,
            &mut opened.journal,
            &mut state,
            &fixture.hot,
            &fixture.cold,
            &mut provider,
            &fixture.consent,
            &admission.copy_id,
            BASE + 2,
            ColdPublicationFailpointV1::None,
        )
        .is_err());
        assert_eq!(
            inspect_cold_copy_residue(&fixture.cold, &admission).unwrap(),
            ColdResidueDispositionV1::None
        );

        let collision_fixture = Fixture::new();
        let mut collision_provider = FakeProvider::new(&collision_fixture.cold);
        let collision = admit(
            &collision_fixture,
            &mut collision_provider,
            "evidence-1",
            BASE + 1,
        );
        let final_archive = collision_fixture
            .cold_root
            .path()
            .join(&collision.archive_path.components[0]);
        write_private(&final_archive, b"existing final");
        let owner = collision_fixture
            .scheduler
            .try_owner_admission("test/r3d3c-collision")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let mut state = opened.snapshot.state.clone();
        assert!(publish_admitted_cold_copy(
            &owner,
            &mut opened.journal,
            &mut state,
            &collision_fixture.hot,
            &collision_fixture.cold,
            &mut collision_provider,
            &collision_fixture.consent,
            &collision.copy_id,
            BASE + 2,
            ColdPublicationFailpointV1::None,
        )
        .is_err());
        assert_eq!(std::fs::read(&final_archive).unwrap(), b"existing final");

        let recovery_fixture = Fixture::new();
        let mut recovery_provider = FakeProvider::new(&recovery_fixture.cold);
        let recovery = admit(
            &recovery_fixture,
            &mut recovery_provider,
            "evidence-1",
            BASE + 1,
        );
        let owner = recovery_fixture
            .scheduler
            .try_owner_admission("test/r3d3c-final-crash")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let mut state = opened.snapshot.state.clone();
        assert!(publish_admitted_cold_copy(
            &owner,
            &mut opened.journal,
            &mut state,
            &recovery_fixture.hot,
            &recovery_fixture.cold,
            &mut recovery_provider,
            &recovery_fixture.consent,
            &recovery.copy_id,
            BASE + 2,
            ColdPublicationFailpointV1::AfterFinalPublication,
        )
        .is_err());
        assert_eq!(
            inspect_cold_copy_residue(&recovery_fixture.cold, &recovery).unwrap(),
            ColdResidueDispositionV1::Published
        );
        assert!(matches!(
            state.entries["evidence-1"].cold_path,
            OptionalRelativeEvidencePathV1::Absent
        ));
        publish_admitted_cold_copy(
            &owner,
            &mut opened.journal,
            &mut state,
            &recovery_fixture.hot,
            &recovery_fixture.cold,
            &mut recovery_provider,
            &recovery_fixture.consent,
            &recovery.copy_id,
            BASE + 3,
            ColdPublicationFailpointV1::None,
        )
        .unwrap();
        assert!(matches!(
            state.entries["evidence-1"].cold_path,
            OptionalRelativeEvidencePathV1::RelativePath { .. }
        ));
    }

    #[test]
    fn hot_eviction_requires_fresh_synced_materialized_content_and_no_reader() {
        let fixture = Fixture::new();
        let mut provider = FakeProvider::new(&fixture.cold);
        let admission = admit(&fixture, &mut provider, "evidence-1", BASE + 1);
        let owner = fixture
            .scheduler
            .try_owner_admission("test/r3d3c-publish")
            .unwrap()
            .try_authority_state("test/r3d3c-eviction-authority")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let mut state = opened.snapshot.state.clone();
        publish_admitted_cold_copy(
            &owner,
            &mut opened.journal,
            &mut state,
            &fixture.hot,
            &fixture.cold,
            &mut provider,
            &fixture.consent,
            &admission.copy_id,
            BASE + 2,
            ColdPublicationFailpointV1::None,
        )
        .unwrap();

        assert!(evict_hot_evidence(
            &owner,
            &mut opened.journal,
            &mut state,
            &fixture.hot,
            &fixture.cold,
            &mut provider,
            "evidence-1",
            BASE + 3,
            HotEvictionFailpointV1::None,
        )
        .is_err());
        assert!(state.entries["evidence-1"].hot_present);

        provider.synchronization = FileProviderSynchronizationV1::Synchronized;
        let reader = acquire_evidence_read_lease(&owner, "evidence-1").unwrap();
        assert!(evict_hot_evidence(
            &owner,
            &mut opened.journal,
            &mut state,
            &fixture.hot,
            &fixture.cold,
            &mut provider,
            "evidence-1",
            BASE + 4,
            HotEvictionFailpointV1::None,
        )
        .is_err());
        drop(reader);

        let result = evict_hot_evidence(
            &owner,
            &mut opened.journal,
            &mut state,
            &fixture.hot,
            &fixture.cold,
            &mut provider,
            "evidence-1",
            BASE + 5,
            HotEvictionFailpointV1::None,
        )
        .unwrap();
        assert!(!result.cleanup_required);
        assert!(!state.entries["evidence-1"].hot_present);
        assert!(!fixture
            .hot_root
            .path()
            .join(&state.entries["evidence-1"].hot_path.components[0])
            .join(&state.entries["evidence-1"].hot_path.components[1])
            .exists());
    }

    #[test]
    fn revoked_admission_consent_blocks_later_hot_eviction() {
        let fixture = Fixture::new();
        let mut provider = FakeProvider::new(&fixture.cold);
        provider.synchronization = FileProviderSynchronizationV1::Synchronized;
        let admission = admit(&fixture, &mut provider, "evidence-1", BASE + 1);
        let owner = fixture
            .scheduler
            .try_owner_admission("test/r3d3c-revoked-eviction-publish")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let mut state = opened.snapshot.state.clone();
        publish_admitted_cold_copy(
            &owner,
            &mut opened.journal,
            &mut state,
            &fixture.hot,
            &fixture.cold,
            &mut provider,
            &fixture.consent,
            &admission.copy_id,
            BASE + 2,
            ColdPublicationFailpointV1::None,
        )
        .unwrap();
        drop(opened);
        drop(owner);

        fixture.revoke_consent(BASE + 3);
        let combined = fixture
            .scheduler
            .try_owner_admission("test/r3d3c-revoked-eviction-owner")
            .unwrap()
            .try_authority_state("test/r3d3c-revoked-eviction-authority")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&combined).unwrap();
        let mut state = opened.snapshot.state.clone();
        assert!(evict_hot_evidence(
            &combined,
            &mut opened.journal,
            &mut state,
            &fixture.hot,
            &fixture.cold,
            &mut provider,
            "evidence-1",
            BASE + 4,
            HotEvictionFailpointV1::None,
        )
        .is_err());
        assert!(state.entries["evidence-1"].hot_present);
    }

    #[test]
    fn hot_eviction_cleanup_recovers_after_the_first_unlink() {
        let fixture = Fixture::new();
        let mut provider = FakeProvider::new(&fixture.cold);
        provider.synchronization = FileProviderSynchronizationV1::Synchronized;
        let admission = admit(&fixture, &mut provider, "evidence-1", BASE + 1);
        let combined = fixture
            .scheduler
            .try_owner_admission("test/r3d3c-hot-cleanup-owner")
            .unwrap()
            .try_authority_state("test/r3d3c-hot-cleanup-authority")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&combined).unwrap();
        let mut state = opened.snapshot.state.clone();
        publish_admitted_cold_copy(
            &combined,
            &mut opened.journal,
            &mut state,
            &fixture.hot,
            &fixture.cold,
            &mut provider,
            &fixture.consent,
            &admission.copy_id,
            BASE + 2,
            ColdPublicationFailpointV1::None,
        )
        .unwrap();
        let result = evict_hot_evidence(
            &combined,
            &mut opened.journal,
            &mut state,
            &fixture.hot,
            &fixture.cold,
            &mut provider,
            "evidence-1",
            BASE + 3,
            HotEvictionFailpointV1::AfterArchiveCleanup,
        )
        .unwrap();
        assert!(result.cleanup_required);
        assert!(!state.entries["evidence-1"].hot_present);
        let payload = fixture
            .hot_root
            .path()
            .join(&state.entries["evidence-1"].hot_path.components[0])
            .join(&state.entries["evidence-1"].hot_path.components[1]);
        assert!(!payload.join("evidence.tar.gz").exists());
        assert!(payload.join("manifest.json").exists());
        drop(opened);
        drop(combined);

        let owner = fixture
            .scheduler
            .try_owner_admission("test/r3d3c-hot-cleanup-recovery")
            .unwrap();
        let opened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let reader = acquire_evidence_read_lease(&owner, "evidence-1").unwrap();
        assert!(cleanup_evicted_hot_evidence(
            &owner,
            &opened.snapshot.state,
            &fixture.hot,
            "evidence-1",
        )
        .is_err());
        drop(reader);
        cleanup_evicted_hot_evidence(&owner, &opened.snapshot.state, &fixture.hot, "evidence-1")
            .unwrap();
        assert!(!payload.exists());
        cleanup_evicted_hot_evidence(&owner, &opened.snapshot.state, &fixture.hot, "evidence-1")
            .unwrap();
    }

    #[test]
    fn cold_publication_recovers_crash_between_final_renames() {
        let fixture = Fixture::new();
        let mut provider = FakeProvider::new(&fixture.cold);
        let admission = admit(&fixture, &mut provider, "evidence-1", BASE + 1);
        let owner = fixture
            .scheduler
            .try_owner_admission("test/r3d3c-between-final-renames")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let mut state = opened.snapshot.state.clone();
        assert!(publish_admitted_cold_copy(
            &owner,
            &mut opened.journal,
            &mut state,
            &fixture.hot,
            &fixture.cold,
            &mut provider,
            &fixture.consent,
            &admission.copy_id,
            BASE + 2,
            ColdPublicationFailpointV1::AfterArchivePublication,
        )
        .is_err());
        assert_eq!(
            inspect_cold_copy_residue(&fixture.cold, &admission).unwrap(),
            ColdResidueDispositionV1::ArchivePublishedManifestPartial
        );
        publish_admitted_cold_copy(
            &owner,
            &mut opened.journal,
            &mut state,
            &fixture.hot,
            &fixture.cold,
            &mut provider,
            &fixture.consent,
            &admission.copy_id,
            BASE + 3,
            ColdPublicationFailpointV1::None,
        )
        .unwrap();
        assert!(matches!(
            state.cold_copies[&admission.copy_id].lifecycle,
            ColdCopyLifecycleV1::Published { .. }
        ));
    }

    #[test]
    fn cold_entries_reject_offloaded_root_symlink_and_hard_link() {
        let fixture = Fixture::new();
        let mut provider = FakeProvider::new(&fixture.cold);
        provider.materialization = FileProviderMaterializationV1::Offloaded;
        let combined = fixture
            .scheduler
            .try_owner_admission("test/r3d3c-offloaded-root-owner")
            .unwrap()
            .try_authority_state("test/r3d3c-offloaded-root-authority")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&combined).unwrap();
        let mut state = opened.snapshot.state.clone();
        assert!(admit_cold_copy(
            &combined,
            &mut opened.journal,
            &mut state,
            &fixture.cold,
            &mut provider,
            &Fixture::request("evidence-1"),
            BASE + 1,
        )
        .is_err());
        drop(opened);
        drop(combined);

        provider.materialization = FileProviderMaterializationV1::Materialized;
        let admission = admit(&fixture, &mut provider, "evidence-1", BASE + 2);
        let archive_partial = fixture
            .cold_root
            .path()
            .join(partial_name(&admission.archive_path.components[0]));
        std::os::unix::fs::symlink("outside", &archive_partial).unwrap();
        assert_eq!(
            inspect_cold_copy_residue(&fixture.cold, &admission).unwrap(),
            ColdResidueDispositionV1::Ambiguous
        );
        std::fs::remove_file(&archive_partial).unwrap();

        let hard_link_source = fixture.cold_root.path().join("hard-link-source");
        write_private(&hard_link_source, b"untrusted");
        std::fs::hard_link(&hard_link_source, &archive_partial).unwrap();
        assert_eq!(
            inspect_cold_copy_residue(&fixture.cold, &admission).unwrap(),
            ColdResidueDispositionV1::Ambiguous
        );
        assert!(state.entries["evidence-1"].hot_present);
    }

    #[test]
    fn rotating_verification_rehydrates_and_corruption_blocks_all_hot_eviction() {
        let fixture = Fixture::new();
        let mut provider = FakeProvider::new(&fixture.cold);
        provider.synchronization = FileProviderSynchronizationV1::Synchronized;
        let first = admit(&fixture, &mut provider, "evidence-1", BASE + 1);
        let owner = fixture
            .scheduler
            .try_owner_admission("test/r3d3c-first-publish")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let mut state = opened.snapshot.state.clone();
        publish_admitted_cold_copy(
            &owner,
            &mut opened.journal,
            &mut state,
            &fixture.hot,
            &fixture.cold,
            &mut provider,
            &fixture.consent,
            &first.copy_id,
            BASE + 2,
            ColdPublicationFailpointV1::None,
        )
        .unwrap();
        drop(opened);
        drop(owner);

        let second = admit(&fixture, &mut provider, "evidence-2", BASE + 3);
        let owner = fixture
            .scheduler
            .try_owner_admission("test/r3d3c-second-publish")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let mut state = opened.snapshot.state.clone();
        publish_admitted_cold_copy(
            &owner,
            &mut opened.journal,
            &mut state,
            &fixture.hot,
            &fixture.cold,
            &mut provider,
            &fixture.consent,
            &second.copy_id,
            BASE + 4,
            ColdPublicationFailpointV1::None,
        )
        .unwrap();

        assert!(plan_rotating_cold_verifications(&state, 0).is_err());
        assert_eq!(
            plan_rotating_cold_verifications(&state, 1).unwrap(),
            vec!["evidence-1"]
        );
        provider.materialization = FileProviderMaterializationV1::Offloaded;
        let outcome = verify_cold_evidence(
            &owner,
            &mut opened.journal,
            &mut state,
            &fixture.cold,
            &mut provider,
            "evidence-1",
            BASE + 5,
        )
        .unwrap();
        assert!(matches!(
            outcome,
            ColdVerificationOutcomeV1::Verified { .. }
        ));
        assert_eq!(provider.materialize_calls, 2);
        assert_eq!(
            plan_rotating_cold_verifications(&state, 1).unwrap(),
            vec!["evidence-2"]
        );
        drop(opened);
        drop(owner);

        let archive_name = first.archive_path.components[0].clone();
        std::fs::write(fixture.cold_root.path().join(archive_name), b"corrupt").unwrap();
        let owner = fixture
            .scheduler
            .try_owner_admission("test/r3d3c-corruption")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let mut state = opened.snapshot.state.clone();
        let outcome = verify_cold_evidence(
            &owner,
            &mut opened.journal,
            &mut state,
            &fixture.cold,
            &mut provider,
            "evidence-1",
            BASE + 6,
        )
        .unwrap();
        assert!(matches!(
            outcome,
            ColdVerificationOutcomeV1::IntegrityBlocked {
                snapshot_sha256: Some(_),
                ..
            }
        ));
        assert_eq!(state.storage_integrity_holds.len(), 1);
        let repeated = verify_cold_evidence(
            &owner,
            &mut opened.journal,
            &mut state,
            &fixture.cold,
            &mut provider,
            "evidence-1",
            BASE + 7,
        )
        .unwrap();
        assert!(matches!(
            repeated,
            ColdVerificationOutcomeV1::IntegrityBlocked {
                snapshot_sha256: None,
                ..
            }
        ));
        drop(opened);
        drop(owner);

        let combined = fixture
            .scheduler
            .try_owner_admission("test/r3d3c-held-eviction")
            .unwrap()
            .try_authority_state("test/r3d3c-held-eviction-authority")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&combined).unwrap();
        let mut state = opened.snapshot.state.clone();
        assert!(evict_hot_evidence(
            &combined,
            &mut opened.journal,
            &mut state,
            &fixture.hot,
            &fixture.cold,
            &mut provider,
            "evidence-2",
            BASE + 8,
            HotEvictionFailpointV1::None,
        )
        .is_err());
        assert!(state.entries["evidence-2"].hot_present);
    }

    #[test]
    fn weekly_metadata_reconciliation_records_closed_states_and_cold_cap_edges() {
        assert_eq!(
            reserve_cold_capacity(COLD_CAP_BYTES - 1, 1).unwrap(),
            COLD_CAP_BYTES
        );
        assert!(reserve_cold_capacity(COLD_CAP_BYTES, 1).is_err());
        assert!(reserve_cold_capacity(u64::MAX, 1).is_err());

        let fixture = Fixture::new();
        let mut provider = FakeProvider::new(&fixture.cold);
        provider.synchronization = FileProviderSynchronizationV1::Synchronized;
        let admission = admit(&fixture, &mut provider, "evidence-1", BASE + 1);
        let owner = fixture
            .scheduler
            .try_owner_admission("test/r3d3c-reconcile")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let mut state = opened.snapshot.state.clone();
        publish_admitted_cold_copy(
            &owner,
            &mut opened.journal,
            &mut state,
            &fixture.hot,
            &fixture.cold,
            &mut provider,
            &fixture.consent,
            &admission.copy_id,
            BASE + 2,
            ColdPublicationFailpointV1::None,
        )
        .unwrap();
        provider.overrides.insert(
            admission.archive_path.components.join("/"),
            FileProviderObjectStateV1::Unknown {
                reason_code: "metadata_unavailable".into(),
            },
        );
        provider.overrides.insert(
            admission.manifest_path.components.join("/"),
            FileProviderObjectStateV1::Known {
                materialization: FileProviderMaterializationV1::Offloaded,
                synchronization: FileProviderSynchronizationV1::Synchronized,
            },
        );
        let reconciled = reconcile_cold_metadata(
            &owner,
            &mut opened.journal,
            &mut state,
            &fixture.cold,
            &mut provider,
            BASE + 3,
        )
        .unwrap();
        assert_eq!(reconciled.observed_objects, 2);
        assert_eq!(reconciled.blocked_evidence_ids, vec!["evidence-1"]);
        assert_eq!(state.storage_integrity_holds.len(), 1);
        let ColdCopyLifecycleV1::Published {
            archive_observation,
            manifest_observation,
            ..
        } = &state.cold_copies[&admission.copy_id].lifecycle
        else {
            panic!("copy must remain published")
        };
        assert!(matches!(
            archive_observation.state,
            FileProviderObjectStateV1::Unknown { .. }
        ));
        assert!(matches!(
            manifest_observation.state,
            FileProviderObjectStateV1::Known {
                materialization: FileProviderMaterializationV1::Offloaded,
                synchronization: FileProviderSynchronizationV1::Synchronized,
            }
        ));
    }

    #[test]
    fn expired_partial_can_be_durably_abandoned_cleaned_and_readmitted() {
        let fixture = Fixture::new();
        let mut provider = FakeProvider::new(&fixture.cold);
        let admission = admit(&fixture, &mut provider, "evidence-1", BASE + 1);
        let owner = fixture
            .scheduler
            .try_owner_admission("test/r3d3c-abandon-partial")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let mut state = opened.snapshot.state.clone();
        assert!(publish_admitted_cold_copy(
            &owner,
            &mut opened.journal,
            &mut state,
            &fixture.hot,
            &fixture.cold,
            &mut provider,
            &fixture.consent,
            &admission.copy_id,
            BASE + 2,
            ColdPublicationFailpointV1::AfterArchivePartial,
        )
        .is_err());
        let archive_partial = fixture
            .cold_root
            .path()
            .join(partial_name(&admission.archive_path.components[0]));
        let exact_archive = std::fs::read(
            fixture
                .hot_root
                .path()
                .join(&state.entries["evidence-1"].hot_path.components[0])
                .join(&state.entries["evidence-1"].hot_path.components[1])
                .join("evidence.tar.gz"),
        )
        .unwrap();
        let mut wrong_archive = exact_archive.clone();
        wrong_archive[0] ^= 1;
        write_private(&archive_partial, &wrong_archive);
        let abandoned = abandon_cold_copy(
            &owner,
            &mut opened.journal,
            &mut state,
            &fixture.cold,
            &admission.copy_id,
            "deadline_expired",
            BASE + 101,
        )
        .unwrap();
        assert!(abandoned.cleanup_required);
        assert!(matches!(
            state.cold_copies[&admission.copy_id].lifecycle,
            ColdCopyLifecycleV1::Abandoned { .. }
        ));
        write_private(&archive_partial, &exact_archive);
        assert_eq!(
            cleanup_abandoned_cold_copy(&owner, &state, &fixture.cold, &admission.copy_id).unwrap(),
            ColdResidueDispositionV1::ArchivePartialOnly
        );
        assert_eq!(
            cleanup_abandoned_cold_copy(&owner, &state, &fixture.cold, &admission.copy_id).unwrap(),
            ColdResidueDispositionV1::None
        );
        assert_eq!(
            inspect_cold_copy_residue(&fixture.cold, &admission).unwrap(),
            ColdResidueDispositionV1::None
        );
        drop(opened);
        drop(owner);

        let combined = fixture
            .scheduler
            .try_owner_admission("test/r3d3c-readmit-owner")
            .unwrap()
            .try_authority_state("test/r3d3c-readmit-authority")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&combined).unwrap();
        let mut state = opened.snapshot.state.clone();
        let mut request = Fixture::request("evidence-1");
        request.deadline_ms = BASE + 200;
        let replacement = admit_cold_copy(
            &combined,
            &mut opened.journal,
            &mut state,
            &fixture.cold,
            &mut provider,
            &request,
            BASE + 102,
        )
        .unwrap();
        assert_ne!(replacement.copy_id, admission.copy_id);
        assert_ne!(replacement.archive_path, admission.archive_path);
    }
}
