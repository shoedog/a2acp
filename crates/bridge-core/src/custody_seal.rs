//! Provider-free records for local custody manifests and seals.
//!
//! This module canonicalizes and validates data only. It has no filesystem, Git, encryption, network,
//! restoration, authorization, or destructive-effect API.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::custody_inventory::{CustodyReasonCodeV1, CustodyStateClassV1, LosslessPathV1};
use crate::execution_policy::Sha256HexV1;

const MANIFEST_SCHEMA_V1: &str = "custody-manifest.v1";
const SEAL_SCHEMA_V1: &str = "custody-seal.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustodyCoverageClassV1 {
    RefsAndHead,
    ObjectDatabase,
    Index,
    Worktree,
    StashAndReflogs,
    InProgressGitOperations,
    LinkedWorktrees,
    NestedRepositoriesAndSubmodules,
    LfsAndExternalPayloads,
    AlternatesAndSharedStores,
    GitConfigurationAndHooks,
    BridgeEvidence,
    ExternalEvidence,
    ReproducibleOutputs,
}

impl CustodyCoverageClassV1 {
    pub const ALL: [Self; 14] = [
        Self::RefsAndHead,
        Self::ObjectDatabase,
        Self::Index,
        Self::Worktree,
        Self::StashAndReflogs,
        Self::InProgressGitOperations,
        Self::LinkedWorktrees,
        Self::NestedRepositoriesAndSubmodules,
        Self::LfsAndExternalPayloads,
        Self::AlternatesAndSharedStores,
        Self::GitConfigurationAndHooks,
        Self::BridgeEvidence,
        Self::ExternalEvidence,
        Self::ReproducibleOutputs,
    ];
}

#[derive(Deserialize)]
struct CustodyCoverageEntryWireV1 {
    class: CustodyCoverageClassV1,
    state: CustodyStateClassV1,
    reasons: Vec<CustodyReasonCodeV1>,
    exclusion_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CustodyCoverageEntryWireV1")]
pub struct CustodyCoverageEntryV1 {
    class: CustodyCoverageClassV1,
    state: CustodyStateClassV1,
    reasons: Vec<CustodyReasonCodeV1>,
    exclusion_id: Option<String>,
}

impl TryFrom<CustodyCoverageEntryWireV1> for CustodyCoverageEntryV1 {
    type Error = CustodySealErrorV1;

    fn try_from(value: CustodyCoverageEntryWireV1) -> Result<Self, Self::Error> {
        Self::new(value.class, value.state, value.reasons, value.exclusion_id)
    }
}

impl CustodyCoverageEntryV1 {
    pub fn new(
        class: CustodyCoverageClassV1,
        state: CustodyStateClassV1,
        reasons: Vec<CustodyReasonCodeV1>,
        exclusion_id: Option<String>,
    ) -> Result<Self, CustodySealErrorV1> {
        let reasons = canonical_reasons(state, reasons)?;
        match (state, exclusion_id.as_deref()) {
            (CustodyStateClassV1::ExcludedReproducible, None | Some("")) => {
                return Err(CustodySealErrorV1::ExcludedWithoutExclusion);
            }
            (CustodyStateClassV1::ExcludedReproducible, Some(_)) => {}
            (_, Some(_)) => return Err(CustodySealErrorV1::UnexpectedExclusion),
            (_, None) => {}
        }
        Ok(Self {
            class,
            state,
            reasons,
            exclusion_id,
        })
    }

    #[must_use]
    pub const fn class(&self) -> CustodyCoverageClassV1 {
        self.class
    }

