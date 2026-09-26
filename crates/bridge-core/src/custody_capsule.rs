//! Provider-free custody capsule contracts.
//!
//! This module defines canonical records and pure validation only. It has no filesystem, Git,
//! encryption, decryption, provider, network, restore, or source-discovery API.

use std::collections::{BTreeMap, BTreeSet};

use ring::digest;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::custody_inventory::{CustodyStateClassV1, LosslessPathV1};
use crate::custody_seal::{
    CustodyCoverageClassV1, CustodyGitObjectKindV1, CustodyManifestV1, CustodySealV1,
    CustodySealabilityV1, CustodySealedArtifactV1,
};
use crate::execution_policy::Sha256HexV1;

const INDEX_SCHEMA_V1: &str = "custody-capsule-index.v1";
const RESTORE_POLICY_SCHEMA_V1: &str = "custody-restore-policy.v1";
const ENVELOPE_FORMAT_SCHEMA_V1: &str = "custody-envelope-format.v1";
const ENVELOPE_CONTEXT_SCHEMA_V1: &str = "custody-envelope-context.v1";
const MAX_CANONICAL_JSON_BYTES_V1: usize = 1024 * 1024;
const MAX_ARTIFACT_NAME_BYTES_V1: usize = 4096;
const MAX_ENVELOPE_FIELD_BYTES_V1: usize = 4096;
const MAX_ENVELOPE_RECIPIENTS_V1: usize = 64;
const MAX_ENVELOPE_RECIPIENT_BYTES_V1: usize = 262_144;
const MAX_ENVELOPE_METADATA_ROWS_V1: usize = 64;
const MAX_ENVELOPE_METADATA_BYTES_V1: usize = 524_288;
const MAX_ENVELOPE_CHUNK_BYTES_V1: u64 = 1024 * 1024;
const MAX_ENVELOPE_CHUNKS_V1: u32 = 16_384;
const MAX_ENVELOPE_TOTAL_BYTES_V1: u64 = 10_737_418_240;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CustodyCapsuleArtifactRoleRowWireV1 {
    name: LosslessPathV1,
    role: CustodyCapsuleArtifactRoleV1,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "CustodyCapsuleArtifactRoleRowWireV1")]
pub struct CustodyCapsuleArtifactRoleRowV1 {
    name: LosslessPathV1,
    role: CustodyCapsuleArtifactRoleV1,
}

impl TryFrom<CustodyCapsuleArtifactRoleRowWireV1> for CustodyCapsuleArtifactRoleRowV1 {
    type Error = CustodyCapsuleErrorV1;

    fn try_from(value: CustodyCapsuleArtifactRoleRowWireV1) -> Result<Self, Self::Error> {
        Self::new(value.name, value.role)
    }
}

impl CustodyCapsuleArtifactRoleRowV1 {
    pub fn new(
        name: LosslessPathV1,
        role: CustodyCapsuleArtifactRoleV1,
    ) -> Result<Self, CustodyCapsuleErrorV1> {
        validate_artifact_name(name.as_bytes())?;
        validate_reserved_name_for_role(&name, &role)?;
        Ok(Self { name, role })
    }

    #[must_use]
    pub fn name(&self) -> &LosslessPathV1 {
        &self.name
    }

    #[must_use]
    pub const fn role(&self) -> &CustodyCapsuleArtifactRoleV1 {
        &self.role
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "role", content = "coverage_class", rename_all = "snake_case")]
pub enum CustodyCapsuleArtifactRoleV1 {
    Manifest,
    CapsuleIndex,
    RestorePolicy,
    GitObjectPack,
    CoveragePayload(CustodyCoverageClassV1),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CustodyCapsuleIndexWireV1 {
    schema: String,
    manifest_digest: Sha256HexV1,
    artifacts: Vec<CustodyCapsuleArtifactRoleRowV1>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CustodyCapsuleIndexWireV1")]
pub struct CustodyCapsuleIndexV1 {
    schema: String,
    manifest_digest: Sha256HexV1,
    artifacts: Vec<CustodyCapsuleArtifactRoleRowV1>,
}

impl TryFrom<CustodyCapsuleIndexWireV1> for CustodyCapsuleIndexV1 {
    type Error = CustodyCapsuleErrorV1;

    fn try_from(value: CustodyCapsuleIndexWireV1) -> Result<Self, Self::Error> {
        if value.schema != INDEX_SCHEMA_V1 {
            return Err(CustodyCapsuleErrorV1::WrongSchema);
        }
        Self::new(value.manifest_digest, value.artifacts)
    }
}

impl CustodyCapsuleIndexV1 {
    pub fn new(
        manifest_digest: Sha256HexV1,
        artifacts: Vec<CustodyCapsuleArtifactRoleRowV1>,
    ) -> Result<Self, CustodyCapsuleErrorV1> {
        let artifacts = canonical_role_rows(artifacts)?;
        Ok(Self {
            schema: INDEX_SCHEMA_V1.to_owned(),
            manifest_digest,
            artifacts,
        })
    }

    pub fn validate(&self) -> Result<(), CustodyCapsuleErrorV1> {
        if self.schema != INDEX_SCHEMA_V1 {
            return Err(CustodyCapsuleErrorV1::WrongSchema);
        }
        let canonical = Self::new(self.manifest_digest.clone(), self.artifacts.clone())?;
        if canonical != *self {
            return Err(CustodyCapsuleErrorV1::NonCanonicalRecord);
        }
        Ok(())
    }

    pub fn encode_canonical(&self) -> Result<Vec<u8>, CustodyCapsuleErrorV1> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|_| CustodyCapsuleErrorV1::CanonicalEncoding)
    }

    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, CustodyCapsuleErrorV1> {
        validate_canonical_decode_size(bytes)?;
        let wire: CustodyCapsuleIndexWireV1 =
            serde_json::from_slice(bytes).map_err(|_| CustodyCapsuleErrorV1::InvalidEncoding)?;
        let value = Self::try_from(wire)?;
        if value.encode_canonical()? != bytes {
            return Err(CustodyCapsuleErrorV1::NonCanonicalEncoding);
        }
        Ok(value)
    }

    pub fn content_digest(&self) -> Result<Sha256HexV1, CustodyCapsuleErrorV1> {
        Ok(Sha256HexV1::digest(&self.encode_canonical()?))
    }

    #[must_use]
    pub fn manifest_digest(&self) -> &Sha256HexV1 {
        &self.manifest_digest
    }

    #[must_use]
    pub fn artifacts(&self) -> &[CustodyCapsuleArtifactRoleRowV1] {
        &self.artifacts
    }
}

/// The derived 2B1 capsule layout: the complete exterior artifact population for one manifest.
///
/// ADR-0041 slice 2B2 control 24a. The doctest below has exactly one statement, so module privacy
/// is its single barrier: `lib.rs` declares the exporter as `mod custody_export;`, which no
/// external crate can name. Changing only that declaration to `pub mod custody_export;` makes this
/// doctest compile and so turns the control red. It deliberately does not mention any item inside
/// the module, so function privacy cannot mask the module barrier.
///
/// ```compile_fail
/// use bridge_core::custody_export;
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustodyCapsuleLayoutV1 {
    index: CustodyCapsuleIndexV1,
}

impl CustodyCapsuleLayoutV1 {
    pub fn derive(manifest: &CustodyManifestV1) -> Result<Self, CustodyCapsuleErrorV1> {
        manifest
            .validate()
            .map_err(|_| CustodyCapsuleErrorV1::InvalidInput)?;
        if !matches!(manifest.sealability(), CustodySealabilityV1::Sealable) {
            return Err(CustodyCapsuleErrorV1::NonSealableManifest);
        }
        if manifest
            .original_objects()
            .iter()
            .any(|object| object.kind() == CustodyGitObjectKindV1::Unknown)
        {
            return Err(CustodyCapsuleErrorV1::UnknownObjectKind);
        }

        let object_database_state = manifest
            .coverage()
            .iter()
            .find(|row| row.class() == CustodyCoverageClassV1::ObjectDatabase)
            .ok_or(CustodyCapsuleErrorV1::CoverageStateMismatch)?
            .state();
        let object_inventory_is_empty = manifest.original_objects().is_empty();
        match (object_database_state, object_inventory_is_empty) {
            (CustodyStateClassV1::Captured, false) | (CustodyStateClassV1::Empty, true) => {}
            (CustodyStateClassV1::ExcludedReproducible, _) => {
                return Err(CustodyCapsuleErrorV1::CoverageStateMismatch);
            }
            (CustodyStateClassV1::Unresolved, _) => {
                return Err(CustodyCapsuleErrorV1::NonSealableManifest);
            }
            (CustodyStateClassV1::Captured, true) | (CustodyStateClassV1::Empty, false) => {
                return Err(CustodyCapsuleErrorV1::CoverageStateMismatch);
            }
        }

        let mut candidates = vec![
            ArtifactRoleCandidateV1::new(
                "control/manifest.json.enc",
                CustodyCapsuleArtifactRoleV1::Manifest,
            ),
            ArtifactRoleCandidateV1::new(
                "control/capsule-index.json.enc",
                CustodyCapsuleArtifactRoleV1::CapsuleIndex,
            ),
            ArtifactRoleCandidateV1::new(
                "control/restore-policy.json.enc",
                CustodyCapsuleArtifactRoleV1::RestorePolicy,
            ),
        ];

        for coverage in manifest.coverage() {
            match (coverage.class(), coverage.state()) {
                (CustodyCoverageClassV1::ObjectDatabase, CustodyStateClassV1::Captured) => {
                    candidates.push(ArtifactRoleCandidateV1::new(
                        "git/objects.pack.enc",
                        CustodyCapsuleArtifactRoleV1::GitObjectPack,
                    ));
                }
                (CustodyCoverageClassV1::ObjectDatabase, CustodyStateClassV1::Empty) => {}
                (CustodyCoverageClassV1::ObjectDatabase, _) => {
                    return Err(CustodyCapsuleErrorV1::CoverageStateMismatch);
                }
                (class, CustodyStateClassV1::Captured) => {
                    candidates.push(ArtifactRoleCandidateV1::new(
                        format!("payload/{}.bin.enc", coverage_class_name(class)),
                        CustodyCapsuleArtifactRoleV1::CoveragePayload(class),
                    ));
                }
                (_, CustodyStateClassV1::Empty | CustodyStateClassV1::ExcludedReproducible) => {}
                (_, CustodyStateClassV1::Unresolved) => {
                    return Err(CustodyCapsuleErrorV1::NonSealableManifest);
                }
            }
        }

        validate_candidate_ownership(&candidates)?;
        let rows = candidates
            .into_iter()
            .map(ArtifactRoleCandidateV1::into_row)
            .collect::<Result<Vec<_>, _>>()?;
        let index = CustodyCapsuleIndexV1::new(
            manifest
                .content_digest()
                .map_err(|_| CustodyCapsuleErrorV1::InvalidInput)?,
            rows,
        )?;
        let total = index.artifacts().len();
        if (manifest
            .coverage()
            .iter()
            .all(|row| row.state() == CustodyStateClassV1::Empty)
            && total != 3)
            || (manifest
                .coverage()
                .iter()
                .all(|row| row.state() == CustodyStateClassV1::Captured)
                && total != 17)
        {
            return Err(CustodyCapsuleErrorV1::DerivedLayoutMismatch);
        }
        Ok(Self { index })
    }

    #[must_use]
    pub fn index(&self) -> &CustodyCapsuleIndexV1 {
        &self.index
    }

