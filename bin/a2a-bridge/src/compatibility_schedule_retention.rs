//! Independently authorized cold evidence publication, verification, and hot-cache eviction.
//!
//! R3d5 remains the production root and FileProvider adapter owner. This module accepts only
//! injected retained roots and probes.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::io::Write as _;
use std::os::unix::fs::MetadataExt as _;

use serde::{Deserialize, Serialize};

use crate::compatibility_schedule_authority::{
    validate_sealed_storage_consent, validate_storage_consent, FileAuthorityJournal,
    StorageConsentRequestV1,
};
use crate::compatibility_schedule_evidence::{
    acquire_evidence_read_lease, try_acquire_evidence_gc_lease,
    try_acquire_evidence_gc_lease_optional, BundleGcActionV1, BundleGcLifecycleV1,
    ColdCopyLifecycleV1, ColdCopyRecordV1, EvidenceHotStoreV1, EvidenceStateModelV1,
    FileEvidenceJournal, FileProviderMaterializationV1, FileProviderObjectStateV1,
    FileProviderObservationV1, FileProviderSynchronizationV1, ImageGcActionV1, ImageGcLifecycleV1,
    IndexedEvidenceV1, StorageIntegrityHoldV1, TombstoneLifecycleV1, DAY_MS,
};
use crate::compatibility_schedule_schema::{
    portable_evidence_path_key, relative_evidence_path, ColdStorageBindingV1, EvidenceClassV1,
    OptionalRelativeEvidencePathV1, RelativeEvidencePathV1, StorageConsentV1,
};
use crate::compatibility_schedule_state::{AuthorityStateCapability, EvidenceStateCapability};
use crate::{local_file, BoxError};

#[cfg_attr(not(test), allow(dead_code))]
const COLD_ROOT_LITERAL: &str = "~/Documents/a2a-bridge/evidence-archive";
#[cfg_attr(not(test), allow(dead_code))]
const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
#[cfg_attr(not(test), allow(dead_code))]
const PRIVATE_FILE_MODE: u32 = 0o600;
#[cfg_attr(not(test), allow(dead_code))]
const COLD_CAP_BYTES: u64 = 25 * 1024 * 1024 * 1024;
#[cfg_attr(not(test), allow(dead_code))]
const MAX_COLD_ROOT_ENTRIES: usize = 2_048;

#[derive(Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct ColdEvidenceStoreV1 {
    root: local_file::PinnedDirectory,
}

impl ColdEvidenceStoreV1 {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn open_existing(root: &local_file::PinnedDirectory) -> Result<Self, BoxError> {
        validate_private_directory(root, "cold evidence root")?;
        Ok(Self { root: root.clone() })
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn root_sha256(&self) -> &str {
        self.root.object_sha256()
    }
}

#[cfg_attr(not(test), allow(dead_code))]
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

#[cfg_attr(not(test), allow(dead_code))]
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

#[cfg_attr(not(test), allow(dead_code))]
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

#[cfg_attr(not(test), allow(dead_code))]
fn reserve_cold_capacity(current_bytes: u64, reserved_bytes: u64) -> Result<u64, BoxError> {
    let total = current_bytes
        .checked_add(reserved_bytes)
        .ok_or("schedule retention: cold capacity arithmetic overflow")?;
    if total > COLD_CAP_BYTES {
        return Err("schedule retention: cold evidence cap would be exceeded".into());
    }
    Ok(total)
}

#[cfg_attr(not(test), allow(dead_code))]
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

#[cfg_attr(not(test), allow(dead_code))]
fn cold_copy_bytes(copy: &ColdCopyRecordV1) -> Result<u64, BoxError> {
    copy.archive_bytes
        .checked_add(copy.manifest_bytes)
        .ok_or_else(|| "schedule retention: cold-copy bytes overflow".into())
}

#[cfg_attr(not(test), allow(dead_code))]
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
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct FileProviderProbeRequestV1 {
    pub(super) cold_root_sha256: String,
    pub(super) file_provider_domain_id: String,
    pub(super) object_path: OptionalRelativeEvidencePathV1,
    pub(super) observed_at_ms: i64,
}

#[cfg_attr(not(test), allow(dead_code))]
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

#[cfg_attr(not(test), allow(dead_code))]
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

#[cfg_attr(not(test), allow(dead_code))]
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

#[cfg_attr(not(test), allow(dead_code))]
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
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct ColdCopyAdmissionRequestV1 {
    pub(super) evidence_id: String,
    pub(super) consent_id: String,
    pub(super) operator: String,
    pub(super) environment_owner: String,
    pub(super) deadline_ms: i64,
}

#[cfg_attr(not(test), allow(dead_code))]
fn cold_copy_id(evidence_id: &str, consent_sha256: &str, admitted_at_ms: i64) -> String {
    let material = format!("{evidence_id}\n{consent_sha256}\n{admitted_at_ms}\n");
    format!("cold-copy:{}", local_file::sha256_hex(material.as_bytes()))
}

#[cfg_attr(not(test), allow(dead_code))]
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

#[cfg_attr(not(test), allow(dead_code))]
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
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum ColdPublicationFailpointV1 {
    None,
    AfterArchivePartial,
    AfterManifestPartial,
    AfterArchivePublication,
    AfterFinalPublication,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
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
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct ColdPublicationResultV1 {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) snapshot_sha256: String,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) archive_path: RelativeEvidencePathV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
enum ChildDispositionV1 {
    Absent,
    PrivateRegular,
    Other,
}

#[cfg_attr(not(test), allow(dead_code))]
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

#[cfg_attr(not(test), allow(dead_code))]
fn removal_child_disposition(
    directory: &local_file::PinnedDirectory,
    name: &str,
) -> Result<ChildDispositionV1, BoxError> {
    let Some(candidate_name) = directory
        .regular_child_removal_candidate(OsStr::new(name), "cold evidence removal candidate")?
    else {
        return Ok(ChildDispositionV1::Absent);
    };
    let Some(metadata) =
        directory.child_metadata_no_follow(&candidate_name, "cold evidence removal candidate")?
    else {
        return Err("schedule retention: cold removal candidate disappeared".into());
    };
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

#[cfg_attr(not(test), allow(dead_code))]
fn single_component<'a>(
    path: &'a RelativeEvidencePathV1,
    label: &str,
) -> Result<&'a str, BoxError> {
    if path.components.len() != 1 {
        return Err(format!("schedule retention: {label} is not a single cold-root child").into());
    }
    Ok(&path.components[0])
}

#[cfg_attr(not(test), allow(dead_code))]
fn partial_name(final_name: &str) -> String {
    format!("{final_name}.partial")
}

#[cfg_attr(not(test), allow(dead_code))]
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
        removal_child_disposition(&cold.root, &partial_name(archive))?,
        removal_child_disposition(&cold.root, &partial_name(manifest))?,
        removal_child_disposition(&cold.root, archive)?,
        removal_child_disposition(&cold.root, manifest)?,
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
#[cfg_attr(not(test), allow(dead_code))]
struct PayloadBytesV1 {
    archive: Vec<u8>,
    manifest: Vec<u8>,
}

#[cfg_attr(not(test), allow(dead_code))]
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

#[cfg_attr(not(test), allow(dead_code))]
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

#[cfg_attr(not(test), allow(dead_code))]
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

#[cfg_attr(not(test), allow(dead_code))]
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

#[cfg_attr(not(test), allow(dead_code))]
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

#[cfg_attr(not(test), allow(dead_code))]
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

#[cfg_attr(not(test), allow(dead_code))]
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

// Each argument is a separately validated consent, storage, quota, or crash-recovery fence.
#[allow(clippy::too_many_arguments)]
#[cfg_attr(not(test), allow(dead_code))]
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