    #[must_use]
    pub const fn state(&self) -> CustodyStateClassV1 {
        self.state
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustodyGitObjectFormatV1 {
    Sha1,
    Sha256,
}

impl CustodyGitObjectFormatV1 {
    const fn hex_length(self) -> usize {
        match self {
            Self::Sha1 => 40,
            Self::Sha256 => 64,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustodyGitObjectKindV1 {
    Commit,
    Tree,
    Blob,
    Tag,
    Unknown,
}

#[derive(Deserialize)]
struct CustodyOriginalObjectWireV1 {
    format: CustodyGitObjectFormatV1,
    object_id: String,
    kind: CustodyGitObjectKindV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CustodyOriginalObjectWireV1")]
pub struct CustodyOriginalObjectV1 {
    format: CustodyGitObjectFormatV1,
    object_id: String,
    kind: CustodyGitObjectKindV1,
}

impl TryFrom<CustodyOriginalObjectWireV1> for CustodyOriginalObjectV1 {
    type Error = CustodySealErrorV1;

    fn try_from(value: CustodyOriginalObjectWireV1) -> Result<Self, Self::Error> {
        Self::new(value.format, value.object_id, value.kind)
    }
}

impl CustodyOriginalObjectV1 {
    pub fn new(
        format: CustodyGitObjectFormatV1,
        object_id: impl Into<String>,
        kind: CustodyGitObjectKindV1,
    ) -> Result<Self, CustodySealErrorV1> {
        let object_id = object_id.into();
        if object_id.len() != format.hex_length()
            || !object_id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(CustodySealErrorV1::InvalidGitObjectId);
        }
        Ok(Self {
            format,
            object_id,
            kind,
        })
    }

    #[must_use]
    pub const fn kind(&self) -> CustodyGitObjectKindV1 {
        self.kind
    }

    /// Read-only object format, for the crate-private exporter's capability binding.
    ///
    /// The exporter must compare a capture capability's `(format, object_id, kind)` inventory
    /// with the manifest's exactly. Re-parsing the canonical JSON to do that would make the
    /// comparison depend on the encoder rather than on the record, so the two missing components
    /// are exposed here as accessors alongside the existing [`Self::kind`]. Neither changes
    /// validation or the wire format.
    #[must_use]
    pub(crate) const fn format(&self) -> CustodyGitObjectFormatV1 {
        self.format
    }

    /// Read-only object id. See [`Self::format`].
    #[must_use]
    pub(crate) fn object_id(&self) -> &str {
        &self.object_id
    }
}

#[derive(Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
enum CustodyRefTargetWireV1 {
    Direct(CustodyOriginalObjectV1),
    Symbolic(LosslessPathV1),
    Unborn,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CustodyRefTargetWireV1")]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum CustodyRefTargetV1 {
    Direct(CustodyOriginalObjectV1),
    Symbolic(LosslessPathV1),
    Unborn,
}

impl TryFrom<CustodyRefTargetWireV1> for CustodyRefTargetV1 {
    type Error = CustodySealErrorV1;

    fn try_from(value: CustodyRefTargetWireV1) -> Result<Self, Self::Error> {
        match value {
            CustodyRefTargetWireV1::Direct(object) => Ok(Self::Direct(object)),
            CustodyRefTargetWireV1::Symbolic(name) if name.as_bytes().is_empty() => {
                Err(CustodySealErrorV1::EmptySymbolicRefName)
            }
            CustodyRefTargetWireV1::Symbolic(name) => Ok(Self::Symbolic(name)),
            CustodyRefTargetWireV1::Unborn => Ok(Self::Unborn),
        }
    }
}

#[derive(Deserialize)]
struct CustodyOriginalRefWireV1 {
    name: LosslessPathV1,
    target: CustodyRefTargetV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CustodyOriginalRefWireV1")]
pub struct CustodyOriginalRefV1 {
    name: LosslessPathV1,
    target: CustodyRefTargetV1,
}

impl TryFrom<CustodyOriginalRefWireV1> for CustodyOriginalRefV1 {
    type Error = CustodySealErrorV1;

    fn try_from(value: CustodyOriginalRefWireV1) -> Result<Self, Self::Error> {
        Self::new(value.name, value.target)
    }
}

impl CustodyOriginalRefV1 {
    pub fn new(
        name: LosslessPathV1,
        target: CustodyRefTargetV1,
    ) -> Result<Self, CustodySealErrorV1> {
        if name.as_bytes().is_empty() {
            return Err(CustodySealErrorV1::EmptyRefName);
        }
        match &target {
            CustodyRefTargetV1::Direct(object) => {
                CustodyOriginalObjectV1::new(object.format, &object.object_id, object.kind)?;
            }
            CustodyRefTargetV1::Symbolic(name) if name.as_bytes().is_empty() => {
                return Err(CustodySealErrorV1::EmptySymbolicRefName);
            }
            CustodyRefTargetV1::Symbolic(_) | CustodyRefTargetV1::Unborn => {}
        }
        Ok(Self { name, target })
    }
}

#[derive(Deserialize)]
struct CustodyDependencyWireV1 {
    dependency_id: String,
    kind: String,
    binding_digest: Sha256HexV1,
    state: CustodyStateClassV1,
    reasons: Vec<CustodyReasonCodeV1>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CustodyDependencyWireV1")]
pub struct CustodyDependencyV1 {
    dependency_id: String,
    kind: String,
    binding_digest: Sha256HexV1,
    state: CustodyStateClassV1,
    reasons: Vec<CustodyReasonCodeV1>,
}

impl TryFrom<CustodyDependencyWireV1> for CustodyDependencyV1 {
    type Error = CustodySealErrorV1;

    fn try_from(value: CustodyDependencyWireV1) -> Result<Self, Self::Error> {
        Self::new(
            value.dependency_id,
            value.kind,
            value.binding_digest,
            value.state,
            value.reasons,
        )
    }
}

impl CustodyDependencyV1 {
    pub fn new(
        dependency_id: impl Into<String>,
        kind: impl Into<String>,
        binding_digest: Sha256HexV1,
        state: CustodyStateClassV1,
        reasons: Vec<CustodyReasonCodeV1>,
    ) -> Result<Self, CustodySealErrorV1> {
        let dependency_id = dependency_id.into();
        let kind = kind.into();
        if dependency_id.is_empty() || kind.is_empty() {
            return Err(CustodySealErrorV1::EmptyDependencyIdentity);
        }
        let reasons = canonical_reasons(state, reasons)?;
        Ok(Self {
            dependency_id,
            kind,
            binding_digest,
            state,
            reasons,
        })
    }
}

#[derive(Deserialize)]
struct CustodyExclusionWireV1 {
    exclusion_id: String,
    content_class: String,
    policy_version: String,
    reconstruction_dependency_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CustodyExclusionWireV1")]
pub struct CustodyExclusionV1 {
    exclusion_id: String,
    content_class: String,
    policy_version: String,
    reconstruction_dependency_ids: Vec<String>,
}

impl TryFrom<CustodyExclusionWireV1> for CustodyExclusionV1 {
    type Error = CustodySealErrorV1;

    fn try_from(value: CustodyExclusionWireV1) -> Result<Self, Self::Error> {
        Self::new(
            value.exclusion_id,
            value.content_class,
            value.policy_version,
            value.reconstruction_dependency_ids,
        )
    }
}

impl CustodyExclusionV1 {
    pub fn new(
        exclusion_id: impl Into<String>,
        content_class: impl Into<String>,
        policy_version: impl Into<String>,
        reconstruction_dependency_ids: Vec<String>,
    ) -> Result<Self, CustodySealErrorV1> {
        let exclusion_id = exclusion_id.into();
        let content_class = content_class.into();
        let policy_version = policy_version.into();
        if exclusion_id.is_empty() || content_class.is_empty() || policy_version.is_empty() {
            return Err(CustodySealErrorV1::EmptyExclusionIdentity);
        }
        if reconstruction_dependency_ids.is_empty() {
            return Err(CustodySealErrorV1::NoReconstructionDependencies);
        }
        if reconstruction_dependency_ids.iter().any(String::is_empty) {
            return Err(CustodySealErrorV1::EmptyReconstructionDependency);
        }
        let reconstruction_dependency_ids = BTreeSet::from_iter(reconstruction_dependency_ids)
            .into_iter()
            .collect();
        Ok(Self {
            exclusion_id,
            content_class,
            policy_version,
            reconstruction_dependency_ids,
        })
    }
}

#[derive(Deserialize)]
struct CustodyManifestWireV1 {
    schema: String,
    unit_id: String,
    run_id: String,
    materialization_id: String,
    generation_id: String,
    coverage: Vec<CustodyCoverageEntryV1>,
    original_refs: Vec<CustodyOriginalRefV1>,
    original_objects: Vec<CustodyOriginalObjectV1>,
    dependencies: Vec<CustodyDependencyV1>,
    exclusions: Vec<CustodyExclusionV1>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CustodyManifestWireV1")]
pub struct CustodyManifestV1 {
    schema: String,
    unit_id: String,
    run_id: String,
    materialization_id: String,
    generation_id: String,
    coverage: Vec<CustodyCoverageEntryV1>,
    original_refs: Vec<CustodyOriginalRefV1>,
    original_objects: Vec<CustodyOriginalObjectV1>,
    dependencies: Vec<CustodyDependencyV1>,
    exclusions: Vec<CustodyExclusionV1>,
}

impl TryFrom<CustodyManifestWireV1> for CustodyManifestV1 {
    type Error = CustodySealErrorV1;

    fn try_from(value: CustodyManifestWireV1) -> Result<Self, Self::Error> {
        if value.schema != MANIFEST_SCHEMA_V1 {
            return Err(CustodySealErrorV1::WrongManifestSchema);
        }
        Self::new(
            value.unit_id,
            value.run_id,
            value.materialization_id,
            value.generation_id,
            value.coverage,
            value.original_refs,
            value.original_objects,
            value.dependencies,
            value.exclusions,
        )
    }
}

impl CustodyManifestV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        unit_id: impl Into<String>,
        run_id: impl Into<String>,
        materialization_id: impl Into<String>,
        generation_id: impl Into<String>,
        mut coverage: Vec<CustodyCoverageEntryV1>,
        original_refs: Vec<CustodyOriginalRefV1>,
        original_objects: Vec<CustodyOriginalObjectV1>,
        dependencies: Vec<CustodyDependencyV1>,
        exclusions: Vec<CustodyExclusionV1>,
    ) -> Result<Self, CustodySealErrorV1> {
        let unit_id = unit_id.into();
        let run_id = run_id.into();
        let materialization_id = materialization_id.into();
        let generation_id = generation_id.into();
        if [&unit_id, &run_id, &materialization_id, &generation_id]
            .into_iter()
            .any(|value| value.is_empty())
        {
            return Err(CustodySealErrorV1::EmptyManifestIdentity);
        }

        let mut canonical_coverage = Vec::with_capacity(coverage.len());
        for row in coverage.drain(..) {
            canonical_coverage.push(CustodyCoverageEntryV1::new(
                row.class,
                row.state,
                row.reasons,
                row.exclusion_id,
            )?);
        }
        canonical_coverage.sort_by_key(|row| row.class);
        if canonical_coverage
            .windows(2)
            .any(|pair| pair[0].class == pair[1].class)
        {
            return Err(CustodySealErrorV1::DuplicateCoverageClass);
        }
        if canonical_coverage.len() != CustodyCoverageClassV1::ALL.len()
            || !canonical_coverage
                .iter()
                .map(|row| row.class)
                .eq(CustodyCoverageClassV1::ALL)
        {
            return Err(CustodySealErrorV1::IncompleteCoverage);
        }

        let original_refs = canonical_refs(original_refs)?;
        let original_objects = canonical_objects(original_objects)?;
        let dependencies = canonical_dependencies(dependencies)?;
        let exclusions = canonical_exclusions(exclusions)?;

        validate_cross_references(
            &canonical_coverage,
            &original_refs,
            &original_objects,
            &dependencies,
            &exclusions,
        )?;

        Ok(Self {
            schema: MANIFEST_SCHEMA_V1.to_owned(),
            unit_id,
            run_id,
            materialization_id,
            generation_id,
            coverage: canonical_coverage,
            original_refs,
            original_objects,
            dependencies,
            exclusions,
        })
    }

    pub fn validate(&self) -> Result<(), CustodySealErrorV1> {
        if self.schema != MANIFEST_SCHEMA_V1 {
            return Err(CustodySealErrorV1::WrongManifestSchema);
        }
        let canonical = Self::new(
            &self.unit_id,
            &self.run_id,
            &self.materialization_id,
            &self.generation_id,
            self.coverage.clone(),
            self.original_refs.clone(),
            self.original_objects.clone(),
            self.dependencies.clone(),
            self.exclusions.clone(),
        )?;
        if canonical != *self {
            return Err(CustodySealErrorV1::NonCanonicalRecord);
        }
        Ok(())
    }

    #[must_use]
    pub fn sealability(&self) -> CustodySealabilityV1 {
        let reasons = self
            .coverage
            .iter()
            .filter(|row| row.state == CustodyStateClassV1::Unresolved)
            .flat_map(|row| row.reasons.iter().copied())
            .chain(
                self.dependencies
                    .iter()
                    .filter(|dependency| dependency.state == CustodyStateClassV1::Unresolved)
                    .flat_map(|dependency| dependency.reasons.iter().copied()),
            )
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if reasons.is_empty() {
            CustodySealabilityV1::Sealable
        } else {
            CustodySealabilityV1::Blocked(reasons)
        }
    }

    pub fn encode_canonical(&self) -> Result<Vec<u8>, CustodySealErrorV1> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|_| CustodySealErrorV1::CanonicalEncoding)
    }

    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, CustodySealErrorV1> {
        let wire: CustodyManifestWireV1 =
            serde_json::from_slice(bytes).map_err(|_| CustodySealErrorV1::InvalidEncoding)?;
        let value = Self::try_from(wire)?;
        if value.encode_canonical()? != bytes {
            return Err(CustodySealErrorV1::NonCanonicalEncoding);
        }
        Ok(value)
    }

    pub fn content_digest(&self) -> Result<Sha256HexV1, CustodySealErrorV1> {
        Ok(Sha256HexV1::digest(&self.encode_canonical()?))
    }

    #[must_use]
    pub fn coverage(&self) -> &[CustodyCoverageEntryV1] {
        &self.coverage
    }

    #[must_use]
    pub fn original_objects(&self) -> &[CustodyOriginalObjectV1] {
        &self.original_objects
    }

    /// Read-only manifest identity, for the crate-private exporter's capability binding.
    ///
    /// The exporter must prove a generation-bound capture capability describes the same
    /// generation as the manifest before any write. These four accessors make that a record
    /// comparison rather than a canonical-JSON re-parse. None of them changes validation or the
    /// wire format.
    #[must_use]
    pub(crate) fn unit_id(&self) -> &str {
        &self.unit_id
    }

    /// See [`Self::unit_id`].
    #[must_use]
    pub(crate) fn run_id(&self) -> &str {
        &self.run_id
    }

    /// See [`Self::unit_id`].
    #[must_use]
    pub(crate) fn materialization_id(&self) -> &str {
        &self.materialization_id
    }

    /// See [`Self::unit_id`].
    #[must_use]
    pub(crate) fn generation_id(&self) -> &str {
        &self.generation_id
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CustodySealabilityV1 {
    Sealable,
    Blocked(Vec<CustodyReasonCodeV1>),
}

#[derive(Deserialize)]
struct CustodySealedArtifactWireV1 {
    name: LosslessPathV1,
    byte_length: u64,
    sha256: Sha256HexV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CustodySealedArtifactWireV1")]
pub struct CustodySealedArtifactV1 {
    name: LosslessPathV1,
    byte_length: u64,
    sha256: Sha256HexV1,
}

impl TryFrom<CustodySealedArtifactWireV1> for CustodySealedArtifactV1 {
    type Error = CustodySealErrorV1;

    fn try_from(value: CustodySealedArtifactWireV1) -> Result<Self, Self::Error> {
        Self::new(value.name, value.byte_length, value.sha256)
    }
}

impl CustodySealedArtifactV1 {
    pub fn new(
        name: LosslessPathV1,
        byte_length: u64,
        sha256: Sha256HexV1,
    ) -> Result<Self, CustodySealErrorV1> {
        validate_artifact_name(name.as_bytes())?;
        Ok(Self {
            name,
            byte_length,
            sha256,
        })
    }

    #[must_use]
    pub fn name(&self) -> &LosslessPathV1 {
        &self.name
    }

    #[must_use]
    pub const fn byte_length(&self) -> u64 {
        self.byte_length
    }

    #[must_use]
    pub fn sha256(&self) -> &Sha256HexV1 {
        &self.sha256
    }
}

#[derive(Deserialize)]
struct CustodySealWireV1 {
    schema: String,
    manifest_digest: Sha256HexV1,
    artifacts: Vec<CustodySealedArtifactV1>,
    encryption_recipients: Vec<String>,
    capsule_format: String,
    sealing_tool: String,
    sealing_tool_version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CustodySealWireV1")]
pub struct CustodySealV1 {
    schema: String,
    manifest_digest: Sha256HexV1,
    artifacts: Vec<CustodySealedArtifactV1>,
    encryption_recipients: Vec<String>,
    capsule_format: String,
    sealing_tool: String,
    sealing_tool_version: String,
}

impl TryFrom<CustodySealWireV1> for CustodySealV1 {
    type Error = CustodySealErrorV1;

    fn try_from(value: CustodySealWireV1) -> Result<Self, Self::Error> {
        if value.schema != SEAL_SCHEMA_V1 {
            return Err(CustodySealErrorV1::WrongSealSchema);
        }
        Self::new(
            value.manifest_digest,
            value.artifacts,
            value.encryption_recipients,
            value.capsule_format,
            value.sealing_tool,
            value.sealing_tool_version,
        )
    }
}

impl CustodySealV1 {
    pub fn new(
        manifest_digest: Sha256HexV1,
        artifacts: Vec<CustodySealedArtifactV1>,
        encryption_recipients: Vec<String>,
        capsule_format: impl Into<String>,
        sealing_tool: impl Into<String>,
        sealing_tool_version: impl Into<String>,
    ) -> Result<Self, CustodySealErrorV1> {
        if artifacts.is_empty() {
            return Err(CustodySealErrorV1::NoArtifacts);
        }
        let artifacts = canonical_artifacts(artifacts)?;
        if encryption_recipients.is_empty() {
            return Err(CustodySealErrorV1::NoRecipients);
        }
        if encryption_recipients.iter().any(String::is_empty) {
            return Err(CustodySealErrorV1::EmptyRecipient);
        }
        let encryption_recipients = BTreeSet::from_iter(encryption_recipients)
            .into_iter()
            .collect();
        let capsule_format = capsule_format.into();
        let sealing_tool = sealing_tool.into();
        let sealing_tool_version = sealing_tool_version.into();
        if capsule_format.is_empty() {
            return Err(CustodySealErrorV1::EmptyCapsuleFormat);
        }
        if sealing_tool.is_empty() {
            return Err(CustodySealErrorV1::EmptyTool);
        }
        if sealing_tool_version.is_empty() {
            return Err(CustodySealErrorV1::EmptyToolVersion);
        }
        Ok(Self {
            schema: SEAL_SCHEMA_V1.to_owned(),
            manifest_digest,
            artifacts,
            encryption_recipients,
            capsule_format,
            sealing_tool,
            sealing_tool_version,
        })
    }

    pub fn validate(&self) -> Result<(), CustodySealErrorV1> {
        self.validate_borrowed_canonical()?;
        let canonical = Self::new(
            self.manifest_digest.clone(),
            self.artifacts.clone(),
            self.encryption_recipients.clone(),
            &self.capsule_format,
            &self.sealing_tool,
            &self.sealing_tool_version,
        )?;
        if canonical != *self {
            return Err(CustodySealErrorV1::NonCanonicalRecord);
        }
        Ok(())
    }

    pub fn validate_borrowed_canonical(&self) -> Result<(), CustodySealErrorV1> {
        if self.schema != SEAL_SCHEMA_V1 {
            return Err(CustodySealErrorV1::WrongSealSchema);
        }
        if self.artifacts.is_empty() {
            return Err(CustodySealErrorV1::NoArtifacts);
        }
        for artifact in &self.artifacts {
            validate_artifact_name(artifact.name().as_bytes())?;
        }
        for window in self.artifacts.windows(2) {
            if window[0].name().as_bytes() >= window[1].name().as_bytes() {
                return Err(CustodySealErrorV1::NonCanonicalRecord);
            }
        }
        for (left_index, left) in self.artifacts.iter().enumerate() {
            let left_name = left.name().as_bytes();
            for right in self.artifacts.iter().skip(left_index + 1) {
                let right_name = right.name().as_bytes();
                if right_name.len() > left_name.len()
                    && right_name.starts_with(left_name)
                    && right_name.get(left_name.len()) == Some(&b'/')
                {
                    return Err(CustodySealErrorV1::ArtifactPrefixConflict);
                }
            }
        }
        if self.encryption_recipients.is_empty() {
            return Err(CustodySealErrorV1::NoRecipients);
        }
        for recipient in &self.encryption_recipients {
            if recipient.is_empty() {
                return Err(CustodySealErrorV1::EmptyRecipient);
            }
        }
        for window in self.encryption_recipients.windows(2) {
            if window[0] >= window[1] {
                return Err(CustodySealErrorV1::NonCanonicalRecord);
            }
        }
        if self.capsule_format.is_empty() {
            return Err(CustodySealErrorV1::EmptyCapsuleFormat);
        }
        if self.sealing_tool.is_empty() {
            return Err(CustodySealErrorV1::EmptyTool);
        }
        if self.sealing_tool_version.is_empty() {
            return Err(CustodySealErrorV1::EmptyToolVersion);
        }
        Ok(())
    }

    pub fn encode_canonical(&self) -> Result<Vec<u8>, CustodySealErrorV1> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|_| CustodySealErrorV1::CanonicalEncoding)
    }

    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, CustodySealErrorV1> {
        let wire: CustodySealWireV1 =
            serde_json::from_slice(bytes).map_err(|_| CustodySealErrorV1::InvalidEncoding)?;
        let value = Self::try_from(wire)?;
        if value.encode_canonical()? != bytes {
            return Err(CustodySealErrorV1::NonCanonicalEncoding);
        }
        Ok(value)
    }

    pub fn content_digest(&self) -> Result<Sha256HexV1, CustodySealErrorV1> {
        Ok(Sha256HexV1::digest(&self.encode_canonical()?))
    }

    #[must_use]
    pub fn manifest_digest(&self) -> &Sha256HexV1 {
        &self.manifest_digest
    }

    #[must_use]
    pub fn artifacts(&self) -> &[CustodySealedArtifactV1] {
        &self.artifacts
    }

    #[must_use]
    pub fn encryption_recipients(&self) -> &[String] {
        &self.encryption_recipients
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

fn canonical_reasons(
    state: CustodyStateClassV1,
    reasons: Vec<CustodyReasonCodeV1>,
) -> Result<Vec<CustodyReasonCodeV1>, CustodySealErrorV1> {
    let reasons = BTreeSet::from_iter(reasons).into_iter().collect::<Vec<_>>();
    match (state, reasons.is_empty()) {
        (CustodyStateClassV1::Unresolved, true) => Err(CustodySealErrorV1::UnresolvedWithoutReason),
        (CustodyStateClassV1::Unresolved, false) | (_, true) => Ok(reasons),
        (_, false) => Err(CustodySealErrorV1::ResolvedWithBlockingReasons),
    }
}

fn canonical_refs(
    refs: Vec<CustodyOriginalRefV1>,
) -> Result<Vec<CustodyOriginalRefV1>, CustodySealErrorV1> {
    let mut by_name = BTreeMap::<LosslessPathV1, CustodyOriginalRefV1>::new();
    for record in refs {
        let record = CustodyOriginalRefV1::new(record.name, record.target)?;
        match by_name.get(&record.name) {
            Some(existing) if existing != &record => {
                return Err(CustodySealErrorV1::ConflictingRefName);
            }
            Some(_) => {}
            None => {
                by_name.insert(record.name.clone(), record);
            }
        }
    }
    Ok(by_name.into_values().collect())
}

fn canonical_objects(
    objects: Vec<CustodyOriginalObjectV1>,
) -> Result<Vec<CustodyOriginalObjectV1>, CustodySealErrorV1> {
    let mut by_identity = BTreeMap::new();
    for object in objects {
        let object = CustodyOriginalObjectV1::new(object.format, object.object_id, object.kind)?;
        let key = (object.format, object.object_id.clone());
        match by_identity.get(&key) {
            Some(existing) if existing != &object => {
                return Err(CustodySealErrorV1::ConflictingObjectKind);
            }
            Some(_) => {}
            None => {
                by_identity.insert(key, object);
            }
        }
    }
    Ok(by_identity.into_values().collect())
}

fn canonical_dependencies(
    dependencies: Vec<CustodyDependencyV1>,
) -> Result<Vec<CustodyDependencyV1>, CustodySealErrorV1> {
    let mut by_id = BTreeMap::new();
    for dependency in dependencies {
        let dependency = CustodyDependencyV1::new(
            dependency.dependency_id,
            dependency.kind,
            dependency.binding_digest,
            dependency.state,
            dependency.reasons,
        )?;
        match by_id.get(&dependency.dependency_id) {
            Some(existing) if existing != &dependency => {
                return Err(CustodySealErrorV1::ConflictingDependencyId);
            }
            Some(_) => {}
            None => {
                by_id.insert(dependency.dependency_id.clone(), dependency);
            }
        }
    }
    Ok(by_id.into_values().collect())
}

fn canonical_exclusions(
    exclusions: Vec<CustodyExclusionV1>,
) -> Result<Vec<CustodyExclusionV1>, CustodySealErrorV1> {
    let mut by_id = BTreeMap::new();
    for exclusion in exclusions {
        let exclusion = CustodyExclusionV1::new(
            exclusion.exclusion_id,
            exclusion.content_class,
            exclusion.policy_version,
            exclusion.reconstruction_dependency_ids,
        )?;
        match by_id.get(&exclusion.exclusion_id) {
            Some(existing) if existing != &exclusion => {
                return Err(CustodySealErrorV1::ConflictingExclusionId);
            }
            Some(_) => {}
            None => {
                by_id.insert(exclusion.exclusion_id.clone(), exclusion);
            }
        }
    }
    Ok(by_id.into_values().collect())
}

fn validate_cross_references(
    coverage: &[CustodyCoverageEntryV1],
    original_refs: &[CustodyOriginalRefV1],
    original_objects: &[CustodyOriginalObjectV1],
    dependencies: &[CustodyDependencyV1],
    exclusions: &[CustodyExclusionV1],
) -> Result<(), CustodySealErrorV1> {
    if original_refs.iter().any(|record| {
        matches!(
            &record.target,
            CustodyRefTargetV1::Direct(target) if !original_objects.contains(target)
        )
    }) {
        return Err(CustodySealErrorV1::RefTargetObjectMissing);
    }

    let dependency_ids = dependencies
        .iter()
        .map(|dependency| dependency.dependency_id.as_str())
        .collect::<BTreeSet<_>>();
    for exclusion in exclusions {
        if exclusion
            .reconstruction_dependency_ids
            .iter()
            .any(|id| !dependency_ids.contains(id.as_str()))
        {
            return Err(CustodySealErrorV1::UnknownReconstructionDependency);
        }
    }

    let exclusion_ids = exclusions
        .iter()
        .map(|exclusion| exclusion.exclusion_id.as_str())
        .collect::<BTreeSet<_>>();
    let referenced = coverage
        .iter()
        .filter_map(|row| row.exclusion_id.as_deref())
        .collect::<BTreeSet<_>>();
    if referenced.iter().any(|id| !exclusion_ids.contains(id)) {
        return Err(CustodySealErrorV1::UnknownExclusion);
    }
    if exclusion_ids.iter().any(|id| !referenced.contains(id)) {
        return Err(CustodySealErrorV1::UnreferencedExclusion);
    }
    Ok(())
}

fn validate_artifact_name(name: &[u8]) -> Result<(), CustodySealErrorV1> {
    if name.is_empty()
        || name.starts_with(b"/")
        || name.contains(&0)
        || name.contains(&b'\\')
        || (name.get(1) == Some(&b':')
            && name.first().is_some_and(|byte| byte.is_ascii_alphabetic()))
        || name
            .split(|byte| *byte == b'/')
            .any(|component| component.is_empty() || component == b"." || component == b"..")
    {
        return Err(CustodySealErrorV1::UnsafeArtifactName);
    }
    Ok(())
}

fn canonical_artifacts(
    artifacts: Vec<CustodySealedArtifactV1>,
) -> Result<Vec<CustodySealedArtifactV1>, CustodySealErrorV1> {
    let mut by_name = BTreeMap::<LosslessPathV1, CustodySealedArtifactV1>::new();
    for artifact in artifacts {
        let artifact =
            CustodySealedArtifactV1::new(artifact.name, artifact.byte_length, artifact.sha256)?;
        if by_name.insert(artifact.name.clone(), artifact).is_some() {
            return Err(CustodySealErrorV1::DuplicateArtifactName);
        }
    }
    let names = by_name
        .keys()
        .map(|name| name.as_bytes().to_vec())
        .collect::<BTreeSet<_>>();
    for name in &names {
        for separator in name
            .iter()
            .enumerate()
            .filter_map(|(index, byte)| (*byte == b'/').then_some(index))
        {
            if names.contains(&name[..separator]) {
                return Err(CustodySealErrorV1::ArtifactPrefixConflict);
            }
        }
    }
    Ok(by_name.into_values().collect())
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum CustodySealErrorV1 {
    #[error("a manifest identity is empty")]
    EmptyManifestIdentity,
    #[error("custody coverage is incomplete")]
    IncompleteCoverage,
    #[error("a custody coverage class appears more than once")]
    DuplicateCoverageClass,
    #[error("an unresolved row requires a blocking reason")]
    UnresolvedWithoutReason,
    #[error("a resolved row cannot carry blocking reasons")]
    ResolvedWithBlockingReasons,
    #[error("excluded-reproducible coverage requires an exclusion id")]
    ExcludedWithoutExclusion,
    #[error("non-excluded coverage cannot carry an exclusion id")]
    UnexpectedExclusion,
    #[error("a ref name is empty")]
    EmptyRefName,
    #[error("a symbolic-ref target is empty")]
    EmptySymbolicRefName,
    #[error("a Git object id is invalid for its object format")]
    InvalidGitObjectId,
    #[error("one ref name has conflicting targets")]
    ConflictingRefName,
    #[error("one Git object identity has conflicting kinds")]
    ConflictingObjectKind,
    #[error("a direct ref target is absent from the original-object inventory")]
    RefTargetObjectMissing,
    #[error("a dependency id or kind is empty")]
    EmptyDependencyIdentity,
    #[error("one dependency id has conflicting records")]
    ConflictingDependencyId,
    #[error("an exclusion identity field is empty")]
    EmptyExclusionIdentity,
    #[error("an exclusion has no reconstruction dependencies")]
    NoReconstructionDependencies,
    #[error("an exclusion has an empty reconstruction dependency id")]
    EmptyReconstructionDependency,
    #[error("one exclusion id has conflicting records")]
    ConflictingExclusionId,
    #[error("an exclusion names an unknown reconstruction dependency")]
    UnknownReconstructionDependency,
    #[error("coverage names an unknown exclusion")]
    UnknownExclusion,
    #[error("an exclusion is not referenced by coverage")]
    UnreferencedExclusion,
    #[error("manifest schema is unsupported")]
    WrongManifestSchema,
    #[error("seal schema is unsupported")]
    WrongSealSchema,
    #[error("a decoded record is not in canonical field order")]
    NonCanonicalRecord,
    #[error("canonical JSON encoding failed")]
    CanonicalEncoding,
    #[error("JSON decoding failed")]
    InvalidEncoding,
    #[error("input bytes are not the canonical JSON encoding")]
    NonCanonicalEncoding,
    #[error("a seal requires at least one artifact")]
    NoArtifacts,
    #[error("an artifact name is not a safe relative path")]
    UnsafeArtifactName,
    #[error("artifact names must be unique")]
    DuplicateArtifactName,
    #[error("artifact names cannot be path prefixes of other artifacts")]
    ArtifactPrefixConflict,
    #[error("a seal requires at least one encryption recipient")]
    NoRecipients,
    #[error("an encryption recipient is empty")]
    EmptyRecipient,
    #[error("capsule format is empty")]
    EmptyCapsuleFormat,
    #[error("sealing tool is empty")]
    EmptyTool,
    #[error("sealing tool version is empty")]
    EmptyToolVersion,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(byte: u8) -> Sha256HexV1 {
        Sha256HexV1::digest(&[byte])
    }

    #[test]
    fn borrowed_seal_validation_rejects_noncanonical_artifact_order() {
        let name_a = LosslessPathV1::from_bytes(b"a.enc".to_vec());
        let name_b = LosslessPathV1::from_bytes(b"b.enc".to_vec());
        let seal = CustodySealV1 {
            schema: SEAL_SCHEMA_V1.to_owned(),
            manifest_digest: digest(1),
            artifacts: vec![
                CustodySealedArtifactV1::new(name_b, 1, digest(2)).unwrap(),
                CustodySealedArtifactV1::new(name_a, 1, digest(3)).unwrap(),
            ],
            encryption_recipients: vec!["recipient".to_owned()],
            capsule_format: "capsule-v1".to_owned(),
            sealing_tool: "tool".to_owned(),
            sealing_tool_version: "1".to_owned(),
        };

        assert_eq!(
            seal.validate_borrowed_canonical().unwrap_err(),
            CustodySealErrorV1::NonCanonicalRecord
        );
    }

    fn coverage_all(state: CustodyStateClassV1) -> Vec<CustodyCoverageEntryV1> {
        CustodyCoverageClassV1::ALL
            .into_iter()
            .map(|class| CustodyCoverageEntryV1::new(class, state, vec![], None).unwrap())
            .collect()
    }

    /// The four manifest-identity accessors report the constructor's own values, and each one
    /// reports a DIFFERENT field: a manifest whose four identities are pairwise distinct pins
    /// the mapping, so an accessor wired to the wrong field is visible here rather than in the
    /// exporter's capability binding.
    #[test]
    fn crate_private_manifest_identity_accessors_report_each_distinct_field() {
        let manifest = CustodyManifestV1::new(
            "unit-a",
            "run-b",
            "materialization-c",
            "generation-d",
            coverage_all(CustodyStateClassV1::Empty),
            vec![],
            vec![],
            vec![],
            vec![],
        )
        .unwrap();

        assert_eq!(manifest.unit_id(), "unit-a");
        assert_eq!(manifest.run_id(), "run-b");
        assert_eq!(manifest.materialization_id(), "materialization-c");
        assert_eq!(manifest.generation_id(), "generation-d");

        let observed = [
            manifest.unit_id(),
            manifest.run_id(),
            manifest.materialization_id(),
            manifest.generation_id(),
        ];
        assert_eq!(
            BTreeSet::from(observed).len(),
            4,
            "the four identity accessors must not alias one field"
        );
    }

    /// `format` and `object_id` report the constructor's own values for both object formats, and
    /// the existing `kind` accessor still reports the third component. Together they are the
    /// exact `(format, object_id, kind)` triple the exporter compares.
    #[test]
    fn crate_private_original_object_accessors_report_format_and_object_id() {
        let sha1 = CustodyOriginalObjectV1::new(
            CustodyGitObjectFormatV1::Sha1,
            "a".repeat(40),
            CustodyGitObjectKindV1::Blob,
        )
        .unwrap();
        assert_eq!(sha1.format(), CustodyGitObjectFormatV1::Sha1);
        assert_eq!(sha1.object_id(), "a".repeat(40));
        assert_eq!(sha1.kind(), CustodyGitObjectKindV1::Blob);

        let sha256 = CustodyOriginalObjectV1::new(
            CustodyGitObjectFormatV1::Sha256,
            "b".repeat(64),
            CustodyGitObjectKindV1::Tree,
        )
        .unwrap();
        assert_eq!(sha256.format(), CustodyGitObjectFormatV1::Sha256);
        assert_eq!(sha256.object_id(), "b".repeat(64));
        assert_eq!(sha256.kind(), CustodyGitObjectKindV1::Tree);

        assert_ne!(sha1.format(), sha256.format());
        assert_ne!(sha1.object_id(), sha256.object_id());
    }

    /// The accessors are read-only: they neither validate nor canonicalize, so the records they
    /// describe still encode byte for byte as they did before the accessors were read.
    #[test]
    fn crate_private_accessors_do_not_change_the_canonical_encoding() {
        let manifest = CustodyManifestV1::new(
            "unit",
            "run",
            "materialization",
            "generation",
            coverage_all(CustodyStateClassV1::Empty),
            vec![],
            vec![],
            vec![],
            vec![],
        )
        .unwrap();
        let before = manifest.encode_canonical().unwrap();
        let _ = (
            manifest.unit_id(),
            manifest.run_id(),
            manifest.materialization_id(),
            manifest.generation_id(),
        );
        assert_eq!(manifest.encode_canonical().unwrap(), before);
    }
}