    #[must_use]
    pub fn artifact_count(&self) -> usize {
        self.index.artifacts().len()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CustodyRestorePolicyWireV1 {
    schema: String,
    hooks: RestoreDisabledV1,
    executable_config_includes: RestoreDisabledV1,
    filters: RestoreDisabledV1,
    network_lazy_fetch: RestoreDisabledV1,
    archived_external_path_activation: RestoreDisabledV1,
    workflow_resume: RestoreDisabledV1,
    source_project_ref_mutation: RestoreForbiddenV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CustodyRestorePolicyWireV1")]
pub struct CustodyRestorePolicyV1 {
    schema: String,
    hooks: RestoreDisabledV1,
    executable_config_includes: RestoreDisabledV1,
    filters: RestoreDisabledV1,
    network_lazy_fetch: RestoreDisabledV1,
    archived_external_path_activation: RestoreDisabledV1,
    workflow_resume: RestoreDisabledV1,
    source_project_ref_mutation: RestoreForbiddenV1,
}

impl TryFrom<CustodyRestorePolicyWireV1> for CustodyRestorePolicyV1 {
    type Error = CustodyCapsuleErrorV1;

    fn try_from(value: CustodyRestorePolicyWireV1) -> Result<Self, Self::Error> {
        if value.schema != RESTORE_POLICY_SCHEMA_V1 {
            return Err(CustodyCapsuleErrorV1::WrongSchema);
        }
        Self::new(
            value.hooks,
            value.executable_config_includes,
            value.filters,
            value.network_lazy_fetch,
            value.archived_external_path_activation,
            value.workflow_resume,
            value.source_project_ref_mutation,
        )
    }
}

impl CustodyRestorePolicyV1 {
    pub fn inert() -> Self {
        Self::default()
    }

    pub fn new(
        hooks: RestoreDisabledV1,
        executable_config_includes: RestoreDisabledV1,
        filters: RestoreDisabledV1,
        network_lazy_fetch: RestoreDisabledV1,
        archived_external_path_activation: RestoreDisabledV1,
        workflow_resume: RestoreDisabledV1,
        source_project_ref_mutation: RestoreForbiddenV1,
    ) -> Result<Self, CustodyCapsuleErrorV1> {
        if [
            hooks,
            executable_config_includes,
            filters,
            network_lazy_fetch,
            archived_external_path_activation,
            workflow_resume,
        ]
        .into_iter()
        .any(|value| value != RestoreDisabledV1::Disabled)
            || source_project_ref_mutation != RestoreForbiddenV1::Forbidden
        {
            return Err(CustodyCapsuleErrorV1::UnsupportedRestoreBehavior);
        }
        Ok(Self {
            schema: RESTORE_POLICY_SCHEMA_V1.to_owned(),
            hooks,
            executable_config_includes,
            filters,
            network_lazy_fetch,
            archived_external_path_activation,
            workflow_resume,
            source_project_ref_mutation,
        })
    }

    pub fn validate(&self) -> Result<(), CustodyCapsuleErrorV1> {
        if self.schema != RESTORE_POLICY_SCHEMA_V1 {
            return Err(CustodyCapsuleErrorV1::WrongSchema);
        }
        let canonical = Self::new(
            self.hooks,
            self.executable_config_includes,
            self.filters,
            self.network_lazy_fetch,
            self.archived_external_path_activation,
            self.workflow_resume,
            self.source_project_ref_mutation,
        )?;
        if canonical != *self {
            return Err(CustodyCapsuleErrorV1::NonCanonicalRecord);
        }
        Ok(())
    }

    pub fn encode_canonical(&self) -> Result<Vec<u8>, CustodyCapsuleErrorV1> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|_| CustodyCapsuleErrorV1::CanonicalEncoding)
    }

    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, CustodyCapsuleErrorV1> {
        validate_canonical_decode_size(bytes)?;
        let wire: CustodyRestorePolicyWireV1 =
            serde_json::from_slice(bytes).map_err(|_| CustodyCapsuleErrorV1::InvalidEncoding)?;
        let value = Self::try_from(wire)?;
        if value.encode_canonical()? != bytes {
            return Err(CustodyCapsuleErrorV1::NonCanonicalEncoding);
        }
        Ok(value)
    }

    pub fn content_digest(&self) -> Result<Sha256HexV1, CustodyCapsuleErrorV1> {
        Ok(Sha256HexV1::digest(&self.encode_canonical()?))
    }
}

impl Default for CustodyRestorePolicyV1 {
    fn default() -> Self {
        Self::new(
            RestoreDisabledV1::Disabled,
            RestoreDisabledV1::Disabled,
            RestoreDisabledV1::Disabled,
            RestoreDisabledV1::Disabled,
            RestoreDisabledV1::Disabled,
            RestoreDisabledV1::Disabled,
            RestoreForbiddenV1::Forbidden,
        )
        .expect("the closed inert restore policy is valid")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreDisabledV1 {
    Disabled,
    Enabled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreForbiddenV1 {
    Forbidden,
    Allowed,
}

/// The receipt-derived capsule seal proof: the only value the binding and the open request accept.
///
/// Deferred 2B1 control (retained through the 2B1 provenance review), added before 2B2 widens any
/// capsule API: a generic [`CustodySealV1`] — one assembled from caller-chosen artifact rows rather
/// than derived from sink receipts — cannot substitute for this proof. Each doctest below builds
/// an otherwise valid call whose only error is that substitution, so reverting the one signature
/// it targets to take `&CustodySealV1` makes exactly that doctest compile and so turns it red.
///
/// ```compile_fail
/// use bridge_core::custody_capsule::{
///     CustodyCapsuleBindingV1, CustodyCapsuleLayoutV1, CustodyRestorePolicyV1,
/// };
/// use bridge_core::custody_inventory::CustodyStateClassV1;
/// use bridge_core::custody_seal::{
///     CustodyCoverageClassV1, CustodyCoverageEntryV1, CustodyManifestV1, CustodySealV1,
///     CustodySealedArtifactV1,
/// };
///
/// let coverage = CustodyCoverageClassV1::ALL
///     .into_iter()
///     .map(|class| {
///         CustodyCoverageEntryV1::new(class, CustodyStateClassV1::Empty, vec![], None).unwrap()
///     })
///     .collect();
/// let manifest = CustodyManifestV1::new(
///     "unit", "run", "materialization", "generation", coverage, vec![], vec![], vec![], vec![],
/// )
/// .unwrap();
/// let layout = CustodyCapsuleLayoutV1::derive(&manifest).unwrap();
/// let digest = manifest.content_digest().unwrap();
/// let rows = layout
///     .index()
///     .artifacts()
///     .iter()
///     .map(|row| CustodySealedArtifactV1::new(row.name().clone(), 1, digest.clone()).unwrap())
///     .collect();
/// let generic = CustodySealV1::new(
///     digest, rows, vec!["recipient".to_owned()], "capsule-v1", "tool", "1",
/// )
/// .unwrap();
/// let _ = CustodyCapsuleBindingV1::new(
///     &manifest,
///     layout.index(),
///     &CustodyRestorePolicyV1::inert(),
///     &generic,
/// );
/// ```
///
/// ```compile_fail
/// use bridge_core::custody_capsule::{CustodyCapsuleLayoutV1, CustodyEnvelopeOpenRequestV1};
/// use bridge_core::custody_inventory::CustodyStateClassV1;
/// use bridge_core::custody_seal::{
///     CustodyCoverageClassV1, CustodyCoverageEntryV1, CustodyManifestV1, CustodySealV1,
///     CustodySealedArtifactV1,
/// };
///
/// let coverage = CustodyCoverageClassV1::ALL
///     .into_iter()
///     .map(|class| {
///         CustodyCoverageEntryV1::new(class, CustodyStateClassV1::Empty, vec![], None).unwrap()
///     })
///     .collect();
/// let manifest = CustodyManifestV1::new(
///     "unit", "run", "materialization", "generation", coverage, vec![], vec![], vec![], vec![],
/// )
/// .unwrap();
/// let layout = CustodyCapsuleLayoutV1::derive(&manifest).unwrap();
/// let digest = manifest.content_digest().unwrap();
/// let name = layout.index().artifacts()[0].name().clone();
/// let generic = CustodySealV1::new(
///     digest.clone(),
///     vec![CustodySealedArtifactV1::new(name.clone(), 1, digest).unwrap()],
///     vec!["recipient".to_owned()],
///     "capsule-v1",
///     "tool",
///     "1",
/// )
/// .unwrap();
/// let _ = CustodyEnvelopeOpenRequestV1::from_seal_artifact(&generic, name);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustodyCapsuleSealProofV1 {
    seal: CustodySealV1,
}

impl CustodyCapsuleSealProofV1 {
    pub fn from_receipts(
        receipts: Vec<CustodyEnvelopeSealReceiptV1>,
    ) -> Result<Self, CustodyCapsuleErrorV1> {
        let mut receipts = receipts.into_iter();
        let first = receipts
            .next()
            .ok_or(CustodyCapsuleErrorV1::MissingArtifact)?;
        let manifest_digest = first.manifest_digest().clone();
        let format = first.format().clone();
        let recipients = first.recipients().to_vec();
        let mut artifacts = vec![first.to_sealed_artifact()?];

        for receipt in receipts {
            if receipt.manifest_digest() != &manifest_digest
                || receipt.format() != &format
                || receipt.recipients() != recipients.as_slice()
                || receipt.ciphertext_length() == 0
            {
                return Err(CustodyCapsuleErrorV1::InvalidInput);
            }
            artifacts.push(receipt.to_sealed_artifact()?);
        }

        let seal = CustodySealV1::new(
            manifest_digest,
            artifacts,
            recipients,
            format.capsule_format(),
            format.sealing_tool(),
            format.sealing_tool_version(),
        )
        .map_err(|_| CustodyCapsuleErrorV1::InvalidInput)?;
        validate_capsule_seal_for_open_request(&seal)?;
        Ok(Self { seal })
    }

    pub fn preflight_generic_seal_for_capsule_v1(
        seal: &CustodySealV1,
    ) -> Result<(), CustodyCapsuleErrorV1> {
        validate_capsule_seal_for_open_request(seal)
    }

    pub fn preflight_generic_seal_artifact_for_capsule_v1(
        seal: &CustodySealV1,
        artifact_name: &LosslessPathV1,
    ) -> Result<(), CustodyCapsuleErrorV1> {
        validate_capsule_seal_for_open_request(seal)?;
        let artifact = seal
            .artifacts()
            .iter()
            .find(|artifact| artifact.name().as_bytes() == artifact_name.as_bytes())
            .ok_or(CustodyCapsuleErrorV1::MissingArtifact)?;
        validate_selected_seal_artifact_limits(artifact)?;
        if artifact.byte_length() == 0 {
            return Err(CustodyCapsuleErrorV1::InvalidInput);
        }
        Ok(())
    }

    #[must_use]
    pub fn seal(&self) -> &CustodySealV1 {
        &self.seal
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustodyCapsuleBindingV1 {
    manifest_digest: Sha256HexV1,
    index_digest: Sha256HexV1,
    seal_digest: Sha256HexV1,
    artifact_names: Vec<LosslessPathV1>,
}

impl CustodyCapsuleBindingV1 {
    pub fn new(
        manifest: &CustodyManifestV1,
        index: &CustodyCapsuleIndexV1,
        restore_policy: &CustodyRestorePolicyV1,
        seal_proof: &CustodyCapsuleSealProofV1,
    ) -> Result<Self, CustodyCapsuleErrorV1> {
        manifest
            .validate()
            .map_err(|_| CustodyCapsuleErrorV1::InvalidInput)?;
        index.validate()?;
        restore_policy.validate()?;
        let seal = seal_proof.seal();
        validate_capsule_seal_for_open_request(seal)?;

        let expected = CustodyCapsuleLayoutV1::derive(manifest)?;
        let manifest_digest = manifest
            .content_digest()
            .map_err(|_| CustodyCapsuleErrorV1::InvalidInput)?;
        if &manifest_digest != index.manifest_digest() || &manifest_digest != seal.manifest_digest()
        {
            return Err(CustodyCapsuleErrorV1::ThreeWayDigestMismatch);
        }
        compare_index_to_layout(index, expected.index())?;

        let index_names = index_name_set(index);
        let seal_names = seal_name_set(seal)?;
        if let Some(_missing_from_seal) = index_names.difference(&seal_names).next() {
            return Err(CustodyCapsuleErrorV1::MissingArtifact);
        }
        if let Some(_unmapped) = seal_names.difference(&index_names).next() {
            return Err(CustodyCapsuleErrorV1::UnmappedArtifact);
        }

        Ok(Self {
            manifest_digest,
            index_digest: index.content_digest()?,
            seal_digest: seal
                .content_digest()
                .map_err(|_| CustodyCapsuleErrorV1::InvalidInput)?,
            artifact_names: index
                .artifacts()
                .iter()
                .map(|row| row.name.clone())
                .collect(),
        })
    }

    #[must_use]
    pub fn manifest_digest(&self) -> &Sha256HexV1 {
        &self.manifest_digest
    }

    #[must_use]
    pub fn index_digest(&self) -> &Sha256HexV1 {
        &self.index_digest
    }

    #[must_use]
    pub fn seal_digest(&self) -> &Sha256HexV1 {
        &self.seal_digest
    }

    #[must_use]
    pub fn artifact_names(&self) -> &[LosslessPathV1] {
        &self.artifact_names
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CustodyEnvelopeFormatWireV1 {
    schema: String,
    capsule_format: String,
    sealing_tool: String,
    sealing_tool_version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CustodyEnvelopeFormatWireV1")]
pub struct CustodyEnvelopeFormatV1 {
    schema: String,
    capsule_format: String,
    sealing_tool: String,
    sealing_tool_version: String,
}

impl TryFrom<CustodyEnvelopeFormatWireV1> for CustodyEnvelopeFormatV1 {
    type Error = CustodyCapsuleErrorV1;

    fn try_from(value: CustodyEnvelopeFormatWireV1) -> Result<Self, Self::Error> {
        if value.schema != ENVELOPE_FORMAT_SCHEMA_V1 {
            return Err(CustodyCapsuleErrorV1::WrongSchema);
        }
        Self::new(
            value.capsule_format,
            value.sealing_tool,
            value.sealing_tool_version,
        )
    }
}

impl CustodyEnvelopeFormatV1 {
    pub fn new(
        capsule_format: impl Into<String>,
        sealing_tool: impl Into<String>,
        sealing_tool_version: impl Into<String>,
    ) -> Result<Self, CustodyCapsuleErrorV1> {
        let capsule_format = capsule_format.into();
        let sealing_tool = sealing_tool.into();
        let sealing_tool_version = sealing_tool_version.into();
        validate_envelope_field(&capsule_format)?;
        validate_envelope_field(&sealing_tool)?;
        validate_envelope_field(&sealing_tool_version)?;
        let value = Self {
            schema: ENVELOPE_FORMAT_SCHEMA_V1.to_owned(),
            capsule_format,
            sealing_tool,
            sealing_tool_version,
        };
        validate_persisted_record_size(&value)?;
        Ok(value)
    }

    pub fn from_seal(seal: &CustodySealV1) -> Result<Self, CustodyCapsuleErrorV1> {
        seal.validate()
            .map_err(|_| CustodyCapsuleErrorV1::InvalidInput)?;
        Self::new(
            seal.capsule_format(),
            seal.sealing_tool(),
            seal.sealing_tool_version(),
        )
    }

    pub fn validate(&self) -> Result<(), CustodyCapsuleErrorV1> {
        if self.schema != ENVELOPE_FORMAT_SCHEMA_V1 {
            return Err(CustodyCapsuleErrorV1::WrongSchema);
        }
        let canonical = Self::new(
            &self.capsule_format,
            &self.sealing_tool,
            &self.sealing_tool_version,
        )?;
        if canonical != *self {
            return Err(CustodyCapsuleErrorV1::NonCanonicalRecord);
        }
        Ok(())
    }

    pub fn encode_canonical(&self) -> Result<Vec<u8>, CustodyCapsuleErrorV1> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|_| CustodyCapsuleErrorV1::CanonicalEncoding)
    }

    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, CustodyCapsuleErrorV1> {
        validate_canonical_decode_size(bytes)?;
        let wire: CustodyEnvelopeFormatWireV1 =
            serde_json::from_slice(bytes).map_err(|_| CustodyCapsuleErrorV1::InvalidEncoding)?;
        let value = Self::try_from(wire)?;
        if value.encode_canonical()? != bytes {
            return Err(CustodyCapsuleErrorV1::NonCanonicalEncoding);
        }
        Ok(value)
    }

    pub fn content_digest(&self) -> Result<Sha256HexV1, CustodyCapsuleErrorV1> {
        Ok(Sha256HexV1::digest(&self.encode_canonical()?))
    }

    #[must_use]
    pub fn capsule_format(&self) -> &str {
        &self.capsule_format
    }

    #[must_use]
    pub fn sealing_tool(&self) -> &str {
        &self.sealing_tool
    }

    #[must_use]
    pub fn sealing_tool_version(&self) -> &str {
        &self.sealing_tool_version
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CustodyEnvelopeContextWireV1 {
    schema: String,
    artifact_name: LosslessPathV1,
    manifest_digest: Sha256HexV1,
    format: CustodyEnvelopeFormatV1,
    recipients: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CustodyEnvelopeContextWireV1")]
pub struct CustodyEnvelopeContextV1 {
    schema: String,
    artifact_name: LosslessPathV1,
    manifest_digest: Sha256HexV1,
    format: CustodyEnvelopeFormatV1,
    recipients: Vec<String>,
}

impl TryFrom<CustodyEnvelopeContextWireV1> for CustodyEnvelopeContextV1 {
    type Error = CustodyCapsuleErrorV1;

    fn try_from(value: CustodyEnvelopeContextWireV1) -> Result<Self, Self::Error> {
        if value.schema != ENVELOPE_CONTEXT_SCHEMA_V1 {
            return Err(CustodyCapsuleErrorV1::WrongSchema);
        }
        Self::new(
            value.artifact_name,
            value.manifest_digest,
            value.format,
            value.recipients,
        )
    }
}

impl CustodyEnvelopeContextV1 {
    pub fn new(
        artifact_name: LosslessPathV1,
        manifest_digest: Sha256HexV1,
        format: CustodyEnvelopeFormatV1,
        recipients: Vec<String>,
    ) -> Result<Self, CustodyCapsuleErrorV1> {
        validate_artifact_name(artifact_name.as_bytes())?;
        format.validate()?;
        let recipients = canonical_recipients(recipients)?;
        let value = Self {
            schema: ENVELOPE_CONTEXT_SCHEMA_V1.to_owned(),
            artifact_name,
            manifest_digest,
            format,
            recipients,
        };
        validate_persisted_record_size(&value)?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), CustodyCapsuleErrorV1> {
        if self.schema != ENVELOPE_CONTEXT_SCHEMA_V1 {
            return Err(CustodyCapsuleErrorV1::WrongSchema);
        }
        let canonical = Self::new(
            self.artifact_name.clone(),
            self.manifest_digest.clone(),
            self.format.clone(),
            self.recipients.clone(),
        )?;
        if canonical != *self {
            return Err(CustodyCapsuleErrorV1::NonCanonicalRecord);
        }
        Ok(())
    }

    pub fn encode_canonical(&self) -> Result<Vec<u8>, CustodyCapsuleErrorV1> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|_| CustodyCapsuleErrorV1::CanonicalEncoding)
    }

    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, CustodyCapsuleErrorV1> {
        validate_canonical_decode_size(bytes)?;
        let wire: CustodyEnvelopeContextWireV1 =
            serde_json::from_slice(bytes).map_err(|_| CustodyCapsuleErrorV1::InvalidEncoding)?;
        let value = Self::try_from(wire)?;
        if value.encode_canonical()? != bytes {
            return Err(CustodyCapsuleErrorV1::NonCanonicalEncoding);
        }
        Ok(value)
    }

    pub fn content_digest(&self) -> Result<Sha256HexV1, CustodyCapsuleErrorV1> {
        Ok(Sha256HexV1::digest(&self.encode_canonical()?))
    }

    #[must_use]
    pub fn artifact_name(&self) -> &LosslessPathV1 {
        &self.artifact_name
    }

    #[must_use]
    pub fn manifest_digest(&self) -> &Sha256HexV1 {
        &self.manifest_digest
    }

    #[must_use]
    pub fn format(&self) -> &CustodyEnvelopeFormatV1 {
        &self.format
    }

    #[must_use]
    pub fn recipients(&self) -> &[String] {
        &self.recipients
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustodyEnvelopeOpenRequestV1 {
    context: CustodyEnvelopeContextV1,
    ciphertext_length: u64,
    ciphertext_sha256: Sha256HexV1,
}

impl CustodyEnvelopeOpenRequestV1 {
    pub fn from_seal_artifact(
        seal_proof: &CustodyCapsuleSealProofV1,
        artifact_name: LosslessPathV1,
    ) -> Result<Self, CustodyCapsuleErrorV1> {
        let seal = seal_proof.seal();
        validate_capsule_seal_for_open_request(seal)?;
        let artifact = seal
            .artifacts()
            .iter()
            .find(|artifact| artifact.name().as_bytes() == artifact_name.as_bytes())
            .ok_or(CustodyCapsuleErrorV1::MissingArtifact)?;
        validate_selected_seal_artifact_limits(artifact)?;
        if artifact.byte_length() == 0 {
            return Err(CustodyCapsuleErrorV1::InvalidInput);
        }
        let context = CustodyEnvelopeContextV1::new(
            artifact.name().clone(),
            seal.manifest_digest().clone(),
            CustodyEnvelopeFormatV1::new(
                seal.capsule_format(),
                seal.sealing_tool(),
                seal.sealing_tool_version(),
            )?,
            seal.encryption_recipients().to_vec(),
        )?;
        Ok(Self {
            context,
            ciphertext_length: artifact.byte_length(),
            ciphertext_sha256: artifact.sha256().clone(),
        })
    }

    #[must_use]
    pub fn context(&self) -> &CustodyEnvelopeContextV1 {
        &self.context
    }

    #[must_use]
    pub const fn ciphertext_length(&self) -> u64 {
        self.ciphertext_length
    }

    #[must_use]
    pub fn ciphertext_sha256(&self) -> &Sha256HexV1 {
        &self.ciphertext_sha256
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustodyEnvelopeMetadataV1 {
    rows: BTreeMap<String, String>,
}

impl CustodyEnvelopeMetadataV1 {
    pub fn new(rows: BTreeMap<String, String>) -> Result<Self, CustodyCapsuleErrorV1> {
        if rows.len() > MAX_ENVELOPE_METADATA_ROWS_V1 {
            return Err(CustodyCapsuleErrorV1::InvalidInput);
        }
        let mut aggregate = 0usize;
        for (key, value) in &rows {
            validate_envelope_field(key)?;
            validate_envelope_field(value)?;
            aggregate = aggregate
                .checked_add(key.len())
                .and_then(|total| total.checked_add(value.len()))
                .ok_or(CustodyCapsuleErrorV1::InvalidInput)?;
            if aggregate > MAX_ENVELOPE_METADATA_BYTES_V1 {
                return Err(CustodyCapsuleErrorV1::InvalidInput);
            }
        }
        if serde_json::to_vec(&rows)
            .map_err(|_| CustodyCapsuleErrorV1::CanonicalEncoding)?
            .len()
            > MAX_CANONICAL_JSON_BYTES_V1
        {
            return Err(CustodyCapsuleErrorV1::InvalidInput);
        }
        Ok(Self { rows })
    }

    #[must_use]
    pub fn rows(&self) -> &BTreeMap<String, String> {
        &self.rows
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CustodyEnvelopeStreamLimitsV1 {
    max_total_bytes: u64,
    max_chunk_bytes: u64,
    max_chunks: u32,
}

impl CustodyEnvelopeStreamLimitsV1 {
    pub fn new(
        max_total_bytes: u64,
        max_chunk_bytes: u64,
        max_chunks: u32,
    ) -> Result<Self, CustodyCapsuleErrorV1> {
        if max_total_bytes == 0 || max_chunk_bytes == 0 || max_chunks == 0 {
            return Err(CustodyCapsuleErrorV1::InvalidInput);
        }
        let capacity = max_chunk_bytes
            .checked_mul(u64::from(max_chunks))
            .ok_or(CustodyCapsuleErrorV1::InvalidInput)?;
        if max_total_bytes > MAX_ENVELOPE_TOTAL_BYTES_V1
            || max_chunk_bytes > MAX_ENVELOPE_CHUNK_BYTES_V1
            || max_chunks > MAX_ENVELOPE_CHUNKS_V1
            || capacity < max_total_bytes
        {
            return Err(CustodyCapsuleErrorV1::InvalidInput);
        }
        Ok(Self {
            max_total_bytes,
            max_chunk_bytes,
            max_chunks,
        })
    }

    #[must_use]
    pub const fn max_total_bytes(&self) -> u64 {
        self.max_total_bytes
    }

    #[must_use]
    pub const fn max_chunk_bytes(&self) -> u64 {
        self.max_chunk_bytes
    }

    #[must_use]
    pub const fn max_chunks(&self) -> u32 {
        self.max_chunks
    }

    fn capacity(&self) -> Result<u64, CustodyCapsuleErrorV1> {
        self.max_chunk_bytes
            .checked_mul(u64::from(self.max_chunks))
            .ok_or(CustodyCapsuleErrorV1::InvalidInput)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustodyEnvelopeSourceDescriptorV1 {
    total_bytes: u64,
    limits: CustodyEnvelopeStreamLimitsV1,
}

impl CustodyEnvelopeSourceDescriptorV1 {
    pub fn new(
        total_bytes: u64,
        limits: CustodyEnvelopeStreamLimitsV1,
    ) -> Result<Self, CustodyCapsuleErrorV1> {
        if total_bytes > limits.max_total_bytes()
            || total_bytes > limits.capacity()?
            || total_bytes > MAX_ENVELOPE_TOTAL_BYTES_V1
        {
            return Err(CustodyCapsuleErrorV1::InvalidInput);
        }
        Ok(Self {
            total_bytes,
            limits,
        })
    }

    #[must_use]
    pub const fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    #[must_use]
    pub const fn limits(&self) -> CustodyEnvelopeStreamLimitsV1 {
        self.limits
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustodyEnvelopeChunkV1 {
    ordinal: u32,
    bytes: Vec<u8>,
    final_chunk: bool,
}

impl CustodyEnvelopeChunkV1 {
    pub fn new(
        ordinal: u32,
        bytes: Vec<u8>,
        final_chunk: bool,
    ) -> Result<Self, CustodyCapsuleErrorV1> {
        if u64::try_from(bytes.len()).map_err(|_| CustodyCapsuleErrorV1::InvalidInput)?
            > MAX_ENVELOPE_CHUNK_BYTES_V1
        {
            return Err(CustodyCapsuleErrorV1::InvalidInput);
        }
        Ok(Self {
            ordinal,
            bytes,
            final_chunk,
        })
    }

    #[must_use]
    pub const fn ordinal(&self) -> u32 {
        self.ordinal
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub const fn final_chunk(&self) -> bool {
        self.final_chunk
    }
}

/// Finalized receipt returned by the exact-total source validator.
///
/// The receipt cannot be constructed outside this module; callers may only clone values
/// returned by `CustodyEnvelopeSourceValidatorV1::finish`. Source receipts are intentionally
/// distinct from ciphertext sink completions and cannot authorize a capsule seal.
///
/// ```compile_fail
/// use bridge_core::custody_capsule::{
///     CustodyEnvelopeChunkV1, CustodyEnvelopeCiphertextReceiptV1,
///     CustodyEnvelopeSourceDescriptorV1, CustodyEnvelopeSourceValidatorV1,
///     CustodyEnvelopeStreamLimitsV1,
/// };
///
/// fn requires_ciphertext(_: CustodyEnvelopeCiphertextReceiptV1) {}
///
/// let limits = CustodyEnvelopeStreamLimitsV1::new(1, 1, 1).unwrap();
/// let descriptor = CustodyEnvelopeSourceDescriptorV1::new(1, limits).unwrap();
/// let mut source = CustodyEnvelopeSourceValidatorV1::new(descriptor);
/// source
///     .accept_chunk(&CustodyEnvelopeChunkV1::new(0, b"p".to_vec(), true).unwrap())
///     .unwrap();
/// requires_ciphertext(source.finish().unwrap());
/// ```
///
/// ```compile_fail
/// use bridge_core::custody_capsule::CustodyEnvelopeStreamReceiptV1;
/// use bridge_core::execution_policy::Sha256HexV1;
///
/// let _forged = CustodyEnvelopeStreamReceiptV1 {
///     total_bytes: 1,
///     sha256: Sha256HexV1::digest(b"forged"),
/// };
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustodyEnvelopeStreamReceiptV1 {
    total_bytes: u64,
    sha256: Sha256HexV1,
}

impl CustodyEnvelopeStreamReceiptV1 {
    #[must_use]
    pub const fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    #[must_use]
    pub fn sha256(&self) -> &Sha256HexV1 {
        &self.sha256
    }
}

/// Finalized completion returned by the budget-only ciphertext sink validator.
///
/// The completion is minted only by `CustodyEnvelopeSinkValidatorV1::finish` after observing
/// the exact ciphertext bytes accepted by the sink.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustodyEnvelopeCiphertextReceiptV1 {
    total_bytes: u64,
    sha256: Sha256HexV1,
}

impl CustodyEnvelopeCiphertextReceiptV1 {
    fn from_stream_receipt(receipt: CustodyEnvelopeStreamReceiptV1) -> Self {
        Self {
            total_bytes: receipt.total_bytes,
            sha256: receipt.sha256,
        }
    }

    #[must_use]
    pub const fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    #[must_use]
    pub fn sha256(&self) -> &Sha256HexV1 {
        &self.sha256
    }
}

struct CustodyEnvelopeStreamStateV1 {
    limits: CustodyEnvelopeStreamLimitsV1,
    declared_total: Option<u64>,
    next_ordinal: u32,
    observed_bytes: u64,
    chunk_count: u32,
    final_seen: bool,
    digest: digest::Context,
}

impl CustodyEnvelopeStreamStateV1 {
    fn new(limits: CustodyEnvelopeStreamLimitsV1, declared_total: Option<u64>) -> Self {
        Self {
            limits,
            declared_total,
            next_ordinal: 0,
            observed_bytes: 0,
            chunk_count: 0,
            final_seen: false,
            digest: digest::Context::new(&digest::SHA256),
        }
    }

    fn accept_chunk(
        &mut self,
        chunk: &CustodyEnvelopeChunkV1,
    ) -> Result<(), CustodyCapsuleErrorV1> {
        if self.final_seen
            || chunk.ordinal() != self.next_ordinal
            || self.chunk_count >= self.limits.max_chunks()
            || u64::try_from(chunk.bytes().len())
                .map_err(|_| CustodyCapsuleErrorV1::InvalidInput)?
                > self.limits.max_chunk_bytes()
        {
            return Err(CustodyCapsuleErrorV1::InvalidInput);
        }

        let chunk_len =
            u64::try_from(chunk.bytes().len()).map_err(|_| CustodyCapsuleErrorV1::InvalidInput)?;
        let new_total = self
            .observed_bytes
            .checked_add(chunk_len)
            .ok_or(CustodyCapsuleErrorV1::InvalidInput)?;
        if new_total > self.limits.max_total_bytes() {
            return Err(CustodyCapsuleErrorV1::InvalidInput);
        }
        if let Some(declared_total) = self.declared_total {
            if new_total > declared_total {
                return Err(CustodyCapsuleErrorV1::InvalidInput);
            }
        }

        if chunk_len == 0 {
            let valid_zero = self.observed_bytes == 0
                && self.chunk_count == 0
                && chunk.ordinal() == 0
                && chunk.final_chunk()
                && self.declared_total.unwrap_or(0) == 0;
            if !valid_zero {
                return Err(CustodyCapsuleErrorV1::InvalidInput);
            }
        } else if let Some(declared_total) = self.declared_total {
            if chunk.final_chunk() && new_total != declared_total {
                return Err(CustodyCapsuleErrorV1::InvalidInput);
            }
            if !chunk.final_chunk() && new_total == declared_total {
                return Err(CustodyCapsuleErrorV1::InvalidInput);
            }
        }

        self.digest.update(chunk.bytes());
        self.observed_bytes = new_total;
        self.chunk_count += 1;
        self.next_ordinal += 1;
        if chunk.final_chunk() {
            self.final_seen = true;
        }
        Ok(())
    }

    fn finish(self) -> Result<CustodyEnvelopeStreamReceiptV1, CustodyCapsuleErrorV1> {
        if !self.final_seen {
            return Err(CustodyCapsuleErrorV1::InvalidInput);
        }
        if let Some(declared_total) = self.declared_total {
            if self.observed_bytes != declared_total {
                return Err(CustodyCapsuleErrorV1::InvalidInput);
            }
        }
        Ok(CustodyEnvelopeStreamReceiptV1 {
            total_bytes: self.observed_bytes,
            sha256: sha256_from_context(self.digest),
        })
    }
}

pub struct CustodyEnvelopeSourceValidatorV1 {
    state: CustodyEnvelopeStreamStateV1,
}

impl CustodyEnvelopeSourceValidatorV1 {
    pub fn new(descriptor: CustodyEnvelopeSourceDescriptorV1) -> Self {
        Self {
            state: CustodyEnvelopeStreamStateV1::new(
                descriptor.limits(),
                Some(descriptor.total_bytes()),
            ),
        }
    }

    pub fn accept_chunk(
        &mut self,
        chunk: &CustodyEnvelopeChunkV1,
    ) -> Result<(), CustodyCapsuleErrorV1> {
        self.state.accept_chunk(chunk)
    }

    pub fn finish(self) -> Result<CustodyEnvelopeStreamReceiptV1, CustodyCapsuleErrorV1> {
        self.state.finish()
    }
}

pub struct CustodyEnvelopeSinkValidatorV1 {
    state: CustodyEnvelopeStreamStateV1,
}

impl CustodyEnvelopeSinkValidatorV1 {
    pub fn new(limits: CustodyEnvelopeStreamLimitsV1) -> Self {
        Self {
            state: CustodyEnvelopeStreamStateV1::new(limits, None),
        }
    }

    pub fn accept_chunk(
        &mut self,
        chunk: &CustodyEnvelopeChunkV1,
    ) -> Result<(), CustodyCapsuleErrorV1> {
        self.state.accept_chunk(chunk)
    }

    pub fn finish(self) -> Result<CustodyEnvelopeCiphertextReceiptV1, CustodyCapsuleErrorV1> {
        self.state
            .finish()
            .map(CustodyEnvelopeCiphertextReceiptV1::from_stream_receipt)
    }
}

/// Production seal receipt for one sealed envelope artifact.
///
/// Public callers can inspect genuine receipts, but production construction is crate-private and
/// consumes only a ciphertext sink completion minted from bytes accepted by the sink validator.
///
/// ```compile_fail
/// use bridge_core::custody_capsule::{
///     CustodyEnvelopeChunkV1, CustodyEnvelopeContextV1, CustodyEnvelopeFormatV1,
///     CustodyEnvelopeSealReceiptV1, CustodyEnvelopeSinkValidatorV1,
///     CustodyEnvelopeStreamLimitsV1,
/// };
/// use bridge_core::custody_inventory::LosslessPathV1;
/// use bridge_core::execution_policy::Sha256HexV1;
///
/// let context = CustodyEnvelopeContextV1::new(
///     LosslessPathV1::from_bytes(b"control/manifest.json.enc".to_vec()),
///     Sha256HexV1::digest(b"manifest"),
///     CustodyEnvelopeFormatV1::new("capsule-v1", "tool", "1").unwrap(),
///     vec!["recipient".to_owned()],
/// )
/// .unwrap();
/// let mut sink = CustodyEnvelopeSinkValidatorV1::new(
///     CustodyEnvelopeStreamLimitsV1::new(1, 1, 1).unwrap(),
/// );
/// sink.accept_chunk(&CustodyEnvelopeChunkV1::new(0, b"c".to_vec(), true).unwrap())
///     .unwrap();
/// let _receipt = CustodyEnvelopeSealReceiptV1::new(&context, sink.finish().unwrap());
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustodyEnvelopeSealReceiptV1 {
    artifact_name: LosslessPathV1,
    manifest_digest: Sha256HexV1,
    format: CustodyEnvelopeFormatV1,
    recipients: Vec<String>,
    ciphertext_length: u64,
    ciphertext_sha256: Sha256HexV1,
}

impl CustodyEnvelopeSealReceiptV1 {
    // Slice 2B1 defines the crate-private construction seam before the first in-crate
    // sealer adapter is implemented in 2B2. Module tests exercise it in the interim.
    #[allow(dead_code)]
    pub(crate) fn new(
        context: &CustodyEnvelopeContextV1,
        ciphertext_receipt: CustodyEnvelopeCiphertextReceiptV1,
    ) -> Result<Self, CustodyCapsuleErrorV1> {
        context.validate()?;
        if ciphertext_receipt.total_bytes() == 0
            || ciphertext_receipt.total_bytes() > MAX_ENVELOPE_TOTAL_BYTES_V1
        {
            return Err(CustodyCapsuleErrorV1::InvalidInput);
        }
        Ok(Self {
            artifact_name: context.artifact_name().clone(),
            manifest_digest: context.manifest_digest().clone(),
            format: context.format().clone(),
            recipients: context.recipients().to_vec(),
            ciphertext_length: ciphertext_receipt.total_bytes(),
            ciphertext_sha256: ciphertext_receipt.sha256().clone(),
        })
    }

    pub fn to_sealed_artifact(&self) -> Result<CustodySealedArtifactV1, CustodyCapsuleErrorV1> {
        CustodySealedArtifactV1::new(
            self.artifact_name.clone(),
            self.ciphertext_length,
            self.ciphertext_sha256.clone(),
        )
        .map_err(|_| CustodyCapsuleErrorV1::InvalidInput)
    }

    #[must_use]
    pub fn artifact_name(&self) -> &LosslessPathV1 {
        &self.artifact_name
    }

    #[must_use]
    pub fn manifest_digest(&self) -> &Sha256HexV1 {
        &self.manifest_digest
    }

    #[must_use]
    pub fn format(&self) -> &CustodyEnvelopeFormatV1 {
        &self.format
    }

    #[must_use]
    pub fn recipients(&self) -> &[String] {
        &self.recipients
    }

    #[must_use]
    pub const fn ciphertext_length(&self) -> u64 {
        self.ciphertext_length
    }

    #[must_use]
    pub fn ciphertext_sha256(&self) -> &Sha256HexV1 {
        &self.ciphertext_sha256
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustodyEnvelopeOpenReceiptV1 {
    plaintext_length: u64,
    plaintext_sha256: Sha256HexV1,
}

impl CustodyEnvelopeOpenReceiptV1 {
    pub fn new(
        plaintext_length: u64,
        plaintext_sha256: Sha256HexV1,
    ) -> Result<Self, CustodyCapsuleErrorV1> {
        if plaintext_length > MAX_ENVELOPE_TOTAL_BYTES_V1 {
            return Err(CustodyCapsuleErrorV1::InvalidInput);
        }
        Ok(Self {
            plaintext_length,
            plaintext_sha256,
        })
    }

    #[must_use]
    pub const fn plaintext_length(&self) -> u64 {
        self.plaintext_length
    }

    #[must_use]
    pub fn plaintext_sha256(&self) -> &Sha256HexV1 {
        &self.plaintext_sha256
    }
}

pub trait CustodyEnvelopeChunkSourceV1: sealed::Sealed {
    fn descriptor(&self) -> &CustodyEnvelopeSourceDescriptorV1;
    fn next_chunk(&mut self) -> Result<Option<CustodyEnvelopeChunkV1>, CustodyCapsuleErrorV1>;
}

pub trait CustodyEnvelopeChunkSinkV1: sealed::Sealed {
    fn limits(&self) -> CustodyEnvelopeStreamLimitsV1;
    fn write_chunk(&mut self, chunk: CustodyEnvelopeChunkV1) -> Result<(), CustodyCapsuleErrorV1>;
}

/// The envelope sealer an exporter drives. Sealed: only this crate may implement it.
///
/// ADR-0041 slice 2B2 control 24b. The single barrier here is trait sealing: `sealed::Sealed` is
/// `pub(crate)`, so an external crate cannot name it and therefore cannot satisfy the supertrait
/// bound. Removing the `: sealed::Sealed` bound below makes this doctest compile and so turns the
/// control red; module privacy (control 24a) and receipt-constructor privacy (control 24c) are
/// separate arms on separate items.
///
/// ```compile_fail
/// use bridge_core::custody_capsule::{
///     CustodyCapsuleErrorV1, CustodyEnvelopeChunkSinkV1, CustodyEnvelopeChunkSourceV1,
///     CustodyEnvelopeContextV1, CustodyEnvelopeMetadataV1, CustodyEnvelopeSealReceiptV1,
///     CustodyEnvelopeSealerV1,
/// };
///
/// struct ForeignSealer;
///
/// impl CustodyEnvelopeSealerV1 for ForeignSealer {
///     fn seal(
///         &self,
///         _context: &CustodyEnvelopeContextV1,
///         _plaintext: &mut dyn CustodyEnvelopeChunkSourceV1,
///         _metadata: &CustodyEnvelopeMetadataV1,
///         _ciphertext: &mut dyn CustodyEnvelopeChunkSinkV1,
///     ) -> Result<CustodyEnvelopeSealReceiptV1, CustodyCapsuleErrorV1> {
///         Err(CustodyCapsuleErrorV1::InvalidInput)
///     }
/// }
/// ```
pub trait CustodyEnvelopeSealerV1: sealed::Sealed {
    fn seal(
        &self,
        context: &CustodyEnvelopeContextV1,
        plaintext: &mut dyn CustodyEnvelopeChunkSourceV1,
        metadata: &CustodyEnvelopeMetadataV1,
        ciphertext: &mut dyn CustodyEnvelopeChunkSinkV1,
    ) -> Result<CustodyEnvelopeSealReceiptV1, CustodyCapsuleErrorV1>;
}

pub trait CustodyEnvelopeOpenerV1: sealed::Sealed {
    fn open(
        &self,
        request: &CustodyEnvelopeOpenRequestV1,
        ciphertext: &mut dyn CustodyEnvelopeChunkSourceV1,
        plaintext: &mut dyn CustodyEnvelopeChunkSinkV1,
    ) -> Result<CustodyEnvelopeOpenReceiptV1, CustodyCapsuleErrorV1>;
}

/// The sealing module for the four envelope traits above.
///
/// `pub(crate)` rather than private so the sibling `custody_export` module can implement the
/// source and sink traits for its own exporter-owned types. The inner trait is still unnameable
/// from outside this crate, so no external `impl` of a sealed envelope trait can exist — control
/// 24b's compile-fail doctest is the discriminating proof.
pub(crate) mod sealed {
    pub trait Sealed {}
}

#[derive(Clone, Debug)]
struct ArtifactRoleCandidateV1 {
    name: LosslessPathV1,
    role: CustodyCapsuleArtifactRoleV1,
}

impl ArtifactRoleCandidateV1 {
    fn new(name: impl AsRef<str>, role: CustodyCapsuleArtifactRoleV1) -> Self {
        Self {
            name: LosslessPathV1::from_bytes(name.as_ref().as_bytes().to_vec()),
            role,
        }
    }

    fn into_row(self) -> Result<CustodyCapsuleArtifactRoleRowV1, CustodyCapsuleErrorV1> {
        CustodyCapsuleArtifactRoleRowV1::new(self.name, self.role)
    }
}

fn validate_candidate_ownership(
    candidates: &[ArtifactRoleCandidateV1],
) -> Result<(), CustodyCapsuleErrorV1> {
    let mut names = BTreeSet::<Vec<u8>>::new();
    let mut manifest = 0usize;
    let mut index = 0usize;
    let mut restore_policy = 0usize;
    let mut git_pack = 0usize;
    let mut coverage_owners = BTreeSet::new();

    for candidate in candidates {
        validate_artifact_name(candidate.name.as_bytes())?;
        if !names.insert(candidate.name.as_bytes().to_vec()) {
            return Err(CustodyCapsuleErrorV1::DuplicateArtifactName);
        }
        match candidate.role {
            CustodyCapsuleArtifactRoleV1::Manifest => manifest += 1,
            CustodyCapsuleArtifactRoleV1::CapsuleIndex => index += 1,
            CustodyCapsuleArtifactRoleV1::RestorePolicy => restore_policy += 1,
            CustodyCapsuleArtifactRoleV1::GitObjectPack => git_pack += 1,
            CustodyCapsuleArtifactRoleV1::CoveragePayload(class) => {
                if class == CustodyCoverageClassV1::ObjectDatabase {
                    return Err(CustodyCapsuleErrorV1::UnmappedArtifact);
                }
                if !coverage_owners.insert(class) {
                    return Err(CustodyCapsuleErrorV1::DuplicateArtifactRole);
                }
            }
        }
    }

    if manifest > 1 || index > 1 || restore_policy > 1 || git_pack > 1 {
        return Err(CustodyCapsuleErrorV1::DuplicateArtifactRole);
    }
    if manifest != 1 || index != 1 || restore_policy != 1 {
        return Err(CustodyCapsuleErrorV1::MissingArtifact);
    }
    Ok(())
}

fn canonical_role_rows(
    rows: Vec<CustodyCapsuleArtifactRoleRowV1>,
) -> Result<Vec<CustodyCapsuleArtifactRoleRowV1>, CustodyCapsuleErrorV1> {
    let mut by_name = BTreeMap::<LosslessPathV1, CustodyCapsuleArtifactRoleRowV1>::new();
    let mut manifest = 0usize;
    let mut index = 0usize;
    let mut restore_policy = 0usize;
    let mut git_pack = 0usize;
    let mut coverage_owners = BTreeSet::new();

    for row in rows {
        let row = CustodyCapsuleArtifactRoleRowV1::new(row.name, row.role)?;
        if by_name.contains_key(&row.name) {
            return Err(CustodyCapsuleErrorV1::DuplicateArtifactName);
        }
        match row.role {
            CustodyCapsuleArtifactRoleV1::Manifest => manifest += 1,
            CustodyCapsuleArtifactRoleV1::CapsuleIndex => index += 1,
            CustodyCapsuleArtifactRoleV1::RestorePolicy => restore_policy += 1,
            CustodyCapsuleArtifactRoleV1::GitObjectPack => git_pack += 1,
            CustodyCapsuleArtifactRoleV1::CoveragePayload(class) => {
                if class == CustodyCoverageClassV1::ObjectDatabase || !coverage_owners.insert(class)
                {
                    return Err(CustodyCapsuleErrorV1::DuplicateArtifactRole);
                }
            }
        }
        by_name.insert(row.name.clone(), row);
    }
    if manifest > 1 || index > 1 || restore_policy > 1 || git_pack > 1 {
        return Err(CustodyCapsuleErrorV1::DuplicateArtifactRole);
    }
    if manifest != 1 || index != 1 || restore_policy != 1 {
        return Err(CustodyCapsuleErrorV1::MissingArtifact);
    }
    Ok(by_name.into_values().collect())
}

fn validate_reserved_name_for_role(
    name: &LosslessPathV1,
    role: &CustodyCapsuleArtifactRoleV1,
) -> Result<(), CustodyCapsuleErrorV1> {
    let expected = match role {
        CustodyCapsuleArtifactRoleV1::Manifest => "control/manifest.json.enc".to_owned(),
        CustodyCapsuleArtifactRoleV1::CapsuleIndex => "control/capsule-index.json.enc".to_owned(),
        CustodyCapsuleArtifactRoleV1::RestorePolicy => "control/restore-policy.json.enc".to_owned(),
        CustodyCapsuleArtifactRoleV1::GitObjectPack => "git/objects.pack.enc".to_owned(),
        CustodyCapsuleArtifactRoleV1::CoveragePayload(class) => {
            if *class == CustodyCoverageClassV1::ObjectDatabase {
                return Err(CustodyCapsuleErrorV1::UnmappedArtifact);
            }
            format!("payload/{}.bin.enc", coverage_class_name(*class))
        }
    };
    if name.as_bytes() != expected.as_bytes() {
        return Err(CustodyCapsuleErrorV1::UnmappedArtifact);
    }
    Ok(())
}

fn validate_artifact_name(name: &[u8]) -> Result<(), CustodyCapsuleErrorV1> {
    if name.is_empty()
        || name.len() > MAX_ARTIFACT_NAME_BYTES_V1
        || name.starts_with(b"/")
        || name.contains(&0)
        || name.contains(&b'\\')
        || (name.get(1) == Some(&b':')
            && name.first().is_some_and(|byte| byte.is_ascii_alphabetic()))
        || name
            .split(|byte| *byte == b'/')
            .any(|component| component.is_empty() || component == b"." || component == b"..")
    {
        return Err(CustodyCapsuleErrorV1::InvalidInput);
    }
    Ok(())
}

fn coverage_class_name(class: CustodyCoverageClassV1) -> &'static str {
    match class {
        CustodyCoverageClassV1::RefsAndHead => "refs_and_head",
        CustodyCoverageClassV1::ObjectDatabase => "object_database",
        CustodyCoverageClassV1::Index => "index",
        CustodyCoverageClassV1::Worktree => "worktree",
        CustodyCoverageClassV1::StashAndReflogs => "stash_and_reflogs",
        CustodyCoverageClassV1::InProgressGitOperations => "in_progress_git_operations",
        CustodyCoverageClassV1::LinkedWorktrees => "linked_worktrees",
        CustodyCoverageClassV1::NestedRepositoriesAndSubmodules => {
            "nested_repositories_and_submodules"
        }
        CustodyCoverageClassV1::LfsAndExternalPayloads => "lfs_and_external_payloads",
        CustodyCoverageClassV1::AlternatesAndSharedStores => "alternates_and_shared_stores",
        CustodyCoverageClassV1::GitConfigurationAndHooks => "git_configuration_and_hooks",
        CustodyCoverageClassV1::BridgeEvidence => "bridge_evidence",
        CustodyCoverageClassV1::ExternalEvidence => "external_evidence",
        CustodyCoverageClassV1::ReproducibleOutputs => "reproducible_outputs",
    }
}

fn compare_index_to_layout(
    index: &CustodyCapsuleIndexV1,
    expected: &CustodyCapsuleIndexV1,
) -> Result<(), CustodyCapsuleErrorV1> {
    let expected_by_name = expected
        .artifacts()
        .iter()
        .map(|row| (row.name.clone(), row.role.clone()))
        .collect::<BTreeMap<_, _>>();
    let actual_by_name = index
        .artifacts()
        .iter()
        .map(|row| (row.name.clone(), row.role.clone()))
        .collect::<BTreeMap<_, _>>();
    if expected_by_name
        .keys()
        .any(|name| !actual_by_name.contains_key(name))
    {
        return Err(CustodyCapsuleErrorV1::MissingArtifact);
    }
    if actual_by_name
        .keys()
        .any(|name| !expected_by_name.contains_key(name))
    {
        return Err(CustodyCapsuleErrorV1::ExtraArtifact);
    }
    if expected_by_name != actual_by_name {
        return Err(CustodyCapsuleErrorV1::DerivedLayoutMismatch);
    }
    Ok(())
}

fn index_name_set(index: &CustodyCapsuleIndexV1) -> BTreeSet<Vec<u8>> {
    index
        .artifacts()
        .iter()
        .map(|row| row.name().as_bytes().to_vec())
        .collect()
}

fn seal_name_set(seal: &CustodySealV1) -> Result<BTreeSet<Vec<u8>>, CustodyCapsuleErrorV1> {
    let mut names = BTreeSet::new();
    for artifact in seal.artifacts() {
        let inserted = names.insert(artifact.name().as_bytes().to_vec());
        if !inserted {
            return Err(CustodyCapsuleErrorV1::DuplicateArtifactName);
        }
    }
    Ok(names)
}

fn validate_canonical_decode_size(bytes: &[u8]) -> Result<(), CustodyCapsuleErrorV1> {
    if bytes.len() > MAX_CANONICAL_JSON_BYTES_V1 {
        return Err(CustodyCapsuleErrorV1::InvalidInput);
    }
    Ok(())
}

fn canonical_recipients(recipients: Vec<String>) -> Result<Vec<String>, CustodyCapsuleErrorV1> {
    validate_recipient_rows(recipients.iter().map(String::as_str))?;
    let mut recipients = BTreeSet::from_iter(recipients)
        .into_iter()
        .collect::<Vec<_>>();
    recipients.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    validate_recipient_rows(recipients.iter().map(String::as_str))?;
    Ok(recipients)
}

fn validate_recipient_rows<'a>(
    recipients: impl Iterator<Item = &'a str>,
) -> Result<(), CustodyCapsuleErrorV1> {
    let mut count = 0usize;
    let mut bytes = 0usize;
    for recipient in recipients {
        count = count
            .checked_add(1)
            .ok_or(CustodyCapsuleErrorV1::InvalidInput)?;
        validate_envelope_field(recipient)?;
        bytes = bytes
            .checked_add(recipient.len())
            .ok_or(CustodyCapsuleErrorV1::InvalidInput)?;
        if count > MAX_ENVELOPE_RECIPIENTS_V1 || bytes > MAX_ENVELOPE_RECIPIENT_BYTES_V1 {
            return Err(CustodyCapsuleErrorV1::InvalidInput);
        }
    }
    if count == 0 {
        return Err(CustodyCapsuleErrorV1::InvalidInput);
    }
    Ok(())
}

fn validate_envelope_field(value: &str) -> Result<(), CustodyCapsuleErrorV1> {
    if value.is_empty() || value.len() > MAX_ENVELOPE_FIELD_BYTES_V1 {
        return Err(CustodyCapsuleErrorV1::InvalidInput);
    }
    Ok(())
}

fn validate_persisted_record_size<T: Serialize>(value: &T) -> Result<(), CustodyCapsuleErrorV1> {
    if serde_json::to_vec(value)
        .map_err(|_| CustodyCapsuleErrorV1::CanonicalEncoding)?
        .len()
        > MAX_CANONICAL_JSON_BYTES_V1
    {
        return Err(CustodyCapsuleErrorV1::InvalidInput);
    }
    Ok(())
}

fn validate_capsule_seal_for_open_request(
    seal: &CustodySealV1,
) -> Result<(), CustodyCapsuleErrorV1> {
    validate_seal_wide_capsule_limits(seal)?;
    seal.validate_borrowed_canonical()
        .map_err(|_| CustodyCapsuleErrorV1::InvalidInput)
}

fn validate_seal_wide_capsule_limits(seal: &CustodySealV1) -> Result<(), CustodyCapsuleErrorV1> {
    validate_envelope_field(seal.capsule_format())
        .map_err(|_| CustodyCapsuleErrorV1::SealExceedsV1Limits)?;
    validate_envelope_field(seal.sealing_tool())
        .map_err(|_| CustodyCapsuleErrorV1::SealExceedsV1Limits)?;
    validate_envelope_field(seal.sealing_tool_version())
        .map_err(|_| CustodyCapsuleErrorV1::SealExceedsV1Limits)?;
    validate_recipient_rows(seal.encryption_recipients().iter().map(String::as_str))
        .map_err(|_| CustodyCapsuleErrorV1::SealExceedsV1Limits)
}

fn validate_selected_seal_artifact_limits(
    artifact: &CustodySealedArtifactV1,
) -> Result<(), CustodyCapsuleErrorV1> {
    if artifact.name().as_bytes().len() > MAX_ARTIFACT_NAME_BYTES_V1
        || artifact.byte_length() > MAX_ENVELOPE_TOTAL_BYTES_V1
    {
        return Err(CustodyCapsuleErrorV1::SealExceedsV1Limits);
    }
    validate_artifact_name(artifact.name().as_bytes())
}

fn sha256_from_context(context: digest::Context) -> Sha256HexV1 {
    let digest = context.finish();
    let mut value = String::with_capacity(64);
    for byte in digest.as_ref() {
        use std::fmt::Write as _;
        let _ = write!(&mut value, "{byte:02x}");
    }
    Sha256HexV1::parse(value).expect("ring SHA-256 digest renders as 64 lower-hex bytes")
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum CustodyCapsuleErrorV1 {
    #[error("custody capsule input is invalid")]
    InvalidInput,
    #[error("custody capsule schema is unsupported")]
    WrongSchema,
    #[error("custody capsule record is not canonical")]
    NonCanonicalRecord,
    #[error("custody capsule encoding failed")]
    CanonicalEncoding,
    #[error("custody capsule encoding is invalid")]
    InvalidEncoding,
    #[error("custody capsule JSON is not canonical")]
    NonCanonicalEncoding,
    #[error("custody manifest is not sealable")]
    NonSealableManifest,
    #[error("custody seal exceeds capsule v1 limits")]
    SealExceedsV1Limits,
    #[error("custody capsule manifest, index, and seal digests do not agree")]
    ThreeWayDigestMismatch,
    #[error("custody capsule index does not match the manifest-derived layout")]
    DerivedLayoutMismatch,
    #[error("a required custody capsule artifact is missing")]
    MissingArtifact,
    #[error("a custody capsule artifact appears more than once")]
    DuplicateArtifactName,
    #[error("a custody capsule role appears more than once")]
    DuplicateArtifactRole,
    #[error("a custody capsule contains an unexpected artifact")]
    ExtraArtifact,
    #[error("a custody capsule artifact is not mapped")]
    UnmappedArtifact,
    #[error("custody coverage state does not match capsule requirements")]
    CoverageStateMismatch,
    #[error("custody capsule layout cannot prove an unknown Git object kind")]
    UnknownObjectKind,
    #[error("custody restore behavior is unsupported")]
    UnsupportedRestoreBehavior,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custody_seal::{
        CustodyCoverageEntryV1, CustodyGitObjectFormatV1, CustodyOriginalObjectV1,
    };

    #[test]
    fn pre_canonical_duplicate_owner_helper_uses_distinct_path_safe_names() {
        let candidates = vec![
            ArtifactRoleCandidateV1::new(
                "control/manifest.json.enc",
                CustodyCapsuleArtifactRoleV1::Manifest,
            ),
            ArtifactRoleCandidateV1::new(
                "control/capsule-index.json.enc",
                CustodyCapsuleArtifactRoleV1::CapsuleIndex,
            ),
            ArtifactRoleCandidateV1::new(
                "control/restore-policy.json.enc",
                CustodyCapsuleArtifactRoleV1::RestorePolicy,
            ),
            ArtifactRoleCandidateV1::new(
                "payload/worktree-a.bin.enc",
                CustodyCapsuleArtifactRoleV1::CoveragePayload(CustodyCoverageClassV1::Worktree),
            ),
            ArtifactRoleCandidateV1::new(
                "payload/worktree-b.bin.enc",
                CustodyCapsuleArtifactRoleV1::CoveragePayload(CustodyCoverageClassV1::Worktree),
            ),
        ];
        assert_eq!(
            validate_candidate_ownership(&candidates).unwrap_err(),
            CustodyCapsuleErrorV1::DuplicateArtifactRole
        );

        let focused_branch_flipped = vec![
            ArtifactRoleCandidateV1::new(
                "control/manifest.json.enc",
                CustodyCapsuleArtifactRoleV1::Manifest,
            ),
            ArtifactRoleCandidateV1::new(
                "control/capsule-index.json.enc",
                CustodyCapsuleArtifactRoleV1::CapsuleIndex,
            ),
            ArtifactRoleCandidateV1::new(
                "control/restore-policy.json.enc",
                CustodyCapsuleArtifactRoleV1::RestorePolicy,
            ),
            ArtifactRoleCandidateV1::new(
                "payload/worktree-a.bin.enc",
                CustodyCapsuleArtifactRoleV1::CoveragePayload(CustodyCoverageClassV1::Worktree),
            ),
            ArtifactRoleCandidateV1::new(
                "payload/index.bin.enc",
                CustodyCapsuleArtifactRoleV1::CoveragePayload(CustodyCoverageClassV1::Index),
            ),
        ];
        assert!(validate_candidate_ownership(&focused_branch_flipped).is_ok());
    }

    fn digest(byte: u8) -> Sha256HexV1 {
        Sha256HexV1::digest(&[byte])
    }

    fn object(kind: CustodyGitObjectKindV1) -> CustodyOriginalObjectV1 {
        CustodyOriginalObjectV1::new(CustodyGitObjectFormatV1::Sha1, "a".repeat(40), kind).unwrap()
    }

    fn coverage_with(default: CustodyStateClassV1) -> Vec<CustodyCoverageEntryV1> {
        CustodyCoverageClassV1::ALL
            .into_iter()
            .map(|class| {
                let reasons = if default == CustodyStateClassV1::Unresolved {
                    vec![crate::custody_inventory::CustodyReasonCodeV1::ContentUnresolved]
                } else {
                    vec![]
                };
                CustodyCoverageEntryV1::new(class, default, reasons, None).unwrap()
            })
            .collect()
    }

    fn set_coverage(
        mut rows: Vec<CustodyCoverageEntryV1>,
        class: CustodyCoverageClassV1,
        state: CustodyStateClassV1,
    ) -> Vec<CustodyCoverageEntryV1> {
        let position = rows.iter().position(|row| row.class() == class).unwrap();
        let reasons = if state == CustodyStateClassV1::Unresolved {
            vec![crate::custody_inventory::CustodyReasonCodeV1::ContentUnresolved]
        } else {
            vec![]
        };
        rows[position] = CustodyCoverageEntryV1::new(class, state, reasons, None).unwrap();
        rows
    }

    fn make_manifest(
        coverage: Vec<CustodyCoverageEntryV1>,
        objects: Vec<CustodyOriginalObjectV1>,
    ) -> CustodyManifestV1 {
        CustodyManifestV1::new(
            "unit",
            "run",
            "materialization",
            "generation",
            coverage,
            vec![],
            objects,
            vec![],
            vec![],
        )
        .unwrap()
    }

    fn empty_manifest() -> CustodyManifestV1 {
        make_manifest(coverage_with(CustodyStateClassV1::Empty), vec![])
    }

    fn full_manifest() -> CustodyManifestV1 {
        make_manifest(
            coverage_with(CustodyStateClassV1::Captured),
            vec![object(CustodyGitObjectKindV1::Commit)],
        )
    }

    fn envelope_format() -> CustodyEnvelopeFormatV1 {
        CustodyEnvelopeFormatV1::new("capsule-v1", "tool", "1").unwrap()
    }

    fn envelope_context() -> CustodyEnvelopeContextV1 {
        CustodyEnvelopeContextV1::new(
            LosslessPathV1::from_bytes(b"control/manifest.json.enc".to_vec()),
            digest(1),
            envelope_format(),
            vec!["recipient".to_owned()],
        )
        .unwrap()
    }

    fn ciphertext_receipt(bytes: &[u8]) -> CustodyEnvelopeCiphertextReceiptV1 {
        let max_total = u64::try_from(bytes.len().max(1)).unwrap();
        let max_chunk = u64::try_from(bytes.len().max(1)).unwrap();
        let mut sink = CustodyEnvelopeSinkValidatorV1::new(
            CustodyEnvelopeStreamLimitsV1::new(max_total, max_chunk, 1).unwrap(),
        );
        sink.accept_chunk(&CustodyEnvelopeChunkV1::new(0, bytes.to_vec(), true).unwrap())
            .unwrap();
        sink.finish().unwrap()
    }

    fn source_receipt(bytes: &[u8]) -> CustodyEnvelopeStreamReceiptV1 {
        let max_total = u64::try_from(bytes.len()).unwrap();
        let limits = CustodyEnvelopeStreamLimitsV1::new(max_total, max_total, 1).unwrap();
        let descriptor = CustodyEnvelopeSourceDescriptorV1::new(max_total, limits).unwrap();
        let mut source = CustodyEnvelopeSourceValidatorV1::new(descriptor);
        source
            .accept_chunk(&CustodyEnvelopeChunkV1::new(0, bytes.to_vec(), true).unwrap())
            .unwrap();
        source.finish().unwrap()
    }

    fn receipt_for(
        name: &LosslessPathV1,
        manifest_digest: Sha256HexV1,
        format: CustodyEnvelopeFormatV1,
        recipients: Vec<String>,
        bytes: &[u8],
    ) -> CustodyEnvelopeSealReceiptV1 {
        let context =
            CustodyEnvelopeContextV1::new(name.clone(), manifest_digest, format, recipients)
                .unwrap();
        CustodyEnvelopeSealReceiptV1::new(&context, ciphertext_receipt(bytes)).unwrap()
    }

    fn proof_for(
        index: &CustodyCapsuleIndexV1,
        manifest_digest: Sha256HexV1,
    ) -> CustodyCapsuleSealProofV1 {
        let format = CustodyEnvelopeFormatV1::new("capsule-v1", "a2a-bridge", "0.1.0").unwrap();
        let receipts = index
            .artifacts()
            .iter()
            .enumerate()
            .map(|(offset, row)| {
                receipt_for(
                    row.name(),
                    manifest_digest.clone(),
                    format.clone(),
                    vec!["recipient-b".to_owned(), "recipient-a".to_owned()],
                    &[u8::try_from(offset + 1).unwrap()],
                )
            })
            .collect();
        CustodyCapsuleSealProofV1::from_receipts(receipts).unwrap()
    }

    #[test]
    fn seal_receipt_requires_ciphertext_sink_completion_and_binds_exact_bytes() {
        let context = envelope_context();
        let ciphertext = b"ciphertext";
        let receipt =
            CustodyEnvelopeSealReceiptV1::new(&context, ciphertext_receipt(ciphertext)).unwrap();
        let cloned =
            CustodyEnvelopeSealReceiptV1::new(&context, ciphertext_receipt(ciphertext)).unwrap();

        assert_eq!(receipt, cloned);
        assert_eq!(receipt.artifact_name(), context.artifact_name());
        assert_eq!(receipt.manifest_digest(), context.manifest_digest());
        assert_eq!(receipt.format(), context.format());
        assert_eq!(receipt.recipients(), context.recipients());
        assert_eq!(receipt.ciphertext_length(), 10);
        assert_eq!(
            receipt.ciphertext_sha256(),
            &Sha256HexV1::digest(ciphertext)
        );

        let proof = CustodyCapsuleSealProofV1::from_receipts(vec![receipt.clone()]).unwrap();
        let request = CustodyEnvelopeOpenRequestV1::from_seal_artifact(
            &proof,
            context.artifact_name().clone(),
        )
        .unwrap();
        assert_eq!(request.ciphertext_length(), 10);
        assert_eq!(
            request.ciphertext_sha256(),
            &Sha256HexV1::digest(ciphertext)
        );
        assert_eq!(
            receipt.to_sealed_artifact().unwrap().sha256(),
            &Sha256HexV1::digest(ciphertext)
        );
    }

    #[test]
    fn decoy_sink_bytes_cannot_authorize_a_different_ciphertext_identity() {
        let context = envelope_context();
        let ciphertext = b"ciphertext";
        let decoy = b"plaintext";
        let source = source_receipt(decoy);
        assert_eq!(source.total_bytes(), u64::try_from(decoy.len()).unwrap());
        assert_eq!(source.sha256(), &Sha256HexV1::digest(decoy));

        let decoy_receipt =
            CustodyEnvelopeSealReceiptV1::new(&context, ciphertext_receipt(decoy)).unwrap();
        let artifact = decoy_receipt.to_sealed_artifact().unwrap();
        assert_eq!(artifact.byte_length(), u64::try_from(decoy.len()).unwrap());
        assert_eq!(artifact.sha256(), &Sha256HexV1::digest(decoy));
        assert_ne!(
            artifact.byte_length(),
            u64::try_from(ciphertext.len()).unwrap()
        );
        assert_ne!(artifact.sha256(), &Sha256HexV1::digest(ciphertext));
    }

    #[test]
    fn zero_byte_ciphertext_sink_completion_cannot_mint_a_seal_receipt() {
        assert_eq!(
            CustodyEnvelopeSealReceiptV1::new(&envelope_context(), ciphertext_receipt(b""))
                .unwrap_err(),
            CustodyCapsuleErrorV1::InvalidInput
        );
    }

    /// Deferred 2B1 control: the capsule proof is derived from a NONEMPTY receipt population. An
    /// empty population is refused directly, as a missing artifact, rather than reaching the
    /// generic seal constructor.
    #[test]
    fn capsule_seal_proof_refuses_an_empty_receipt_population() {
        assert_eq!(
            CustodyCapsuleSealProofV1::from_receipts(Vec::new()).unwrap_err(),
            CustodyCapsuleErrorV1::MissingArtifact
        );
    }

    /// Deferred 2B1 control: exact V1 boundaries for the envelope constructors 2B2's exporter
    /// drives. Each limit is admitted at its maximum and refused at maximum + 1, through the
    /// validators' own arithmetic and without materializing a 10 GiB input.
    #[test]
    fn v1_envelope_limits_admit_max_and_refuse_max_plus_one() {
        let total = MAX_ENVELOPE_TOTAL_BYTES_V1;
        let chunk = MAX_ENVELOPE_CHUNK_BYTES_V1;
        let chunks = MAX_ENVELOPE_CHUNKS_V1;
        assert!(CustodyEnvelopeStreamLimitsV1::new(total, chunk, chunks).is_ok());
        assert!(CustodyEnvelopeStreamLimitsV1::new(total + 1, chunk, chunks).is_err());
        assert!(CustodyEnvelopeStreamLimitsV1::new(chunk, chunk + 1, chunks).is_err());
        assert!(CustodyEnvelopeStreamLimitsV1::new(chunk, chunk, chunks + 1).is_err());

        let chunk_len = usize::try_from(chunk).unwrap();
        assert!(CustodyEnvelopeChunkV1::new(0, vec![0; chunk_len], true).is_ok());
        assert_eq!(
            CustodyEnvelopeChunkV1::new(0, vec![0; chunk_len + 1], true).unwrap_err(),
            CustodyCapsuleErrorV1::InvalidInput
        );

        let field = "f".repeat(MAX_ENVELOPE_FIELD_BYTES_V1);
        assert!(CustodyEnvelopeFormatV1::new(field.clone(), "tool", "1").is_ok());
        assert!(CustodyEnvelopeFormatV1::new(format!("{field}f"), "tool", "1").is_err());

        let recipients = |count: usize| -> Vec<String> {
            (0..count)
                .map(|index| format!("recipient-{index:03}"))
                .collect()
        };
        let context = |count: usize| {
            CustodyEnvelopeContextV1::new(
                LosslessPathV1::from_bytes(b"control/manifest.json.enc".to_vec()),
                digest(1),
                envelope_format(),
                recipients(count),
            )
        };
        assert!(context(MAX_ENVELOPE_RECIPIENTS_V1).is_ok());
        assert!(context(MAX_ENVELOPE_RECIPIENTS_V1 + 1).is_err());

        let metadata = |count: usize| {
            CustodyEnvelopeMetadataV1::new(
                (0..count)
                    .map(|index| (format!("key-{index:03}"), "value".to_owned()))
                    .collect(),
            )
        };
        assert!(metadata(MAX_ENVELOPE_METADATA_ROWS_V1).is_ok());
        assert!(metadata(MAX_ENVELOPE_METADATA_ROWS_V1 + 1).is_err());
    }

    #[test]
    fn capsule_seal_proof_requires_receipt_derived_shared_identity() {
        let name = LosslessPathV1::from_bytes(b"control/manifest.json.enc".to_vec());
        let context = envelope_context();
        let baseline =
            CustodyEnvelopeSealReceiptV1::new(&context, ciphertext_receipt(&[1, 2, 3])).unwrap();
        let valid = CustodyCapsuleSealProofV1::from_receipts(vec![baseline.clone()]).unwrap();
        assert_eq!(valid.seal().manifest_digest(), context.manifest_digest());
        assert_eq!(valid.seal().artifacts()[0].name(), &name);
        assert_eq!(valid.seal().artifacts()[0].byte_length(), 3);
        assert_eq!(
            valid.seal().artifacts()[0].sha256(),
            &Sha256HexV1::digest(&[1, 2, 3])
        );
        let second_name = LosslessPathV1::from_bytes(b"control/capsule-index.json.enc".to_vec());

        for changed in [
            CustodyEnvelopeContextV1::new(
                second_name.clone(),
                digest(8),
                context.format().clone(),
                context.recipients().to_vec(),
            )
            .unwrap(),
            CustodyEnvelopeContextV1::new(
                second_name.clone(),
                context.manifest_digest().clone(),
                CustodyEnvelopeFormatV1::new("other", "tool", "1").unwrap(),
                context.recipients().to_vec(),
            )
            .unwrap(),
            CustodyEnvelopeContextV1::new(
                second_name.clone(),
                context.manifest_digest().clone(),
                CustodyEnvelopeFormatV1::new("capsule-v1", "other", "1").unwrap(),
                context.recipients().to_vec(),
            )
            .unwrap(),
            CustodyEnvelopeContextV1::new(
                second_name.clone(),
                context.manifest_digest().clone(),
                CustodyEnvelopeFormatV1::new("capsule-v1", "tool", "2").unwrap(),
                context.recipients().to_vec(),
            )
            .unwrap(),
            CustodyEnvelopeContextV1::new(
                second_name,
                context.manifest_digest().clone(),
                context.format().clone(),
                vec!["other-recipient".to_owned()],
            )
            .unwrap(),
        ] {
            let changed =
                CustodyEnvelopeSealReceiptV1::new(&changed, ciphertext_receipt(&[4])).unwrap();
            assert_eq!(
                CustodyCapsuleSealProofV1::from_receipts(vec![baseline.clone(), changed])
                    .unwrap_err(),
                CustodyCapsuleErrorV1::InvalidInput
            );
        }
    }

    #[test]
    fn binding_accepts_and_rejects_receipt_derived_proofs() {
        let manifest = full_manifest();
        let layout = CustodyCapsuleLayoutV1::derive(&manifest).unwrap();
        let manifest_digest = manifest.content_digest().unwrap();
        let proof = proof_for(layout.index(), manifest_digest.clone());
        let binding = CustodyCapsuleBindingV1::new(
            &manifest,
            layout.index(),
            &CustodyRestorePolicyV1::default(),
            &proof,
        )
        .unwrap();
        assert_eq!(binding.manifest_digest(), &manifest_digest);
        assert_eq!(binding.artifact_names().len(), 17);

        let foreign_index =
            CustodyCapsuleIndexV1::new(digest(8), layout.index().artifacts().to_vec()).unwrap();
        assert_eq!(
            CustodyCapsuleBindingV1::new(
                &manifest,
                &foreign_index,
                &CustodyRestorePolicyV1::default(),
                &proof,
            )
            .unwrap_err(),
            CustodyCapsuleErrorV1::ThreeWayDigestMismatch
        );

        let foreign_proof = proof_for(layout.index(), digest(7));
        assert_eq!(
            CustodyCapsuleBindingV1::new(
                &manifest,
                layout.index(),
                &CustodyRestorePolicyV1::default(),
                &foreign_proof,
            )
            .unwrap_err(),
            CustodyCapsuleErrorV1::ThreeWayDigestMismatch
        );

        let other_manifest = make_manifest(
            set_coverage(
                coverage_with(CustodyStateClassV1::Captured),
                CustodyCoverageClassV1::Worktree,
                CustodyStateClassV1::Empty,
            ),
            vec![object(CustodyGitObjectKindV1::Commit)],
        );
        assert_eq!(
            CustodyCapsuleBindingV1::new(
                &other_manifest,
                layout.index(),
                &CustodyRestorePolicyV1::default(),
                &proof,
            )
            .unwrap_err(),
            CustodyCapsuleErrorV1::ThreeWayDigestMismatch
        );
    }

    #[test]
    fn binding_reports_missing_extra_and_unmapped_receipt_artifacts() {
        let manifest = full_manifest();
        let layout = CustodyCapsuleLayoutV1::derive(&manifest).unwrap();
        let manifest_digest = manifest.content_digest().unwrap();
        let proof = proof_for(layout.index(), manifest_digest.clone());

        let mut missing_rows = layout.index().artifacts().to_vec();
        missing_rows.retain(|row| row.name().as_bytes() != b"payload/worktree.bin.enc");
        let missing_index = CustodyCapsuleIndexV1::new(manifest_digest, missing_rows).unwrap();
        assert_eq!(
            CustodyCapsuleBindingV1::new(
                &manifest,
                &missing_index,
                &CustodyRestorePolicyV1::default(),
                &proof,
            )
            .unwrap_err(),
            CustodyCapsuleErrorV1::MissingArtifact
        );

        let extra_row = CustodyCapsuleArtifactRoleRowV1::new(
            LosslessPathV1::from_bytes(b"payload/reproducible_outputs.bin.enc".to_vec()),
            CustodyCapsuleArtifactRoleV1::CoveragePayload(
                CustodyCoverageClassV1::ReproducibleOutputs,
            ),
        )
        .unwrap();
        let mut empty_rows = CustodyCapsuleLayoutV1::derive(&empty_manifest())
            .unwrap()
            .index()
            .artifacts()
            .to_vec();
        empty_rows.push(extra_row);
        let extra_index =
            CustodyCapsuleIndexV1::new(empty_manifest().content_digest().unwrap(), empty_rows)
                .unwrap();
        let empty_proof = proof_for(&extra_index, empty_manifest().content_digest().unwrap());
        assert_eq!(
            CustodyCapsuleBindingV1::new(
                &empty_manifest(),
                &extra_index,
                &CustodyRestorePolicyV1::default(),
                &empty_proof,
            )
            .unwrap_err(),
            CustodyCapsuleErrorV1::ExtraArtifact
        );

        let empty = empty_manifest();
        let empty_layout = CustodyCapsuleLayoutV1::derive(&empty).unwrap();
        let empty_digest = empty.content_digest().unwrap();
        let format = CustodyEnvelopeFormatV1::new("capsule-v1", "a2a-bridge", "0.1.0").unwrap();
        let mut extra_receipts: Vec<_> = empty_layout
            .index()
            .artifacts()
            .iter()
            .enumerate()
            .map(|(offset, row)| {
                receipt_for(
                    row.name(),
                    empty_digest.clone(),
                    format.clone(),
                    vec!["recipient".to_owned()],
                    &[u8::try_from(offset + 1).unwrap()],
                )
            })
            .collect();
        extra_receipts.push(receipt_for(
            &LosslessPathV1::from_bytes(b"payload/worktree.bin.enc".to_vec()),
            empty_digest,
            format,
            vec!["recipient".to_owned()],
            &[9],
        ));
        let extra_proof = CustodyCapsuleSealProofV1::from_receipts(extra_receipts).unwrap();
        assert_eq!(
            CustodyCapsuleBindingV1::new(
                &empty,
                empty_layout.index(),
                &CustodyRestorePolicyV1::default(),
                &extra_proof,
            )
            .unwrap_err(),
            CustodyCapsuleErrorV1::UnmappedArtifact
        );
    }

    #[test]
    fn seal_derived_open_request_binds_membership_limits_and_ciphertext_identity() {
        let name = LosslessPathV1::from_bytes(b"control/manifest.json.enc".to_vec());
        let context = CustodyEnvelopeContextV1::new(
            name.clone(),
            digest(1),
            envelope_format(),
            vec!["z".to_owned(), "a".to_owned(), "a".to_owned()],
        )
        .unwrap();
        let proof =
            CustodyCapsuleSealProofV1::from_receipts(vec![CustodyEnvelopeSealReceiptV1::new(
                &context,
                ciphertext_receipt(&[1, 2, 3, 4, 5]),
            )
            .unwrap()])
            .unwrap();
        let request =
            CustodyEnvelopeOpenRequestV1::from_seal_artifact(&proof, name.clone()).unwrap();
        assert_eq!(
            request.context().encode_canonical().unwrap(),
            context.encode_canonical().unwrap()
        );
        assert_eq!(request.ciphertext_length(), 5);
        assert_eq!(
            request.ciphertext_sha256(),
            &Sha256HexV1::digest(&[1, 2, 3, 4, 5])
        );
        assert_eq!(
            CustodyEnvelopeOpenRequestV1::from_seal_artifact(
                &proof,
                LosslessPathV1::from_bytes(b"payload/worktree.bin.enc".to_vec()),
            )
            .unwrap_err(),
            CustodyCapsuleErrorV1::MissingArtifact
        );
    }
}