#[cfg_attr(not(test), allow(dead_code))]
fn cleanup_exact_partial(
    cold: &ColdEvidenceStoreV1,
    name: &str,
    expected_bytes: u64,
    expected_sha256: &str,
) -> Result<(), BoxError> {
    let Some(candidate_name) = cold
        .root
        .regular_child_removal_candidate(OsStr::new(name), "abandoned cold partial")?
    else {
        return Ok(());
    };
    let file = cold
        .root
        .open_regular_file(&candidate_name, "abandoned cold partial")?;
    validate_private_file(&file.metadata()?, "abandoned cold partial")?;
    let snapshot = local_file::read_open_regular_file_bounded(
        &file,
        "abandoned cold partial",
        expected_bytes,
    )?;
    if snapshot.bytes.len() as u64 != expected_bytes || snapshot.sha256 != expected_sha256 {
        return Err("schedule retention: abandoned cold partial length or hash mismatch".into());
    }
    cold.root.remove_regular_child_candidate(
        OsStr::new(name),
        local_file::RegularChildRef::new(&candidate_name, &file),
        "abandoned cold partial cleanup",
    )?;
    cold.root.sync()?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct ColdAbandonmentResultV1 {
    pub(super) snapshot_sha256: String,
    pub(super) residue: ColdResidueDispositionV1,
    pub(super) cleanup_required: bool,
}

#[cfg_attr(not(test), allow(dead_code))]
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

#[cfg_attr(not(test), allow(dead_code))]
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

#[cfg_attr(not(test), allow(dead_code))]
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

#[cfg_attr(not(test), allow(dead_code))]
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

#[cfg_attr(not(test), allow(dead_code))]
fn verified_optional_hot_file(
    payload: &local_file::PinnedDirectory,
    name: &str,
    expected_bytes: u64,
    expected_sha256: &str,
) -> Result<Option<(std::ffi::OsString, std::fs::File)>, BoxError> {
    let Some(candidate_name) = payload
        .regular_child_removal_candidate(OsStr::new(name), "hot evidence cleanup payload child")?
    else {
        return Ok(None);
    };
    let metadata = payload
        .child_metadata_no_follow(&candidate_name, "hot evidence cleanup payload child")?
        .ok_or("schedule retention: hot cleanup candidate disappeared")?;
    validate_private_child(metadata, "hot evidence cleanup payload child")?;
    let file = payload.open_regular_file(&candidate_name, "hot evidence cleanup payload child")?;
    validate_private_file(&file.metadata()?, "hot evidence cleanup payload child")?;
    let snapshot = local_file::read_open_regular_file_bounded(
        &file,
        "hot evidence cleanup payload child",
        expected_bytes,
    )?;
    if snapshot.bytes.len() as u64 != expected_bytes || snapshot.sha256 != expected_sha256 {
        return Err("schedule retention: hot cleanup payload length or hash mismatch".into());
    }
    Ok(Some((candidate_name, file)))
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct FullEvidenceUnlinkProofV1 {
    tombstone_id: String,
    evidence_id: String,
    unlinked_at_ms: i64,
}

impl FullEvidenceUnlinkProofV1 {
    pub(super) fn tombstone_id(&self) -> &str {
        &self.tombstone_id
    }

    pub(super) fn evidence_id(&self) -> &str {
        &self.evidence_id
    }

    pub(super) fn unlinked_at_ms(&self) -> i64 {
        self.unlinked_at_ms
    }

    #[cfg(test)]
    pub(super) fn for_test(tombstone_id: &str, evidence_id: &str, unlinked_at_ms: i64) -> Self {
        Self {
            tombstone_id: tombstone_id.into(),
            evidence_id: evidence_id.into(),
            unlinked_at_ms,
        }
    }
}

#[allow(clippy::too_many_arguments)]
#[cfg_attr(not(test), allow(dead_code))]
fn cleanup_exact_hot_payload(
    hot: &EvidenceHotStoreV1,
    evidence_id: &str,
    hot_path: &RelativeEvidencePathV1,
    archive_bytes: u64,
    archive_sha256: &str,
    manifest_bytes: u64,
    manifest_sha256: &str,
    failpoint: HotEvictionFailpointV1,
) -> Result<(), BoxError> {
    if hot_path.components.len() != 2
        || hot_path.components[0] != "sealed"
        || hot_path.components[1] != local_file::sha256_hex(evidence_id.as_bytes())
    {
        return Err("schedule retention: indexed hot cleanup path is invalid".into());
    }
    let object_name = &hot_path.components[1];
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
    let archive_quarantine = local_file::removal_quarantine_name(
        OsStr::new("evidence.tar.gz"),
        "hot evidence archive eviction",
    )?
    .into_string()
    .map_err(|_| "schedule retention: hot archive quarantine name is not UTF-8")?;
    let manifest_quarantine = local_file::removal_quarantine_name(
        OsStr::new("manifest.json"),
        "hot evidence manifest eviction",
    )?
    .into_string()
    .map_err(|_| "schedule retention: hot manifest quarantine name is not UTF-8")?;
    if names.iter().any(|name| {
        name != "evidence.tar.gz"
            && name != "manifest.json"
            && name != &archive_quarantine
            && name != &manifest_quarantine
    }) {
        return Err("schedule retention: hot cleanup payload has an unexpected child".into());
    }
    let archive =
        verified_optional_hot_file(&payload, "evidence.tar.gz", archive_bytes, archive_sha256)?;
    let manifest =
        verified_optional_hot_file(&payload, "manifest.json", manifest_bytes, manifest_sha256)?;
    if let Some((candidate_name, file)) = archive {
        payload.remove_regular_child_candidate(
            OsStr::new("evidence.tar.gz"),
            local_file::RegularChildRef::new(&candidate_name, &file),
            "hot evidence archive eviction",
        )?;
        payload.sync()?;
        if failpoint == HotEvictionFailpointV1::AfterArchiveCleanup {
            return Err("schedule retention: injected crash after hot archive cleanup".into());
        }
    }
    if let Some((candidate_name, file)) = manifest {
        payload.remove_regular_child_candidate(
            OsStr::new("manifest.json"),
            local_file::RegularChildRef::new(&candidate_name, &file),
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

#[cfg_attr(not(test), allow(dead_code))]
fn cleanup_hot_payload(
    hot: &EvidenceHotStoreV1,
    entry: &IndexedEvidenceV1,
    failpoint: HotEvictionFailpointV1,
) -> Result<(), BoxError> {
    cleanup_exact_hot_payload(
        hot,
        &entry.evidence_id,
        &entry.hot_path,
        entry.archive_bytes,
        &entry.full_evidence_sha256,
        entry.manifest_bytes,
        &entry.manifest_sha256,
        failpoint,
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum HotEvictionFailpointV1 {
    None,
    AfterIndexPublication,
    AfterArchiveCleanup,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct HotEvictionResultV1 {
    pub(super) snapshot_sha256: String,
    pub(super) cleanup_required: bool,
}

// Eviction keeps its action-time consent, inventory, lease, and journal fences explicit.
#[allow(clippy::too_many_arguments)]
#[cfg_attr(not(test), allow(dead_code))]
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

#[cfg_attr(not(test), allow(dead_code))]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum EvidenceTombstoneFailpointV1 {
    None,
    AfterPendingIntent,
    AfterFirstUnlink,
    AfterAllUnlinks,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum EvidenceTombstoneOutcomeV1 {
    Completed { snapshot_sha256: String },
    DeferredLeaseBusy,
    AlreadyComplete,
}

#[cfg_attr(not(test), allow(dead_code))]
fn materialize_for_deletion(
    cold: &ColdEvidenceStoreV1,
    probe: &mut dyn FileProviderStateProbeV1,
    copy: &ColdCopyRecordV1,
    path: &RelativeEvidencePathV1,
    observed_at_ms: i64,
) -> Result<(), BoxError> {
    let request = probe_request(
        Some(copy),
        cold,
        &copy.file_provider_domain_id,
        OptionalRelativeEvidencePathV1::RelativePath {
            value: path.clone(),
        },
        observed_at_ms,
    )?;
    let observed = probe.probe(&request)?;
    validate_observation(&request, &observed)?;
    match observed.state {
        FileProviderObjectStateV1::Known {
            materialization: FileProviderMaterializationV1::Materialized,
            ..
        } => Ok(()),
        FileProviderObjectStateV1::Known {
            materialization: FileProviderMaterializationV1::Offloaded,
            ..
        } => {
            let materialized = probe.materialize(&request)?;
            validate_observation(&request, &materialized)?;
            if !matches!(
                materialized.state,
                FileProviderObjectStateV1::Known {
                    materialization: FileProviderMaterializationV1::Materialized,
                    ..
                }
            ) {
                return Err(
                    "schedule retention: cold deletion materialization did not complete".into(),
                );
            }
            Ok(())
        }
        FileProviderObjectStateV1::Unavailable { .. }
        | FileProviderObjectStateV1::Unknown { .. } => {
            Err("schedule retention: cold deletion provider state is blocking".into())
        }
    }
}

#[allow(clippy::too_many_arguments)]
#[cfg_attr(not(test), allow(dead_code))]
fn open_optional_cold_deletion_file(
    cold: &ColdEvidenceStoreV1,
    provider_probe: &mut Option<&mut dyn FileProviderStateProbeV1>,
    copy: &ColdCopyRecordV1,
    path: &RelativeEvidencePathV1,
    expected_bytes: u64,
    expected_sha256: &str,
    observed_at_ms: i64,
    label: &str,
) -> Result<Option<(std::ffi::OsString, std::fs::File)>, BoxError> {
    let name = single_component(path, label)?;
    let Some(candidate_name) = cold
        .root
        .regular_child_removal_candidate(OsStr::new(name), label)?
    else {
        return Ok(None);
    };
    let candidate_component = candidate_name
        .to_str()
        .ok_or_else(|| format!("schedule retention: {label} candidate is not UTF-8"))?;
    match child_disposition(&cold.root, candidate_component)? {
        ChildDispositionV1::Absent => return Ok(None),
        ChildDispositionV1::Other => {
            return Err(format!("schedule retention: {label} has unsafe metadata").into())
        }
        ChildDispositionV1::PrivateRegular => {}
    }
    let probe = provider_probe
        .as_deref_mut()
        .ok_or("schedule retention: cold deletion probe is unavailable")?;
    let candidate_path = RelativeEvidencePathV1 {
        components: vec![candidate_component.into()],
    };
    materialize_for_deletion(cold, probe, copy, &candidate_path, observed_at_ms)?;
    let file = cold.root.open_regular_file(&candidate_name, label)?;
    validate_private_file(&file.metadata()?, label)?;
    let snapshot = local_file::read_open_regular_file_bounded(&file, label, expected_bytes)?;
    if snapshot.bytes.len() as u64 != expected_bytes || snapshot.sha256 != expected_sha256 {
        return Err(format!("schedule retention: {label} length or hash mismatch").into());
    }
    Ok(Some((candidate_name, file)))
}

#[cfg_attr(not(test), allow(dead_code))]
fn cleanup_exact_cold_payload(
    cold: &ColdEvidenceStoreV1,
    mut provider_probe: Option<&mut dyn FileProviderStateProbeV1>,
    copy: &ColdCopyRecordV1,
    observed_at_ms: i64,
    fail_after_first_unlink: bool,
) -> Result<(), BoxError> {
    validate_private_directory(&cold.root, "cold evidence deletion root")?;
    if copy.cold_root_sha256 != cold.root_sha256() {
        return Err("schedule retention: cold deletion root binding changed".into());
    }
    let archive_name = single_component(&copy.archive_path, "cold archive deletion path")?;
    let manifest_name = single_component(&copy.manifest_path, "cold manifest deletion path")?;
    let archive_partial_path = RelativeEvidencePathV1 {
        components: vec![partial_name(archive_name)],
    };
    let manifest_partial_path = RelativeEvidencePathV1 {
        components: vec![partial_name(manifest_name)],
    };
    let files = [
        (
            copy.archive_path.clone(),
            copy.archive_bytes,
            copy.archive_sha256.as_str(),
            "cold evidence archive deletion",
        ),
        (
            copy.manifest_path.clone(),
            copy.manifest_bytes,
            copy.manifest_sha256.as_str(),
            "cold evidence manifest deletion",
        ),
        (
            archive_partial_path,
            copy.archive_bytes,
            copy.archive_sha256.as_str(),
            "cold evidence archive partial deletion",
        ),
        (
            manifest_partial_path,
            copy.manifest_bytes,
            copy.manifest_sha256.as_str(),
            "cold evidence manifest partial deletion",
        ),
    ];
    let mut opened = Vec::with_capacity(files.len());
    for (path, expected_bytes, expected_sha256, label) in &files {
        let file = open_optional_cold_deletion_file(
            cold,
            &mut provider_probe,
            copy,
            path,
            *expected_bytes,
            expected_sha256,
            observed_at_ms,
            label,
        )?;
        opened.push((path.clone(), file, *label));
    }
    let mut unlinked_any = false;
    for (path, candidate, label) in opened {
        let Some((candidate_name, file)) = candidate else {
            continue;
        };
        let name = single_component(&path, label)?;
        cold.root.remove_regular_child_candidate(
            OsStr::new(name),
            local_file::RegularChildRef::new(&candidate_name, &file),
            label,
        )?;
        cold.root.sync()?;
        if !unlinked_any && fail_after_first_unlink {
            return Err("schedule retention: injected crash after first cold unlink".into());
        }
        unlinked_any = true;
    }
    for (path, _, _, label) in &files {
        let name = single_component(path, label)?;
        if cold
            .root
            .regular_child_removal_candidate(OsStr::new(name), label)?
            .is_some()
        {
            return Err("schedule retention: cold evidence remained after deletion".into());
        }
    }
    cold.root.sync()?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn execute_evidence_tombstone<C: EvidenceStateCapability + ?Sized>(
    capability: &C,
    journal: &mut FileEvidenceJournal<'_>,
    state: &mut EvidenceStateModelV1,
    hot: &EvidenceHotStoreV1,
    cold: Option<&ColdEvidenceStoreV1>,
    mut provider_probe: Option<&mut dyn FileProviderStateProbeV1>,
    tombstone_id: &str,
    evidence_id: &str,
    reason_code: &str,
    started_at_ms: i64,
    completed_at_ms: i64,
    failpoint: EvidenceTombstoneFailpointV1,
) -> Result<EvidenceTombstoneOutcomeV1, BoxError> {
    state.validate()?;
    validate_private_directory(hot.root_directory(), "hot evidence deletion root")?;
    if state.hot_root_sha256 != hot.root_sha256() || completed_at_ms <= started_at_ms {
        return Err("schedule retention: tombstone root binding or time is invalid".into());
    }
    let existing = state.tombstones.get(tombstone_id);
    if let Some(existing) = existing {
        if existing.evidence_id != evidence_id
            || existing.reason_code != reason_code
            || started_at_ms < existing.created_at_ms
        {
            return Err("schedule retention: tombstone recovery identity changed".into());
        }
    } else {
        if state
            .storage_integrity_holds
            .values()
            .any(|hold| hold.evidence_id == evidence_id)
        {
            return Err("schedule retention: integrity hold blocks evidence deletion".into());
        }
        let mut candidate = state.clone();
        candidate.begin_tombstone(tombstone_id, evidence_id, reason_code, started_at_ms)?;
        journal.append(&candidate, started_at_ms)?;
        *state = candidate;
    }
    if failpoint == EvidenceTombstoneFailpointV1::AfterPendingIntent {
        return Err("schedule retention: injected crash after tombstone intent".into());
    }

    let Some(_lease) = try_acquire_evidence_gc_lease_optional(capability, evidence_id)? else {
        return Ok(EvidenceTombstoneOutcomeV1::DeferredLeaseBusy);
    };
    let reopened = FileEvidenceJournal::open_existing(capability)?;
    if reopened.snapshot.state != *state {
        return Err("schedule retention: tombstone state changed before deletion".into());
    }
    drop(reopened);
    let tombstone = state
        .tombstones
        .get(tombstone_id)
        .ok_or("schedule retention: persisted tombstone disappeared")?
        .clone();
    if tombstone.lifecycle == TombstoneLifecycleV1::Pending
        && state
            .storage_integrity_holds
            .values()
            .any(|hold| hold.evidence_id == evidence_id)
    {
        return Err("schedule retention: integrity hold blocks evidence deletion".into());
    }
    if tombstone.lifecycle == TombstoneLifecycleV1::Pending && state.has_active_pin(evidence_id) {
        return Err("schedule retention: active pin blocks evidence deletion".into());
    }

    cleanup_exact_hot_payload(
        hot,
        &tombstone.evidence_id,
        &tombstone.hot_path,
        tombstone.archive_bytes,
        &tombstone.full_evidence_sha256,
        tombstone.manifest_bytes,
        &tombstone.manifest_sha256,
        if failpoint == EvidenceTombstoneFailpointV1::AfterFirstUnlink && tombstone.hot_was_present
        {
            HotEvictionFailpointV1::AfterArchiveCleanup
        } else {
            HotEvictionFailpointV1::None
        },
    )?;

    if let OptionalRelativeEvidencePathV1::RelativePath { value } = &tombstone.cold_path {
        let copy = state
            .cold_copies
            .values()
            .find(|copy| {
                copy.evidence_id == evidence_id
                    && matches!(copy.lifecycle, ColdCopyLifecycleV1::Published { .. })
            })
            .ok_or("schedule retention: tombstoned cold copy disappeared")?
            .clone();
        if &copy.archive_path != value {
            return Err("schedule retention: tombstoned cold path identity changed".into());
        }
        let cold = cold.ok_or("schedule retention: cold deletion root is unavailable")?;
        cleanup_exact_cold_payload(
            cold,
            provider_probe.take(),
            &copy,
            completed_at_ms,
            failpoint == EvidenceTombstoneFailpointV1::AfterFirstUnlink
                && !tombstone.hot_was_present,
        )?;
    }
    if failpoint == EvidenceTombstoneFailpointV1::AfterAllUnlinks {
        return Err("schedule retention: injected crash after all evidence unlinks".into());
    }

    if matches!(
        tombstone.lifecycle,
        TombstoneLifecycleV1::FullEvidenceUnlinked { .. }
    ) {
        return Ok(EvidenceTombstoneOutcomeV1::AlreadyComplete);
    }
    let proof = FullEvidenceUnlinkProofV1 {
        tombstone_id: tombstone_id.into(),
        evidence_id: evidence_id.into(),
        unlinked_at_ms: completed_at_ms,
    };
    let mut candidate = state.clone();
    candidate.complete_tombstone(proof)?;
    let (_snapshot, snapshot_sha256) = journal.append(&candidate, completed_at_ms)?;
    *state = candidate;
    Ok(EvidenceTombstoneOutcomeV1::Completed { snapshot_sha256 })
}

#[cfg_attr(not(test), allow(dead_code))]
const MAX_ROTATING_VERIFICATION_BATCH: usize = 64;

#[cfg_attr(not(test), allow(dead_code))]
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

#[cfg_attr(not(test), allow(dead_code))]
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

#[cfg_attr(not(test), allow(dead_code))]
fn integrity_hold_id(evidence_id: &str) -> String {
    format!(
        "cold-integrity:{}",
        local_file::sha256_hex(evidence_id.as_bytes())
    )
}

#[cfg_attr(not(test), allow(dead_code))]
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
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum ColdVerificationOutcomeV1 {
    Verified {
        snapshot_sha256: String,
    },
    IntegrityBlocked {
        hold_id: String,
        snapshot_sha256: Option<String>,
    },
}

#[cfg_attr(not(test), allow(dead_code))]
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

#[cfg_attr(not(test), allow(dead_code))]
fn observation_is_integrity_blocking(observation: &FileProviderObservationV1) -> bool {
    matches!(
        observation.state,
        FileProviderObjectStateV1::Unavailable { .. } | FileProviderObjectStateV1::Unknown { .. }
    )
}

#[cfg_attr(not(test), allow(dead_code))]
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
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct ColdMetadataReconciliationV1 {
    pub(super) observed_objects: usize,
    pub(super) blocked_evidence_ids: Vec<String>,
    pub(super) snapshot_sha256: String,
}

#[cfg_attr(not(test), allow(dead_code))]
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

#[cfg_attr(not(test), allow(dead_code))]
const MAX_CACHE_INVENTORY_ITEMS: usize = 1_024;

#[cfg_attr(not(test), allow(dead_code))]
fn cache_stable_id(label: &str, value: &str) -> Result<(), BoxError> {
    if value.is_empty()
        || value.len() > 128
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'-' | b'_' | b':' | b'/' | b'.')
        })
    {
        return Err(format!("schedule retention: {label} is not a bounded stable id").into());
    }
    Ok(())
}

#[cfg_attr(not(test), allow(dead_code))]
fn lowercase_sha256(label: &str, value: &str) -> Result<(), BoxError> {
    if !local_file::valid_sha256(value)
        || value.len() != 64
        || value.bytes().any(|byte| byte.is_ascii_uppercase())
    {
        return Err(format!("schedule retention: {label} is not lowercase SHA-256").into());
    }
    Ok(())
}

#[cfg_attr(not(test), allow(dead_code))]
fn add_retention_days(timestamp_ms: i64, days: u32) -> Result<i64, BoxError> {
    if timestamp_ms <= 0 {
        return Err("schedule retention: cache timestamp must be positive".into());
    }
    timestamp_ms
        .checked_add(
            i64::from(days)
                .checked_mul(DAY_MS)
                .ok_or("schedule retention: cache duration overflow")?,
        )
        .ok_or_else(|| "schedule retention: cache deadline overflow".into())
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum BundleCacheKindV1 {
    ReconstructiblePayload,
    ManifestOrInventory,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct BundleCacheEntryV1 {
    pub(super) bundle_id: String,
    pub(super) evidence_id: String,
    pub(super) provider_id: String,
    pub(super) case_id: String,
    pub(super) evidence_class: EvidenceClassV1,
    pub(super) kind: BundleCacheKindV1,
    pub(super) created_at_ms: i64,
    pub(super) path: RelativeEvidencePathV1,
    pub(super) content_sha256: String,
    pub(super) length_bytes: u64,
    pub(super) preserved_in_full_evidence_sha256: String,
}

impl BundleCacheEntryV1 {
    #[cfg_attr(not(test), allow(dead_code))]
    fn validate(&self) -> Result<(), BoxError> {
        cache_stable_id("bundle id", &self.bundle_id)?;
        cache_stable_id("bundle evidence id", &self.evidence_id)?;
        cache_stable_id("bundle provider id", &self.provider_id)?;
        cache_stable_id("bundle case id", &self.case_id)?;
        relative_evidence_path("bundle cache path", &self.path)?;
        lowercase_sha256("bundle content", &self.content_sha256)?;
        lowercase_sha256(
            "bundle preserved full evidence",
            &self.preserved_in_full_evidence_sha256,
        )?;
        if self.created_at_ms <= 0 || self.length_bytes == 0 || self.path.components.len() != 1 {
            return Err("schedule retention: bundle cache entry identity is invalid".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct BundleCacheInventoryV1 {
    pub(super) cache_root_sha256: String,
    pub(super) observed_at_ms: i64,
    pub(super) entries: Vec<BundleCacheEntryV1>,
    pub(super) referenced_bundle_ids: BTreeSet<String>,
}

impl BundleCacheInventoryV1 {
    #[cfg_attr(not(test), allow(dead_code))]
    fn normalized(&self) -> Result<Self, BoxError> {
        lowercase_sha256("bundle cache root", &self.cache_root_sha256)?;
        if self.observed_at_ms <= 0
            || self.entries.len() > MAX_CACHE_INVENTORY_ITEMS
            || self.referenced_bundle_ids.len() > MAX_CACHE_INVENTORY_ITEMS
        {
            return Err("schedule retention: bundle inventory exceeds its bound".into());
        }
        let mut value = self.clone();
        value.entries.sort_by(|left, right| {
            left.bundle_id
                .cmp(&right.bundle_id)
                .then_with(|| left.path.components.cmp(&right.path.components))
        });
        let mut ids = BTreeSet::new();
        let mut paths = BTreeSet::new();
        for entry in &value.entries {
            entry.validate()?;
            if !ids.insert(entry.bundle_id.clone())
                || !paths.insert(portable_evidence_path_key(&entry.path))
            {
                return Err("schedule retention: bundle inventory identity is duplicated".into());
            }
        }
        for reference in &value.referenced_bundle_ids {
            cache_stable_id("referenced bundle id", reference)?;
            if !ids.contains(reference) {
                return Err("schedule retention: bundle reference targets an absent entry".into());
            }
        }
        Ok(value)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn sha256(&self) -> Result<String, BoxError> {
        Ok(local_file::sha256_hex(&serde_json::to_vec(
            &self.normalized()?,
        )?))
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) trait BundleCacheInventoryProbeV1 {
    fn inventory_all(&mut self) -> Result<BundleCacheInventoryV1, BoxError>;
}

impl<F> BundleCacheInventoryProbeV1 for F
where
    F: FnMut() -> Result<BundleCacheInventoryV1, BoxError>,
{
    fn inventory_all(&mut self) -> Result<BundleCacheInventoryV1, BoxError> {
        self()
    }
}

#[derive(Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct BundleCacheStoreV1 {
    root: local_file::PinnedDirectory,
}

impl BundleCacheStoreV1 {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn open_existing(root: &local_file::PinnedDirectory) -> Result<Self, BoxError> {
        validate_private_directory(root, "bundle cache root")?;
        Ok(Self { root: root.clone() })
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn root_sha256(&self) -> &str {
        self.root.object_sha256()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum BundleGcProtectionV1 {
    MinimumAge,
    KeepLatestThree,
    ActivePin,
    ActiveReference,
    FullEvidenceNotPreserved,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct ProtectedBundleV1 {
    pub(super) bundle_id: String,
    pub(super) reason: BundleGcProtectionV1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct BundleGcPlanItemV1 {
    pub(super) action_id: String,
    pub(super) cache_root_sha256: String,
    pub(super) inventory_sha256: String,
    pub(super) planned_at_ms: i64,
    pub(super) reason_code: String,
    pub(super) entry: BundleCacheEntryV1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct BundleGcPlanV1 {
    pub(super) planned_at_ms: i64,
    pub(super) inventory_sha256: String,
    pub(super) removals: Vec<BundleGcPlanItemV1>,
    pub(super) protected: Vec<ProtectedBundleV1>,
}

#[cfg_attr(not(test), allow(dead_code))]
fn preserved_full_evidence_matches(
    state: &EvidenceStateModelV1,
    entry: &BundleCacheEntryV1,
) -> bool {
    state
        .entries
        .get(&entry.evidence_id)
        .is_some_and(|evidence| {
            evidence.evidence_class == entry.evidence_class
                && evidence.full_evidence_sha256 == entry.preserved_in_full_evidence_sha256
        })
        || state.tombstones.values().any(|tombstone| {
            tombstone.evidence_id == entry.evidence_id
                && tombstone.evidence_class == entry.evidence_class
                && tombstone.full_evidence_sha256 == entry.preserved_in_full_evidence_sha256
                && matches!(
                tombstone.lifecycle,
                crate::compatibility_schedule_evidence::TombstoneLifecycleV1::FullEvidenceUnlinked {
                    ..
                }
            )
        })
}

#[cfg_attr(not(test), allow(dead_code))]
fn bundle_deadline(entry: &BundleCacheEntryV1) -> Result<i64, BoxError> {
    let days = match entry.kind {
        BundleCacheKindV1::ManifestOrInventory => 180,
        BundleCacheKindV1::ReconstructiblePayload => match entry.evidence_class {
            EvidenceClassV1::RoutineGreen => 14,
            EvidenceClassV1::FailedOrUnknown => 90,
            EvidenceClassV1::PreflightBlocked | EvidenceClassV1::ManualCompatibility => 30,
            EvidenceClassV1::Incident
            | EvidenceClassV1::PromotionRelease
            | EvidenceClassV1::AuthorizationBudgetAudit => 0,
        },
    };
    add_retention_days(entry.created_at_ms, days)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn plan_bundle_gc(
    state: &EvidenceStateModelV1,
    inventory: &BundleCacheInventoryV1,
    planned_at_ms: i64,
) -> Result<BundleGcPlanV1, BoxError> {
    state.validate()?;
    if planned_at_ms <= 0 {
        return Err("schedule retention: bundle GC plan time must be positive".into());
    }
    let inventory = inventory.normalized()?;
    if inventory.observed_at_ms > planned_at_ms {
        return Err("schedule retention: bundle inventory postdates the GC plan".into());
    }
    let inventory_sha256 = inventory.sha256()?;
    let mut routine_groups = BTreeMap::<(String, String), Vec<&BundleCacheEntryV1>>::new();
    for entry in &inventory.entries {
        if entry.created_at_ms > planned_at_ms {
            return Err("schedule retention: bundle entry postdates the plan".into());
        }
        if entry.kind == BundleCacheKindV1::ReconstructiblePayload
            && entry.evidence_class == EvidenceClassV1::RoutineGreen
        {
            routine_groups
                .entry((entry.provider_id.clone(), entry.case_id.clone()))
                .or_default()
                .push(entry);
        }
    }
    let mut routine_rank = BTreeMap::new();
    for entries in routine_groups.values_mut() {
        entries.sort_by(|left, right| {
            right
                .created_at_ms
                .cmp(&left.created_at_ms)
                .then_with(|| right.bundle_id.cmp(&left.bundle_id))
        });
        for (rank, entry) in entries.iter().enumerate() {
            routine_rank.insert(entry.bundle_id.clone(), rank);
        }
    }

    let mut removals = Vec::new();
    let mut protected = Vec::new();
    for entry in inventory.entries {
        let protection = if !preserved_full_evidence_matches(state, &entry) {
            Some(BundleGcProtectionV1::FullEvidenceNotPreserved)
        } else if state.has_active_pin(&entry.evidence_id) {
            Some(BundleGcProtectionV1::ActivePin)
        } else if inventory.referenced_bundle_ids.contains(&entry.bundle_id) {
            Some(BundleGcProtectionV1::ActiveReference)
        } else if planned_at_ms < bundle_deadline(&entry)? {
            Some(BundleGcProtectionV1::MinimumAge)
        } else if entry.kind == BundleCacheKindV1::ReconstructiblePayload
            && entry.evidence_class == EvidenceClassV1::RoutineGreen
            && routine_rank
                .get(&entry.bundle_id)
                .is_some_and(|rank| *rank < 3)
            && planned_at_ms < add_retention_days(entry.created_at_ms, 30)?
        {
            Some(BundleGcProtectionV1::KeepLatestThree)
        } else {
            None
        };
        if let Some(reason) = protection {
            protected.push(ProtectedBundleV1 {
                bundle_id: entry.bundle_id,
                reason,
            });
            continue;
        }
        let reason_code = match entry.kind {
            BundleCacheKindV1::ManifestOrInventory => "manifest_inventory_retention_elapsed",
            BundleCacheKindV1::ReconstructiblePayload
                if entry.evidence_class == EvidenceClassV1::RoutineGreen
                    && routine_rank
                        .get(&entry.bundle_id)
                        .is_some_and(|rank| *rank >= 3) =>
            {
                "routine_beyond_keep_three"
            }
            BundleCacheKindV1::ReconstructiblePayload => "bundle_cache_retention_elapsed",
        };
        let action_id = format!(
            "bundle-gc:{}",
            local_file::sha256_hex(
                format!(
                    "{}\0{}\0{}",
                    inventory_sha256, entry.bundle_id, planned_at_ms
                )
                .as_bytes()
            )
        );
        removals.push(BundleGcPlanItemV1 {
            action_id,
            cache_root_sha256: inventory.cache_root_sha256.clone(),
            inventory_sha256: inventory_sha256.clone(),
            planned_at_ms,
            reason_code: reason_code.into(),
            entry,
        });
    }
    removals.sort_by(|left, right| left.entry.bundle_id.cmp(&right.entry.bundle_id));
    protected.sort_by(|left, right| left.bundle_id.cmp(&right.bundle_id));
    Ok(BundleGcPlanV1 {
        planned_at_ms,
        inventory_sha256,
        removals,
        protected,
    })
}

#[cfg_attr(not(test), allow(dead_code))]
fn bundle_lease_id(bundle_id: &str) -> Result<String, BoxError> {
    cache_stable_id("bundle lease id", bundle_id)?;
    Ok(format!("bundle:{bundle_id}"))
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn acquire_bundle_read_lease<C: EvidenceStateCapability + ?Sized>(
    capability: &C,
    bundle_id: &str,
) -> Result<std::fs::File, BoxError> {
    acquire_evidence_read_lease(capability, &bundle_lease_id(bundle_id)?)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum BundleGcFailpointV1 {
    None,
    AfterUnlink,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum BundleGcOutcomeV1 {
    Removed,
    RecoveredAlreadyAbsent,
    DeferredLeaseBusy,
    SafeSkipped { reason_code: String },
    AlreadyTerminal,
}

#[cfg_attr(not(test), allow(dead_code))]
fn bundle_action(item: &BundleGcPlanItemV1, started_at_ms: i64) -> BundleGcActionV1 {
    BundleGcActionV1 {
        action_id: item.action_id.clone(),
        bundle_id: item.entry.bundle_id.clone(),
        evidence_id: item.entry.evidence_id.clone(),
        provider_id: item.entry.provider_id.clone(),
        case_id: item.entry.case_id.clone(),
        evidence_class: item.entry.evidence_class,
        cache_root_sha256: item.cache_root_sha256.clone(),
        path: item.entry.path.clone(),
        content_sha256: item.entry.content_sha256.clone(),
        length_bytes: item.entry.length_bytes,
        preserved_in_full_evidence_sha256: item.entry.preserved_in_full_evidence_sha256.clone(),
        reason_code: item.reason_code.clone(),
        planned_at_ms: item.planned_at_ms,
        started_at_ms,
        lifecycle: BundleGcLifecycleV1::Pending,
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn bundle_action_matches_item(action: &BundleGcActionV1, item: &BundleGcPlanItemV1) -> bool {
    action.action_id == item.action_id
        && action.bundle_id == item.entry.bundle_id
        && action.evidence_id == item.entry.evidence_id
        && action.provider_id == item.entry.provider_id
        && action.case_id == item.entry.case_id
        && action.evidence_class == item.entry.evidence_class
        && action.cache_root_sha256 == item.cache_root_sha256
        && action.path == item.entry.path
        && action.content_sha256 == item.entry.content_sha256
        && action.length_bytes == item.entry.length_bytes
        && action.preserved_in_full_evidence_sha256 == item.entry.preserved_in_full_evidence_sha256
        && action.reason_code == item.reason_code
        && action.planned_at_ms == item.planned_at_ms
}

#[cfg_attr(not(test), allow(dead_code))]
fn finish_bundle_safe_skip(
    journal: &mut FileEvidenceJournal<'_>,
    state: &mut EvidenceStateModelV1,
    action_id: &str,
    reason_code: &str,
    completed_at_ms: i64,
) -> Result<BundleGcOutcomeV1, BoxError> {
    cache_stable_id("bundle GC safe-skip reason", reason_code)?;
    let mut candidate = state.clone();
    candidate.safe_skip_bundle_gc(action_id, reason_code, completed_at_ms)?;
    journal.append(&candidate, completed_at_ms)?;
    *state = candidate;
    Ok(BundleGcOutcomeV1::SafeSkipped {
        reason_code: reason_code.into(),
    })
}

// GC effect identity is intentionally explicit at this narrow effect boundary.
#[allow(clippy::too_many_arguments)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn execute_bundle_gc_item<
    C: EvidenceStateCapability + ?Sized,
    P: BundleCacheInventoryProbeV1 + ?Sized,
>(
    capability: &C,
    journal: &mut FileEvidenceJournal<'_>,
    state: &mut EvidenceStateModelV1,
    store: &BundleCacheStoreV1,
    planned_inventory: &BundleCacheInventoryV1,
    inventory_probe: &mut P,
    item: &BundleGcPlanItemV1,
    started_at_ms: i64,
    completed_at_ms: i64,
    failpoint: BundleGcFailpointV1,
) -> Result<BundleGcOutcomeV1, BoxError> {
    state.validate()?;
    validate_private_directory(&store.root, "bundle cache root")?;
    if item.cache_root_sha256 != store.root_sha256() {
        return Err("schedule retention: bundle GC inventory/root binding changed".into());
    }
    if completed_at_ms <= started_at_ms {
        return Err("schedule retention: bundle GC completion time is invalid".into());
    }
    let existing_action = state.bundle_gc_actions.get(&item.action_id);
    match existing_action {
        Some(action) if !bundle_action_matches_item(action, item) => {
            return Err("schedule retention: bundle GC action identity conflicts".into())
        }
        Some(action) if action.lifecycle != BundleGcLifecycleV1::Pending => {
            return Ok(BundleGcOutcomeV1::AlreadyTerminal)
        }
        Some(_) => {}
        None => {
            if planned_inventory.cache_root_sha256 != store.root_sha256()
                || item.inventory_sha256 != planned_inventory.sha256()?
            {
                return Err("schedule retention: bundle GC inventory/root binding changed".into());
            }
            let original_plan = plan_bundle_gc(state, planned_inventory, item.planned_at_ms)?;
            if original_plan
                .removals
                .iter()
                .find(|candidate| candidate.action_id == item.action_id)
                != Some(item)
            {
                return Err(
                    "schedule retention: bundle is not eligible for its exact GC intent".into(),
                );
            }
            let mut candidate = state.clone();
            candidate.begin_bundle_gc(bundle_action(item, started_at_ms))?;
            journal.append(&candidate, started_at_ms)?;
            *state = candidate;
        }
    }

    let Some(_lease) = try_acquire_evidence_gc_lease_optional(
        capability,
        &bundle_lease_id(&item.entry.bundle_id)?,
    )?
    else {
        return Ok(BundleGcOutcomeV1::DeferredLeaseBusy);
    };

    let observed_inventory = match inventory_probe.inventory_all() {
        Ok(inventory) => inventory,
        Err(_) => {
            return finish_bundle_safe_skip(
                journal,
                state,
                &item.action_id,
                "bundle_inventory_probe_failed",
                completed_at_ms,
            )
        }
    };
    let current_inventory = match observed_inventory.normalized() {
        Ok(inventory) if inventory.cache_root_sha256 == store.root_sha256() => inventory,
        _ => {
            return finish_bundle_safe_skip(
                journal,
                state,
                &item.action_id,
                "bundle_inventory_invalid",
                completed_at_ms,
            )
        }
    };
    if current_inventory.observed_at_ms < started_at_ms
        || current_inventory.observed_at_ms > completed_at_ms
    {
        return finish_bundle_safe_skip(
            journal,
            state,
            &item.action_id,
            "bundle_inventory_stale",
            completed_at_ms,
        );
    }
    let current_entry = current_inventory
        .entries
        .iter()
        .find(|entry| entry.bundle_id == item.entry.bundle_id);
    let name = OsStr::new(&item.entry.path.components[0]);
    let removal_candidate = store
        .root
        .regular_child_removal_candidate(name, "bundle GC target")?;
    if let Some(current_entry) = current_entry {
        if current_entry != &item.entry {
            return finish_bundle_safe_skip(
                journal,
                state,
                &item.action_id,
                "bundle_identity_changed",
                completed_at_ms,
            );
        }
        let current_plan =
            plan_bundle_gc(state, &current_inventory, current_inventory.observed_at_ms)?;
        if !current_plan
            .removals
            .iter()
            .any(|candidate| candidate.entry == item.entry)
        {
            return finish_bundle_safe_skip(
                journal,
                state,
                &item.action_id,
                "freshly_protected",
                completed_at_ms,
            );
        }
    } else if removal_candidate.as_deref() == Some(name) {
        return finish_bundle_safe_skip(
            journal,
            state,
            &item.action_id,
            "inventory_target_missing",
            completed_at_ms,
        );
    }

    let existed = if let Some(candidate_name) = removal_candidate {
        let file = store
            .root
            .open_regular_file(&candidate_name, "bundle GC target")?;
        validate_private_file(&file.metadata()?, "bundle GC target")?;
        let snapshot = local_file::read_open_regular_file_bounded(
            &file,
            "bundle GC target",
            item.entry.length_bytes,
        )?;
        if snapshot.bytes.len() as u64 != item.entry.length_bytes
            || snapshot.sha256 != item.entry.content_sha256
        {
            return Err("schedule retention: bundle GC target identity changed".into());
        }
        store.root.remove_regular_child_candidate(
            name,
            local_file::RegularChildRef::new(&candidate_name, &file),
            "bundle GC target",
        )?;
        store.root.sync()?;
        true
    } else {
        false
    };
    if failpoint == BundleGcFailpointV1::AfterUnlink {
        return Err("schedule retention: injected crash after bundle unlink".into());
    }
    let mut candidate = state.clone();
    candidate.complete_bundle_gc(&item.action_id, completed_at_ms)?;
    journal.append(&candidate, completed_at_ms)?;
    *state = candidate;
    Ok(if existed {
        BundleGcOutcomeV1::Removed
    } else {
        BundleGcOutcomeV1::RecoveredAlreadyAbsent
    })
}

#[cfg_attr(not(test), allow(dead_code))]
fn immutable_image_digest(label: &str, value: &str) -> Result<(), BoxError> {
    let Some(sha256) = value.strip_prefix("sha256:") else {
        return Err(format!("schedule retention: {label} is not an immutable digest").into());
    };
    lowercase_sha256(label, sha256)
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum RuntimeImageOwnershipV1 {
    BridgeManaged,
    Unrelated,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct RuntimeImageV1 {
    pub(super) digest: String,
    pub(super) ownership: RuntimeImageOwnershipV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct SuccessfulImageCandidateV1 {
    pub(super) provider_id: String,
    pub(super) digest: String,
    pub(super) succeeded_at_ms: i64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum ContainerLifecycleV1 {
    Running,
    Stopped,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct ContainerImageReferenceV1 {
    pub(super) container_id: String,
    pub(super) digest: String,
    pub(super) state: ContainerLifecycleV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct RuntimeImageInventoryV1 {
    pub(super) observed_at_ms: i64,
    pub(super) images: Vec<RuntimeImageV1>,
    pub(super) current_production_digests: BTreeSet<String>,
    pub(super) pinned_digests: BTreeSet<String>,
    pub(super) successful_candidates: Vec<SuccessfulImageCandidateV1>,
    pub(super) container_references: Vec<ContainerImageReferenceV1>,
}

impl RuntimeImageInventoryV1 {
    #[cfg_attr(not(test), allow(dead_code))]
    fn normalized(&self) -> Result<Self, BoxError> {
        if self.observed_at_ms <= 0
            || self.images.len() > MAX_CACHE_INVENTORY_ITEMS
            || self.current_production_digests.len() > MAX_CACHE_INVENTORY_ITEMS
            || self.pinned_digests.len() > MAX_CACHE_INVENTORY_ITEMS
            || self.successful_candidates.len() > MAX_CACHE_INVENTORY_ITEMS * 4
            || self.container_references.len() > MAX_CACHE_INVENTORY_ITEMS * 4
        {
            return Err(
                "schedule retention: runtime image inventory is invalid or unbounded".into(),
            );
        }
        let mut value = self.clone();
        value
            .images
            .sort_by(|left, right| left.digest.cmp(&right.digest));
        value.successful_candidates.sort_by(|left, right| {
            left.provider_id
                .cmp(&right.provider_id)
                .then_with(|| right.succeeded_at_ms.cmp(&left.succeeded_at_ms))
                .then_with(|| left.digest.cmp(&right.digest))
        });
        value.container_references.sort_by(|left, right| {
            left.container_id
                .cmp(&right.container_id)
                .then_with(|| left.digest.cmp(&right.digest))
        });
        let mut image_digests = BTreeSet::new();
        for image in &value.images {
            immutable_image_digest("runtime image", &image.digest)?;
            if !image_digests.insert(image.digest.clone()) {
                return Err("schedule retention: runtime image digest is duplicated".into());
            }
        }
        for digest in value
            .current_production_digests
            .iter()
            .chain(value.pinned_digests.iter())
        {
            immutable_image_digest("protected runtime image", digest)?;
        }
        let mut candidate_keys = BTreeSet::new();
        for candidate in &value.successful_candidates {
            cache_stable_id("image candidate provider", &candidate.provider_id)?;
            immutable_image_digest("image candidate", &candidate.digest)?;
            if candidate.succeeded_at_ms <= 0
                || candidate.succeeded_at_ms > value.observed_at_ms
                || !candidate_keys.insert((
                    candidate.provider_id.clone(),
                    candidate.digest.clone(),
                    candidate.succeeded_at_ms,
                ))
            {
                return Err("schedule retention: image candidate identity is invalid".into());
            }
        }
        let mut container_ids = BTreeSet::new();
        for reference in &value.container_references {
            cache_stable_id("container reference id", &reference.container_id)?;
            immutable_image_digest("container image reference", &reference.digest)?;
            if !container_ids.insert(reference.container_id.clone()) {
                return Err("schedule retention: container reference id is duplicated".into());
            }
        }
        Ok(value)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn sha256(&self) -> Result<String, BoxError> {
        Ok(local_file::sha256_hex(&serde_json::to_vec(
            &self.normalized()?,
        )?))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum ImageGcProtectionV1 {
    CurrentProduction,
    LatestSuccessfulCandidate,
    Pinned,
    ContainerReference,
    Unrelated,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct ProtectedImageV1 {
    pub(super) digest: String,
    pub(super) reason: ImageGcProtectionV1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct ImageGcPlanItemV1 {
    pub(super) action_id: String,
    pub(super) digest: String,
    pub(super) inventory_sha256: String,
    pub(super) planned_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct ImageGcPlanV1 {
    pub(super) planned_at_ms: i64,
    pub(super) inventory_sha256: String,
    pub(super) removals: Vec<ImageGcPlanItemV1>,
    pub(super) protected: Vec<ProtectedImageV1>,
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn plan_image_gc(
    inventory: &RuntimeImageInventoryV1,
    planned_at_ms: i64,
) -> Result<ImageGcPlanV1, BoxError> {
    let inventory = inventory.normalized()?;
    if planned_at_ms <= 0 || inventory.observed_at_ms > planned_at_ms {
        return Err("schedule retention: image GC plan has no current observation".into());
    }
    let inventory_sha256 = inventory.sha256()?;
    let mut latest_successful = BTreeSet::new();
    let mut by_provider = BTreeMap::<String, Vec<&SuccessfulImageCandidateV1>>::new();
    for candidate in &inventory.successful_candidates {
        by_provider
            .entry(candidate.provider_id.clone())
            .or_default()
            .push(candidate);
    }
    for candidates in by_provider.values_mut() {
        candidates.sort_by(|left, right| {
            right
                .succeeded_at_ms
                .cmp(&left.succeeded_at_ms)
                .then_with(|| right.digest.cmp(&left.digest))
        });
        for candidate in candidates.iter().take(2) {
            latest_successful.insert(candidate.digest.clone());
        }
    }
    let container_references = inventory
        .container_references
        .iter()
        .map(|reference| reference.digest.clone())
        .collect::<BTreeSet<_>>();
    let mut removals = Vec::new();
    let mut protected = Vec::new();
    for image in inventory.images {
        let protection = if image.ownership == RuntimeImageOwnershipV1::Unrelated {
            Some(ImageGcProtectionV1::Unrelated)
        } else if container_references.contains(&image.digest) {
            Some(ImageGcProtectionV1::ContainerReference)
        } else if inventory.current_production_digests.contains(&image.digest) {
            Some(ImageGcProtectionV1::CurrentProduction)
        } else if inventory.pinned_digests.contains(&image.digest) {
            Some(ImageGcProtectionV1::Pinned)
        } else if latest_successful.contains(&image.digest) {
            Some(ImageGcProtectionV1::LatestSuccessfulCandidate)
        } else {
            None
        };
        if let Some(reason) = protection {
            protected.push(ProtectedImageV1 {
                digest: image.digest,
                reason,
            });
            continue;
        }
        let action_id = format!(
            "image-gc:{}",
            local_file::sha256_hex(
                format!("{}\0{}\0{}", inventory_sha256, image.digest, planned_at_ms).as_bytes()
            )
        );
        removals.push(ImageGcPlanItemV1 {
            action_id,
            digest: image.digest,
            inventory_sha256: inventory_sha256.clone(),
            planned_at_ms,
        });
    }
    removals.sort_by(|left, right| left.digest.cmp(&right.digest));
    protected.sort_by(|left, right| left.digest.cmp(&right.digest));
    Ok(ImageGcPlanV1 {
        planned_at_ms,
        inventory_sha256,
        removals,
        protected,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum RuntimeImageRemovalV1 {
    Removed,
    Absent,
    Refused { reason_code: String },
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) trait RuntimeImageEffectsV1 {
    fn inventory_all(&mut self) -> Result<RuntimeImageInventoryV1, BoxError>;

    fn remove_exact_digest(&mut self, digest: &str) -> Result<RuntimeImageRemovalV1, BoxError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum ImageGcFailpointV1 {
    None,
    AfterRemoval,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum ImageGcOutcomeV1 {
    Removed,
    RecoveredAlreadyAbsent,
    SafeSkipped { reason_code: String },
    AlreadyTerminal,
}

#[cfg_attr(not(test), allow(dead_code))]
fn image_action(item: &ImageGcPlanItemV1, started_at_ms: i64) -> ImageGcActionV1 {
    ImageGcActionV1 {
        action_id: item.action_id.clone(),
        digest: item.digest.clone(),
        planned_inventory_sha256: item.inventory_sha256.clone(),
        planned_at_ms: item.planned_at_ms,
        started_at_ms,
        lifecycle: ImageGcLifecycleV1::Pending,
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn image_action_matches_item(action: &ImageGcActionV1, item: &ImageGcPlanItemV1) -> bool {
    action.action_id == item.action_id
        && action.digest == item.digest
        && action.planned_inventory_sha256 == item.inventory_sha256
        && action.planned_at_ms == item.planned_at_ms
}

#[cfg_attr(not(test), allow(dead_code))]
fn validate_image_gc_item(item: &ImageGcPlanItemV1) -> Result<(), BoxError> {
    immutable_image_digest("image GC item", &item.digest)?;
    lowercase_sha256("image GC planned inventory", &item.inventory_sha256)?;
    if item.planned_at_ms <= 0 {
        return Err("schedule retention: image GC plan time is invalid".into());
    }
    let expected_action_id = format!(
        "image-gc:{}",
        local_file::sha256_hex(
            format!(
                "{}\0{}\0{}",
                item.inventory_sha256, item.digest, item.planned_at_ms
            )
            .as_bytes()
        )
    );
    if item.action_id != expected_action_id {
        return Err("schedule retention: image GC action identity is not deterministic".into());
    }
    Ok(())
}

#[cfg_attr(not(test), allow(dead_code))]
fn finish_image_safe_skip(
    journal: &mut FileEvidenceJournal<'_>,
    state: &mut EvidenceStateModelV1,
    action_id: &str,
    reason_code: &str,
    completed_at_ms: i64,
) -> Result<ImageGcOutcomeV1, BoxError> {
    cache_stable_id("image GC safe-skip reason", reason_code)?;
    let mut candidate = state.clone();
    candidate.safe_skip_image_gc(action_id, reason_code, completed_at_ms)?;
    journal.append(&candidate, completed_at_ms)?;
    *state = candidate;
    Ok(ImageGcOutcomeV1::SafeSkipped {
        reason_code: reason_code.into(),
    })
}

// Runtime removal keeps the fresh inventory, plan, journal, lease, and injected effect separate.
#[allow(clippy::too_many_arguments)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn execute_image_gc_item<
    C: EvidenceStateCapability + ?Sized,
    R: RuntimeImageEffectsV1 + ?Sized,
>(
    _capability: &C,
    journal: &mut FileEvidenceJournal<'_>,
    state: &mut EvidenceStateModelV1,
    runtime: &mut R,
    planned_inventory: Option<&RuntimeImageInventoryV1>,
    item: &ImageGcPlanItemV1,
    started_at_ms: i64,
    completed_at_ms: i64,
    failpoint: ImageGcFailpointV1,
) -> Result<ImageGcOutcomeV1, BoxError> {
    state.validate()?;
    validate_image_gc_item(item)?;
    if completed_at_ms <= started_at_ms {
        return Err("schedule retention: image GC completion time is invalid".into());
    }
    match state.image_gc_actions.get(&item.action_id) {
        Some(action) if !image_action_matches_item(action, item) => {
            return Err("schedule retention: image GC action identity conflicts".into())
        }
        Some(action) if action.lifecycle != ImageGcLifecycleV1::Pending => {
            return Ok(ImageGcOutcomeV1::AlreadyTerminal)
        }
        Some(_) => {}
        None => {
            let planned_inventory = planned_inventory
                .ok_or("schedule retention: image GC first intent has no planned inventory")?;
            if item.inventory_sha256 != planned_inventory.sha256()? {
                return Err(
                    "schedule retention: image GC planned inventory binding changed".into(),
                );
            }
            let original_plan = plan_image_gc(planned_inventory, item.planned_at_ms)?;
            if original_plan
                .removals
                .iter()
                .find(|candidate| candidate.action_id == item.action_id)
                != Some(item)
            {
                return Err(
                    "schedule retention: image is not eligible for its exact GC intent".into(),
                );
            }
            let mut candidate = state.clone();
            candidate.begin_image_gc(image_action(item, started_at_ms))?;
            journal.append(&candidate, started_at_ms)?;
            *state = candidate;
        }
    }

    let fresh = match runtime.inventory_all() {
        Ok(inventory) => inventory,
        Err(_) => {
            return finish_image_safe_skip(
                journal,
                state,
                &item.action_id,
                "runtime_inventory_failed",
                completed_at_ms,
            )
        }
    };
    let fresh = match fresh.normalized() {
        Ok(inventory)
            if inventory.observed_at_ms >= started_at_ms
                && inventory.observed_at_ms <= completed_at_ms =>
        {
            inventory
        }
        _ => {
            return finish_image_safe_skip(
                journal,
                state,
                &item.action_id,
                "runtime_inventory_invalid",
                completed_at_ms,
            )
        }
    };
    if !fresh.images.iter().any(|image| image.digest == item.digest) {
        let mut candidate = state.clone();
        candidate.complete_image_gc(&item.action_id, completed_at_ms)?;
        journal.append(&candidate, completed_at_ms)?;
        *state = candidate;
        return Ok(ImageGcOutcomeV1::RecoveredAlreadyAbsent);
    }
    let fresh_plan = plan_image_gc(&fresh, fresh.observed_at_ms)?;
    if !fresh_plan
        .removals
        .iter()
        .any(|candidate| candidate.digest == item.digest)
    {
        return finish_image_safe_skip(
            journal,
            state,
            &item.action_id,
            "freshly_protected",
            completed_at_ms,
        );
    }

    let removal = match runtime.remove_exact_digest(&item.digest) {
        Ok(removal) => removal,
        Err(_) => {
            return finish_image_safe_skip(
                journal,
                state,
                &item.action_id,
                "runtime_remove_failed",
                completed_at_ms,
            )
        }
    };
    match removal {
        RuntimeImageRemovalV1::Refused { .. } => finish_image_safe_skip(
            journal,
            state,
            &item.action_id,
            "runtime_remove_refused",
            completed_at_ms,
        ),
        RuntimeImageRemovalV1::Removed => {
            if failpoint == ImageGcFailpointV1::AfterRemoval {
                return Err("schedule retention: injected crash after image removal".into());
            }
            let mut candidate = state.clone();
            candidate.complete_image_gc(&item.action_id, completed_at_ms)?;
            journal.append(&candidate, completed_at_ms)?;
            *state = candidate;
            Ok(ImageGcOutcomeV1::Removed)
        }
        RuntimeImageRemovalV1::Absent => {
            let mut candidate = state.clone();
            candidate.complete_image_gc(&item.action_id, completed_at_ms)?;
            journal.append(&candidate, completed_at_ms)?;
            *state = candidate;
            Ok(ImageGcOutcomeV1::RecoveredAlreadyAbsent)
        }
    }
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

    fn gc_test_guard() -> std::sync::MutexGuard<'static, ()> {
        static GC_TEST_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        GC_TEST_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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
        let publication = publish_admitted_cold_copy(
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
        assert_eq!(publication.archive_path, admission.archive_path);
        let reopened = FileEvidenceJournal::open_existing(&owner).unwrap();
        assert_eq!(publication.snapshot_sha256, reopened.snapshot_sha256);
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

    #[allow(clippy::too_many_arguments)] // Fixture spells out every GC selection dimension.
    fn bundle_entry(
        state: &EvidenceStateModelV1,
        bundle_id: &str,
        evidence_id: &str,
        provider_id: &str,
        case_id: &str,
        created_at_ms: i64,
        kind: BundleCacheKindV1,
        bytes: &[u8],
    ) -> BundleCacheEntryV1 {
        BundleCacheEntryV1 {
            bundle_id: bundle_id.into(),
            evidence_id: evidence_id.into(),
            provider_id: provider_id.into(),
            case_id: case_id.into(),
            evidence_class: state.entries[evidence_id].evidence_class,
            kind,
            created_at_ms,
            path: RelativeEvidencePathV1 {
                components: vec![format!("{bundle_id}.bundle")],
            },
            content_sha256: local_file::sha256_hex(bytes),
            length_bytes: bytes.len() as u64,
            preserved_in_full_evidence_sha256: state.entries[evidence_id]
                .full_evidence_sha256
                .clone(),
        }
    }

    fn fixed_bundle_inventory_probe(
        inventory: BundleCacheInventoryV1,
    ) -> impl BundleCacheInventoryProbeV1 {
        move || -> Result<BundleCacheInventoryV1, BoxError> { Ok(inventory.clone()) }
    }

    #[test]
    fn tombstone_persists_before_lease_and_recovers_after_the_first_exact_unlink() {
        let _gc_test_guard = gc_test_guard();
        let fixture = Fixture::new();
        let owner = fixture
            .scheduler
            .try_owner_admission("test/r3d3a-tombstone-effects")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let mut state = opened.snapshot.state.clone();
        let evidence_id = "evidence-1";
        let object_name = local_file::sha256_hex(evidence_id.as_bytes());
        let payload = fixture.hot_root.path().join("sealed").join(object_name);
        let reader = acquire_evidence_read_lease(&owner, evidence_id).unwrap();

        assert_eq!(
            execute_evidence_tombstone(
                &owner,
                &mut opened.journal,
                &mut state,
                &fixture.hot,
                None,
                None,
                "tombstone-1",
                evidence_id,
                "retention_expired",
                BASE + 1,
                BASE + 2,
                EvidenceTombstoneFailpointV1::None,
            )
            .unwrap(),
            EvidenceTombstoneOutcomeV1::DeferredLeaseBusy
        );
        assert!(payload.join("evidence.tar.gz").exists());
        assert!(payload.join("manifest.json").exists());
        assert!(matches!(
            state.tombstones["tombstone-1"].lifecycle,
            crate::compatibility_schedule_evidence::TombstoneLifecycleV1::Pending
        ));
        drop(reader);

        assert!(execute_evidence_tombstone(
            &owner,
            &mut opened.journal,
            &mut state,
            &fixture.hot,
            None,
            None,
            "tombstone-1",
            evidence_id,
            "retention_expired",
            BASE + 3,
            BASE + 4,
            EvidenceTombstoneFailpointV1::AfterFirstUnlink,
        )
        .is_err());
        assert!(!payload.join("evidence.tar.gz").exists());
        assert!(payload.join("manifest.json").exists());
        assert!(matches!(
            state.tombstones["tombstone-1"].lifecycle,
            crate::compatibility_schedule_evidence::TombstoneLifecycleV1::Pending
        ));

        let reopened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let mut recovered_state = reopened.snapshot.state.clone();
        let mut recovered_journal = reopened.journal;
        assert!(matches!(
            execute_evidence_tombstone(
                &owner,
                &mut recovered_journal,
                &mut recovered_state,
                &fixture.hot,
                None,
                None,
                "tombstone-1",
                evidence_id,
                "retention_expired",
                BASE + 5,
                BASE + 6,
                EvidenceTombstoneFailpointV1::None,
            )
            .unwrap(),
            EvidenceTombstoneOutcomeV1::Completed { .. }
        ));
        assert!(!payload.exists());
        assert!(!recovered_state.entries.contains_key(evidence_id));
        assert!(matches!(
            recovered_state.tombstones["tombstone-1"].lifecycle,
            crate::compatibility_schedule_evidence::TombstoneLifecycleV1::FullEvidenceUnlinked { .. }
        ));
    }

    #[test]
    fn tombstone_refuses_hash_tampering_without_unlinking_or_completing() {
        let _gc_test_guard = gc_test_guard();
        let fixture = Fixture::new();
        let owner = fixture
            .scheduler
            .try_owner_admission("test/r3d3a-tombstone-tamper")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let mut state = opened.snapshot.state.clone();
        let evidence_id = "evidence-1";
        let payload = fixture
            .hot_root
            .path()
            .join("sealed")
            .join(local_file::sha256_hex(evidence_id.as_bytes()));
        let manifest_path = payload.join("manifest.json");
        let mut tampered = std::fs::read(&manifest_path).unwrap();
        tampered[0] ^= 1;
        write_private(&manifest_path, &tampered);

        assert!(execute_evidence_tombstone(
            &owner,
            &mut opened.journal,
            &mut state,
            &fixture.hot,
            None,
            None,
            "tombstone-tamper",
            evidence_id,
            "retention_expired",
            BASE + 1,
            BASE + 2,
            EvidenceTombstoneFailpointV1::None,
        )
        .is_err());
        assert!(payload.join("evidence.tar.gz").exists());
        assert!(manifest_path.exists());
        assert!(state.entries.contains_key(evidence_id));
        assert!(matches!(
            state.tombstones["tombstone-tamper"].lifecycle,
            TombstoneLifecycleV1::Pending
        ));
    }

    #[test]
    fn tombstone_recovers_crashes_after_pending_and_after_all_unlinks() {
        let _gc_test_guard = gc_test_guard();
        let fixture = Fixture::new();
        let owner = fixture
            .scheduler
            .try_owner_admission("test/r3d3a-tombstone-crash-boundaries")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let mut state = opened.snapshot.state.clone();
        let evidence_id = "evidence-1";
        let payload = fixture
            .hot_root
            .path()
            .join("sealed")
            .join(local_file::sha256_hex(evidence_id.as_bytes()));

        assert!(execute_evidence_tombstone(
            &owner,
            &mut opened.journal,
            &mut state,
            &fixture.hot,
            None,
            None,
            "tombstone-crash",
            evidence_id,
            "retention_expired",
            BASE + 1,
            BASE + 2,
            EvidenceTombstoneFailpointV1::AfterPendingIntent,
        )
        .is_err());
        assert!(payload.exists());
        assert!(matches!(
            state.tombstones["tombstone-crash"].lifecycle,
            TombstoneLifecycleV1::Pending
        ));

        assert!(execute_evidence_tombstone(
            &owner,
            &mut opened.journal,
            &mut state,
            &fixture.hot,
            None,
            None,
            "tombstone-crash",
            evidence_id,
            "retention_expired",
            BASE + 3,
            BASE + 4,
            EvidenceTombstoneFailpointV1::AfterAllUnlinks,
        )
        .is_err());
        assert!(!payload.exists());
        assert!(state.entries.contains_key(evidence_id));
        assert!(matches!(
            state.tombstones["tombstone-crash"].lifecycle,
            TombstoneLifecycleV1::Pending
        ));

        assert!(matches!(
            execute_evidence_tombstone(
                &owner,
                &mut opened.journal,
                &mut state,
                &fixture.hot,
                None,
                None,
                "tombstone-crash",
                evidence_id,
                "retention_expired",
                BASE + 5,
                BASE + 6,
                EvidenceTombstoneFailpointV1::None,
            )
            .unwrap(),
            EvidenceTombstoneOutcomeV1::Completed { .. }
        ));
        assert!(!state.entries.contains_key(evidence_id));
    }

    #[test]
    fn tombstone_recovers_a_synced_hot_removal_quarantine_without_fabricating_absence() {
        let _gc_test_guard = gc_test_guard();
        let fixture = Fixture::new();
        let owner = fixture
            .scheduler
            .try_owner_admission("test/r3d3a-tombstone-quarantine-recovery")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let mut state = opened.snapshot.state.clone();
        let evidence_id = "evidence-1";
        let payload = fixture
            .hot_root
            .path()
            .join("sealed")
            .join(local_file::sha256_hex(evidence_id.as_bytes()));

        assert!(execute_evidence_tombstone(
            &owner,
            &mut opened.journal,
            &mut state,
            &fixture.hot,
            None,
            None,
            "tombstone-quarantine",
            evidence_id,
            "retention_expired",
            BASE + 1,
            BASE + 2,
            EvidenceTombstoneFailpointV1::AfterPendingIntent,
        )
        .is_err());

        let archive_name = OsStr::new("evidence.tar.gz");
        let quarantine_name =
            local_file::removal_quarantine_name(archive_name, "hot evidence archive eviction")
                .unwrap();
        std::fs::rename(payload.join(archive_name), payload.join(&quarantine_name)).unwrap();
        pin(&payload, "test hot quarantine").sync().unwrap();
        assert!(!payload.join(archive_name).exists());
        assert!(payload.join(&quarantine_name).exists());
        assert!(state.entries.contains_key(evidence_id));

        assert!(matches!(
            execute_evidence_tombstone(
                &owner,
                &mut opened.journal,
                &mut state,
                &fixture.hot,
                None,
                None,
                "tombstone-quarantine",
                evidence_id,
                "retention_expired",
                BASE + 3,
                BASE + 4,
                EvidenceTombstoneFailpointV1::None,
            )
            .unwrap(),
            EvidenceTombstoneOutcomeV1::Completed { .. }
        ));
        assert!(!payload.exists());
        assert!(!state.entries.contains_key(evidence_id));
        assert!(matches!(
            state.tombstones["tombstone-quarantine"].lifecycle,
            TombstoneLifecycleV1::FullEvidenceUnlinked { .. }
        ));
    }

    #[test]
    fn pending_tombstone_rejects_a_later_pin_without_deleting_evidence() {
        let _gc_test_guard = gc_test_guard();
        let fixture = Fixture::new();
        let owner = fixture
            .scheduler
            .try_owner_admission("test/r3d3a-tombstone-pin-race")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let mut state = opened.snapshot.state.clone();
        let evidence_id = "evidence-1";
        let payload = fixture
            .hot_root
            .path()
            .join("sealed")
            .join(local_file::sha256_hex(evidence_id.as_bytes()));

        assert!(execute_evidence_tombstone(
            &owner,
            &mut opened.journal,
            &mut state,
            &fixture.hot,
            None,
            None,
            "tombstone-pin-race",
            evidence_id,
            "retention_expired",
            BASE + 1,
            BASE + 2,
            EvidenceTombstoneFailpointV1::AfterPendingIntent,
        )
        .is_err());
        let pending = state.clone();

        let error = state
            .pin(crate::compatibility_schedule_evidence::EvidencePinV1 {
                pin_id: "pin-after-pending".into(),
                evidence_id: evidence_id.into(),
                reason: "late incident investigation".into(),
                created_at_ms: BASE + 3,
                lifecycle: crate::compatibility_schedule_evidence::PinLifecycleV1::Active,
            })
            .unwrap_err();
        assert!(error.to_string().contains("pending tombstone"));
        assert_eq!(state, pending);
        assert!(payload.join("evidence.tar.gz").exists());
        assert!(payload.join("manifest.json").exists());
    }

    #[test]
    fn cold_only_tombstone_recovers_after_the_first_cold_unlink() {
        let _gc_test_guard = gc_test_guard();
        let fixture = Fixture::new();
        let mut provider = FakeProvider::new(&fixture.cold);
        provider.synchronization = FileProviderSynchronizationV1::Synchronized;
        let admission = admit(&fixture, &mut provider, "evidence-1", BASE + 1);
        let combined = fixture
            .scheduler
            .try_owner_admission("test/r3d3a-cold-tombstone-owner")
            .unwrap()
            .try_authority_state("test/r3d3a-cold-tombstone-authority")
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
        evict_hot_evidence(
            &combined,
            &mut opened.journal,
            &mut state,
            &fixture.hot,
            &fixture.cold,
            &mut provider,
            "evidence-1",
            BASE + 3,
            HotEvictionFailpointV1::None,
        )
        .unwrap();
        assert!(!state.entries["evidence-1"].hot_present);

        assert!(execute_evidence_tombstone(
            &combined,
            &mut opened.journal,
            &mut state,
            &fixture.hot,
            Some(&fixture.cold),
            Some(&mut provider),
            "tombstone-cold",
            "evidence-1",
            "retention_expired",
            BASE + 4,
            BASE + 5,
            EvidenceTombstoneFailpointV1::AfterFirstUnlink,
        )
        .is_err());
        let archive_path = fixture
            .cold_root
            .path()
            .join(&admission.archive_path.components[0]);
        let manifest_path = fixture
            .cold_root
            .path()
            .join(&admission.manifest_path.components[0]);
        assert!(!archive_path.exists());
        assert!(manifest_path.exists());
        assert!(matches!(
            state.tombstones["tombstone-cold"].lifecycle,
            TombstoneLifecycleV1::Pending
        ));

        let reopened = FileEvidenceJournal::open_existing(&combined).unwrap();
        let mut recovered_state = reopened.snapshot.state.clone();
        let mut recovered_journal = reopened.journal;
        assert!(matches!(
            execute_evidence_tombstone(
                &combined,
                &mut recovered_journal,
                &mut recovered_state,
                &fixture.hot,
                Some(&fixture.cold),
                Some(&mut provider),
                "tombstone-cold",
                "evidence-1",
                "retention_expired",
                BASE + 6,
                BASE + 7,
                EvidenceTombstoneFailpointV1::None,
            )
            .unwrap(),
            EvidenceTombstoneOutcomeV1::Completed { .. }
        ));
        assert!(!manifest_path.exists());
        assert!(!recovered_state.entries.contains_key("evidence-1"));
        assert_eq!(
            execute_evidence_tombstone(
                &combined,
                &mut recovered_journal,
                &mut recovered_state,
                &fixture.hot,
                Some(&fixture.cold),
                None,
                "tombstone-cold",
                "evidence-1",
                "retention_expired",
                BASE + 8,
                BASE + 9,
                EvidenceTombstoneFailpointV1::None,
            )
            .unwrap(),
            EvidenceTombstoneOutcomeV1::AlreadyComplete
        );
    }

    #[test]
    fn cold_tombstone_recovers_a_synced_removal_quarantine_through_provider_probe() {
        let _gc_test_guard = gc_test_guard();
        let fixture = Fixture::new();
        let mut provider = FakeProvider::new(&fixture.cold);
        provider.synchronization = FileProviderSynchronizationV1::Synchronized;
        let admission = admit(&fixture, &mut provider, "evidence-1", BASE + 1);
        let combined = fixture
            .scheduler
            .try_owner_admission("test/r3d3a-cold-quarantine-owner")
            .unwrap()
            .try_authority_state("test/r3d3a-cold-quarantine-authority")
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
        evict_hot_evidence(
            &combined,
            &mut opened.journal,
            &mut state,
            &fixture.hot,
            &fixture.cold,
            &mut provider,
            "evidence-1",
            BASE + 3,
            HotEvictionFailpointV1::None,
        )
        .unwrap();

        assert!(execute_evidence_tombstone(
            &combined,
            &mut opened.journal,
            &mut state,
            &fixture.hot,
            Some(&fixture.cold),
            Some(&mut provider),
            "tombstone-cold-quarantine",
            "evidence-1",
            "retention_expired",
            BASE + 4,
            BASE + 5,
            EvidenceTombstoneFailpointV1::AfterPendingIntent,
        )
        .is_err());

        let archive_name = OsStr::new(&admission.archive_path.components[0]);
        let quarantine_name =
            local_file::removal_quarantine_name(archive_name, "cold evidence archive deletion")
                .unwrap();
        let archive_path = fixture.cold_root.path().join(archive_name);
        let quarantine_path = fixture.cold_root.path().join(&quarantine_name);
        std::fs::rename(&archive_path, &quarantine_path).unwrap();
        fixture.cold.root.sync().unwrap();
        assert!(!archive_path.exists());
        assert!(quarantine_path.exists());

        assert!(matches!(
            execute_evidence_tombstone(
                &combined,
                &mut opened.journal,
                &mut state,
                &fixture.hot,
                Some(&fixture.cold),
                Some(&mut provider),
                "tombstone-cold-quarantine",
                "evidence-1",
                "retention_expired",
                BASE + 6,
                BASE + 7,
                EvidenceTombstoneFailpointV1::None,
            )
            .unwrap(),
            EvidenceTombstoneOutcomeV1::Completed { .. }
        ));
        assert!(!archive_path.exists());
        assert!(!quarantine_path.exists());
        assert!(!fixture
            .cold_root
            .path()
            .join(&admission.manifest_path.components[0])
            .exists());
        assert!(!state.entries.contains_key("evidence-1"));
    }

    #[test]
    fn bundle_plan_honors_keep_age_pin_reference_and_full_evidence_precedence() {
        let fixture = Fixture::new();
        let owner = fixture
            .scheduler
            .try_owner_admission("test/r3d3d-bundle-plan")
            .unwrap();
        let opened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let mut state = opened.snapshot.state.clone();
        state
            .pin(crate::compatibility_schedule_evidence::EvidencePinV1 {
                pin_id: "pin-evidence-2".into(),
                evidence_id: "evidence-2".into(),
                reason: "active incident investigation".into(),
                created_at_ms: BASE + 1,
                lifecycle: crate::compatibility_schedule_evidence::PinLifecycleV1::Active,
            })
            .unwrap();
        let now = BASE + 200 * DAY_MS;
        let mut entries = vec![
            bundle_entry(
                &state,
                "routine-new-1",
                "evidence-1",
                "codex",
                "case-a",
                now - DAY_MS,
                BundleCacheKindV1::ReconstructiblePayload,
                b"new-1",
            ),
            bundle_entry(
                &state,
                "routine-new-2",
                "evidence-1",
                "codex",
                "case-a",
                now - 2 * DAY_MS,
                BundleCacheKindV1::ReconstructiblePayload,
                b"new-2",
            ),
            bundle_entry(
                &state,
                "routine-new-3",
                "evidence-1",
                "codex",
                "case-a",
                now - 3 * DAY_MS,
                BundleCacheKindV1::ReconstructiblePayload,
                b"new-3",
            ),
            bundle_entry(
                &state,
                "routine-old",
                "evidence-1",
                "codex",
                "case-a",
                now - 20 * DAY_MS,
                BundleCacheKindV1::ReconstructiblePayload,
                b"old",
            ),
            bundle_entry(
                &state,
                "routine-referenced",
                "evidence-1",
                "codex",
                "case-a",
                now - 21 * DAY_MS,
                BundleCacheKindV1::ReconstructiblePayload,
                b"referenced",
            ),
            bundle_entry(
                &state,
                "routine-pinned",
                "evidence-2",
                "codex",
                "case-a",
                now - 22 * DAY_MS,
                BundleCacheKindV1::ReconstructiblePayload,
                b"pinned",
            ),
            bundle_entry(
                &state,
                "manifest-young",
                "evidence-1",
                "codex",
                "case-b",
                now - 179 * DAY_MS,
                BundleCacheKindV1::ManifestOrInventory,
                b"manifest-young",
            ),
            bundle_entry(
                &state,
                "manifest-old",
                "evidence-1",
                "codex",
                "case-b",
                now - 181 * DAY_MS,
                BundleCacheKindV1::ManifestOrInventory,
                b"manifest-old",
            ),
        ];
        let mut unpreserved = bundle_entry(
            &state,
            "unpreserved",
            "evidence-1",
            "claude",
            "case-c",
            now - 181 * DAY_MS,
            BundleCacheKindV1::ManifestOrInventory,
            b"unpreserved",
        );
        unpreserved.preserved_in_full_evidence_sha256 = digest('f');
        entries.push(unpreserved);
        let inventory = BundleCacheInventoryV1 {
            cache_root_sha256: digest('d'),
            observed_at_ms: now,
            entries,
            referenced_bundle_ids: BTreeSet::from(["routine-referenced".into()]),
        };

        let plan = plan_bundle_gc(&state, &inventory, now).unwrap();
        let removed = plan
            .removals
            .iter()
            .map(|item| item.entry.bundle_id.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(removed, BTreeSet::from(["manifest-old", "routine-old"]));
        assert!(plan.protected.iter().any(|item| {
            item.bundle_id == "routine-referenced"
                && item.reason == BundleGcProtectionV1::ActiveReference
        }));
        assert!(plan.protected.iter().any(|item| {
            item.bundle_id == "routine-pinned" && item.reason == BundleGcProtectionV1::ActivePin
        }));
        assert!(plan.protected.iter().any(|item| {
            item.bundle_id == "unpreserved"
                && item.reason == BundleGcProtectionV1::FullEvidenceNotPreserved
        }));
    }

    #[test]
    fn bundle_gc_persists_before_unlink_and_recovers_after_open_reader_or_crash() {
        let _gc_test_guard = gc_test_guard();
        let fixture = Fixture::new();
        let bundle_root = private_root();
        write_private(&bundle_root.path().join("old.bundle"), b"old bundle\n");
        write_private(&bundle_root.path().join("unrelated.bundle"), b"unrelated\n");
        let store =
            BundleCacheStoreV1::open_existing(&pin(bundle_root.path(), "bundle cache")).unwrap();
        let owner = fixture
            .scheduler
            .try_owner_admission("test/r3d3d-bundle-gc")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let mut state = opened.snapshot.state.clone();
        let now = BASE + 200 * DAY_MS;
        let entry = bundle_entry(
            &state,
            "old",
            "evidence-1",
            "codex",
            "case-a",
            now - 40 * DAY_MS,
            BundleCacheKindV1::ReconstructiblePayload,
            b"old bundle\n",
        );
        let inventory = BundleCacheInventoryV1 {
            cache_root_sha256: store.root_sha256().into(),
            observed_at_ms: now,
            entries: vec![entry],
            referenced_bundle_ids: BTreeSet::new(),
        };
        let plan = plan_bundle_gc(&state, &inventory, now).unwrap();
        let item = &plan.removals[0];
        let reader = acquire_bundle_read_lease(&owner, "old").unwrap();
        let mut busy_inventory = inventory.clone();
        busy_inventory.observed_at_ms = now + 1;
        let mut busy_probe = fixed_bundle_inventory_probe(busy_inventory);
        assert_eq!(
            execute_bundle_gc_item(
                &owner,
                &mut opened.journal,
                &mut state,
                &store,
                &inventory,
                &mut busy_probe,
                item,
                now + 1,
                now + 2,
                BundleGcFailpointV1::None,
            )
            .unwrap(),
            BundleGcOutcomeV1::DeferredLeaseBusy
        );
        assert!(bundle_root.path().join("old.bundle").exists());
        assert!(matches!(
            state.bundle_gc_actions[&item.action_id].lifecycle,
            crate::compatibility_schedule_evidence::BundleGcLifecycleV1::Pending
        ));
        drop(reader);
        let mut crash_inventory = inventory.clone();
        crash_inventory.observed_at_ms = now + 3;
        let mut crash_probe = fixed_bundle_inventory_probe(crash_inventory);
        assert!(execute_bundle_gc_item(
            &owner,
            &mut opened.journal,
            &mut state,
            &store,
            &inventory,
            &mut crash_probe,
            item,
            now + 3,
            now + 4,
            BundleGcFailpointV1::AfterUnlink,
        )
        .is_err());
        assert!(!bundle_root.path().join("old.bundle").exists());
        assert!(bundle_root.path().join("unrelated.bundle").exists());
        let recovered_inventory = BundleCacheInventoryV1 {
            cache_root_sha256: store.root_sha256().into(),
            observed_at_ms: now + 5,
            entries: Vec::new(),
            referenced_bundle_ids: BTreeSet::new(),
        };
        let mut recovery_probe = fixed_bundle_inventory_probe(recovered_inventory.clone());
        assert_eq!(
            execute_bundle_gc_item(
                &owner,
                &mut opened.journal,
                &mut state,
                &store,
                &inventory,
                &mut recovery_probe,
                item,
                now + 5,
                now + 6,
                BundleGcFailpointV1::None,
            )
            .unwrap(),
            BundleGcOutcomeV1::RecoveredAlreadyAbsent
        );
    }

    #[test]
    fn bundle_gc_recovers_a_pending_synced_removal_quarantine() {
        let _gc_test_guard = gc_test_guard();
        let fixture = Fixture::new();
        let bundle_root = private_root();
        write_private(&bundle_root.path().join("old.bundle"), b"old bundle\n");
        let store =
            BundleCacheStoreV1::open_existing(&pin(bundle_root.path(), "bundle cache")).unwrap();
        let owner = fixture
            .scheduler
            .try_owner_admission("test/r3d3d-bundle-gc-quarantine")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let mut state = opened.snapshot.state.clone();
        let now = BASE + 200 * DAY_MS;
        let entry = bundle_entry(
            &state,
            "old",
            "evidence-1",
            "codex",
            "case-a",
            now - 40 * DAY_MS,
            BundleCacheKindV1::ReconstructiblePayload,
            b"old bundle\n",
        );
        let inventory = BundleCacheInventoryV1 {
            cache_root_sha256: store.root_sha256().into(),
            observed_at_ms: now,
            entries: vec![entry],
            referenced_bundle_ids: BTreeSet::new(),
        };
        let plan = plan_bundle_gc(&state, &inventory, now).unwrap();
        let item = &plan.removals[0];

        let reader = acquire_bundle_read_lease(&owner, "old").unwrap();
        let mut busy_inventory = inventory.clone();
        busy_inventory.observed_at_ms = now + 1;
        let mut busy_probe = fixed_bundle_inventory_probe(busy_inventory);
        assert_eq!(
            execute_bundle_gc_item(
                &owner,
                &mut opened.journal,
                &mut state,
                &store,
                &inventory,
                &mut busy_probe,
                item,
                now + 1,
                now + 2,
                BundleGcFailpointV1::None,
            )
            .unwrap(),
            BundleGcOutcomeV1::DeferredLeaseBusy
        );
        drop(reader);

        let original_name = OsStr::new("old.bundle");
        let quarantine_name =
            local_file::removal_quarantine_name(original_name, "bundle GC target").unwrap();
        std::fs::rename(
            bundle_root.path().join(original_name),
            bundle_root.path().join(&quarantine_name),
        )
        .unwrap();
        store.root.sync().unwrap();

        let recovered_inventory = BundleCacheInventoryV1 {
            cache_root_sha256: store.root_sha256().into(),
            observed_at_ms: now + 3,
            entries: Vec::new(),
            referenced_bundle_ids: BTreeSet::new(),
        };
        let mut recovery_probe = fixed_bundle_inventory_probe(recovered_inventory);
        assert_eq!(
            execute_bundle_gc_item(
                &owner,
                &mut opened.journal,
                &mut state,
                &store,
                &inventory,
                &mut recovery_probe,
                item,
                now + 3,
                now + 4,
                BundleGcFailpointV1::None,
            )
            .unwrap(),
            BundleGcOutcomeV1::Removed
        );
        assert!(!bundle_root.path().join(original_name).exists());
        assert!(!bundle_root.path().join(quarantine_name).exists());
        assert!(matches!(
            state.bundle_gc_actions[&item.action_id].lifecycle,
            crate::compatibility_schedule_evidence::BundleGcLifecycleV1::Unlinked { .. }
        ));
    }

    #[test]
    fn bundle_gc_pending_intent_safely_skips_a_new_reference_without_unlinking() {
        let _gc_test_guard = gc_test_guard();
        let fixture = Fixture::new();
        let bundle_root = private_root();
        write_private(&bundle_root.path().join("old.bundle"), b"old bundle\n");
        let store =
            BundleCacheStoreV1::open_existing(&pin(bundle_root.path(), "bundle cache")).unwrap();
        let owner = fixture
            .scheduler
            .try_owner_admission("test/r3d3d-bundle-new-reference")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let mut state = opened.snapshot.state.clone();
        let now = BASE + 200 * DAY_MS;
        let entry = bundle_entry(
            &state,
            "old",
            "evidence-1",
            "codex",
            "case-a",
            now - 40 * DAY_MS,
            BundleCacheKindV1::ReconstructiblePayload,
            b"old bundle\n",
        );
        let inventory = BundleCacheInventoryV1 {
            cache_root_sha256: store.root_sha256().into(),
            observed_at_ms: now,
            entries: vec![entry],
            referenced_bundle_ids: BTreeSet::new(),
        };
        let plan = plan_bundle_gc(&state, &inventory, now).unwrap();
        let item = &plan.removals[0];

        let reader = acquire_bundle_read_lease(&owner, "old").unwrap();
        let mut busy_inventory = inventory.clone();
        busy_inventory.observed_at_ms = now + 1;
        let mut busy_probe = fixed_bundle_inventory_probe(busy_inventory);
        assert_eq!(
            execute_bundle_gc_item(
                &owner,
                &mut opened.journal,
                &mut state,
                &store,
                &inventory,
                &mut busy_probe,
                item,
                now + 1,
                now + 2,
                BundleGcFailpointV1::None,
            )
            .unwrap(),
            BundleGcOutcomeV1::DeferredLeaseBusy
        );
        drop(reader);

        let mut referenced_inventory = inventory;
        referenced_inventory.observed_at_ms = now + 3;
        referenced_inventory
            .referenced_bundle_ids
            .insert("old".into());
        let planned_inventory = BundleCacheInventoryV1 {
            observed_at_ms: now,
            referenced_bundle_ids: BTreeSet::new(),
            ..referenced_inventory.clone()
        };
        let mut referenced_probe = fixed_bundle_inventory_probe(referenced_inventory);
        assert_eq!(
            execute_bundle_gc_item(
                &owner,
                &mut opened.journal,
                &mut state,
                &store,
                &planned_inventory,
                &mut referenced_probe,
                item,
                now + 3,
                now + 4,
                BundleGcFailpointV1::None,
            )
            .unwrap(),
            BundleGcOutcomeV1::SafeSkipped {
                reason_code: "freshly_protected".into()
            }
        );
        assert!(bundle_root.path().join("old.bundle").exists());
        assert!(matches!(
            state.bundle_gc_actions[&item.action_id].lifecycle,
            crate::compatibility_schedule_evidence::BundleGcLifecycleV1::SafeSkipped { .. }
        ));
    }

    #[test]
    fn bundle_gc_probes_for_new_references_after_acquiring_the_exclusive_lease() {
        let _gc_test_guard = gc_test_guard();
        let fixture = Fixture::new();
        let bundle_root = private_root();
        write_private(&bundle_root.path().join("old.bundle"), b"old bundle\n");
        let store =
            BundleCacheStoreV1::open_existing(&pin(bundle_root.path(), "bundle cache")).unwrap();
        let owner = fixture
            .scheduler
            .try_owner_admission("test/r3d3d-bundle-action-time-inventory")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let mut state = opened.snapshot.state.clone();
        let now = BASE + 200 * DAY_MS;
        let entry = bundle_entry(
            &state,
            "old",
            "evidence-1",
            "codex",
            "case-a",
            now - 40 * DAY_MS,
            BundleCacheKindV1::ReconstructiblePayload,
            b"old bundle\n",
        );
        let planned_inventory = BundleCacheInventoryV1 {
            cache_root_sha256: store.root_sha256().into(),
            observed_at_ms: now,
            entries: vec![entry],
            referenced_bundle_ids: BTreeSet::new(),
        };
        let item = plan_bundle_gc(&state, &planned_inventory, now)
            .unwrap()
            .removals
            .remove(0);
        let mut observed_inventory = planned_inventory.clone();
        observed_inventory.observed_at_ms = now + 1;
        observed_inventory
            .referenced_bundle_ids
            .insert("old".into());
        let mut probe = || Ok(observed_inventory.clone());

        assert_eq!(
            execute_bundle_gc_item(
                &owner,
                &mut opened.journal,
                &mut state,
                &store,
                &planned_inventory,
                &mut probe,
                &item,
                now + 1,
                now + 2,
                BundleGcFailpointV1::None,
            )
            .unwrap(),
            BundleGcOutcomeV1::SafeSkipped {
                reason_code: "freshly_protected".into()
            }
        );
        assert!(bundle_root.path().join("old.bundle").exists());
    }

    #[test]
    fn bundle_gc_safe_skips_failed_stale_and_future_action_time_inventories() {
        let _gc_test_guard = gc_test_guard();
        let fixture = Fixture::new();
        let bundle_root = private_root();
        for bundle_id in ["probe-failed", "stale", "future"] {
            write_private(
                &bundle_root.path().join(format!("{bundle_id}.bundle")),
                format!("{bundle_id} bundle\n").as_bytes(),
            );
        }
        let store =
            BundleCacheStoreV1::open_existing(&pin(bundle_root.path(), "bundle cache")).unwrap();
        let owner = fixture
            .scheduler
            .try_owner_admission("test/r3d3d-bundle-action-time-bounds")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let mut state = opened.snapshot.state.clone();
        let now = BASE + 200 * DAY_MS;
        let planned_inventory = BundleCacheInventoryV1 {
            cache_root_sha256: store.root_sha256().into(),
            observed_at_ms: now,
            entries: ["probe-failed", "stale", "future"]
                .into_iter()
                .map(|bundle_id| {
                    bundle_entry(
                        &state,
                        bundle_id,
                        "evidence-1",
                        "codex",
                        "case-a",
                        now - 40 * DAY_MS,
                        BundleCacheKindV1::ReconstructiblePayload,
                        format!("{bundle_id} bundle\n").as_bytes(),
                    )
                })
                .collect(),
            referenced_bundle_ids: BTreeSet::new(),
        };
        let plan = plan_bundle_gc(&state, &planned_inventory, now).unwrap();

        let failed_item = plan
            .removals
            .iter()
            .find(|item| item.entry.bundle_id == "probe-failed")
            .unwrap();
        let mut failed_probe = || -> Result<BundleCacheInventoryV1, BoxError> {
            Err("injected inventory probe failure".into())
        };
        assert_eq!(
            execute_bundle_gc_item(
                &owner,
                &mut opened.journal,
                &mut state,
                &store,
                &planned_inventory,
                &mut failed_probe,
                failed_item,
                now + 1,
                now + 2,
                BundleGcFailpointV1::None,
            )
            .unwrap(),
            BundleGcOutcomeV1::SafeSkipped {
                reason_code: "bundle_inventory_probe_failed".into()
            }
        );

        let stale_item = plan
            .removals
            .iter()
            .find(|item| item.entry.bundle_id == "stale")
            .unwrap();
        let mut stale_inventory = planned_inventory.clone();
        stale_inventory.observed_at_ms = now + 2;
        let mut stale_probe = fixed_bundle_inventory_probe(stale_inventory);
        assert_eq!(
            execute_bundle_gc_item(
                &owner,
                &mut opened.journal,
                &mut state,
                &store,
                &planned_inventory,
                &mut stale_probe,
                stale_item,
                now + 3,
                now + 4,
                BundleGcFailpointV1::None,
            )
            .unwrap(),
            BundleGcOutcomeV1::SafeSkipped {
                reason_code: "bundle_inventory_stale".into()
            }
        );

        let future_item = plan
            .removals
            .iter()
            .find(|item| item.entry.bundle_id == "future")
            .unwrap();
        let mut future_inventory = planned_inventory.clone();
        future_inventory.observed_at_ms = now + 7;
        let mut future_probe = fixed_bundle_inventory_probe(future_inventory);
        assert_eq!(
            execute_bundle_gc_item(
                &owner,
                &mut opened.journal,
                &mut state,
                &store,
                &planned_inventory,
                &mut future_probe,
                future_item,
                now + 5,
                now + 6,
                BundleGcFailpointV1::None,
            )
            .unwrap(),
            BundleGcOutcomeV1::SafeSkipped {
                reason_code: "bundle_inventory_stale".into()
            }
        );

        for bundle_id in ["probe-failed", "stale", "future"] {
            assert!(bundle_root
                .path()
                .join(format!("{bundle_id}.bundle"))
                .exists());
        }
    }

    #[test]
    fn bundle_gc_pending_intents_safely_skip_invalid_changed_or_omitted_inventory() {
        let _gc_test_guard = gc_test_guard();
        let fixture = Fixture::new();
        let bundle_root = private_root();
        for bundle_id in ["invalid", "changed", "omitted"] {
            write_private(
                &bundle_root.path().join(format!("{bundle_id}.bundle")),
                format!("{bundle_id} bundle\n").as_bytes(),
            );
        }
        let store =
            BundleCacheStoreV1::open_existing(&pin(bundle_root.path(), "bundle cache")).unwrap();
        let owner = fixture
            .scheduler
            .try_owner_admission("test/r3d3d-bundle-current-inventory")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let mut state = opened.snapshot.state.clone();
        let now = BASE + 200 * DAY_MS;
        let inventory = BundleCacheInventoryV1 {
            cache_root_sha256: store.root_sha256().into(),
            observed_at_ms: now,
            entries: ["invalid", "changed", "omitted"]
                .into_iter()
                .map(|bundle_id| {
                    bundle_entry(
                        &state,
                        bundle_id,
                        "evidence-1",
                        "codex",
                        "case-a",
                        now - 40 * DAY_MS,
                        BundleCacheKindV1::ReconstructiblePayload,
                        format!("{bundle_id} bundle\n").as_bytes(),
                    )
                })
                .collect(),
            referenced_bundle_ids: BTreeSet::new(),
        };
        let plan = plan_bundle_gc(&state, &inventory, now).unwrap();
        assert_eq!(plan.removals.len(), 3);

        for (index, item) in plan.removals.iter().enumerate() {
            let reader = acquire_bundle_read_lease(&owner, &item.entry.bundle_id).unwrap();
            let started_at_ms = now + 1 + index as i64 * 2;
            let mut observed_inventory = inventory.clone();
            observed_inventory.observed_at_ms = started_at_ms;
            let mut probe = fixed_bundle_inventory_probe(observed_inventory);
            assert_eq!(
                execute_bundle_gc_item(
                    &owner,
                    &mut opened.journal,
                    &mut state,
                    &store,
                    &inventory,
                    &mut probe,
                    item,
                    started_at_ms,
                    started_at_ms + 1,
                    BundleGcFailpointV1::None,
                )
                .unwrap(),
                BundleGcOutcomeV1::DeferredLeaseBusy
            );
            drop(reader);
        }

        let item = plan
            .removals
            .iter()
            .find(|item| item.entry.bundle_id == "invalid")
            .unwrap();
        let mut invalid_inventory = inventory.clone();
        invalid_inventory.cache_root_sha256 = digest('f');
        invalid_inventory.observed_at_ms = now + 7;
        let mut invalid_probe = fixed_bundle_inventory_probe(invalid_inventory);
        assert_eq!(
            execute_bundle_gc_item(
                &owner,
                &mut opened.journal,
                &mut state,
                &store,
                &inventory,
                &mut invalid_probe,
                item,
                now + 7,
                now + 8,
                BundleGcFailpointV1::None,
            )
            .unwrap(),
            BundleGcOutcomeV1::SafeSkipped {
                reason_code: "bundle_inventory_invalid".into()
            }
        );

        let item = plan
            .removals
            .iter()
            .find(|item| item.entry.bundle_id == "changed")
            .unwrap();
        let mut changed_inventory = inventory.clone();
        changed_inventory
            .entries
            .iter_mut()
            .find(|entry| entry.bundle_id == "changed")
            .unwrap()
            .content_sha256 = digest('e');
        changed_inventory.observed_at_ms = now + 9;
        let mut changed_probe = fixed_bundle_inventory_probe(changed_inventory);
        assert_eq!(
            execute_bundle_gc_item(
                &owner,
                &mut opened.journal,
                &mut state,
                &store,
                &inventory,
                &mut changed_probe,
                item,
                now + 9,
                now + 10,
                BundleGcFailpointV1::None,
            )
            .unwrap(),
            BundleGcOutcomeV1::SafeSkipped {
                reason_code: "bundle_identity_changed".into()
            }
        );

        let item = plan
            .removals
            .iter()
            .find(|item| item.entry.bundle_id == "omitted")
            .unwrap();
        let mut omitted_inventory = inventory.clone();
        omitted_inventory.observed_at_ms = now + 11;
        omitted_inventory
            .entries
            .retain(|entry| entry.bundle_id != "omitted");
        let mut omitted_probe = fixed_bundle_inventory_probe(omitted_inventory);
        assert_eq!(
            execute_bundle_gc_item(
                &owner,
                &mut opened.journal,
                &mut state,
                &store,
                &inventory,
                &mut omitted_probe,
                item,
                now + 11,
                now + 12,
                BundleGcFailpointV1::None,
            )
            .unwrap(),
            BundleGcOutcomeV1::SafeSkipped {
                reason_code: "inventory_target_missing".into()
            }
        );
        for bundle_id in ["invalid", "changed", "omitted"] {
            assert!(bundle_root
                .path()
                .join(format!("{bundle_id}.bundle"))
                .exists());
        }
    }

    fn image(digest_char: char, ownership: RuntimeImageOwnershipV1) -> RuntimeImageV1 {
        RuntimeImageV1 {
            digest: format!("sha256:{}", digest(digest_char)),
            ownership,
        }
    }

    fn image_inventory() -> RuntimeImageInventoryV1 {
        RuntimeImageInventoryV1 {
            observed_at_ms: BASE + 1,
            images: vec![
                image('1', RuntimeImageOwnershipV1::BridgeManaged),
                image('2', RuntimeImageOwnershipV1::BridgeManaged),
                image('3', RuntimeImageOwnershipV1::BridgeManaged),
                image('4', RuntimeImageOwnershipV1::BridgeManaged),
                image('5', RuntimeImageOwnershipV1::BridgeManaged),
                image('6', RuntimeImageOwnershipV1::BridgeManaged),
                image('7', RuntimeImageOwnershipV1::BridgeManaged),
                image('8', RuntimeImageOwnershipV1::Unrelated),
            ],
            current_production_digests: BTreeSet::from([format!("sha256:{}", digest('1'))]),
            pinned_digests: BTreeSet::from([format!("sha256:{}", digest('2'))]),
            successful_candidates: vec![
                SuccessfulImageCandidateV1 {
                    provider_id: "codex".into(),
                    digest: format!("sha256:{}", digest('3')),
                    succeeded_at_ms: BASE - 1,
                },
                SuccessfulImageCandidateV1 {
                    provider_id: "codex".into(),
                    digest: format!("sha256:{}", digest('4')),
                    succeeded_at_ms: BASE - 2,
                },
                SuccessfulImageCandidateV1 {
                    provider_id: "codex".into(),
                    digest: format!("sha256:{}", digest('5')),
                    succeeded_at_ms: BASE - 3,
                },
            ],
            container_references: vec![
                ContainerImageReferenceV1 {
                    container_id: "running-1".into(),
                    digest: format!("sha256:{}", digest('6')),
                    state: ContainerLifecycleV1::Running,
                },
                ContainerImageReferenceV1 {
                    container_id: "stopped-1".into(),
                    digest: format!("sha256:{}", digest('7')),
                    state: ContainerLifecycleV1::Stopped,
                },
            ],
        }
    }

    #[test]
    fn image_plan_keeps_production_two_latest_pins_all_containers_and_unrelated_images() {
        let plan = plan_image_gc(&image_inventory(), BASE + 2).unwrap();
        assert_eq!(
            plan.removals
                .iter()
                .map(|item| item.digest.as_str())
                .collect::<Vec<_>>(),
            vec![format!("sha256:{}", digest('5'))]
        );
        assert!(plan.protected.iter().any(|item| {
            item.digest == format!("sha256:{}", digest('6'))
                && item.reason == ImageGcProtectionV1::ContainerReference
        }));
        assert!(plan.protected.iter().any(|item| {
            item.digest == format!("sha256:{}", digest('7'))
                && item.reason == ImageGcProtectionV1::ContainerReference
        }));
        assert!(plan.protected.iter().any(|item| {
            item.digest == format!("sha256:{}", digest('8'))
                && item.reason == ImageGcProtectionV1::Unrelated
        }));
    }

    struct FakeImageRuntime {
        inventory: RuntimeImageInventoryV1,
        inventory_error: bool,
        remove_error: bool,
        refuse_removal: bool,
        remove_calls: Vec<String>,
    }

    impl RuntimeImageEffectsV1 for FakeImageRuntime {
        fn inventory_all(&mut self) -> Result<RuntimeImageInventoryV1, crate::BoxError> {
            if self.inventory_error {
                Err("injected runtime inventory failure".into())
            } else {
                Ok(self.inventory.clone())
            }
        }

        fn remove_exact_digest(
            &mut self,
            digest: &str,
        ) -> Result<RuntimeImageRemovalV1, crate::BoxError> {
            self.remove_calls.push(digest.into());
            if self.remove_error {
                return Err("injected runtime removal failure".into());
            }
            if self.refuse_removal {
                return Ok(RuntimeImageRemovalV1::Refused {
                    reason_code: "Image is in use by container 123".into(),
                });
            }
            if let Some(index) = self
                .inventory
                .images
                .iter()
                .position(|image| image.digest == digest)
            {
                self.inventory.images.remove(index);
                Ok(RuntimeImageRemovalV1::Removed)
            } else {
                Ok(RuntimeImageRemovalV1::Absent)
            }
        }
    }

    #[test]
    fn image_gc_requeries_before_effect_and_safely_skips_new_stopped_reference_or_error() {
        let _gc_test_guard = gc_test_guard();
        let fixture = Fixture::new();
        let initial = image_inventory();
        let plan = plan_image_gc(&initial, BASE + 2).unwrap();
        let item = &plan.removals[0];
        let owner = fixture
            .scheduler
            .try_owner_admission("test/r3d3d-image-race")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let mut state = opened.snapshot.state.clone();
        let mut fresh = initial.clone();
        fresh.container_references.push(ContainerImageReferenceV1 {
            container_id: "newly-stopped".into(),
            digest: item.digest.clone(),
            state: ContainerLifecycleV1::Stopped,
        });
        fresh.observed_at_ms = BASE + 3;
        let mut runtime = FakeImageRuntime {
            inventory: fresh,
            inventory_error: false,
            remove_error: false,
            refuse_removal: false,
            remove_calls: Vec::new(),
        };
        assert_eq!(
            execute_image_gc_item(
                &owner,
                &mut opened.journal,
                &mut state,
                &mut runtime,
                Some(&initial),
                item,
                BASE + 3,
                BASE + 4,
                ImageGcFailpointV1::None,
            )
            .unwrap(),
            ImageGcOutcomeV1::SafeSkipped {
                reason_code: "freshly_protected".into()
            }
        );
        assert!(runtime.remove_calls.is_empty());

        let remove_error_plan = plan_image_gc(&initial, BASE + 30).unwrap();
        let remove_error_item = &remove_error_plan.removals[0];
        runtime.inventory = initial.clone();
        runtime.inventory.observed_at_ms = BASE + 31;
        runtime.remove_error = true;
        assert_eq!(
            execute_image_gc_item(
                &owner,
                &mut opened.journal,
                &mut state,
                &mut runtime,
                Some(&initial),
                remove_error_item,
                BASE + 31,
                BASE + 32,
                ImageGcFailpointV1::None,
            )
            .unwrap(),
            ImageGcOutcomeV1::SafeSkipped {
                reason_code: "runtime_remove_failed".into()
            }
        );
        assert_eq!(runtime.remove_calls, vec![remove_error_item.digest.clone()]);

        let error_plan = plan_image_gc(&initial, BASE + 40).unwrap();
        let error_item = &error_plan.removals[0];
        runtime.inventory_error = true;
        assert_eq!(
            execute_image_gc_item(
                &owner,
                &mut opened.journal,
                &mut state,
                &mut runtime,
                Some(&initial),
                error_item,
                BASE + 41,
                BASE + 42,
                ImageGcFailpointV1::None,
            )
            .unwrap(),
            ImageGcOutcomeV1::SafeSkipped {
                reason_code: "runtime_inventory_failed".into()
            }
        );
        assert_eq!(runtime.remove_calls, vec![remove_error_item.digest.clone()]);

        let future_plan = plan_image_gc(&initial, BASE + 50).unwrap();
        let future_item = &future_plan.removals[0];
        runtime.inventory_error = false;
        runtime.inventory = initial.clone();
        runtime.inventory.observed_at_ms = BASE + 53;
        assert_eq!(
            execute_image_gc_item(
                &owner,
                &mut opened.journal,
                &mut state,
                &mut runtime,
                Some(&initial),
                future_item,
                BASE + 51,
                BASE + 52,
                ImageGcFailpointV1::None,
            )
            .unwrap(),
            ImageGcOutcomeV1::SafeSkipped {
                reason_code: "runtime_inventory_invalid".into()
            }
        );
        assert_eq!(runtime.remove_calls, vec![remove_error_item.digest.clone()]);
    }

    #[test]
    fn image_gc_recovers_a_crash_after_exact_digest_removal_without_touching_other_images() {
        let _gc_test_guard = gc_test_guard();
        let fixture = Fixture::new();
        let inventory = image_inventory();
        let plan = plan_image_gc(&inventory, BASE + 2).unwrap();
        let item = &plan.removals[0];
        let owner = fixture
            .scheduler
            .try_owner_admission("test/r3d3d-image-crash")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let mut state = opened.snapshot.state.clone();
        let mut runtime_inventory = inventory.clone();
        runtime_inventory.observed_at_ms = BASE + 3;
        let mut runtime = FakeImageRuntime {
            inventory: runtime_inventory,
            inventory_error: false,
            remove_error: false,
            refuse_removal: false,
            remove_calls: Vec::new(),
        };
        assert!(execute_image_gc_item(
            &owner,
            &mut opened.journal,
            &mut state,
            &mut runtime,
            Some(&inventory),
            item,
            BASE + 3,
            BASE + 4,
            ImageGcFailpointV1::AfterRemoval,
        )
        .is_err());
        assert_eq!(runtime.remove_calls, vec![item.digest.clone()]);
        assert_eq!(runtime.inventory.images.len(), 7);
        runtime.inventory.observed_at_ms = BASE + 5;
        assert_eq!(
            execute_image_gc_item(
                &owner,
                &mut opened.journal,
                &mut state,
                &mut runtime,
                None,
                item,
                BASE + 5,
                BASE + 6,
                ImageGcFailpointV1::None,
            )
            .unwrap(),
            ImageGcOutcomeV1::RecoveredAlreadyAbsent
        );
        assert_eq!(runtime.remove_calls, vec![item.digest.clone()]);
    }

    #[test]
    fn image_gc_normalizes_runtime_refusal_and_rejects_forged_intent_before_effect() {
        let _gc_test_guard = gc_test_guard();
        let fixture = Fixture::new();
        let inventory = image_inventory();
        let plan = plan_image_gc(&inventory, BASE + 2).unwrap();
        let item = &plan.removals[0];
        let owner = fixture
            .scheduler
            .try_owner_admission("test/r3d3d-image-refusal")
            .unwrap();
        let mut opened = FileEvidenceJournal::open_existing(&owner).unwrap();
        let mut state = opened.snapshot.state.clone();
        let mut runtime_inventory = inventory.clone();
        runtime_inventory.observed_at_ms = BASE + 3;
        let mut runtime = FakeImageRuntime {
            inventory: runtime_inventory,
            inventory_error: false,
            remove_error: false,
            refuse_removal: true,
            remove_calls: Vec::new(),
        };

        assert_eq!(
            execute_image_gc_item(
                &owner,
                &mut opened.journal,
                &mut state,
                &mut runtime,
                Some(&inventory),
                item,
                BASE + 3,
                BASE + 4,
                ImageGcFailpointV1::None,
            )
            .unwrap(),
            ImageGcOutcomeV1::SafeSkipped {
                reason_code: "runtime_remove_refused".into()
            }
        );
        assert_eq!(runtime.remove_calls, vec![item.digest.clone()]);
        assert!(matches!(
            state.image_gc_actions[&item.action_id].lifecycle,
            crate::compatibility_schedule_evidence::ImageGcLifecycleV1::SafeSkipped { .. }
        ));

        let mut forged = plan.removals[0].clone();
        forged.inventory_sha256 = digest('a');
        forged.action_id = format!(
            "image-gc:{}",
            local_file::sha256_hex(
                format!(
                    "{}\0{}\0{}",
                    forged.inventory_sha256, forged.digest, forged.planned_at_ms
                )
                .as_bytes()
            )
        );
        let mut forged_inventory = inventory.clone();
        forged_inventory.observed_at_ms = BASE + 5;
        let mut forged_runtime = FakeImageRuntime {
            inventory: forged_inventory,
            inventory_error: false,
            remove_error: false,
            refuse_removal: false,
            remove_calls: Vec::new(),
        };
        let action_count = state.image_gc_actions.len();
        let error = execute_image_gc_item(
            &owner,
            &mut opened.journal,
            &mut state,
            &mut forged_runtime,
            Some(&inventory),
            &forged,
            BASE + 5,
            BASE + 6,
            ImageGcFailpointV1::None,
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("image GC planned inventory binding changed"));
        assert!(forged_runtime.remove_calls.is_empty());
        assert_eq!(state.image_gc_actions.len(), action_count);
    }
}
