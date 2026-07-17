//! R3c floating-current recipe and exact resolution contracts.
//!
//! This module intentionally owns no subprocess or filesystem-write implementation yet. The contract
//! slice makes recipe and completed-resolution evidence strict before a later slice is allowed to add
//! registry, package-tree, or container-runtime effects.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt as _;

use crate::{compatibility, local_file, BoxError};

pub(super) const DEFAULT_RECIPES: &str = "compatibility/floating-current.toml";

const MAX_RECIPE_BYTES: u64 = 1024 * 1024;
const MAX_RESOLUTION_BYTES: u64 = 16 * 1024 * 1024;
const MAX_LOCK_BYTES: u64 = 32 * 1024 * 1024;
const MAX_EXECUTABLE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;
const MAX_COMMAND_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_ID_BYTES: usize = 128;
const MAX_TEXT_BYTES: usize = 4096;
const MAX_PACKAGE_SETS: usize = 16;
const MAX_IMAGES: usize = 8;
const MAX_CASES: usize = 32;
const MAX_PREREQUISITES_PER_CASE: usize = 32;
const MAX_PROTECTED_INPUTS: usize = 256;
const MAX_OWNED_RESOURCES: usize = 256;
const MAX_TIMEOUT_SECS: u64 = 3600;
const MAX_DOWNLOAD_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_UNPACKED_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_FILES: u64 = 500_000;
const MAX_RETENTION_DAYS: u16 = 90;
const NODE_READER_BASE: &str = "docker.io/library/node:24-slim";
const NPM_REGISTRY: &str = "https://registry.npmjs.org/";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(super) enum RecipeEcosystem {
    Npm,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(super) enum RecipeRegistry {
    Npmjs,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(super) enum ImageTemplate {
    NodeAcpReaderV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(super) enum FloatingTarget {
    HostPackageTree,
    ContainerRoImage,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(super) enum ConfigTemplate {
    CodexHostReadOnlyV1,
    CodexReaderReadOnlyV1,
    ClaudeHostReadOnlyV1,
    ClaudeReaderReadOnlyV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ResolutionLimits {
    pub(super) timeout_secs: u64,
    pub(super) max_download_bytes: u64,
    pub(super) max_unpacked_bytes: u64,
    pub(super) max_files: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ResolutionRedactionPolicy {
    Strict,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResolutionArtifactPolicy {
    pub(super) retention_days: u16,
    pub(super) redaction: ResolutionRedactionPolicy,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct PackageSetRecipe {
    pub(super) id: String,
    pub(super) ecosystem: RecipeEcosystem,
    pub(super) registry: RecipeRegistry,
    pub(super) adapter: String,
    pub(super) adapter_selector: String,
    pub(super) agent_cli: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ImageRecipe {
    pub(super) id: String,
    pub(super) template: ImageTemplate,
    pub(super) base: String,
    pub(super) package_sets: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct FloatingCaseRecipe {
    pub(super) id: String,
    pub(super) baseline_case: String,
    pub(super) package_set: String,
    pub(super) target: FloatingTarget,
    pub(super) config_template: ConfigTemplate,
    #[serde(default)]
    pub(super) image: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FloatingRecipeManifest {
    pub(super) schema_version: u16,
    pub(super) production_manifest: PathBuf,
    pub(super) limits: ResolutionLimits,
    pub(super) artifact: ResolutionArtifactPolicy,
    #[serde(default)]
    pub(super) package_sets: Vec<PackageSetRecipe>,
    #[serde(default)]
    pub(super) images: Vec<ImageRecipe>,
    #[serde(default)]
    pub(super) cases: Vec<FloatingCaseRecipe>,
}

pub(super) struct LoadedRecipes {
    pub(super) recipes: FloatingRecipeManifest,
    pub(super) canonical_path: PathBuf,
    pub(super) canonical_path_text: String,
    pub(super) sha256: String,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct ResolutionRequiredEnvironmentInput {
    pub(super) name: String,
    pub(super) one_of: Vec<String>,
}

#[derive(Clone, Debug)]
pub(super) struct ResolutionArtifactInput {
    pub(super) retention_days: u16,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct ResolutionBudgetInput {
    pub(super) timeout_secs: u64,
    pub(super) max_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) max_cost_usd: Option<f64>,
}

#[derive(Clone, Debug)]
pub(super) struct BaselineConfigInput {
    pub(super) canonical_path: PathBuf,
    pub(super) sha256: String,
    pub(super) bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub(super) struct ResolutionCaseInput {
    pub(super) recipe: FloatingCaseRecipe,
    pub(super) evidence_path: String,
    pub(super) execution_mode: String,
    pub(super) os: String,
    pub(super) architecture: String,
    pub(super) environment_owner: String,
    pub(super) agent: String,
    pub(super) model: String,
    pub(super) effort: Option<String>,
    pub(super) mode: Option<String>,
    pub(super) session_cwd: Option<PathBuf>,
    pub(super) auth_path: String,
    pub(super) credential_env: Option<String>,
    pub(super) required_env: Vec<ResolutionRequiredEnvironmentInput>,
    pub(super) probe: String,
    pub(super) billable: bool,
    pub(super) timeout_secs: u64,
    pub(super) max_tokens: u64,
    pub(super) max_cost_usd: Option<f64>,
    pub(super) retry_cap: u8,
    pub(super) expected_status: String,
    pub(super) artifact: ResolutionArtifactInput,
    pub(super) baseline_config: BaselineConfigInput,
    pub(super) component_pins: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
pub(super) struct ProtectedFileInput {
    pub(super) canonical_path: PathBuf,
    pub(super) sha256: String,
    pub(super) max_bytes: u64,
}

pub(super) struct ProviderFreeResolutionRequest {
    pub(super) output: PathBuf,
    pub(super) recipes: LoadedRecipes,
    pub(super) production_manifest: VersionedArtifactIdentity,
    pub(super) candidate: ExecutableIdentity,
    pub(super) environment_owner: String,
    pub(super) os: String,
    pub(super) architecture: String,
    pub(super) runtime: RuntimeKind,
    pub(super) runtime_executable: ExecutableIdentity,
    pub(super) base_resolver_executable: PathBuf,
    pub(super) npm_executable: PathBuf,
    pub(super) safe_path: OsString,
    pub(super) budget: ResolutionBudgetInput,
    pub(super) cases: Vec<ResolutionCaseInput>,
    pub(super) protected_inputs: Vec<ProtectedFileInput>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ResolvedBinding {
    pub(super) resolution_id: String,
    pub(super) recipe_sha256: String,
    pub(super) config_sha256: String,
    pub(super) adapter: String,
    pub(super) agent_cli: String,
    pub(super) package_inventory_sha256: String,
    pub(super) package_tree_sha256: String,
    #[serde(default)]
    pub(super) image_digest: Option<String>,
    #[serde(default)]
    pub(super) base_image_digest: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ResolutionState {
    SetupIncomplete,
    Complete,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(super) enum RuntimeKind {
    Docker,
    Podman,
}

impl RuntimeKind {
    pub(super) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "docker" => Ok(Self::Docker),
            "podman" => Ok(Self::Podman),
            _ => Err("compatibility resolve: --runtime must be docker or podman".into()),
        }
    }

    pub(super) fn wire(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Podman => "podman",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ArtifactIdentity {
    pub(super) canonical_path: String,
    pub(super) sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct VersionedArtifactIdentity {
    pub(super) schema_version: u16,
    pub(super) canonical_path: String,
    pub(super) sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ExecutableIdentity {
    pub(super) canonical_path: String,
    pub(super) sha256: String,
    pub(super) byte_length: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ResolutionEnvironment {
    pub(super) environment_owner: String,
    pub(super) os: String,
    pub(super) architecture: String,
    pub(super) runtime: RuntimeKind,
    pub(super) runtime_executable: ExecutableIdentity,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct RequestedPackageSet {
    pub(super) adapter: String,
    pub(super) adapter_selector: String,
    pub(super) agent_cli: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ExactNpmPackage {
    pub(super) name: String,
    pub(super) version: String,
    pub(super) integrity: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ResolvedPackageSet {
    pub(super) id: String,
    pub(super) requested: RequestedPackageSet,
    pub(super) adapter: ExactNpmPackage,
    pub(super) agent_cli: ExactNpmPackage,
    #[serde(default)]
    pub(super) bundled_cli_version: Option<String>,
    pub(super) resolution_lock_sha256: String,
    pub(super) inventory_sha256: String,
    pub(super) tree_sha256: String,
    pub(super) adapter_executable: ArtifactIdentity,
    pub(super) adapter_executable_relative: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ResolvedImage {
    pub(super) id: String,
    pub(super) requested_base: String,
    pub(super) package_sets: Vec<String>,
    pub(super) registry_index_digest: String,
    pub(super) platform_manifest_digest: String,
    pub(super) build_template_sha256: String,
    pub(super) final_image_id: String,
    pub(super) owned_tag: String,
    pub(super) labels: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct NonSecretPrerequisite {
    pub(super) name: String,
    #[serde(default)]
    pub(super) destination: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ResolvedCase {
    pub(super) id: String,
    pub(super) baseline_case: String,
    pub(super) package_set: String,
    #[serde(default)]
    pub(super) image: Option<String>,
    pub(super) model: String,
    #[serde(default)]
    pub(super) effort: Option<String>,
    #[serde(default)]
    pub(super) mode: Option<String>,
    #[serde(default)]
    pub(super) prerequisites: Vec<NonSecretPrerequisite>,
    pub(super) generated_config: ArtifactIdentity,
    pub(super) binding: ResolvedBinding,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum CatalogResolutionState {
    DeferredToAuthorizedSmoke,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ModelCatalogResolution {
    pub(super) state: CatalogResolutionState,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProtectedInput {
    pub(super) path: String,
    pub(super) before_sha256: String,
    pub(super) after_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResolutionFailure {
    pub(super) code: ResolutionFailureCode,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ResolutionFailureCode {
    RecipeInvalid,
    ResolutionAckMissing,
    NpmSpawnFailed,
    NpmTimeout,
    NpmNonzero,
    NpmOutputTooLarge,
    NpmOutputUnreadable,
    NpmDownloadBudgetExceeded,
    BaseDigestUnavailable,
    PackageIdentityMismatch,
    PackageTreeDrift,
    ConfigTemplateMismatch,
    ImageLabelMismatch,
    ImageTagAlreadyExists,
    ImageTagStateUnknown,
    ProtectedStateChanged,
    WriteScopeEscape,
    RuntimeSpawnFailed,
    RuntimeTimeout,
    RuntimeNonzero,
    RuntimeOutputTooLarge,
    RuntimeOutputUnreadable,
    PublicationSetupFailed,
    PublicationResourceFailed,
    PublicationFinalFailed,
    PublicationRenameFailed,
    PublicationDirectorySyncFailed,
    PublicationRollbackFailed,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub(super) enum OwnedResourceKind {
    Bundle,
    ImageTag,
    RuntimeCache,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OwnedResource {
    pub(super) kind: OwnedResourceKind,
    pub(super) identity: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResolutionArtifact {
    pub(super) schema_version: u16,
    pub(super) state: ResolutionState,
    pub(super) resolution_id: String,
    pub(super) recipes: VersionedArtifactIdentity,
    pub(super) production_manifest: VersionedArtifactIdentity,
    pub(super) candidate: ExecutableIdentity,
    pub(super) environment: ResolutionEnvironment,
    pub(super) limits: ResolutionLimits,
    #[serde(default)]
    pub(super) execution_manifest: Option<VersionedArtifactIdentity>,
    #[serde(default)]
    pub(super) packages: Vec<ResolvedPackageSet>,
    #[serde(default)]
    pub(super) images: Vec<ResolvedImage>,
    #[serde(default)]
    pub(super) cases: Vec<ResolvedCase>,
    pub(super) model_catalog: ModelCatalogResolution,
    #[serde(default)]
    pub(super) protected_inputs: Vec<ProtectedInput>,
    #[serde(default)]
    pub(super) failure: Option<ResolutionFailure>,
    #[serde(default)]
    pub(super) owned_resources: Vec<OwnedResource>,
}

pub(super) struct LoadedResolution {
    pub(super) artifact: ResolutionArtifact,
    pub(super) canonical_path: PathBuf,
    pub(super) canonical_path_text: String,
    pub(super) sha256: String,
}

fn bounded_text(label: &str, value: &str, max: usize) -> Result<(), String> {
    if value.is_empty()
        || value.trim() != value
        || value.len() > max
        || value.chars().any(char::is_control)
    {
        return Err(format!(
            "{label} must be non-empty, unpadded, control-free, and at most {max} bytes"
        ));
    }
    if compatibility::looks_like_secret(value) {
        return Err(format!("{label} contains secret-shaped material"));
    }
    Ok(())
}

fn stable_id(label: &str, value: &str) -> Result<(), String> {
    bounded_text(label, value, MAX_ID_BYTES)?;
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return Err(format!("{label} is empty"));
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(format!(
            "{label} must start with a lowercase ASCII letter or digit"
        ));
    }
    if !bytes.all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
    }) {
        return Err(format!(
            "{label} must contain only lowercase ASCII letters, digits, '.', '_', or '-'"
        ));
    }
    Ok(())
}

fn secret_free_raw(label: &str, raw: &str) -> Result<(), String> {
    if compatibility::looks_like_secret(raw) {
        return Err(format!("{label} contains secret-shaped material"));
    }
    Ok(())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value == value.to_ascii_lowercase()
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn validate_sha256(label: &str, value: &str) -> Result<(), String> {
    if valid_sha256(value) {
        Ok(())
    } else {
        Err(format!(
            "{label} must be 64 lowercase hexadecimal characters"
        ))
    }
}

fn valid_image_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(valid_sha256)
}

fn validate_image_digest(label: &str, value: &str) -> Result<(), String> {
    if valid_image_digest(value) {
        Ok(())
    } else {
        Err(format!("{label} must be an immutable sha256 digest"))
    }
}

fn validate_exact_package(label: &str, value: &str) -> Result<(), String> {
    bounded_text(label, value, MAX_TEXT_BYTES)?;
    let Some((name, version)) = value.split_once('=') else {
        return Err(format!("{label} must use exact <package>=<version> form"));
    };
    if name.is_empty() || version.is_empty() || version.contains('=') {
        return Err(format!("{label} must use exact <package>=<version> form"));
    }
    semver::Version::parse(version)
        .map_err(|_| format!("{label} must contain one complete semantic version"))?;
    Ok(())
}

fn validate_exact_npm_package(label: &str, package: &ExactNpmPackage) -> Result<(), String> {
    bounded_text(&format!("{label} name"), &package.name, MAX_TEXT_BYTES)?;
    semver::Version::parse(&package.version)
        .map_err(|_| format!("{label} version must be one complete semantic version"))?;
    bounded_text(
        &format!("{label} integrity"),
        &package.integrity,
        MAX_TEXT_BYTES,
    )?;
    let Some(encoded) = package.integrity.strip_prefix("sha512-") else {
        return Err(format!(
            "{label} integrity must be one canonical sha512 integrity value"
        ));
    };
    // SHA-512 is 64 bytes: canonical padded base64 is exactly 88 characters and ends in `==`.
    let encoded = encoded.as_bytes();
    if encoded.len() != 88
        || !encoded.ends_with(b"==")
        || !encoded[..86]
            .iter()
            .copied()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/'))
    {
        return Err(format!(
            "{label} integrity must be one canonical sha512 integrity value"
        ));
    }
    Ok(())
}

fn validate_limits(label: &str, limits: &ResolutionLimits) -> Result<(), String> {
    if !(1..=MAX_TIMEOUT_SECS).contains(&limits.timeout_secs) {
        return Err(format!(
            "{label}.timeout_secs must be in 1..={MAX_TIMEOUT_SECS}"
        ));
    }
    if limits.max_download_bytes == 0 || limits.max_download_bytes > MAX_DOWNLOAD_BYTES {
        return Err(format!(
            "{label}.max_download_bytes must be in 1..={MAX_DOWNLOAD_BYTES}"
        ));
    }
    if limits.max_unpacked_bytes == 0
        || limits.max_unpacked_bytes > MAX_UNPACKED_BYTES
        || limits.max_unpacked_bytes < limits.max_download_bytes
    {
        return Err(format!(
            "{label}.max_unpacked_bytes must be between max_download_bytes and {MAX_UNPACKED_BYTES}"
        ));
    }
    if limits.max_files == 0 || limits.max_files > MAX_FILES {
        return Err(format!("{label}.max_files must be in 1..={MAX_FILES}"));
    }
    Ok(())
}

fn validate_safe_relative_path(label: &str, path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(format!("{label} must be a non-empty relative path"));
    }
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(format!(
                "{label} must not contain parent, root, or current-directory components"
            ));
        }
    }
    bounded_text(label, &path.to_string_lossy(), MAX_TEXT_BYTES)
}

fn artifact_path(label: &str, path: &Path) -> Result<String, String> {
    let value = path
        .to_str()
        .ok_or_else(|| format!("{label} canonical path must be UTF-8"))?;
    bounded_text(label, value, MAX_TEXT_BYTES)?;
    Ok(value.to_owned())
}

fn expected_package_pair(adapter: &str) -> Option<&'static str> {
    match adapter {
        "@agentclientprotocol/codex-acp" => Some("@openai/codex"),
        "@agentclientprotocol/claude-agent-acp" => Some("@anthropic-ai/claude-agent-sdk"),
        _ => None,
    }
}

fn template_matches_package(template: ConfigTemplate, adapter: &str) -> bool {
    matches!(
        (template, adapter),
        (
            ConfigTemplate::CodexHostReadOnlyV1 | ConfigTemplate::CodexReaderReadOnlyV1,
            "@agentclientprotocol/codex-acp"
        ) | (
            ConfigTemplate::ClaudeHostReadOnlyV1 | ConfigTemplate::ClaudeReaderReadOnlyV1,
            "@agentclientprotocol/claude-agent-acp"
        )
    )
}

fn template_matches_target(template: ConfigTemplate, target: FloatingTarget) -> bool {
    matches!(
        (template, target),
        (
            ConfigTemplate::CodexHostReadOnlyV1 | ConfigTemplate::ClaudeHostReadOnlyV1,
            FloatingTarget::HostPackageTree
        ) | (
            ConfigTemplate::CodexReaderReadOnlyV1 | ConfigTemplate::ClaudeReaderReadOnlyV1,
            FloatingTarget::ContainerRoImage
        )
    )
}

pub(super) fn validate_recipes(recipes: &FloatingRecipeManifest) -> Result<(), String> {
    if recipes.schema_version != 1 {
        return Err("floating recipes: schema_version must be 1".into());
    }
    validate_safe_relative_path(
        "floating recipes production_manifest",
        &recipes.production_manifest,
    )?;
    if recipes.production_manifest != Path::new("manifest.toml") {
        return Err(
            "floating recipes: production_manifest must be the sibling manifest.toml".into(),
        );
    }
    validate_limits("floating recipes: limits", &recipes.limits)?;
    if recipes.artifact.retention_days == 0 || recipes.artifact.retention_days > MAX_RETENTION_DAYS
    {
        return Err(format!(
            "floating recipes: artifact.retention_days must be in 1..={MAX_RETENTION_DAYS}"
        ));
    }
    if recipes.package_sets.is_empty() || recipes.package_sets.len() > MAX_PACKAGE_SETS {
        return Err(format!(
            "floating recipes: package_sets must contain 1..={MAX_PACKAGE_SETS} entries"
        ));
    }
    if recipes.images.len() > MAX_IMAGES {
        return Err(format!(
            "floating recipes: at most {MAX_IMAGES} images are allowed"
        ));
    }
    if recipes.cases.is_empty() || recipes.cases.len() > MAX_CASES {
        return Err(format!(
            "floating recipes: cases must contain 1..={MAX_CASES} entries"
        ));
    }

    let mut package_ids = BTreeSet::new();
    let mut package_by_id = BTreeMap::new();
    for package in &recipes.package_sets {
        stable_id("floating package-set id", &package.id)?;
        if !package_ids.insert(package.id.as_str()) {
            return Err(format!(
                "floating recipes: duplicate package-set id {:?}",
                package.id
            ));
        }
        let expected_cli = expected_package_pair(&package.adapter).ok_or_else(|| {
            format!(
                "floating recipes: package set {:?} uses an unsupported adapter",
                package.id
            )
        })?;
        if package.agent_cli != expected_cli {
            return Err(format!(
                "floating recipes: package set {:?} has the wrong nested agent CLI/SDK",
                package.id
            ));
        }
        if package.adapter_selector != "latest" {
            return Err(format!(
                "floating recipes: package set {:?} adapter_selector must be the reviewed literal \"latest\"",
                package.id
            ));
        }
        package_by_id.insert(package.id.as_str(), package);
    }

    let mut image_ids = BTreeSet::new();
    let mut image_by_id = BTreeMap::new();
    for image in &recipes.images {
        stable_id("floating image id", &image.id)?;
        if !image_ids.insert(image.id.as_str()) {
            return Err(format!(
                "floating recipes: duplicate image id {:?}",
                image.id
            ));
        }
        if image.base != NODE_READER_BASE {
            return Err(format!(
                "floating recipes: image {:?} must use the reviewed Node 24 slim base request",
                image.id
            ));
        }
        if image.package_sets.is_empty() {
            return Err(format!(
                "floating recipes: image {:?} must contain at least one package set",
                image.id
            ));
        }
        let mut seen = BTreeSet::new();
        for package_id in &image.package_sets {
            if !seen.insert(package_id) {
                return Err(format!(
                    "floating recipes: image {:?} repeats package set {:?}",
                    image.id, package_id
                ));
            }
            if !package_by_id.contains_key(package_id.as_str()) {
                return Err(format!(
                    "floating recipes: image {:?} references unknown package set {:?}",
                    image.id, package_id
                ));
            }
        }
        image_by_id.insert(image.id.as_str(), image);
    }

    let mut case_ids = BTreeSet::new();
    let mut baseline_ids = BTreeSet::new();
    for case in &recipes.cases {
        stable_id("floating case id", &case.id)?;
        stable_id("floating baseline case id", &case.baseline_case)?;
        if !case_ids.insert(case.id.as_str()) {
            return Err(format!("floating recipes: duplicate case id {:?}", case.id));
        }
        if !baseline_ids.insert(case.baseline_case.as_str()) {
            return Err(format!(
                "floating recipes: duplicate baseline mapping {:?}",
                case.baseline_case
            ));
        }
        let package = package_by_id
            .get(case.package_set.as_str())
            .ok_or_else(|| {
                format!(
                    "floating recipes: case {:?} references unknown package set {:?}",
                    case.id, case.package_set
                )
            })?;
        if !template_matches_package(case.config_template, &package.adapter)
            || !template_matches_target(case.config_template, case.target)
        {
            return Err(format!(
                "floating recipes: case {:?} config template does not match its package/target",
                case.id
            ));
        }
        match (case.target, case.image.as_deref()) {
            (FloatingTarget::HostPackageTree, None) => {}
            (FloatingTarget::HostPackageTree, Some(_)) => {
                return Err(format!(
                    "floating recipes: host case {:?} must not declare an image",
                    case.id
                ))
            }
            (FloatingTarget::ContainerRoImage, Some(image_id)) => {
                let image = image_by_id.get(image_id).ok_or_else(|| {
                    format!(
                        "floating recipes: case {:?} references unknown image {:?}",
                        case.id, image_id
                    )
                })?;
                if !image
                    .package_sets
                    .iter()
                    .any(|candidate| candidate == &case.package_set)
                {
                    return Err(format!(
                        "floating recipes: case {:?} image does not contain its package set",
                        case.id
                    ));
                }
            }
            (FloatingTarget::ContainerRoImage, None) => {
                return Err(format!(
                    "floating recipes: container case {:?} requires an image",
                    case.id
                ))
            }
        }
    }
    Ok(())
}

pub(super) fn load_recipes(path: &Path) -> Result<LoadedRecipes, BoxError> {
    let snapshot =
        local_file::read_regular_file_bounded(path, "floating recipes", MAX_RECIPE_BYTES)?;
    let canonical_path_text = artifact_path("floating recipes", &snapshot.canonical_path)?;
    let raw =
        std::str::from_utf8(&snapshot.bytes).map_err(|_| "floating recipes: file must be UTF-8")?;
    secret_free_raw("floating recipes", raw)?;
    let recipes: FloatingRecipeManifest =
        toml::from_str(raw).map_err(|error| format!("floating recipes: invalid TOML: {error}"))?;
    validate_recipes(&recipes)?;
    Ok(LoadedRecipes {
        recipes,
        canonical_path: snapshot.canonical_path,
        canonical_path_text,
        sha256: snapshot.sha256,
    })
}

pub(super) fn production_manifest_path(recipes: &LoadedRecipes) -> PathBuf {
    recipes
        .canonical_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(&recipes.recipes.production_manifest)
}

pub(super) fn validate_resolved_binding(
    binding: &ResolvedBinding,
    container: bool,
    expected_image_digest: Option<&str>,
) -> Result<(), String> {
    stable_id("resolved binding resolution_id", &binding.resolution_id)?;
    for (label, digest) in [
        ("resolved binding recipe_sha256", &binding.recipe_sha256),
        ("resolved binding config_sha256", &binding.config_sha256),
        (
            "resolved binding package_inventory_sha256",
            &binding.package_inventory_sha256,
        ),
        (
            "resolved binding package_tree_sha256",
            &binding.package_tree_sha256,
        ),
    ] {
        validate_sha256(label, digest)?;
    }
    validate_exact_package("resolved binding adapter", &binding.adapter)?;
    validate_exact_package("resolved binding agent_cli", &binding.agent_cli)?;
    let adapter_name = binding
        .adapter
        .split_once('=')
        .map(|(name, _)| name)
        .expect("exact package validation checked adapter shape");
    let cli_name = binding
        .agent_cli
        .split_once('=')
        .map(|(name, _)| name)
        .expect("exact package validation checked agent CLI shape");
    if expected_package_pair(adapter_name) != Some(cli_name) {
        return Err("resolved binding package pair is not a reviewed adapter/CLI pair".into());
    }
    match (
        container,
        binding.image_digest.as_deref(),
        binding.base_image_digest.as_deref(),
    ) {
        (true, Some(image), Some(base)) => {
            validate_image_digest("resolved binding image_digest", image)?;
            validate_image_digest("resolved binding base_image_digest", base)?;
            if expected_image_digest != Some(image) {
                return Err(
                    "resolved binding image_digest must equal expected_image_digest".into(),
                );
            }
        }
        (true, _, _) => {
            return Err(
                "resolved container binding requires image_digest and base_image_digest".into(),
            )
        }
        (false, None, None) => {}
        (false, _, _) => return Err("resolved host binding must not declare image digests".into()),
    }
    Ok(())
}

fn validate_artifact_identity(label: &str, identity: &ArtifactIdentity) -> Result<(), String> {
    bounded_text(
        &format!("{label} canonical_path"),
        &identity.canonical_path,
        MAX_TEXT_BYTES,
    )?;
    validate_sha256(&format!("{label} sha256"), &identity.sha256)
}

fn validate_versioned_artifact_identity(
    label: &str,
    identity: &VersionedArtifactIdentity,
) -> Result<(), String> {
    if identity.schema_version != 1 {
        return Err(format!("{label} schema_version must be 1"));
    }
    bounded_text(
        &format!("{label} canonical_path"),
        &identity.canonical_path,
        MAX_TEXT_BYTES,
    )?;
    validate_sha256(&format!("{label} sha256"), &identity.sha256)
}

fn validate_executable_identity(label: &str, identity: &ExecutableIdentity) -> Result<(), String> {
    bounded_text(
        &format!("{label} canonical_path"),
        &identity.canonical_path,
        MAX_TEXT_BYTES,
    )?;
    validate_sha256(&format!("{label} sha256"), &identity.sha256)?;
    if identity.byte_length == 0 || identity.byte_length > MAX_EXECUTABLE_BYTES {
        return Err(format!(
            "{label} byte_length must be positive and at most {MAX_EXECUTABLE_BYTES} bytes"
        ));
    }
    Ok(())
}

pub(super) fn validate_resolution(artifact: &ResolutionArtifact) -> Result<(), String> {
    if artifact.schema_version != 1 {
        return Err("compatibility resolution: schema_version must be 1".into());
    }
    stable_id(
        "compatibility resolution resolution_id",
        &artifact.resolution_id,
    )?;
    validate_versioned_artifact_identity("compatibility resolution recipes", &artifact.recipes)?;
    validate_versioned_artifact_identity(
        "compatibility resolution production_manifest",
        &artifact.production_manifest,
    )?;
    validate_executable_identity("compatibility resolution candidate", &artifact.candidate)?;
    stable_id(
        "compatibility resolution environment_owner",
        &artifact.environment.environment_owner,
    )?;
    stable_id("compatibility resolution os", &artifact.environment.os)?;
    stable_id(
        "compatibility resolution architecture",
        &artifact.environment.architecture,
    )?;
    validate_executable_identity(
        "compatibility resolution runtime_executable",
        &artifact.environment.runtime_executable,
    )?;
    validate_limits("compatibility resolution limits", &artifact.limits)?;
    if artifact.protected_inputs.len() > MAX_PROTECTED_INPUTS {
        return Err(format!(
            "compatibility resolution: at most {MAX_PROTECTED_INPUTS} protected inputs are allowed"
        ));
    }
    if artifact.owned_resources.len() > MAX_OWNED_RESOURCES {
        return Err(format!(
            "compatibility resolution: at most {MAX_OWNED_RESOURCES} owned resources are allowed"
        ));
    }
    match artifact.state {
        ResolutionState::Complete => {
            if artifact.execution_manifest.is_none()
                || artifact.packages.is_empty()
                || artifact.cases.is_empty()
                || artifact.protected_inputs.is_empty()
                || artifact.owned_resources.is_empty()
                || artifact.failure.is_some()
            {
                return Err(
                    "compatibility resolution: complete evidence requires execution manifest, packages, cases, protected inputs, owned resources, and no failure"
                        .into(),
                );
            }
        }
        ResolutionState::Failed => {
            if artifact.failure.is_none() {
                return Err(
                    "compatibility resolution: failed evidence requires a typed failure".into(),
                );
            }
        }
        ResolutionState::SetupIncomplete => {
            if artifact.failure.is_some() {
                return Err(
                    "compatibility resolution: setup_incomplete evidence must not declare a failure"
                        .into(),
                );
            }
        }
    }
    if let Some(execution_manifest) = &artifact.execution_manifest {
        validate_versioned_artifact_identity(
            "compatibility resolution execution_manifest",
            execution_manifest,
        )?;
    }
    if artifact.packages.len() > MAX_PACKAGE_SETS
        || artifact.images.len() > MAX_IMAGES
        || artifact.cases.len() > MAX_CASES
    {
        return Err("compatibility resolution: collection bound exceeded".into());
    }

    let mut package_ids = BTreeSet::new();
    let mut package_by_id = BTreeMap::new();
    for package in &artifact.packages {
        stable_id("compatibility resolution package id", &package.id)?;
        if !package_ids.insert(package.id.as_str()) {
            return Err(format!(
                "compatibility resolution: duplicate package id {:?}",
                package.id
            ));
        }
        if package.requested.adapter_selector != "latest" {
            return Err(
                "compatibility resolution: requested adapter selector must remain the literal \"latest\""
                    .into(),
            );
        }
        validate_exact_npm_package("resolved adapter", &package.adapter)?;
        validate_exact_npm_package("resolved agent CLI", &package.agent_cli)?;
        if package.requested.adapter != package.adapter.name
            || package.requested.agent_cli != package.agent_cli.name
            || expected_package_pair(&package.adapter.name) != Some(package.agent_cli.name.as_str())
        {
            return Err(
                "compatibility resolution: resolved package identities do not match the requested reviewed pair"
                    .into(),
            );
        }
        if let Some(version) = &package.bundled_cli_version {
            semver::Version::parse(version).map_err(|_| {
                "compatibility resolution: bundled CLI version must be exact".to_string()
            })?;
        }
        for (label, digest) in [
            ("resolution lock", &package.resolution_lock_sha256),
            ("package inventory", &package.inventory_sha256),
            ("package tree", &package.tree_sha256),
        ] {
            validate_sha256(label, digest)?;
        }
        validate_artifact_identity(
            "compatibility resolution adapter executable",
            &package.adapter_executable,
        )?;
        validate_safe_relative_path(
            "compatibility resolution adapter executable relative path",
            Path::new(&package.adapter_executable_relative),
        )?;
        package_by_id.insert(package.id.as_str(), package);
    }

    let mut image_ids = BTreeSet::new();
    let mut image_by_id = BTreeMap::new();
    for image in &artifact.images {
        stable_id("compatibility resolution image id", &image.id)?;
        if !image_ids.insert(image.id.as_str()) {
            return Err(format!(
                "compatibility resolution: duplicate image id {:?}",
                image.id
            ));
        }
        if image.requested_base != NODE_READER_BASE {
            return Err(
                "compatibility resolution: requested image base does not match the reviewed template"
                    .into(),
            );
        }
        if image.package_sets.is_empty() || image.package_sets.len() > MAX_PACKAGE_SETS {
            return Err(
                "compatibility resolution: image requires a bounded non-empty package set".into(),
            );
        }
        let mut image_package_ids = BTreeSet::new();
        let mut image_packages = Vec::new();
        for package_id in &image.package_sets {
            stable_id("compatibility resolution image package id", package_id)?;
            if !image_package_ids.insert(package_id.as_str()) {
                return Err(format!(
                    "compatibility resolution: image {:?} repeats package set {:?}",
                    image.id, package_id
                ));
            }
            let package = package_by_id.get(package_id.as_str()).ok_or_else(|| {
                format!(
                    "compatibility resolution: image {:?} references unknown package set {:?}",
                    image.id, package_id
                )
            })?;
            image_packages.push((*package).clone());
        }
        for (label, digest) in [
            ("registry index", &image.registry_index_digest),
            ("platform manifest", &image.platform_manifest_digest),
            ("final image", &image.final_image_id),
        ] {
            validate_image_digest(label, digest)?;
        }
        validate_sha256("build template", &image.build_template_sha256)?;
        bounded_text(
            "compatibility resolution owned image tag",
            &image.owned_tag,
            MAX_TEXT_BYTES,
        )?;
        if image.owned_tag.ends_with(":latest")
            || !image.owned_tag.contains(&artifact.resolution_id)
        {
            return Err(
                "compatibility resolution: owned image tag must be resolution-unique".into(),
            );
        }
        for (key, value) in &image.labels {
            bounded_text(
                "compatibility resolution image label key",
                key,
                MAX_TEXT_BYTES,
            )?;
            bounded_text(
                "compatibility resolution image label value",
                value,
                MAX_TEXT_BYTES,
            )?;
        }
        if image.labels
            != image_labels(
                &artifact.resolution_id,
                &artifact.recipes.sha256,
                &image.id,
                &image_packages,
            )
        {
            return Err(
                "compatibility resolution: image labels do not match exact package provenance"
                    .into(),
            );
        }
        image_by_id.insert(image.id.as_str(), image);
    }

    let mut case_ids = BTreeSet::new();
    let mut baseline_ids = BTreeSet::new();
    let mut referenced_package_ids = BTreeSet::new();
    let mut referenced_image_ids = BTreeSet::new();
    for case in &artifact.cases {
        stable_id("compatibility resolution case id", &case.id)?;
        stable_id(
            "compatibility resolution baseline case id",
            &case.baseline_case,
        )?;
        if !case_ids.insert(case.id.as_str()) || !baseline_ids.insert(case.baseline_case.as_str()) {
            return Err(
                "compatibility resolution: case and baseline mappings must be unique".into(),
            );
        }
        bounded_text(
            "compatibility resolution case model",
            &case.model,
            MAX_TEXT_BYTES,
        )?;
        for (label, value) in [
            ("compatibility resolution case effort", &case.effort),
            ("compatibility resolution case mode", &case.mode),
        ] {
            if let Some(value) = value {
                bounded_text(label, value, MAX_ID_BYTES)?;
            }
        }
        if case.prerequisites.len() > MAX_PREREQUISITES_PER_CASE {
            return Err(format!(
                "compatibility resolution: case {:?} exceeds the prerequisite bound",
                case.id
            ));
        }
        let mut prerequisite_names = BTreeSet::new();
        for prerequisite in &case.prerequisites {
            bounded_text(
                "compatibility resolution prerequisite name",
                &prerequisite.name,
                MAX_TEXT_BYTES,
            )?;
            if !prerequisite_names.insert(prerequisite.name.as_str()) {
                return Err(format!(
                    "compatibility resolution: case {:?} repeats prerequisite {:?}",
                    case.id, prerequisite.name
                ));
            }
            if let Some(destination) = &prerequisite.destination {
                bounded_text(
                    "compatibility resolution prerequisite destination",
                    destination,
                    MAX_TEXT_BYTES,
                )?;
            }
        }
        let package = package_by_id
            .get(case.package_set.as_str())
            .ok_or_else(|| {
                format!(
                    "compatibility resolution: case {:?} references an unknown package set",
                    case.id
                )
            })?;
        referenced_package_ids.insert(case.package_set.as_str());
        validate_artifact_identity(
            "compatibility resolution generated config",
            &case.generated_config,
        )?;
        if case.binding.resolution_id != artifact.resolution_id
            || case.binding.recipe_sha256 != artifact.recipes.sha256
            || case.binding.config_sha256 != case.generated_config.sha256
            || case.binding.adapter
                != format!("{}={}", package.adapter.name, package.adapter.version)
            || case.binding.agent_cli
                != format!("{}={}", package.agent_cli.name, package.agent_cli.version)
            || case.binding.package_inventory_sha256 != package.inventory_sha256
            || case.binding.package_tree_sha256 != package.tree_sha256
        {
            return Err(format!(
                "compatibility resolution: case {:?} binding does not match its exact resolution evidence",
                case.id
            ));
        }
        match case.image.as_deref() {
            Some(image_id) => {
                let image = image_by_id.get(image_id).ok_or_else(|| {
                    format!(
                        "compatibility resolution: case {:?} references an unknown image",
                        case.id
                    )
                })?;
                referenced_image_ids.insert(image_id);
                validate_resolved_binding(
                    &case.binding,
                    true,
                    Some(image.final_image_id.as_str()),
                )?;
                if case.binding.base_image_digest.as_deref()
                    != Some(image.platform_manifest_digest.as_str())
                {
                    return Err(format!(
                        "compatibility resolution: case {:?} base binding does not match its image",
                        case.id
                    ));
                }
            }
            None => validate_resolved_binding(&case.binding, false, None)?,
        }
    }

    if artifact.state == ResolutionState::Complete
        && (referenced_package_ids != package_ids || referenced_image_ids != image_ids)
    {
        return Err(
            "compatibility resolution: complete evidence must not contain unreferenced packages or images"
                .into(),
        );
    }

    let permits_protected_mismatch = artifact.state == ResolutionState::Failed
        && artifact
            .failure
            .as_ref()
            .is_some_and(|failure| failure.code == ResolutionFailureCode::ProtectedStateChanged);
    let mut protected_paths = BTreeSet::new();
    for input in &artifact.protected_inputs {
        bounded_text(
            "compatibility resolution protected input path",
            &input.path,
            MAX_TEXT_BYTES,
        )?;
        validate_sha256(
            "compatibility resolution protected input before",
            &input.before_sha256,
        )?;
        validate_sha256(
            "compatibility resolution protected input after",
            &input.after_sha256,
        )?;
        if input.before_sha256 != input.after_sha256 && !permits_protected_mismatch {
            return Err("compatibility resolution: protected_state_changed".into());
        }
        if !protected_paths.insert(input.path.as_str()) {
            return Err(format!(
                "compatibility resolution: duplicate protected input path {:?}",
                input.path
            ));
        }
    }
    let mut owned_resource_ids = BTreeSet::new();
    for resource in &artifact.owned_resources {
        bounded_text(
            "compatibility resolution resource identity",
            &resource.identity,
            MAX_TEXT_BYTES,
        )?;
        if !owned_resource_ids.insert((resource.kind, resource.identity.as_str())) {
            return Err(format!(
                "compatibility resolution: duplicate owned resource {:?}",
                resource.identity
            ));
        }
    }
    Ok(())
}

pub(super) fn load_resolution(path: &Path) -> Result<LoadedResolution, BoxError> {
    let snapshot = local_file::read_regular_file_bounded(
        path,
        "compatibility resolution",
        MAX_RESOLUTION_BYTES,
    )?;
    let canonical_path_text = artifact_path("compatibility resolution", &snapshot.canonical_path)?;
    let raw = std::str::from_utf8(&snapshot.bytes)
        .map_err(|_| "compatibility resolution: file must be UTF-8")?;
    secret_free_raw("compatibility resolution", raw)?;
    let artifact: ResolutionArtifact = serde_json::from_slice(&snapshot.bytes)
        .map_err(|error| format!("compatibility resolution: invalid JSON: {error}"))?;
    validate_resolution(&artifact)?;
    Ok(LoadedResolution {
        artifact,
        canonical_path: snapshot.canonical_path,
        canonical_path_text,
        sha256: snapshot.sha256,
    })
}

fn resolution_bytes(artifact: &ResolutionArtifact) -> Result<Vec<u8>, BoxError> {
    validate_resolution(artifact)
        .map_err(|error| format!("compatibility resolution artifact invalid: {error}"))?;
    let mut bytes = serde_json::to_vec_pretty(artifact)?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_RESOLUTION_BYTES {
        return Err(format!(
            "compatibility resolution artifact exceeds {MAX_RESOLUTION_BYTES} bytes"
        )
        .into());
    }
    let raw = std::str::from_utf8(&bytes)
        .map_err(|_| "compatibility resolution serialization was not UTF-8")?;
    secret_free_raw("compatibility resolution artifact", raw)?;
    Ok(bytes)
}

fn write_synced_file(file: &mut File, bytes: &[u8], label: &str) -> Result<(), BoxError> {
    file.write_all(bytes)
        .map_err(|error| format!("{label}: write failed: {error}"))?;
    file.flush()
        .map_err(|error| format!("{label}: flush failed: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("{label}: sync failed: {error}"))?;
    Ok(())
}

fn create_synced_file(
    directory: &local_file::PinnedDirectory,
    name: &OsStr,
    mode: u32,
    bytes: &[u8],
    label: &str,
) -> Result<File, BoxError> {
    let mut file = directory.create_new_file(name, mode, label)?;
    if let Err(error) = write_synced_file(&mut file, bytes, label) {
        drop(file);
        let _ = directory.remove_child(name, false, label);
        return Err(error);
    }
    directory.sync()?;
    Ok(file)
}

struct BundlePublisher {
    pin: local_file::PinnedDirectory,
    canonical_path: PathBuf,
    setup_file: File,
    setup_artifact: ResolutionArtifact,
}

impl BundlePublisher {
    fn create_with_setup<F>(output: &Path, build_setup: F) -> Result<Self, BoxError>
    where
        F: FnOnce(&Path) -> Result<ResolutionArtifact, BoxError>,
    {
        let name = output
            .file_name()
            .ok_or("compatibility resolve: --out must name a new directory")?;
        let parent = output.parent().unwrap_or_else(|| Path::new("."));
        let snapshot = local_file::snapshot_directory(parent, "compatibility resolution parent")?;
        let canonical_parent = PathBuf::from(snapshot.canonical_cwd.as_str());
        if compatibility::repository_root(&canonical_parent).is_some() {
            return Err("compatibility resolve: --out must be outside any repository".into());
        }
        let parent_pin = local_file::PinnedDirectory::open(
            parent,
            &snapshot.canonical_cwd,
            &snapshot.identity,
            "compatibility resolution parent",
        )?;
        if !parent_pin.current_path_matches() {
            return Err("compatibility resolve: output parent identity changed".into());
        }
        let pin = parent_pin.create_child_directory(
            name,
            0o700,
            "compatibility private resolution bundle",
        )?;
        if !parent_pin.current_path_matches() {
            drop(pin);
            let _ = parent_pin.remove_child(
                name,
                true,
                "compatibility private resolution bundle cleanup",
            );
            return Err(
                "compatibility resolve: output parent identity changed during creation".into(),
            );
        }
        let canonical_path = pin.canonical_path();
        let setup_artifact = match build_setup(&canonical_path) {
            Ok(artifact) => artifact,
            Err(error) => {
                drop(pin);
                let _ = parent_pin.remove_child(
                    name,
                    true,
                    "compatibility private resolution bundle cleanup",
                );
                return Err(error);
            }
        };
        if setup_artifact.state != ResolutionState::SetupIncomplete {
            drop(pin);
            let _ = parent_pin.remove_child(
                name,
                true,
                "compatibility private resolution bundle cleanup",
            );
            return Err(
                "compatibility resolve: initial bundle evidence must be setup_incomplete".into(),
            );
        }
        let setup_bytes = match resolution_bytes(&setup_artifact) {
            Ok(bytes) => bytes,
            Err(error) => {
                drop(pin);
                let _ = parent_pin.remove_child(
                    name,
                    true,
                    "compatibility private resolution bundle cleanup",
                );
                return Err(error);
            }
        };
        let setup_file = match create_synced_file(
            &pin,
            OsStr::new("resolution.json"),
            0o600,
            &setup_bytes,
            "compatibility setup resolution evidence",
        ) {
            Ok(file) => file,
            Err(error) => {
                drop(pin);
                let _ = parent_pin.remove_child(
                    name,
                    true,
                    "compatibility private resolution bundle cleanup",
                );
                return Err(error);
            }
        };
        Ok(Self {
            pin,
            canonical_path,
            setup_file,
            setup_artifact,
        })
    }

    fn create_directory(
        &self,
        name: &OsStr,
        label: &str,
    ) -> Result<local_file::PinnedDirectory, BoxError> {
        let child = self.pin.create_child_directory(name, 0o700, label)?;
        self.pin.sync()?;
        Ok(child)
    }

    fn publish_terminal(&self, artifact: &ResolutionArtifact) -> Result<(), BoxError> {
        if !matches!(
            artifact.state,
            ResolutionState::Complete | ResolutionState::Failed
        ) {
            return Err(
                "compatibility resolve: terminal artifact must be complete or failed".into(),
            );
        }
        let final_bytes = resolution_bytes(artifact)?;
        let setup_bytes = resolution_bytes(&self.setup_artifact)?;
        let replacement_name = OsString::from(format!(
            ".resolution-final-{}-{}",
            std::process::id(),
            crate::implement::nonce(20)
        ));
        let rollback_name = OsString::from(format!(
            ".resolution-setup-{}-{}",
            std::process::id(),
            crate::implement::nonce(20)
        ));
        let mut replacement = self.pin.create_new_file(
            &replacement_name,
            0o600,
            "compatibility terminal resolution evidence",
        )?;
        let mut rollback = match self.pin.create_new_file(
            &rollback_name,
            0o600,
            "compatibility setup resolution rollback",
        ) {
            Ok(file) => file,
            Err(error) => {
                drop(replacement);
                let _ = self.pin.remove_child(
                    &replacement_name,
                    false,
                    "compatibility terminal resolution cleanup",
                );
                return Err(error);
            }
        };
        if let Err(error) = write_synced_file(
            &mut replacement,
            &final_bytes,
            "compatibility terminal resolution evidence",
        )
        .and_then(|()| {
            write_synced_file(
                &mut rollback,
                &setup_bytes,
                "compatibility setup resolution rollback",
            )
        }) {
            drop(replacement);
            drop(rollback);
            let _ = self.pin.remove_child(
                &replacement_name,
                false,
                "compatibility terminal resolution cleanup",
            );
            let _ = self.pin.remove_child(
                &rollback_name,
                false,
                "compatibility setup resolution rollback cleanup",
            );
            return Err(error);
        }
        self.pin.replace_regular_child(
            local_file::RegularChildRef::new(OsStr::new("resolution.json"), &self.setup_file),
            local_file::RegularChildRef::new(&replacement_name, &replacement),
            local_file::RegularChildRef::new(&rollback_name, &rollback),
            "compatibility terminal resolution evidence",
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResolutionCommandFamily {
    Npm,
    Runtime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResolutionCommandKind {
    NpmLock,
    NpmMaterialize,
    ResolveBase,
    EnsureImageTagAbsent,
    BuildImage,
    InspectImage,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ResolutionCommandSpec {
    family: ResolutionCommandFamily,
    kind: ResolutionCommandKind,
    program: PathBuf,
    args: Vec<OsString>,
    cwd: PathBuf,
    env: BTreeMap<OsString, OsString>,
    timeout: Duration,
    max_output_bytes: usize,
}

impl ResolutionCommandSpec {
    fn failure_code(&self, failure: CommandFailure) -> ResolutionFailureCode {
        match (self.family, failure) {
            (ResolutionCommandFamily::Npm, CommandFailure::Spawn) => {
                ResolutionFailureCode::NpmSpawnFailed
            }
            (ResolutionCommandFamily::Npm, CommandFailure::Timeout) => {
                ResolutionFailureCode::NpmTimeout
            }
            (ResolutionCommandFamily::Npm, CommandFailure::Nonzero) => {
                ResolutionFailureCode::NpmNonzero
            }
            (ResolutionCommandFamily::Npm, CommandFailure::OutputTooLarge) => {
                ResolutionFailureCode::NpmOutputTooLarge
            }
            (ResolutionCommandFamily::Npm, CommandFailure::OutputUnreadable) => {
                ResolutionFailureCode::NpmOutputUnreadable
            }
            (ResolutionCommandFamily::Runtime, CommandFailure::Spawn) => {
                ResolutionFailureCode::RuntimeSpawnFailed
            }
            (ResolutionCommandFamily::Runtime, CommandFailure::Timeout) => {
                ResolutionFailureCode::RuntimeTimeout
            }
            (ResolutionCommandFamily::Runtime, CommandFailure::Nonzero) => {
                ResolutionFailureCode::RuntimeNonzero
            }
            (ResolutionCommandFamily::Runtime, CommandFailure::OutputTooLarge) => {
                ResolutionFailureCode::RuntimeOutputTooLarge
            }
            (ResolutionCommandFamily::Runtime, CommandFailure::OutputUnreadable) => {
                ResolutionFailureCode::RuntimeOutputUnreadable
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommandFailure {
    Spawn,
    Timeout,
    Nonzero,
    OutputTooLarge,
    OutputUnreadable,
}

#[async_trait]
trait ResolutionExecutor: Send + Sync {
    async fn execute(
        &self,
        command: &ResolutionCommandSpec,
    ) -> Result<Vec<u8>, ResolutionFailureCode>;
}

struct ProcessResolutionExecutor;

#[async_trait]
impl ResolutionExecutor for ProcessResolutionExecutor {
    async fn execute(
        &self,
        command: &ResolutionCommandSpec,
    ) -> Result<Vec<u8>, ResolutionFailureCode> {
        execute_bounded_command(command).await
    }
}

async fn read_bounded_stdout(
    mut stdout: tokio::process::ChildStdout,
    max_bytes: usize,
) -> Result<Vec<u8>, CommandFailure> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let count = stdout
            .read(&mut buffer)
            .await
            .map_err(|_| CommandFailure::OutputUnreadable)?;
        if count == 0 {
            return Ok(output);
        }
        if output.len().saturating_add(count) > max_bytes {
            return Err(CommandFailure::OutputTooLarge);
        }
        output.extend_from_slice(&buffer[..count]);
    }
}

#[cfg(unix)]
async fn terminate_command_process_group(child: &mut tokio::process::Child, process_group: u32) {
    if let Ok(process_group) = libc::pid_t::try_from(process_group) {
        // SAFETY: every real resolver command is placed in a fresh group whose id is its child pid.
        // Negating that positive pid targets only the resolver-owned group.
        unsafe {
            libc::kill(-process_group, libc::SIGKILL);
        }
    }
    let _ = child.wait().await;
}

async fn execute_bounded_command(
    spec: &ResolutionCommandSpec,
) -> Result<Vec<u8>, ResolutionFailureCode> {
    #[cfg(not(unix))]
    {
        return Err(spec.failure_code(CommandFailure::Spawn));
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;

        let mut command = tokio::process::Command::new(&spec.program);
        command
            .args(&spec.args)
            .current_dir(&spec.cwd)
            .env_clear()
            .envs(spec.env.iter())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        command.as_std_mut().process_group(0);
        let mut child = command
            .spawn()
            .map_err(|_| spec.failure_code(CommandFailure::Spawn))?;
        let process_group = child
            .id()
            .ok_or_else(|| spec.failure_code(CommandFailure::Spawn))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| spec.failure_code(CommandFailure::OutputUnreadable))?;
        let mut read_task = tokio::spawn(read_bounded_stdout(stdout, spec.max_output_bytes));
        let deadline = tokio::time::Instant::now() + spec.timeout;

        enum First {
            Read(Result<Vec<u8>, CommandFailure>),
            Wait(std::io::Result<std::process::ExitStatus>),
            Timeout,
        }
        let first = tokio::select! {
            read = &mut read_task => First::Read(
                read.unwrap_or(Err(CommandFailure::OutputUnreadable))
            ),
            status = child.wait() => First::Wait(status),
            () = tokio::time::sleep_until(deadline) => First::Timeout,
        };

        match first {
            First::Timeout => {
                read_task.abort();
                terminate_command_process_group(&mut child, process_group).await;
                Err(spec.failure_code(CommandFailure::Timeout))
            }
            First::Read(Err(failure)) => {
                terminate_command_process_group(&mut child, process_group).await;
                Err(spec.failure_code(failure))
            }
            First::Read(Ok(output)) => {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                let status = match tokio::time::timeout(remaining, child.wait()).await {
                    Ok(Ok(status)) => status,
                    Ok(Err(_)) => {
                        terminate_command_process_group(&mut child, process_group).await;
                        return Err(spec.failure_code(CommandFailure::Nonzero));
                    }
                    Err(_) => {
                        terminate_command_process_group(&mut child, process_group).await;
                        return Err(spec.failure_code(CommandFailure::Timeout));
                    }
                };
                terminate_command_process_group(&mut child, process_group).await;
                if status.success() {
                    Ok(output)
                } else {
                    Err(spec.failure_code(CommandFailure::Nonzero))
                }
            }
            First::Wait(Err(_)) => {
                read_task.abort();
                terminate_command_process_group(&mut child, process_group).await;
                Err(spec.failure_code(CommandFailure::Nonzero))
            }
            First::Wait(Ok(status)) => {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                let output = match tokio::time::timeout(remaining, &mut read_task).await {
                    Ok(Ok(Ok(output))) => output,
                    Ok(Ok(Err(failure))) => {
                        terminate_command_process_group(&mut child, process_group).await;
                        return Err(spec.failure_code(failure));
                    }
                    Ok(Err(_)) => {
                        terminate_command_process_group(&mut child, process_group).await;
                        return Err(spec.failure_code(CommandFailure::OutputUnreadable));
                    }
                    Err(_) => {
                        read_task.abort();
                        terminate_command_process_group(&mut child, process_group).await;
                        return Err(spec.failure_code(CommandFailure::Timeout));
                    }
                };
                terminate_command_process_group(&mut child, process_group).await;
                if status.success() {
                    Ok(output)
                } else {
                    Err(spec.failure_code(CommandFailure::Nonzero))
                }
            }
        }
    }
}

fn isolated_npm_env(path: OsString, cwd: &Path) -> BTreeMap<OsString, OsString> {
    [
        ("PATH", path),
        ("HOME", cwd.join("home").into_os_string()),
        ("TMPDIR", cwd.join("tmp").into_os_string()),
        ("npm_config_cache", cwd.join("cache").into_os_string()),
        ("npm_config_prefix", cwd.join("prefix").into_os_string()),
        ("npm_config_userconfig", cwd.join("npmrc").into_os_string()),
        ("npm_config_audit", OsString::from("false")),
        ("npm_config_fund", OsString::from("false")),
        ("npm_config_ignore_scripts", OsString::from("true")),
        ("npm_config_registry", OsString::from(NPM_REGISTRY)),
    ]
    .into_iter()
    .map(|(key, value)| (OsString::from(key), value))
    .collect()
}

fn npm_command(
    npm: &Path,
    safe_path: OsString,
    cwd: PathBuf,
    timeout: Duration,
    materialize: bool,
) -> ResolutionCommandSpec {
    let (kind, verb) = if materialize {
        (ResolutionCommandKind::NpmMaterialize, "ci")
    } else {
        (ResolutionCommandKind::NpmLock, "install")
    };
    let mut args = vec![OsString::from(verb)];
    if !materialize {
        args.push(OsString::from("--package-lock-only"));
    } else {
        args.push(OsString::from("--prefix=tree"));
    }
    args.extend(
        [
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
            "--no-progress",
            "--registry=https://registry.npmjs.org/",
        ]
        .into_iter()
        .map(OsString::from),
    );
    let env = isolated_npm_env(safe_path, &cwd);
    ResolutionCommandSpec {
        family: ResolutionCommandFamily::Npm,
        kind,
        program: npm.to_path_buf(),
        args,
        cwd,
        env,
        timeout,
        max_output_bytes: MAX_COMMAND_OUTPUT_BYTES,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ParsedPackageLock {
    adapter: ExactNpmPackage,
    agent_cli: ExactNpmPackage,
    sha256: String,
}

fn package_name_from_lock_path(path: &str) -> Option<&str> {
    let (_, tail) = path.rsplit_once("node_modules/")?;
    if tail.starts_with('@') {
        let slash = tail.find('/')?;
        let package_end = tail[slash + 1..]
            .find('/')
            .map(|offset| slash + 1 + offset)
            .unwrap_or(tail.len());
        Some(&tail[..package_end])
    } else {
        Some(tail.split('/').next().unwrap_or(tail))
    }
}

fn dependency_spec_is_external(spec: &str) -> bool {
    let lower = spec.to_ascii_lowercase();
    lower.starts_with("file:")
        || lower.starts_with("link:")
        || lower.starts_with("workspace:")
        || lower.starts_with("git:")
        || lower.starts_with("git+")
        || lower.starts_with("github:")
        || lower.starts_with("http:")
        || lower.starts_with("https:")
        || lower.starts_with("npm:")
        || lower.starts_with('/')
        || lower.starts_with("../")
        || lower.starts_with("./")
        || lower.starts_with('~')
}

fn validate_dependency_specs(
    package: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    for field in [
        "dependencies",
        "optionalDependencies",
        "peerDependencies",
        "devDependencies",
    ] {
        let Some(dependencies) = package.get(field) else {
            continue;
        };
        let dependencies = dependencies
            .as_object()
            .ok_or_else(|| format!("package lock {field} must be an object"))?;
        for (name, spec) in dependencies {
            bounded_text("package lock dependency name", name, MAX_TEXT_BYTES)?;
            let spec = spec
                .as_str()
                .ok_or_else(|| format!("package lock dependency {name:?} must be a string"))?;
            bounded_text("package lock dependency selector", spec, MAX_TEXT_BYTES)?;
            if dependency_spec_is_external(spec) {
                return Err(format!(
                    "package lock dependency {name:?} uses a forbidden external selector"
                ));
            }
        }
    }
    Ok(())
}

fn validate_registry_tarball(value: &str) -> Result<(), String> {
    bounded_text("package lock resolved URL", value, MAX_TEXT_BYTES)?;
    let Some(path) = value.strip_prefix(NPM_REGISTRY) else {
        return Err("package lock resolved URL must use the fixed npmjs registry".into());
    };
    if path.is_empty()
        || path.contains(['?', '#', '\\'])
        || path.split('/').any(|segment| segment == "..")
    {
        return Err("package lock resolved URL is not a safe npmjs tarball URL".into());
    }
    Ok(())
}

fn parse_exact_locked_package(
    label: &str,
    expected_name: &str,
    package: &serde_json::Map<String, serde_json::Value>,
) -> Result<ExactNpmPackage, String> {
    let version = package
        .get("version")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("{label} is missing an exact version"))?;
    let integrity = package
        .get("integrity")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("{label} is missing integrity"))?;
    let exact = ExactNpmPackage {
        name: expected_name.to_owned(),
        version: version.to_owned(),
        integrity: integrity.to_owned(),
    };
    validate_exact_npm_package(label, &exact)?;
    Ok(exact)
}

fn parse_package_lock(
    bytes: &[u8],
    expected_adapter: &str,
    expected_cli: &str,
    max_packages: u64,
) -> Result<ParsedPackageLock, String> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_LOCK_BYTES {
        return Err(format!(
            "package lock must be a non-empty file of at most {MAX_LOCK_BYTES} bytes"
        ));
    }
    let raw = std::str::from_utf8(bytes).map_err(|_| "package lock must be UTF-8")?;
    secret_free_raw("package lock", raw)?;
    let root: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| "package lock is invalid JSON")?;
    let root = root
        .as_object()
        .ok_or("package lock root must be an object")?;
    if root
        .get("lockfileVersion")
        .and_then(serde_json::Value::as_u64)
        != Some(3)
    {
        return Err("package lock lockfileVersion must be 3".into());
    }
    let packages = root
        .get("packages")
        .and_then(serde_json::Value::as_object)
        .ok_or("package lock packages must be an object")?;
    if packages.is_empty() || packages.len() as u64 > max_packages {
        return Err("package lock package count exceeds the resolution bound".into());
    }
    let root_package = packages
        .get("")
        .and_then(serde_json::Value::as_object)
        .ok_or("package lock is missing the root package")?;
    validate_dependency_specs(root_package)?;
    let root_dependencies = root_package
        .get("dependencies")
        .and_then(serde_json::Value::as_object)
        .ok_or("package lock root must declare dependencies")?;
    if root_dependencies.len() != 1
        || root_dependencies
            .get(expected_adapter)
            .and_then(serde_json::Value::as_str)
            != Some("latest")
    {
        return Err("package lock root must request exactly the reviewed adapter at latest".into());
    }

    let mut adapter = None;
    let mut agent_cli = None;
    for (path, package) in packages {
        if path.is_empty() {
            continue;
        }
        validate_safe_relative_path("package lock package path", Path::new(path))?;
        if !path.starts_with("node_modules/") {
            return Err("package lock entries must be installed node_modules paths".into());
        }
        let package = package
            .as_object()
            .ok_or_else(|| format!("package lock entry {path:?} must be an object"))?;
        if package
            .get("link")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
            || package
                .get("hasInstallScript")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        {
            return Err(format!(
                "package lock entry {path:?} uses a link or lifecycle install script"
            ));
        }
        validate_dependency_specs(package)?;
        let resolved = package
            .get("resolved")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("package lock entry {path:?} is missing a resolved URL"))?;
        validate_registry_tarball(resolved)?;
        let integrity = package
            .get("integrity")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("package lock entry {path:?} is missing integrity"))?;
        let package_identity = ExactNpmPackage {
            name: package_name_from_lock_path(path)
                .ok_or_else(|| format!("package lock entry {path:?} has no package name"))?
                .to_owned(),
            version: package
                .get("version")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| format!("package lock entry {path:?} is missing a version"))?
                .to_owned(),
            integrity: integrity.to_owned(),
        };
        validate_exact_npm_package("package lock entry", &package_identity)?;

        if package_identity.name == expected_adapter {
            if adapter.is_some() {
                return Err("package lock contains multiple reviewed adapter installations".into());
            }
            if package
                .get("dependencies")
                .and_then(serde_json::Value::as_object)
                .and_then(|dependencies| dependencies.get(expected_cli))
                .is_none()
            {
                return Err("resolved adapter does not declare the reviewed nested CLI/SDK".into());
            }
            adapter = Some(parse_exact_locked_package(
                "resolved adapter",
                expected_adapter,
                package,
            )?);
        }
        if package_identity.name == expected_cli {
            if agent_cli.is_some() {
                return Err("package lock contains multiple nested CLI/SDK installations".into());
            }
            agent_cli = Some(parse_exact_locked_package(
                "resolved agent CLI",
                expected_cli,
                package,
            )?);
        }
    }
    Ok(ParsedPackageLock {
        adapter: adapter.ok_or("package lock is missing the reviewed adapter")?,
        agent_cli: agent_cli.ok_or("package lock is missing the reviewed nested CLI/SDK")?,
        sha256: local_file::sha256_hex(bytes),
    })
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum InventoryEntryKind {
    Directory,
    File,
    Symlink,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct InventoryEntry {
    path: String,
    kind: InventoryEntryKind,
    executable: bool,
    byte_length: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    symlink_target: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct PackageInventory {
    schema_version: u16,
    entries: Vec<InventoryEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct InspectedTree {
    inventory: PackageInventory,
    inventory_sha256: String,
    tree_sha256: String,
    file_count: u64,
    byte_count: u64,
}

#[cfg(unix)]
fn executable_and_single_link(metadata: &fs::Metadata) -> Result<bool, String> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    if metadata.is_file() && metadata.nlink() != 1 {
        return Err("package tree regular files must have exactly one link".into());
    }
    Ok(metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn executable_and_single_link(_metadata: &fs::Metadata) -> Result<bool, String> {
    Ok(false)
}

fn inspect_package_tree(root: &Path, limits: &ResolutionLimits) -> Result<InspectedTree, String> {
    let canonical_root =
        fs::canonicalize(root).map_err(|_| "package tree root is unavailable".to_string())?;
    if !fs::metadata(&canonical_root).is_ok_and(|metadata| metadata.is_dir()) {
        return Err("package tree root must be a directory".into());
    }
    let mut pending = vec![root.to_path_buf()];
    let mut entries = Vec::new();
    let mut file_count = 0_u64;
    let mut byte_count = 0_u64;
    while let Some(directory) = pending.pop() {
        let read_dir = fs::read_dir(&directory)
            .map_err(|_| "package tree directory is unreadable".to_string())?;
        let mut children = Vec::new();
        for child in read_dir {
            let child = child.map_err(|_| "package tree directory entry is unreadable")?;
            let name = child
                .file_name()
                .into_string()
                .map_err(|_| "package tree paths must be UTF-8")?;
            bounded_text("package tree path component", &name, MAX_TEXT_BYTES)?;
            children.push((name, child.path()));
        }
        children.sort_by(|left, right| left.0.cmp(&right.0));
        for (_name, path) in children.into_iter().rev() {
            file_count = file_count
                .checked_add(1)
                .ok_or("package tree file count overflow")?;
            if file_count > limits.max_files {
                return Err("package tree exceeds the file-count bound".into());
            }
            let relative = path
                .strip_prefix(root)
                .map_err(|_| "package tree path escaped its root")?;
            let relative = relative
                .to_str()
                .ok_or("package tree paths must be UTF-8")?
                .replace(std::path::MAIN_SEPARATOR, "/");
            validate_safe_relative_path("package tree relative path", Path::new(&relative))?;
            let metadata = fs::symlink_metadata(&path)
                .map_err(|_| "package tree entry metadata is unavailable")?;
            let executable = executable_and_single_link(&metadata)?;
            let file_type = metadata.file_type();
            let entry = if file_type.is_dir() {
                pending.push(path);
                InventoryEntry {
                    path: relative,
                    kind: InventoryEntryKind::Directory,
                    executable,
                    byte_length: 0,
                    sha256: None,
                    symlink_target: None,
                }
            } else if file_type.is_file() {
                byte_count = byte_count
                    .checked_add(metadata.len())
                    .ok_or("package tree byte count overflow")?;
                if byte_count > limits.max_unpacked_bytes {
                    return Err("package tree exceeds the unpacked-byte bound".into());
                }
                let snapshot = local_file::read_regular_file_bounded(
                    &path,
                    "package tree file",
                    limits.max_unpacked_bytes,
                )
                .map_err(|_| "package tree file changed or is unreadable")?;
                if snapshot.bytes.len() as u64 != metadata.len() {
                    return Err("package tree file changed while it was inspected".into());
                }
                InventoryEntry {
                    path: relative,
                    kind: InventoryEntryKind::File,
                    executable,
                    byte_length: metadata.len(),
                    sha256: Some(snapshot.sha256),
                    symlink_target: None,
                }
            } else if file_type.is_symlink() {
                let target = fs::read_link(&path)
                    .map_err(|_| "package tree symlink target is unavailable")?;
                let target_text = target
                    .to_str()
                    .ok_or("package tree symlink target must be UTF-8")?;
                bounded_text("package tree symlink target", target_text, MAX_TEXT_BYTES)?;
                let resolved = fs::canonicalize(&path)
                    .map_err(|_| "package tree symlinks must resolve inside the tree")?;
                if !resolved.starts_with(&canonical_root) {
                    return Err("package tree symlink escapes the tree".into());
                }
                InventoryEntry {
                    path: relative,
                    kind: InventoryEntryKind::Symlink,
                    executable: false,
                    byte_length: target_text.len() as u64,
                    sha256: None,
                    symlink_target: Some(target_text.to_owned()),
                }
            } else {
                return Err("package tree contains a device, socket, or other special file".into());
            };
            entries.push(entry);
        }
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    let inventory = PackageInventory {
        schema_version: 1,
        entries,
    };
    let inventory_bytes = serde_json::to_vec(&inventory)
        .map_err(|_| "package inventory serialization failed".to_string())?;
    let mut inventory_material = b"a2a-bridge:r3c:inventory:v1\0".to_vec();
    inventory_material.extend_from_slice(&inventory_bytes);
    let mut tree_material = b"a2a-bridge:r3c:tree:v1\0".to_vec();
    for entry in &inventory.entries {
        let encoded = serde_json::to_vec(entry)
            .map_err(|_| "package tree serialization failed".to_string())?;
        tree_material.extend_from_slice(&(encoded.len() as u64).to_be_bytes());
        tree_material.extend_from_slice(&encoded);
    }
    Ok(InspectedTree {
        inventory,
        inventory_sha256: local_file::sha256_hex(&inventory_material),
        tree_sha256: local_file::sha256_hex(&tree_material),
        file_count,
        byte_count,
    })
}

fn enforce_npm_download_budget(
    cache: &local_file::PinnedDirectory,
    limits: &ResolutionLimits,
) -> Result<(), ResolutionFailureCode> {
    if !cache.current_path_matches() {
        return Err(ResolutionFailureCode::WriteScopeEscape);
    }
    let root = cache.acp_session_cwd();
    if !fs::metadata(&root).is_ok_and(|metadata| metadata.is_dir()) {
        return Err(ResolutionFailureCode::WriteScopeEscape);
    }
    let mut pending = vec![root];
    let mut entry_count = 0_u64;
    let mut byte_count = 0_u64;
    while let Some(directory) = pending.pop() {
        let children =
            fs::read_dir(&directory).map_err(|_| ResolutionFailureCode::WriteScopeEscape)?;
        for child in children {
            let child = child.map_err(|_| ResolutionFailureCode::WriteScopeEscape)?;
            let name = child
                .file_name()
                .into_string()
                .map_err(|_| ResolutionFailureCode::WriteScopeEscape)?;
            bounded_text("npm cache path component", &name, MAX_TEXT_BYTES)
                .map_err(|_| ResolutionFailureCode::WriteScopeEscape)?;
            entry_count = entry_count
                .checked_add(1)
                .ok_or(ResolutionFailureCode::NpmDownloadBudgetExceeded)?;
            if entry_count > limits.max_files {
                return Err(ResolutionFailureCode::NpmDownloadBudgetExceeded);
            }
            let path = child.path();
            let metadata =
                fs::symlink_metadata(&path).map_err(|_| ResolutionFailureCode::WriteScopeEscape)?;
            let file_type = metadata.file_type();
            if file_type.is_dir() {
                pending.push(path);
            } else if file_type.is_file() {
                executable_and_single_link(&metadata)
                    .map_err(|_| ResolutionFailureCode::WriteScopeEscape)?;
                byte_count = byte_count
                    .checked_add(metadata.len())
                    .ok_or(ResolutionFailureCode::NpmDownloadBudgetExceeded)?;
                if byte_count > limits.max_download_bytes {
                    return Err(ResolutionFailureCode::NpmDownloadBudgetExceeded);
                }
            } else {
                return Err(ResolutionFailureCode::WriteScopeEscape);
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_open_file_mode(file: &File, mode: u32, label: &str) -> Result<(), BoxError> {
    use std::os::fd::AsRawFd as _;
    // SAFETY: `file` owns a live descriptor and `mode` is a normal permission mask.
    if unsafe { libc::fchmod(file.as_raw_fd(), mode as libc::mode_t) } == -1 {
        return Err(format!(
            "{label}: cannot set owner-only permissions: {}",
            std::io::Error::last_os_error()
        )
        .into());
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_open_file_mode(_file: &File, _mode: u32, label: &str) -> Result<(), BoxError> {
    Err(format!("{label}: owner-only permission binding is unsupported").into())
}

#[cfg(unix)]
fn seal_package_tree(root: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt as _;

    let mut pending = vec![root.to_path_buf()];
    let mut directories = Vec::new();
    while let Some(directory) = pending.pop() {
        directories.push(directory.clone());
        for entry in fs::read_dir(&directory).map_err(|_| "package tree sealing read failed")? {
            let path = entry
                .map_err(|_| "package tree sealing entry failed")?
                .path();
            let metadata =
                fs::symlink_metadata(&path).map_err(|_| "package tree sealing metadata failed")?;
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                let executable = metadata.permissions().mode() & 0o111 != 0;
                fs::set_permissions(
                    &path,
                    fs::Permissions::from_mode(if executable { 0o500 } else { 0o400 }),
                )
                .map_err(|_| "package tree file sealing failed")?;
            } else if !metadata.file_type().is_symlink() {
                return Err("package tree contains an unsealable special file".into());
            }
        }
    }
    for directory in directories.into_iter().rev() {
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o500))
            .map_err(|_| "package tree directory sealing failed")?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn seal_package_tree(_root: &Path) -> Result<(), String> {
    Err("package tree sealing is unsupported".into())
}

fn node_package_manifest(tree: &Path, package: &str) -> PathBuf {
    package
        .split('/')
        .fold(tree.join("node_modules"), |path, segment| {
            path.join(segment)
        })
        .join("package.json")
}

fn package_json_bytes(recipe: &PackageSetRecipe) -> Result<Vec<u8>, ResolutionFailureCode> {
    let mut dependencies = serde_json::Map::new();
    dependencies.insert(
        recipe.adapter.clone(),
        serde_json::Value::String(recipe.adapter_selector.clone()),
    );
    let mut bytes = serde_json::to_vec_pretty(&serde_json::json!({
        "name": format!("a2a-bridge-r3c-{}", recipe.id),
        "private": true,
        "version": "0.0.0",
        "dependencies": dependencies,
    }))
    .map_err(|_| ResolutionFailureCode::PackageIdentityMismatch)?;
    bytes.push(b'\n');
    Ok(bytes)
}

#[derive(Clone, Debug)]
struct ResolverTooling {
    npm_executable: PathBuf,
    runtime_executable: PathBuf,
    safe_path: OsString,
}

async fn materialize_package_set(
    packages_directory: &local_file::PinnedDirectory,
    recipe: &PackageSetRecipe,
    limits: &ResolutionLimits,
    tooling: &ResolverTooling,
    executor: &dyn ResolutionExecutor,
) -> Result<ResolvedPackageSet, ResolutionFailureCode> {
    let package_directory = packages_directory
        .create_child_directory(
            OsStr::new(&recipe.id),
            0o700,
            "compatibility package-set directory",
        )
        .map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?;
    packages_directory
        .sync()
        .map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?;
    let mut isolated_directories = Vec::new();
    for name in ["home", "tmp", "cache", "prefix"] {
        let directory = package_directory
            .create_child_directory(
                OsStr::new(name),
                0o700,
                "compatibility isolated npm directory",
            )
            .map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?;
        isolated_directories.push((name, directory));
    }
    create_synced_file(
        &package_directory,
        OsStr::new("npmrc"),
        0o600,
        b"registry=https://registry.npmjs.org/\nignore-scripts=true\naudit=false\nfund=false\n",
        "compatibility isolated npm config",
    )
    .map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?;
    let package_json = package_json_bytes(recipe)?;
    create_synced_file(
        &package_directory,
        OsStr::new("package.json"),
        0o600,
        &package_json,
        "compatibility package request",
    )
    .map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?;

    let timeout = Duration::from_secs(limits.timeout_secs);
    executor
        .execute(&npm_command(
            &tooling.npm_executable,
            tooling.safe_path.clone(),
            package_directory.acp_session_cwd(),
            timeout,
            false,
        ))
        .await?;
    let cache_directory = isolated_directories
        .iter()
        .find_map(|(name, directory)| (*name == "cache").then_some(directory))
        .ok_or(ResolutionFailureCode::PublicationResourceFailed)?;
    enforce_npm_download_budget(cache_directory, limits)?;
    let lock_file = package_directory
        .open_regular_file(
            OsStr::new("package-lock.json"),
            "compatibility resolved package lock",
        )
        .map_err(|_| ResolutionFailureCode::PackageIdentityMismatch)?;
    set_open_file_mode(&lock_file, 0o600, "compatibility resolved package lock")
        .map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?;
    let lock_snapshot = local_file::read_open_regular_file_bounded(
        &lock_file,
        "compatibility resolved package lock",
        MAX_LOCK_BYTES,
    )
    .map_err(|_| ResolutionFailureCode::PackageIdentityMismatch)?;
    let parsed_lock = parse_package_lock(
        &lock_snapshot.bytes,
        &recipe.adapter,
        &recipe.agent_cli,
        limits.max_files,
    )
    .map_err(|_| ResolutionFailureCode::PackageIdentityMismatch)?;

    let tree_directory = package_directory
        .create_child_directory(OsStr::new("tree"), 0o700, "compatibility package tree")
        .map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?;
    create_synced_file(
        &tree_directory,
        OsStr::new("package.json"),
        0o600,
        &package_json,
        "compatibility materialized package request",
    )
    .map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?;
    create_synced_file(
        &tree_directory,
        OsStr::new("package-lock.json"),
        0o600,
        &lock_snapshot.bytes,
        "compatibility materialized package lock",
    )
    .map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?;
    executor
        .execute(&npm_command(
            &tooling.npm_executable,
            tooling.safe_path.clone(),
            package_directory.acp_session_cwd(),
            timeout,
            true,
        ))
        .await?;
    enforce_npm_download_budget(cache_directory, limits)?;

    let tree_path = tree_directory.canonical_path();
    let inspected = inspect_package_tree(&tree_path, limits)
        .map_err(|_| ResolutionFailureCode::PackageTreeDrift)?;
    let adapter =
        crate::doctor::read_installed_package(&node_package_manifest(&tree_path, &recipe.adapter))
            .map_err(|_| ResolutionFailureCode::PackageIdentityMismatch)?;
    if adapter.name != parsed_lock.adapter.name || adapter.version != parsed_lock.adapter.version {
        return Err(ResolutionFailureCode::PackageIdentityMismatch);
    }
    let agent_cli = crate::doctor::resolve_installed_dependency(&adapter, &recipe.agent_cli)
        .map_err(|_| ResolutionFailureCode::PackageIdentityMismatch)?;
    if agent_cli.name != parsed_lock.agent_cli.name
        || agent_cli.version != parsed_lock.agent_cli.version
    {
        return Err(ResolutionFailureCode::PackageIdentityMismatch);
    }
    let bundled_cli_version = agent_cli.bundled_cli_version().map(str::to_owned);
    let executable = adapter
        .sole_owned_executable()
        .map_err(|_| ResolutionFailureCode::PackageIdentityMismatch)?;
    let executable_snapshot = local_file::read_regular_file_bounded(
        &executable,
        "compatibility resolved adapter executable",
        MAX_RESOLUTION_BYTES,
    )
    .map_err(|_| ResolutionFailureCode::PackageIdentityMismatch)?;
    let executable_relative = executable_snapshot
        .canonical_path
        .strip_prefix(&tree_path)
        .map_err(|_| ResolutionFailureCode::PackageIdentityMismatch)?;
    let executable_relative = executable_relative
        .to_str()
        .ok_or(ResolutionFailureCode::PackageIdentityMismatch)?
        .replace(std::path::MAIN_SEPARATOR, "/");
    validate_safe_relative_path(
        "compatibility resolved adapter executable",
        Path::new(&executable_relative),
    )
    .map_err(|_| ResolutionFailureCode::PackageIdentityMismatch)?;

    seal_package_tree(&tree_path).map_err(|_| ResolutionFailureCode::PackageTreeDrift)?;
    let sealed = inspect_package_tree(&tree_path, limits)
        .map_err(|_| ResolutionFailureCode::PackageTreeDrift)?;
    if inspected.inventory_sha256 != sealed.inventory_sha256
        || inspected.tree_sha256 != sealed.tree_sha256
    {
        return Err(ResolutionFailureCode::PackageTreeDrift);
    }
    let mut inventory_bytes = serde_json::to_vec_pretty(&sealed.inventory)
        .map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?;
    inventory_bytes.push(b'\n');
    if inventory_bytes.len() as u64 > MAX_RESOLUTION_BYTES {
        return Err(ResolutionFailureCode::PackageTreeDrift);
    }
    create_synced_file(
        &package_directory,
        OsStr::new("inventory.json"),
        0o600,
        &inventory_bytes,
        "compatibility package inventory",
    )
    .map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?;

    for (name, directory) in isolated_directories {
        if !directory.current_path_matches() {
            return Err(ResolutionFailureCode::WriteScopeEscape);
        }
        let path = directory.canonical_path();
        if package_directory.canonical_path().join(name) != path {
            return Err(ResolutionFailureCode::WriteScopeEscape);
        }
        fs::remove_dir_all(&path).map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?;
    }
    package_directory
        .remove_child(
            OsStr::new("npmrc"),
            false,
            "compatibility isolated npm config cleanup",
        )
        .map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?;
    package_directory
        .sync()
        .map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?;

    Ok(ResolvedPackageSet {
        id: recipe.id.clone(),
        requested: RequestedPackageSet {
            adapter: recipe.adapter.clone(),
            adapter_selector: recipe.adapter_selector.clone(),
            agent_cli: recipe.agent_cli.clone(),
        },
        adapter: parsed_lock.adapter,
        agent_cli: parsed_lock.agent_cli,
        bundled_cli_version,
        resolution_lock_sha256: parsed_lock.sha256,
        inventory_sha256: sealed.inventory_sha256,
        tree_sha256: sealed.tree_sha256,
        adapter_executable: ArtifactIdentity {
            canonical_path: executable_snapshot
                .canonical_path
                .to_string_lossy()
                .into_owned(),
            sha256: executable_snapshot.sha256,
        },
        adapter_executable_relative: executable_relative,
    })
}

#[derive(Clone, Debug)]
struct PreparedSettings {
    canonical_path: PathBuf,
    sha256: String,
    bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
struct PreparedCase {
    input: ResolutionCaseInput,
    config: toml::Value,
    settings: Option<PreparedSettings>,
}

fn table_has_exact_keys(
    label: &str,
    table: &toml::map::Map<String, toml::Value>,
    allowed: &[&str],
) -> Result<(), String> {
    let allowed: BTreeSet<_> = allowed.iter().copied().collect();
    if let Some(key) = table.keys().find(|key| !allowed.contains(key.as_str())) {
        return Err(format!("{label} contains unsupported field {key:?}"));
    }
    Ok(())
}

fn expected_template_args(template: ConfigTemplate) -> &'static [&'static str] {
    match template {
        ConfigTemplate::CodexHostReadOnlyV1 => &[
            "-c",
            "sandbox_mode=\"read-only\"",
            "-c",
            "approval_policy=\"never\"",
        ],
        ConfigTemplate::CodexReaderReadOnlyV1 => &[
            "-c",
            "sandbox_mode=\"danger-full-access\"",
            "-c",
            "approval_policy=\"never\"",
        ],
        ConfigTemplate::ClaudeHostReadOnlyV1 | ConfigTemplate::ClaudeReaderReadOnlyV1 => &[],
    }
}

fn template_is_reader(template: ConfigTemplate) -> bool {
    matches!(
        template,
        ConfigTemplate::CodexReaderReadOnlyV1 | ConfigTemplate::ClaudeReaderReadOnlyV1
    )
}

fn prepare_case_source(input: ResolutionCaseInput) -> Result<PreparedCase, String> {
    if local_file::sha256_hex(&input.baseline_config.bytes) != input.baseline_config.sha256 {
        return Err("baseline config bytes do not match their pinned identity".into());
    }
    let raw = std::str::from_utf8(&input.baseline_config.bytes)
        .map_err(|_| "baseline config must be UTF-8")?;
    secret_free_raw("baseline config", raw)?;
    let value: toml::Value = toml::from_str(raw).map_err(|_| "baseline config is invalid TOML")?;
    let root = value
        .as_table()
        .ok_or("baseline config root must be a table")?;
    let reader = template_is_reader(input.recipe.config_template);
    let mut root_keys = vec!["default", "agents", "registry", "server"];
    if reader {
        root_keys.push("allowed_cwd_root");
    }
    table_has_exact_keys("baseline config root", root, &root_keys)?;

    let agents = root
        .get("agents")
        .and_then(toml::Value::as_array)
        .ok_or("baseline config requires one agents array")?;
    let [agent_value] = agents.as_slice() else {
        return Err("baseline config requires exactly one agent".into());
    };
    let agent_table = agent_value
        .as_table()
        .ok_or("baseline agent must be a table")?;
    let mut agent_keys = vec![
        "id",
        "cmd",
        "pre_authenticated",
        "model",
        "effort",
        "mode",
        "args",
    ];
    if reader {
        agent_keys.push("sandbox");
    }
    table_has_exact_keys("baseline agent", agent_table, &agent_keys)?;
    let registry = root
        .get("registry")
        .and_then(toml::Value::as_table)
        .ok_or("baseline config requires a registry table")?;
    table_has_exact_keys("baseline registry", registry, &["allowed_cmds"])?;
    let allowed_commands = registry
        .get("allowed_cmds")
        .and_then(toml::Value::as_array)
        .ok_or("baseline registry requires allowed_cmds")?;
    if allowed_commands.len() != 1
        || !allowed_commands
            .iter()
            .all(|value| value.as_str().is_some())
    {
        return Err("baseline registry requires exactly one string command".into());
    }
    let server = root
        .get("server")
        .and_then(toml::Value::as_table)
        .ok_or("baseline config requires a server table")?;
    table_has_exact_keys("baseline server", server, &["addr"])?;

    let parsed = crate::config::RegistryConfig::parse(raw)
        .map_err(|_| "baseline config does not satisfy the bridge config parser")?;
    if parsed.default != input.agent || parsed.agents.len() != 1 {
        return Err("baseline config default/agent identity mismatch".into());
    }
    let agent = &parsed.agents[0];
    if agent.id != input.agent
        || agent.model.as_deref() != Some(input.model.as_str())
        || agent.effort.as_deref() != input.effort.as_deref()
        || agent.mode.as_deref() != input.mode.as_deref()
        || agent.kind.is_some()
        || agent.base_url.is_some()
        || agent.api_key_env.is_some()
        || agent.model_provider.is_some()
        || agent.cwd.is_some()
        || agent.session_cwd.is_some()
        || agent.watchdog.is_some()
        || agent.auth_method.is_some()
        || agent.host_fallback_eligible
        || agent.name.is_some()
        || agent.description.is_some()
        || !agent.tags.is_empty()
        || agent.version.is_some()
        || !agent.mcp.is_empty()
        || agent.mcp_delivery.is_some()
        || !agent.extensions.is_empty()
    {
        return Err("baseline config agent does not match the closed template input".into());
    }
    let expected_pre_authenticated = input.auth_path == "pre_authenticated";
    if agent.pre_authenticated != expected_pre_authenticated
        || (!expected_pre_authenticated && input.auth_path != "automatic")
    {
        return Err("baseline config auth path does not match the case".into());
    }
    let expected_args: Vec<_> = expected_template_args(input.recipe.config_template)
        .iter()
        .map(|value| (*value).to_owned())
        .collect();
    if agent.args != expected_args {
        return Err("baseline config arguments do not match the closed template".into());
    }
    if parsed.delegation.is_some()
        || parsed.store.is_some()
        || !parsed.prompts.is_empty()
        || !parsed.workflows.is_empty()
        || !parsed.languages.is_empty()
        || parsed.watchdog.is_some()
        || parsed.verify.is_some()
        || parsed.review.is_some()
        || parsed.implement.is_some()
        || parsed.merge.is_some()
        || parsed.batch.is_some()
        || parsed.worktrees.is_some()
    {
        return Err("baseline config contains state outside the closed canary template".into());
    }

    let mut settings = None;
    match (reader, agent.sandbox.as_ref()) {
        (false, None) => {
            if parsed.allowed_cwd_root.is_some() || !input.component_pins.is_empty() {
                return Err("host template must not carry reader-only state".into());
            }
        }
        (false, Some(_)) | (true, None) => {
            return Err("baseline sandbox presence does not match the template".into())
        }
        (true, Some(sandbox)) => {
            if sandbox.runtime.is_some()
                || sandbox.access != "ro"
                || sandbox.egress != "locked"
                || sandbox.network.is_none()
                || sandbox.proxy.is_none()
                || parsed.allowed_cwd_root.as_deref() != Some(sandbox.mount.as_str())
            {
                return Err(
                    "baseline reader sandbox does not match the closed read-only template".into(),
                );
            }
            let sandbox_table = agent_table
                .get("sandbox")
                .and_then(toml::Value::as_table)
                .ok_or("baseline reader requires a sandbox table")?;
            table_has_exact_keys(
                "baseline sandbox",
                sandbox_table,
                &[
                    "image", "mount", "access", "egress", "network", "proxy", "no_proxy", "volumes",
                ],
            )?;
            let expected_destinations: BTreeSet<String> = match input.recipe.config_template {
                ConfigTemplate::CodexReaderReadOnlyV1 => {
                    BTreeSet::from(["/root/.codex/auth.json".to_owned()])
                }
                ConfigTemplate::ClaudeReaderReadOnlyV1 => BTreeSet::from([
                    "/root/.claude/.credentials.json".to_owned(),
                    "/root/.claude/settings.json".to_owned(),
                ]),
                _ => unreachable!("reader match excludes host templates"),
            };
            let mut destinations = BTreeSet::new();
            for volume in &sandbox.volumes {
                let declaration = bridge_core::sandbox::parse_sandbox_volume(volume)
                    .map_err(|_| "baseline sandbox volume is invalid")?;
                if !destinations.insert(declaration.destination().to_owned())
                    || !expected_destinations.contains(declaration.destination())
                {
                    return Err("baseline sandbox volumes do not match the closed template".into());
                }
                let bridge_core::sandbox::SandboxVolumeSource::Host(source) = declaration.source()
                else {
                    return Err(
                        "baseline sandbox template requires absolute host volume sources".into(),
                    );
                };
                if declaration.destination() == "/root/.claude/settings.json" {
                    let expected = input
                        .component_pins
                        .get("fable-settings")
                        .and_then(|value| value.strip_prefix("sha256:"))
                        .ok_or("Claude reader requires the exact fable-settings component pin")?;
                    let snapshot = local_file::read_regular_file_bounded(
                        Path::new(source),
                        "floating non-secret Fable settings",
                        MAX_SETTINGS_BYTES,
                    )
                    .map_err(|_| "non-secret Fable settings are unavailable")?;
                    if snapshot.sha256 != expected {
                        return Err(
                            "non-secret Fable settings do not match their component pin".into()
                        );
                    }
                    let raw = std::str::from_utf8(&snapshot.bytes)
                        .map_err(|_| "non-secret Fable settings must be UTF-8")?;
                    secret_free_raw("non-secret Fable settings", raw)?;
                    let _: serde_json::Value = serde_json::from_slice(&snapshot.bytes)
                        .map_err(|_| "non-secret Fable settings must be valid JSON")?;
                    settings = Some(PreparedSettings {
                        canonical_path: snapshot.canonical_path,
                        sha256: snapshot.sha256,
                        bytes: snapshot.bytes,
                    });
                }
            }
            if destinations != expected_destinations {
                return Err("baseline sandbox is missing a required closed-template volume".into());
            }
            match input.recipe.config_template {
                ConfigTemplate::CodexReaderReadOnlyV1 if !input.component_pins.is_empty() => {
                    return Err("Codex reader template must not carry component pins".into())
                }
                ConfigTemplate::ClaudeReaderReadOnlyV1
                    if input.component_pins.len() != 1 || settings.is_none() =>
                {
                    return Err(
                        "Claude reader template requires exactly the Fable settings pin".into(),
                    )
                }
                _ => {}
            }
        }
    }

    Ok(PreparedCase {
        input,
        config: value,
        settings,
    })
}

fn replace_settings_volume(
    sandbox: &mut toml::map::Map<String, toml::Value>,
    settings_path: &Path,
) -> Result<(), String> {
    let volumes = sandbox
        .get_mut("volumes")
        .and_then(toml::Value::as_array_mut)
        .ok_or("generated Claude reader requires sandbox volumes")?;
    let mut replaced = false;
    for volume in volumes {
        let raw = volume
            .as_str()
            .ok_or("generated sandbox volumes must be strings")?;
        let declaration = bridge_core::sandbox::parse_sandbox_volume(raw)
            .map_err(|_| "generated sandbox volume is invalid")?;
        if declaration.destination() == "/root/.claude/settings.json" {
            if replaced {
                return Err("generated Claude reader repeats its settings volume".into());
            }
            let source = artifact_path("generated Fable settings", settings_path)?;
            *volume = toml::Value::String(format!("{source}:/root/.claude/settings.json:ro"));
            replaced = true;
        }
    }
    if !replaced {
        return Err("generated Claude reader is missing its settings volume".into());
    }
    Ok(())
}

fn render_generated_config(
    prepared: &PreparedCase,
    package: &ResolvedPackageSet,
    image: Option<&ResolvedImage>,
    bundle: &Path,
    runtime_executable: &Path,
    settings_path: Option<&Path>,
) -> Result<Vec<u8>, String> {
    let mut config = prepared.config.clone();
    let root = config
        .as_table_mut()
        .ok_or("generated config root must remain a table")?;
    let agents = root
        .get_mut("agents")
        .and_then(toml::Value::as_array_mut)
        .ok_or("generated config must retain one agent")?;
    let agent = agents
        .first_mut()
        .and_then(toml::Value::as_table_mut)
        .ok_or("generated config agent must remain a table")?;
    let host_executable = bundle
        .join("packages")
        .join(&package.id)
        .join("tree")
        .join(&package.adapter_executable_relative);
    let command = if template_is_reader(prepared.input.recipe.config_template) {
        format!(
            "/opt/a2a/packages/{}/{}",
            package.id, package.adapter_executable_relative
        )
    } else {
        artifact_path("generated host adapter executable", &host_executable)?
    };
    agent.insert("cmd".into(), toml::Value::String(command.clone()));
    if let Some(image) = image {
        let sandbox = agent
            .get_mut("sandbox")
            .and_then(toml::Value::as_table_mut)
            .ok_or("generated reader config requires a sandbox")?;
        sandbox.insert(
            "runtime".into(),
            toml::Value::String(artifact_path(
                "generated runtime executable",
                runtime_executable,
            )?),
        );
        sandbox.insert(
            "image".into(),
            toml::Value::String(image.final_image_id.clone()),
        );
        match (prepared.settings.as_ref(), settings_path) {
            (Some(_), Some(settings_path)) => {
                replace_settings_volume(sandbox, settings_path)?;
            }
            (None, None) => {}
            _ => return Err("generated settings materialization is incomplete".into()),
        }
    } else if agent.contains_key("sandbox") {
        return Err("generated host config unexpectedly retained a sandbox".into());
    }
    let allowed = if image.is_some() {
        artifact_path("generated runtime executable", runtime_executable)?
    } else {
        command
    };
    let registry = root
        .get_mut("registry")
        .and_then(toml::Value::as_table_mut)
        .ok_or("generated config must retain its registry")?;
    registry.insert(
        "allowed_cmds".into(),
        toml::Value::Array(vec![toml::Value::String(allowed)]),
    );
    let mut bytes = toml::to_string_pretty(&config)
        .map_err(|_| "generated config serialization failed")?
        .into_bytes();
    if !bytes.ends_with(b"\n") {
        bytes.push(b'\n');
    }
    let raw = std::str::from_utf8(&bytes).map_err(|_| "generated config must be UTF-8")?;
    secret_free_raw("generated config", raw)?;
    crate::config::RegistryConfig::parse(raw)
        .and_then(crate::config::RegistryConfig::into_snapshot)
        .map_err(|_| "generated config does not satisfy bridge registry invariants")?;
    Ok(bytes)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ResolvedBaseImage {
    registry_index_digest: String,
    platform_manifest_digest: String,
}

fn parse_base_manifest(
    bytes: &[u8],
    os: &str,
    architecture: &str,
) -> Result<ResolvedBaseImage, String> {
    if bytes.is_empty() || bytes.len() > MAX_COMMAND_OUTPUT_BYTES {
        return Err("base manifest output is empty or oversized".into());
    }
    let raw = std::str::from_utf8(bytes).map_err(|_| "base manifest output must be UTF-8")?;
    secret_free_raw("base manifest output", raw)?;
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| "base manifest output is invalid JSON")?;
    let manifests = value
        .get("manifests")
        .and_then(serde_json::Value::as_array)
        .ok_or("base manifest output must contain an index manifest list")?;
    let mut selected = None;
    for manifest in manifests {
        let platform = manifest
            .get("platform")
            .and_then(serde_json::Value::as_object)
            .ok_or("base manifest platform is missing")?;
        let candidate_os = platform
            .get("os")
            .and_then(serde_json::Value::as_str)
            .ok_or("base manifest platform OS is missing")?;
        let candidate_arch = platform
            .get("architecture")
            .and_then(serde_json::Value::as_str)
            .ok_or("base manifest platform architecture is missing")?;
        bounded_text("base manifest platform OS", candidate_os, MAX_ID_BYTES)?;
        bounded_text(
            "base manifest platform architecture",
            candidate_arch,
            MAX_ID_BYTES,
        )?;
        if candidate_os == os && candidate_arch == architecture {
            let digest = manifest
                .get("digest")
                .and_then(serde_json::Value::as_str)
                .ok_or("base manifest platform digest is missing")?;
            validate_image_digest("base manifest platform digest", digest)?;
            if selected.replace(digest.to_owned()).is_some() {
                return Err("base manifest contains multiple matching platform digests".into());
            }
        }
    }
    Ok(ResolvedBaseImage {
        registry_index_digest: format!("sha256:{}", local_file::sha256_hex(bytes)),
        platform_manifest_digest: selected
            .ok_or("base manifest has no exact OS/architecture match")?,
    })
}

fn runtime_env(path: OsString, cwd: &Path) -> BTreeMap<OsString, OsString> {
    [
        ("PATH", path),
        ("HOME", cwd.join("runtime-home").into_os_string()),
        ("DOCKER_CONFIG", cwd.join("runtime-config").into_os_string()),
        (
            "REGISTRY_AUTH_FILE",
            cwd.join("runtime-config/auth.json").into_os_string(),
        ),
    ]
    .into_iter()
    .map(|(key, value)| (OsString::from(key), value))
    .collect()
}

fn resolve_base_command(
    runtime: RuntimeKind,
    executable: &Path,
    safe_path: OsString,
    cwd: PathBuf,
    timeout: Duration,
) -> ResolutionCommandSpec {
    let args = match runtime {
        RuntimeKind::Docker => vec!["buildx", "imagetools", "inspect", "--raw", NODE_READER_BASE],
        RuntimeKind::Podman => vec![
            "inspect",
            "--raw",
            "docker://docker.io/library/node:24-slim",
        ],
    };
    let env = runtime_env(safe_path, &cwd);
    ResolutionCommandSpec {
        family: ResolutionCommandFamily::Runtime,
        kind: ResolutionCommandKind::ResolveBase,
        program: executable.to_path_buf(),
        args: args.into_iter().map(OsString::from).collect(),
        cwd,
        env,
        timeout,
        max_output_bytes: MAX_COMMAND_OUTPUT_BYTES,
    }
}

fn image_tag_absence_command(
    runtime: RuntimeKind,
    executable: &Path,
    safe_path: OsString,
    cwd: PathBuf,
    timeout: Duration,
    tag: &str,
) -> ResolutionCommandSpec {
    let args = match runtime {
        RuntimeKind::Docker | RuntimeKind::Podman => vec![
            OsString::from("image"),
            OsString::from("ls"),
            OsString::from("--quiet"),
            OsString::from("--no-trunc"),
            OsString::from("--filter"),
            OsString::from(format!("reference={tag}")),
        ],
    };
    let env = runtime_env(safe_path, &cwd);
    ResolutionCommandSpec {
        family: ResolutionCommandFamily::Runtime,
        kind: ResolutionCommandKind::EnsureImageTagAbsent,
        program: executable.to_path_buf(),
        args,
        cwd,
        env,
        timeout,
        max_output_bytes: MAX_COMMAND_OUTPUT_BYTES,
    }
}

fn confirm_image_tag_absent(bytes: &[u8]) -> Result<(), ResolutionFailureCode> {
    if bytes.len() > MAX_COMMAND_OUTPUT_BYTES {
        return Err(ResolutionFailureCode::ImageTagStateUnknown);
    }
    let raw =
        std::str::from_utf8(bytes).map_err(|_| ResolutionFailureCode::ImageTagStateUnknown)?;
    secret_free_raw("image tag listing", raw)
        .map_err(|_| ResolutionFailureCode::ImageTagStateUnknown)?;
    if raw.trim().is_empty() {
        Ok(())
    } else {
        Err(ResolutionFailureCode::ImageTagAlreadyExists)
    }
}

fn image_labels(
    resolution_id: &str,
    recipe_sha256: &str,
    image_id: &str,
    packages: &[ResolvedPackageSet],
) -> BTreeMap<String, String> {
    let mut labels = BTreeMap::from([
        (
            "io.a2a-bridge.r3c.resolution-id".into(),
            resolution_id.into(),
        ),
        (
            "io.a2a-bridge.r3c.recipe-sha256".into(),
            recipe_sha256.into(),
        ),
        ("io.a2a-bridge.r3c.image-id".into(), image_id.into()),
    ]);
    for package in packages {
        labels.insert(
            format!("io.a2a-bridge.r3c.package.{}.inventory-sha256", package.id),
            package.inventory_sha256.clone(),
        );
        labels.insert(
            format!("io.a2a-bridge.r3c.package.{}.tree-sha256", package.id),
            package.tree_sha256.clone(),
        );
    }
    labels
}

fn owned_image_tag(resolution_id: &str, image_id: &str) -> Result<String, String> {
    stable_id("resolution image tag resolution id", resolution_id)?;
    stable_id("resolution image tag image id", image_id)?;
    let tag = format!("localhost/a2a-bridge-r3c:{resolution_id}-{image_id}");
    bounded_text("resolution-owned image tag", &tag, MAX_TEXT_BYTES)?;
    Ok(tag)
}

fn render_containerfile(platform_digest: &str, package_ids: &[String]) -> Result<Vec<u8>, String> {
    validate_image_digest("container template base digest", platform_digest)?;
    if package_ids.is_empty() || package_ids.len() > MAX_PACKAGE_SETS {
        return Err("container template requires a bounded non-empty package set".into());
    }
    let mut seen = BTreeSet::new();
    let mut out = format!(
        "# Generated by a2a-bridge R3c; no recipe text is executable.\nFROM docker.io/library/node@{platform_digest}\n"
    );
    for package_id in package_ids {
        stable_id("container template package id", package_id)?;
        if !seen.insert(package_id) {
            return Err("container template repeats a package set".into());
        }
        out.push_str(&format!(
            "COPY packages/{package_id}/tree/ /opt/a2a/packages/{package_id}/\n"
        ));
    }
    out.push_str("WORKDIR /work\n");
    Ok(out.into_bytes())
}

struct ImageBuildCommand<'a> {
    runtime: RuntimeKind,
    executable: &'a Path,
    safe_path: OsString,
    cwd: PathBuf,
    timeout: Duration,
    platform: &'a str,
    containerfile: &'a str,
    tag: &'a str,
    labels: &'a BTreeMap<String, String>,
}

fn image_build_command(input: ImageBuildCommand<'_>) -> ResolutionCommandSpec {
    let mut args = vec![OsString::from("build")];
    match input.runtime {
        RuntimeKind::Docker => {
            args.push(OsString::from("--pull=false"));
        }
        RuntimeKind::Podman => {
            args.push(OsString::from("--pull=never"));
        }
    }
    args.extend([
        OsString::from("--network=none"),
        OsString::from(format!("--platform={}", input.platform)),
        OsString::from("--file"),
        OsString::from(input.containerfile),
        OsString::from("--tag"),
        OsString::from(input.tag),
    ]);
    for (key, value) in input.labels {
        args.push(OsString::from("--label"));
        args.push(OsString::from(format!("{key}={value}")));
    }
    args.push(OsString::from("."));
    let env = runtime_env(input.safe_path, &input.cwd);
    ResolutionCommandSpec {
        family: ResolutionCommandFamily::Runtime,
        kind: ResolutionCommandKind::BuildImage,
        program: input.executable.to_path_buf(),
        args,
        cwd: input.cwd,
        env,
        timeout: input.timeout,
        max_output_bytes: MAX_COMMAND_OUTPUT_BYTES,
    }
}

fn oci_architecture(architecture: &str) -> Result<&'static str, ResolutionFailureCode> {
    match architecture {
        "aarch64" | "arm64" => Ok("arm64"),
        "x86_64" | "amd64" => Ok("amd64"),
        _ => Err(ResolutionFailureCode::BaseDigestUnavailable),
    }
}

struct ImageMaterialization<'a> {
    recipe: &'a ImageRecipe,
    packages: &'a [ResolvedPackageSet],
    resolution_id: &'a str,
    recipe_sha256: &'a str,
    runtime: RuntimeKind,
    runtime_executable: &'a Path,
    base_resolver_executable: &'a Path,
    safe_path: OsString,
    bundle: &'a BundlePublisher,
    image_directory: &'a local_file::PinnedDirectory,
    architecture: &'a str,
    timeout: Duration,
}

async fn materialize_image(
    input: ImageMaterialization<'_>,
    executor: &dyn ResolutionExecutor,
    owned_resources: &mut Vec<OwnedResource>,
) -> Result<ResolvedImage, ResolutionFailureCode> {
    let platform_architecture = oci_architecture(input.architecture)?;
    let platform = format!("linux/{platform_architecture}");
    let raw_manifest = executor
        .execute(&resolve_base_command(
            input.runtime,
            input.base_resolver_executable,
            input.safe_path.clone(),
            input.bundle.pin.acp_session_cwd(),
            input.timeout,
        ))
        .await?;
    let base = parse_base_manifest(&raw_manifest, "linux", platform_architecture)
        .map_err(|_| ResolutionFailureCode::BaseDigestUnavailable)?;
    let package_ids: Vec<_> = input
        .packages
        .iter()
        .map(|package| package.id.clone())
        .collect();
    let containerfile = render_containerfile(&base.platform_manifest_digest, &package_ids)
        .map_err(|_| ResolutionFailureCode::ConfigTemplateMismatch)?;
    let build_template_sha256 = local_file::sha256_hex(&containerfile);
    let image_build_directory = input
        .image_directory
        .create_child_directory(
            OsStr::new(&input.recipe.id),
            0o700,
            "compatibility image build directory",
        )
        .map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?;
    create_synced_file(
        &image_build_directory,
        OsStr::new("Containerfile"),
        0o600,
        &containerfile,
        "compatibility generated Containerfile",
    )
    .map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?;
    let containerfile_path = image_build_directory.canonical_path().join("Containerfile");
    let containerfile_path = artifact_path("generated Containerfile", &containerfile_path)
        .map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?;
    let tag = owned_image_tag(input.resolution_id, &input.recipe.id)
        .map_err(|_| ResolutionFailureCode::ImageTagAlreadyExists)?;
    let tag_listing = executor
        .execute(&image_tag_absence_command(
            input.runtime,
            input.runtime_executable,
            input.safe_path.clone(),
            input.bundle.pin.acp_session_cwd(),
            input.timeout,
            &tag,
        ))
        .await?;
    confirm_image_tag_absent(&tag_listing)?;
    owned_resources.push(OwnedResource {
        kind: OwnedResourceKind::ImageTag,
        identity: tag.clone(),
    });
    let labels = image_labels(
        input.resolution_id,
        input.recipe_sha256,
        &input.recipe.id,
        input.packages,
    );
    executor
        .execute(&image_build_command(ImageBuildCommand {
            runtime: input.runtime,
            executable: input.runtime_executable,
            safe_path: input.safe_path.clone(),
            cwd: input.bundle.pin.acp_session_cwd(),
            timeout: input.timeout,
            platform: &platform,
            containerfile: &containerfile_path,
            tag: &tag,
            labels: &labels,
        }))
        .await?;
    let inspect = executor
        .execute(&image_inspect_command(
            input.runtime_executable,
            input.safe_path,
            input.bundle.pin.acp_session_cwd(),
            input.timeout,
            &tag,
        ))
        .await?;
    let final_image_id = parse_image_inspect(&inspect, &labels)
        .map_err(|_| ResolutionFailureCode::ImageLabelMismatch)?;
    Ok(ResolvedImage {
        id: input.recipe.id.clone(),
        requested_base: input.recipe.base.clone(),
        package_sets: package_ids,
        registry_index_digest: base.registry_index_digest,
        platform_manifest_digest: base.platform_manifest_digest,
        build_template_sha256,
        final_image_id,
        owned_tag: tag,
        labels,
    })
}

fn image_inspect_command(
    executable: &Path,
    safe_path: OsString,
    cwd: PathBuf,
    timeout: Duration,
    tag: &str,
) -> ResolutionCommandSpec {
    let env = runtime_env(safe_path, &cwd);
    ResolutionCommandSpec {
        family: ResolutionCommandFamily::Runtime,
        kind: ResolutionCommandKind::InspectImage,
        program: executable.to_path_buf(),
        args: ["image", "inspect", tag]
            .into_iter()
            .map(OsString::from)
            .collect(),
        cwd,
        env,
        timeout,
        max_output_bytes: MAX_COMMAND_OUTPUT_BYTES,
    }
}

fn parse_image_inspect(
    bytes: &[u8],
    expected_labels: &BTreeMap<String, String>,
) -> Result<String, String> {
    if bytes.is_empty() || bytes.len() > MAX_COMMAND_OUTPUT_BYTES {
        return Err("image inspect output is empty or oversized".into());
    }
    let raw = std::str::from_utf8(bytes).map_err(|_| "image inspect output must be UTF-8")?;
    secret_free_raw("image inspect output", raw)?;
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| "image inspect output is invalid JSON")?;
    let images = value
        .as_array()
        .ok_or("image inspect output must be an array")?;
    let [image] = images.as_slice() else {
        return Err("image inspect output must contain exactly one image".into());
    };
    let id = image
        .get("Id")
        .and_then(serde_json::Value::as_str)
        .ok_or("image inspect output is missing the immutable id")?;
    validate_image_digest("image inspect immutable id", id)?;
    let labels: BTreeMap<String, String> = serde_json::from_value(
        image
            .pointer("/Config/Labels")
            .cloned()
            .ok_or("image inspect output is missing labels")?,
    )
    .map_err(|_| "image inspect labels must be a string map")?;
    if &labels != expected_labels {
        return Err("image inspect labels do not match exact resolution provenance".into());
    }
    Ok(id.to_owned())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// The repeated suffix is deliberate operator vocabulary: every variant identifies the exact identity
// that changed, while `wire` preserves a distinct stable admission code for that drift family.
#[allow(clippy::enum_variant_names)]
pub(super) enum RevalidationFailure {
    ResolutionArtifactChanged,
    RecipeChanged,
    ProductionManifestChanged,
    ExecutionManifestChanged,
    CandidateBinaryChanged,
    RuntimeExecutableChanged,
    ProtectedInputChanged,
    GeneratedConfigChanged,
    PackageLockChanged,
    PackageTreeChanged,
    AdapterExecutableChanged,
    ImageChanged,
    EnvironmentOwnerChanged,
    PlatformChanged,
    PrerequisiteChanged,
    BindingChanged,
}

impl RevalidationFailure {
    pub(super) fn wire(self) -> &'static str {
        match self {
            Self::ResolutionArtifactChanged => "resolution_artifact_changed",
            Self::RecipeChanged => "resolution_recipe_changed",
            Self::ProductionManifestChanged => "resolution_production_manifest_changed",
            Self::ExecutionManifestChanged => "resolution_execution_manifest_changed",
            Self::CandidateBinaryChanged => "candidate_binary_changed",
            Self::RuntimeExecutableChanged => "resolution_runtime_executable_changed",
            Self::ProtectedInputChanged => "resolution_protected_input_changed",
            Self::GeneratedConfigChanged => "resolution_generated_config_changed",
            Self::PackageLockChanged => "resolution_package_lock_changed",
            Self::PackageTreeChanged => "resolution_package_tree_changed",
            Self::AdapterExecutableChanged => "resolution_adapter_executable_changed",
            Self::ImageChanged => "resolution_image_changed",
            Self::EnvironmentOwnerChanged => "resolution_environment_owner_changed",
            Self::PlatformChanged => "resolution_platform_changed",
            Self::PrerequisiteChanged => "resolution_prerequisite_changed",
            Self::BindingChanged => "resolution_binding_changed",
        }
    }
}

fn revalidate_regular_file(
    canonical_path: &str,
    expected_sha256: &str,
    max_bytes: u64,
    failure: RevalidationFailure,
) -> Result<local_file::LocalFileSnapshot, RevalidationFailure> {
    let snapshot = local_file::read_regular_file_bounded(
        Path::new(canonical_path),
        "compatibility resolution revalidation",
        max_bytes,
    )
    .map_err(|_| failure)?;
    if snapshot.canonical_path.to_str() != Some(canonical_path)
        || snapshot.sha256 != expected_sha256
    {
        return Err(failure);
    }
    Ok(snapshot)
}

fn revalidate_executable(
    identity: &ExecutableIdentity,
    failure: RevalidationFailure,
) -> Result<(), RevalidationFailure> {
    let snapshot = revalidate_regular_file(
        &identity.canonical_path,
        &identity.sha256,
        MAX_EXECUTABLE_BYTES,
        failure,
    )?;
    if snapshot.bytes.len() as u64 != identity.byte_length {
        return Err(failure);
    }
    Ok(())
}

fn resolution_bundle_root(loaded: &LoadedResolution) -> Result<PathBuf, RevalidationFailure> {
    let root = loaded
        .canonical_path
        .parent()
        .ok_or(RevalidationFailure::ResolutionArtifactChanged)?;
    let canonical_root =
        fs::canonicalize(root).map_err(|_| RevalidationFailure::ResolutionArtifactChanged)?;
    if canonical_root != root
        || loaded.canonical_path != canonical_root.join("resolution.json")
        || loaded.canonical_path.to_str() != Some(loaded.canonical_path_text.as_str())
        || !loaded.artifact.owned_resources.iter().any(|resource| {
            resource.kind == OwnedResourceKind::Bundle
                && Path::new(&resource.identity) == canonical_root
        })
    {
        return Err(RevalidationFailure::ResolutionArtifactChanged);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

        let metadata = fs::symlink_metadata(&canonical_root)
            .map_err(|_| RevalidationFailure::ResolutionArtifactChanged)?;
        // SAFETY: geteuid has no preconditions and only reads the process credential.
        let owner = unsafe { libc::geteuid() };
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.uid() != owner
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(RevalidationFailure::ResolutionArtifactChanged);
        }
    }
    Ok(canonical_root)
}

pub(super) fn revalidate_resolution_global(
    loaded: &LoadedResolution,
    environment_owner: &str,
) -> Result<(), RevalidationFailure> {
    if loaded.artifact.state != ResolutionState::Complete {
        return Err(RevalidationFailure::ResolutionArtifactChanged);
    }
    let root = resolution_bundle_root(loaded)?;
    let resolution_path = loaded
        .canonical_path
        .to_str()
        .ok_or(RevalidationFailure::ResolutionArtifactChanged)?;
    revalidate_regular_file(
        resolution_path,
        &loaded.sha256,
        MAX_RESOLUTION_BYTES,
        RevalidationFailure::ResolutionArtifactChanged,
    )?;
    if loaded.artifact.environment.environment_owner != environment_owner {
        return Err(RevalidationFailure::EnvironmentOwnerChanged);
    }
    if loaded.artifact.environment.os != std::env::consts::OS
        || loaded.artifact.environment.architecture != std::env::consts::ARCH
    {
        return Err(RevalidationFailure::PlatformChanged);
    }
    revalidate_regular_file(
        &loaded.artifact.recipes.canonical_path,
        &loaded.artifact.recipes.sha256,
        MAX_RECIPE_BYTES,
        RevalidationFailure::RecipeChanged,
    )?;
    revalidate_regular_file(
        &loaded.artifact.production_manifest.canonical_path,
        &loaded.artifact.production_manifest.sha256,
        MAX_RECIPE_BYTES,
        RevalidationFailure::ProductionManifestChanged,
    )?;
    let execution_manifest = loaded
        .artifact
        .execution_manifest
        .as_ref()
        .ok_or(RevalidationFailure::ExecutionManifestChanged)?;
    if Path::new(&execution_manifest.canonical_path) != root.join("execution-manifest.toml") {
        return Err(RevalidationFailure::ExecutionManifestChanged);
    }
    revalidate_regular_file(
        &execution_manifest.canonical_path,
        &execution_manifest.sha256,
        MAX_RECIPE_BYTES,
        RevalidationFailure::ExecutionManifestChanged,
    )?;
    revalidate_executable(
        &loaded.artifact.candidate,
        RevalidationFailure::CandidateBinaryChanged,
    )?;
    revalidate_executable(
        &loaded.artifact.environment.runtime_executable,
        RevalidationFailure::RuntimeExecutableChanged,
    )?;
    for input in &loaded.artifact.protected_inputs {
        revalidate_regular_file(
            &input.path,
            &input.after_sha256,
            MAX_EXECUTABLE_BYTES,
            RevalidationFailure::ProtectedInputChanged,
        )?;
    }
    Ok(())
}

fn revalidate_package(
    loaded: &LoadedResolution,
    package: &ResolvedPackageSet,
) -> Result<(), RevalidationFailure> {
    let root = resolution_bundle_root(loaded)?;
    let package_root = root.join("packages").join(&package.id);
    let lock = local_file::read_regular_file_bounded(
        &package_root.join("package-lock.json"),
        "compatibility resolved package lock revalidation",
        MAX_LOCK_BYTES,
    )
    .map_err(|_| RevalidationFailure::PackageLockChanged)?;
    if lock.canonical_path != package_root.join("package-lock.json")
        || lock.sha256 != package.resolution_lock_sha256
    {
        return Err(RevalidationFailure::PackageLockChanged);
    }
    let parsed = parse_package_lock(
        &lock.bytes,
        &package.requested.adapter,
        &package.requested.agent_cli,
        loaded.artifact.limits.max_files,
    )
    .map_err(|_| RevalidationFailure::PackageLockChanged)?;
    if parsed.adapter != package.adapter || parsed.agent_cli != package.agent_cli {
        return Err(RevalidationFailure::PackageLockChanged);
    }

    let tree = package_root.join("tree");
    let tree_metadata =
        fs::symlink_metadata(&tree).map_err(|_| RevalidationFailure::PackageTreeChanged)?;
    let canonical_tree =
        fs::canonicalize(&tree).map_err(|_| RevalidationFailure::PackageTreeChanged)?;
    if !tree_metadata.is_dir() || tree_metadata.file_type().is_symlink() || canonical_tree != tree {
        return Err(RevalidationFailure::PackageTreeChanged);
    }
    let adapter = crate::doctor::read_installed_package(&node_package_manifest(
        &tree,
        &package.requested.adapter,
    ))
    .map_err(|_| RevalidationFailure::AdapterExecutableChanged)?;
    let cli = crate::doctor::resolve_installed_dependency(&adapter, &package.requested.agent_cli)
        .map_err(|_| RevalidationFailure::AdapterExecutableChanged)?;
    if adapter.name != package.adapter.name
        || adapter.version != package.adapter.version
        || cli.name != package.agent_cli.name
        || cli.version != package.agent_cli.version
        || cli.bundled_cli_version() != package.bundled_cli_version.as_deref()
    {
        return Err(RevalidationFailure::AdapterExecutableChanged);
    }
    let executable = adapter
        .sole_owned_executable()
        .map_err(|_| RevalidationFailure::AdapterExecutableChanged)?;
    let expected_executable = tree.join(&package.adapter_executable_relative);
    let executable_snapshot = local_file::read_regular_file_bounded(
        &executable,
        "compatibility adapter executable revalidation",
        MAX_RESOLUTION_BYTES,
    )
    .map_err(|_| RevalidationFailure::AdapterExecutableChanged)?;
    if executable != expected_executable
        || executable_snapshot.canonical_path != expected_executable
        || executable_snapshot.canonical_path.to_str()
            != Some(package.adapter_executable.canonical_path.as_str())
        || executable_snapshot.sha256 != package.adapter_executable.sha256
    {
        return Err(RevalidationFailure::AdapterExecutableChanged);
    }
    let inspected = inspect_package_tree(&tree, &loaded.artifact.limits)
        .map_err(|_| RevalidationFailure::PackageTreeChanged)?;
    if inspected.inventory_sha256 != package.inventory_sha256
        || inspected.tree_sha256 != package.tree_sha256
    {
        return Err(RevalidationFailure::PackageTreeChanged);
    }
    let mut expected_inventory = serde_json::to_vec_pretty(&inspected.inventory)
        .map_err(|_| RevalidationFailure::PackageTreeChanged)?;
    expected_inventory.push(b'\n');
    let inventory = local_file::read_regular_file_bounded(
        &package_root.join("inventory.json"),
        "compatibility package inventory revalidation",
        MAX_RESOLUTION_BYTES,
    )
    .map_err(|_| RevalidationFailure::PackageTreeChanged)?;
    if inventory.canonical_path != package_root.join("inventory.json")
        || inventory.bytes != expected_inventory
    {
        return Err(RevalidationFailure::PackageTreeChanged);
    }
    Ok(())
}

fn revalidate_generated_prerequisites(
    loaded: &LoadedResolution,
    case: &ResolvedCase,
) -> Result<(), RevalidationFailure> {
    let root = resolution_bundle_root(loaded)?;
    for prerequisite in &case.prerequisites {
        if prerequisite.destination.as_deref() != Some("/root/.claude/settings.json") {
            continue;
        }
        if prerequisite.name != "fable-settings" {
            return Err(RevalidationFailure::PrerequisiteChanged);
        }
        let snapshot = local_file::read_regular_file_bounded(
            &root.join("prerequisites/fable-settings.json"),
            "compatibility generated prerequisite revalidation",
            MAX_SETTINGS_BYTES,
        )
        .map_err(|_| RevalidationFailure::PrerequisiteChanged)?;
        if snapshot.canonical_path != root.join("prerequisites/fable-settings.json")
            || !loaded
                .artifact
                .protected_inputs
                .iter()
                .any(|input| input.after_sha256 == snapshot.sha256)
        {
            return Err(RevalidationFailure::PrerequisiteChanged);
        }
        let raw = std::str::from_utf8(&snapshot.bytes)
            .map_err(|_| RevalidationFailure::PrerequisiteChanged)?;
        secret_free_raw("compatibility generated prerequisite", raw)
            .map_err(|_| RevalidationFailure::PrerequisiteChanged)?;
        serde_json::from_slice::<serde_json::Value>(&snapshot.bytes)
            .map_err(|_| RevalidationFailure::PrerequisiteChanged)?;
    }
    Ok(())
}

async fn revalidate_resolution_case_with_executor(
    loaded: &LoadedResolution,
    environment_owner: &str,
    case_id: &str,
    executor: &dyn ResolutionExecutor,
) -> Result<(), RevalidationFailure> {
    revalidate_resolution_global(loaded, environment_owner)?;
    let root = resolution_bundle_root(loaded)?;
    let case = loaded
        .artifact
        .cases
        .iter()
        .find(|case| case.id == case_id)
        .ok_or(RevalidationFailure::BindingChanged)?;
    let expected_config = root.join("configs").join(format!("{}.toml", case.id));
    if Path::new(&case.generated_config.canonical_path) != expected_config {
        return Err(RevalidationFailure::GeneratedConfigChanged);
    }
    revalidate_regular_file(
        &case.generated_config.canonical_path,
        &case.generated_config.sha256,
        MAX_RECIPE_BYTES,
        RevalidationFailure::GeneratedConfigChanged,
    )?;
    let package = loaded
        .artifact
        .packages
        .iter()
        .find(|package| package.id == case.package_set)
        .ok_or(RevalidationFailure::BindingChanged)?;
    revalidate_package(loaded, package)?;
    revalidate_generated_prerequisites(loaded, case)?;

    if let Some(image_id) = &case.image {
        let image = loaded
            .artifact
            .images
            .iter()
            .find(|image| &image.id == image_id)
            .ok_or(RevalidationFailure::BindingChanged)?;
        let containerfile = local_file::read_regular_file_bounded(
            &root.join("images").join(&image.id).join("Containerfile"),
            "compatibility generated image template revalidation",
            MAX_RECIPE_BYTES,
        )
        .map_err(|_| RevalidationFailure::ImageChanged)?;
        if containerfile.canonical_path != root.join("images").join(&image.id).join("Containerfile")
            || containerfile.sha256 != image.build_template_sha256
        {
            return Err(RevalidationFailure::ImageChanged);
        }
        let runtime_path = Path::new(
            &loaded
                .artifact
                .environment
                .runtime_executable
                .canonical_path,
        );
        let safe_path = std::env::join_paths(
            [
                runtime_path.parent(),
                Some(Path::new("/usr/bin")),
                Some(Path::new("/bin")),
            ]
            .into_iter()
            .flatten(),
        )
        .map_err(|_| RevalidationFailure::RuntimeExecutableChanged)?;
        let inspected = executor
            .execute(&image_inspect_command(
                runtime_path,
                safe_path,
                root,
                Duration::from_secs(loaded.artifact.limits.timeout_secs),
                &image.owned_tag,
            ))
            .await
            .map_err(|_| RevalidationFailure::ImageChanged)?;
        let immutable_id = parse_image_inspect(&inspected, &image.labels)
            .map_err(|_| RevalidationFailure::ImageChanged)?;
        if immutable_id != image.final_image_id {
            return Err(RevalidationFailure::ImageChanged);
        }
    }
    Ok(())
}

pub(super) async fn revalidate_resolution_case(
    loaded: &LoadedResolution,
    environment_owner: &str,
    case_id: &str,
) -> Result<(), RevalidationFailure> {
    revalidate_resolution_case_with_executor(
        loaded,
        environment_owner,
        case_id,
        &ProcessResolutionExecutor,
    )
    .await
}

#[derive(Clone, Debug, Serialize)]
struct GeneratedArtifactPolicy {
    retention_days: u16,
    redaction: &'static str,
}

#[derive(Clone, Debug, Serialize)]
struct GeneratedExecutionCase {
    id: String,
    lane: &'static str,
    evidence_path: String,
    execution_mode: String,
    os: String,
    architecture: String,
    environment_owner: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_image_digest: Option<String>,
    config: PathBuf,
    agent: String,
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_cwd: Option<PathBuf>,
    auth_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    credential_env: Option<String>,
    required_env: Vec<ResolutionRequiredEnvironmentInput>,
    probe: String,
    billable: bool,
    timeout_secs: u64,
    max_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_cost_usd: Option<f64>,
    retry_cap: u8,
    expected_status: String,
    classification: &'static str,
    baseline_case: String,
    artifact: GeneratedArtifactPolicy,
    resolved: ResolvedBinding,
}

#[derive(Clone, Debug, Serialize)]
struct GeneratedExecutionManifest {
    schema_version: u16,
    budget: ResolutionBudgetInput,
    cases: Vec<GeneratedExecutionCase>,
}

fn generated_execution_manifest_bytes(
    budget: &ResolutionBudgetInput,
    cases: Vec<GeneratedExecutionCase>,
) -> Result<Vec<u8>, ResolutionFailureCode> {
    let manifest = GeneratedExecutionManifest {
        schema_version: 1,
        budget: budget.clone(),
        cases,
    };
    let mut bytes = toml::to_string_pretty(&manifest)
        .map_err(|_| ResolutionFailureCode::ConfigTemplateMismatch)?
        .into_bytes();
    if !bytes.ends_with(b"\n") {
        bytes.push(b'\n');
    }
    let raw =
        std::str::from_utf8(&bytes).map_err(|_| ResolutionFailureCode::ConfigTemplateMismatch)?;
    secret_free_raw("generated execution manifest", raw)
        .map_err(|_| ResolutionFailureCode::ConfigTemplateMismatch)?;
    compatibility::validate_manifest_text(raw)
        .map_err(|_| ResolutionFailureCode::ConfigTemplateMismatch)?;
    Ok(bytes)
}

fn add_protected_file(
    files: &mut BTreeMap<PathBuf, ProtectedFileInput>,
    file: ProtectedFileInput,
) -> Result<(), BoxError> {
    if !valid_sha256(&file.sha256) || file.max_bytes == 0 {
        return Err("compatibility resolve: invalid protected input identity".into());
    }
    match files.get_mut(&file.canonical_path) {
        Some(existing) if existing.sha256 != file.sha256 => {
            Err("compatibility resolve: conflicting protected input identities".into())
        }
        Some(existing) => {
            existing.max_bytes = existing.max_bytes.max(file.max_bytes);
            Ok(())
        }
        None => {
            files.insert(file.canonical_path.clone(), file);
            Ok(())
        }
    }
}

fn verify_protected_files(files: &BTreeMap<PathBuf, ProtectedFileInput>) -> Result<(), BoxError> {
    for file in files.values() {
        let snapshot = local_file::read_regular_file_bounded(
            &file.canonical_path,
            "compatibility protected input",
            file.max_bytes,
        )?;
        if snapshot.canonical_path != file.canonical_path || snapshot.sha256 != file.sha256 {
            return Err("compatibility resolve: protected input changed before setup".into());
        }
    }
    Ok(())
}

fn protected_evidence(
    files: &BTreeMap<PathBuf, ProtectedFileInput>,
    recheck: bool,
) -> (Vec<ProtectedInput>, bool) {
    let mut changed = false;
    let mut evidence = Vec::with_capacity(files.len());
    for file in files.values() {
        let after = if recheck {
            match local_file::read_regular_file_bounded(
                &file.canonical_path,
                "compatibility protected input recheck",
                file.max_bytes,
            ) {
                Ok(snapshot) if snapshot.canonical_path == file.canonical_path => snapshot.sha256,
                _ => "0".repeat(64),
            }
        } else {
            file.sha256.clone()
        };
        changed |= after != file.sha256;
        evidence.push(ProtectedInput {
            path: artifact_path("protected input", &file.canonical_path)
                .expect("protected paths are validated before bundle creation"),
            before_sha256: file.sha256.clone(),
            after_sha256: after,
        });
    }
    (evidence, changed)
}

fn prepare_resolution_request(
    request: &mut ProviderFreeResolutionRequest,
) -> Result<(Vec<PreparedCase>, BTreeMap<PathBuf, ProtectedFileInput>), BoxError> {
    validate_recipes(&request.recipes.recipes)
        .map_err(|error| format!("compatibility resolve: invalid recipes: {error}"))?;
    validate_versioned_artifact_identity(
        "resolution request production manifest",
        &request.production_manifest,
    )
    .map_err(|error| format!("compatibility resolve: {error}"))?;
    validate_executable_identity("resolution request candidate", &request.candidate)
        .map_err(|error| format!("compatibility resolve: {error}"))?;
    validate_executable_identity(
        "resolution request runtime executable",
        &request.runtime_executable,
    )
    .map_err(|error| format!("compatibility resolve: {error}"))?;
    if request.cases.is_empty() || request.cases.len() > MAX_CASES {
        return Err(
            "compatibility resolve: selection must contain a bounded non-empty case set".into(),
        );
    }
    stable_id(
        "resolution request environment owner",
        &request.environment_owner,
    )
    .map_err(|error| format!("compatibility resolve: {error}"))?;
    stable_id("resolution request OS", &request.os)
        .map_err(|error| format!("compatibility resolve: {error}"))?;
    stable_id("resolution request architecture", &request.architecture)
        .map_err(|error| format!("compatibility resolve: {error}"))?;

    let recipe_cases: BTreeMap<_, _> = request
        .recipes
        .recipes
        .cases
        .iter()
        .map(|case| (case.id.as_str(), case))
        .collect();
    let mut case_ids = BTreeSet::new();
    let mut baseline_ids = BTreeSet::new();
    let mut prepared = Vec::with_capacity(request.cases.len());
    for input in request.cases.drain(..) {
        if recipe_cases.get(input.recipe.id.as_str()) != Some(&&input.recipe)
            || !case_ids.insert(input.recipe.id.clone())
            || !baseline_ids.insert(input.recipe.baseline_case.clone())
        {
            return Err(
                "compatibility resolve: selected case does not match one unique reviewed recipe"
                    .into(),
            );
        }
        if input.environment_owner != request.environment_owner
            || input.os != request.os
            || input.architecture != request.architecture
        {
            return Err("compatibility resolve: selected baseline owner/platform mismatch".into());
        }
        prepared.push(prepare_case_source(input).map_err(|_| {
            "compatibility resolve: baseline config does not match its closed template"
        })?);
    }

    let mut protected = BTreeMap::new();
    for file in request.protected_inputs.drain(..) {
        add_protected_file(&mut protected, file)?;
    }
    add_protected_file(
        &mut protected,
        ProtectedFileInput {
            canonical_path: request.recipes.canonical_path.clone(),
            sha256: request.recipes.sha256.clone(),
            max_bytes: MAX_RECIPE_BYTES,
        },
    )?;
    add_protected_file(
        &mut protected,
        ProtectedFileInput {
            canonical_path: PathBuf::from(&request.production_manifest.canonical_path),
            sha256: request.production_manifest.sha256.clone(),
            max_bytes: MAX_RECIPE_BYTES,
        },
    )?;
    add_protected_file(
        &mut protected,
        ProtectedFileInput {
            canonical_path: PathBuf::from(&request.candidate.canonical_path),
            sha256: request.candidate.sha256.clone(),
            max_bytes: request.candidate.byte_length,
        },
    )?;
    add_protected_file(
        &mut protected,
        ProtectedFileInput {
            canonical_path: PathBuf::from(&request.runtime_executable.canonical_path),
            sha256: request.runtime_executable.sha256.clone(),
            max_bytes: request.runtime_executable.byte_length,
        },
    )?;
    for case in &prepared {
        add_protected_file(
            &mut protected,
            ProtectedFileInput {
                canonical_path: case.input.baseline_config.canonical_path.clone(),
                sha256: case.input.baseline_config.sha256.clone(),
                max_bytes: MAX_RECIPE_BYTES,
            },
        )?;
        if let Some(settings) = &case.settings {
            add_protected_file(
                &mut protected,
                ProtectedFileInput {
                    canonical_path: settings.canonical_path.clone(),
                    sha256: settings.sha256.clone(),
                    max_bytes: MAX_SETTINGS_BYTES,
                },
            )?;
        }
    }
    if !protected.contains_key(&request.npm_executable)
        || !protected.contains_key(&request.base_resolver_executable)
        || (request.runtime == RuntimeKind::Docker
            && request.base_resolver_executable.as_path()
                != Path::new(&request.runtime_executable.canonical_path))
    {
        return Err(
            "compatibility resolve: resolver tool identities are not fully protected".into(),
        );
    }
    if protected.len() > MAX_PROTECTED_INPUTS {
        return Err("compatibility resolve: protected input bound exceeded".into());
    }
    for path in protected.keys() {
        artifact_path("protected input", path)
            .map_err(|error| format!("compatibility resolve: {error}"))?;
    }
    verify_protected_files(&protected)?;
    Ok((prepared, protected))
}

fn setup_resolution_artifact(
    request: &ProviderFreeResolutionRequest,
    resolution_id: &str,
    bundle: &Path,
    protected: &BTreeMap<PathBuf, ProtectedFileInput>,
) -> Result<ResolutionArtifact, BoxError> {
    let (protected_inputs, changed) = protected_evidence(protected, false);
    if changed {
        return Err("compatibility resolve: protected input changed during setup".into());
    }
    Ok(ResolutionArtifact {
        schema_version: 1,
        state: ResolutionState::SetupIncomplete,
        resolution_id: resolution_id.to_owned(),
        recipes: VersionedArtifactIdentity {
            schema_version: 1,
            canonical_path: request.recipes.canonical_path_text.clone(),
            sha256: request.recipes.sha256.clone(),
        },
        production_manifest: request.production_manifest.clone(),
        candidate: request.candidate.clone(),
        environment: ResolutionEnvironment {
            environment_owner: request.environment_owner.clone(),
            os: request.os.clone(),
            architecture: request.architecture.clone(),
            runtime: request.runtime,
            runtime_executable: request.runtime_executable.clone(),
        },
        limits: request.recipes.recipes.limits.clone(),
        execution_manifest: None,
        packages: Vec::new(),
        images: Vec::new(),
        cases: Vec::new(),
        model_catalog: ModelCatalogResolution {
            state: CatalogResolutionState::DeferredToAuthorizedSmoke,
        },
        protected_inputs,
        failure: None,
        owned_resources: vec![OwnedResource {
            kind: OwnedResourceKind::Bundle,
            identity: artifact_path("resolution bundle", bundle)?,
        }],
    })
}

async fn materialize_resolution_body(
    request: &ProviderFreeResolutionRequest,
    prepared: &[PreparedCase],
    publisher: &BundlePublisher,
    executor: &dyn ResolutionExecutor,
    artifact: &mut ResolutionArtifact,
) -> Result<(), ResolutionFailureCode> {
    let selected_packages: BTreeSet<_> = prepared
        .iter()
        .map(|case| case.input.recipe.package_set.as_str())
        .collect();
    let selected_images: BTreeSet<_> = prepared
        .iter()
        .filter_map(|case| case.input.recipe.image.as_deref())
        .collect();
    let packages_directory = publisher
        .create_directory(OsStr::new("packages"), "compatibility packages")
        .map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?;
    let tooling = ResolverTooling {
        npm_executable: request.npm_executable.clone(),
        runtime_executable: PathBuf::from(&request.runtime_executable.canonical_path),
        safe_path: request.safe_path.clone(),
    };
    for package in &request.recipes.recipes.package_sets {
        if selected_packages.contains(package.id.as_str()) {
            let resolved = materialize_package_set(
                &packages_directory,
                package,
                &request.recipes.recipes.limits,
                &tooling,
                executor,
            )
            .await?;
            artifact.packages.push(resolved);
        }
    }
    if artifact.packages.len() != selected_packages.len() {
        return Err(ResolutionFailureCode::PackageIdentityMismatch);
    }

    if !selected_images.is_empty() {
        publisher
            .create_directory(OsStr::new("runtime-home"), "compatibility runtime home")
            .map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?;
        publisher
            .create_directory(
                OsStr::new("runtime-config"),
                "compatibility isolated runtime config",
            )
            .map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?;
        artifact.owned_resources.push(OwnedResource {
            kind: OwnedResourceKind::RuntimeCache,
            identity: format!("{}:engine-cache", request.runtime.wire()),
        });
        let images_directory = publisher
            .create_directory(OsStr::new("images"), "compatibility image evidence")
            .map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?;
        for image in &request.recipes.recipes.images {
            if !selected_images.contains(image.id.as_str()) {
                continue;
            }
            let image_packages: Vec<_> = image
                .package_sets
                .iter()
                .filter(|id| selected_packages.contains(id.as_str()))
                .map(|id| {
                    artifact
                        .packages
                        .iter()
                        .find(|package| &package.id == id)
                        .cloned()
                        .ok_or(ResolutionFailureCode::PackageIdentityMismatch)
                })
                .collect::<Result<_, _>>()?;
            let resolved = materialize_image(
                ImageMaterialization {
                    recipe: image,
                    packages: &image_packages,
                    resolution_id: &artifact.resolution_id,
                    recipe_sha256: &artifact.recipes.sha256,
                    runtime: request.runtime,
                    runtime_executable: &tooling.runtime_executable,
                    base_resolver_executable: &request.base_resolver_executable,
                    safe_path: tooling.safe_path.clone(),
                    bundle: publisher,
                    image_directory: &images_directory,
                    architecture: &request.architecture,
                    timeout: Duration::from_secs(request.recipes.recipes.limits.timeout_secs),
                },
                executor,
                &mut artifact.owned_resources,
            )
            .await?;
            artifact.images.push(resolved);
        }
        if artifact.images.len() != selected_images.len() {
            return Err(ResolutionFailureCode::ImageLabelMismatch);
        }
    }

    let configs_directory = publisher
        .create_directory(OsStr::new("configs"), "compatibility generated configs")
        .map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?;
    let mut prerequisites_directory = None;
    let mut materialized_settings: Option<(String, PathBuf)> = None;
    let mut execution_cases = Vec::with_capacity(prepared.len());
    for case in prepared {
        let package = artifact
            .packages
            .iter()
            .find(|package| package.id == case.input.recipe.package_set)
            .ok_or(ResolutionFailureCode::PackageIdentityMismatch)?;
        let image = case
            .input
            .recipe
            .image
            .as_deref()
            .map(|image_id| {
                artifact
                    .images
                    .iter()
                    .find(|image| image.id == image_id)
                    .ok_or(ResolutionFailureCode::ImageLabelMismatch)
            })
            .transpose()?;
        let settings_path = if let Some(settings) = &case.settings {
            match &materialized_settings {
                Some((sha256, path)) if sha256 == &settings.sha256 => Some(path.clone()),
                Some(_) => return Err(ResolutionFailureCode::ConfigTemplateMismatch),
                None => {
                    if prerequisites_directory.is_none() {
                        prerequisites_directory = Some(
                            publisher
                                .create_directory(
                                    OsStr::new("prerequisites"),
                                    "compatibility non-secret prerequisites",
                                )
                                .map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?,
                        );
                    }
                    let directory = prerequisites_directory
                        .as_ref()
                        .expect("prerequisites directory was initialized");
                    create_synced_file(
                        directory,
                        OsStr::new("fable-settings.json"),
                        0o600,
                        &settings.bytes,
                        "compatibility non-secret Fable settings",
                    )
                    .map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?;
                    let path = directory.canonical_path().join("fable-settings.json");
                    materialized_settings = Some((settings.sha256.clone(), path.clone()));
                    Some(path)
                }
            }
        } else {
            None
        };
        let config_bytes = render_generated_config(
            case,
            package,
            image,
            &publisher.canonical_path,
            &tooling.runtime_executable,
            settings_path.as_deref(),
        )
        .map_err(|_| ResolutionFailureCode::ConfigTemplateMismatch)?;
        let config_name = format!("{}.toml", case.input.recipe.id);
        create_synced_file(
            &configs_directory,
            OsStr::new(&config_name),
            0o600,
            &config_bytes,
            "compatibility generated config",
        )
        .map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?;
        let config_snapshot = local_file::read_regular_file_bounded(
            &configs_directory.canonical_path().join(&config_name),
            "compatibility generated config",
            MAX_RECIPE_BYTES,
        )
        .map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?;
        let binding = ResolvedBinding {
            resolution_id: artifact.resolution_id.clone(),
            recipe_sha256: artifact.recipes.sha256.clone(),
            config_sha256: config_snapshot.sha256.clone(),
            adapter: format!("{}={}", package.adapter.name, package.adapter.version),
            agent_cli: format!("{}={}", package.agent_cli.name, package.agent_cli.version),
            package_inventory_sha256: package.inventory_sha256.clone(),
            package_tree_sha256: package.tree_sha256.clone(),
            image_digest: image.map(|image| image.final_image_id.clone()),
            base_image_digest: image.map(|image| image.platform_manifest_digest.clone()),
        };
        let mut prerequisites: Vec<_> = case
            .input
            .required_env
            .iter()
            .map(|required| NonSecretPrerequisite {
                name: required.name.clone(),
                destination: None,
            })
            .collect();
        if settings_path.is_some() {
            prerequisites.push(NonSecretPrerequisite {
                name: "fable-settings".into(),
                destination: Some("/root/.claude/settings.json".into()),
            });
        }
        artifact.cases.push(ResolvedCase {
            id: case.input.recipe.id.clone(),
            baseline_case: case.input.recipe.baseline_case.clone(),
            package_set: case.input.recipe.package_set.clone(),
            image: case.input.recipe.image.clone(),
            model: case.input.model.clone(),
            effort: case.input.effort.clone(),
            mode: case.input.mode.clone(),
            prerequisites,
            generated_config: ArtifactIdentity {
                canonical_path: artifact_path("generated config", &config_snapshot.canonical_path)
                    .map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?,
                sha256: config_snapshot.sha256,
            },
            binding: binding.clone(),
        });
        execution_cases.push(GeneratedExecutionCase {
            id: case.input.recipe.id.clone(),
            lane: "floating-current",
            evidence_path: case.input.evidence_path.clone(),
            execution_mode: case.input.execution_mode.clone(),
            os: case.input.os.clone(),
            architecture: case.input.architecture.clone(),
            environment_owner: case.input.environment_owner.clone(),
            expected_image_digest: image.map(|image| image.final_image_id.clone()),
            config: PathBuf::from("configs").join(config_name),
            agent: case.input.agent.clone(),
            model: case.input.model.clone(),
            effort: case.input.effort.clone(),
            mode: case.input.mode.clone(),
            session_cwd: case.input.session_cwd.clone(),
            auth_path: case.input.auth_path.clone(),
            credential_env: case.input.credential_env.clone(),
            required_env: case.input.required_env.clone(),
            probe: case.input.probe.clone(),
            billable: case.input.billable,
            timeout_secs: case.input.timeout_secs,
            max_tokens: case.input.max_tokens,
            max_cost_usd: case.input.max_cost_usd,
            retry_cap: case.input.retry_cap,
            expected_status: case.input.expected_status.clone(),
            classification: "canary",
            baseline_case: case.input.recipe.baseline_case.clone(),
            artifact: GeneratedArtifactPolicy {
                retention_days: case.input.artifact.retention_days,
                redaction: "strict",
            },
            resolved: binding,
        });
    }
    let manifest_bytes = generated_execution_manifest_bytes(&request.budget, execution_cases)?;
    create_synced_file(
        &publisher.pin,
        OsStr::new("execution-manifest.toml"),
        0o600,
        &manifest_bytes,
        "compatibility generated execution manifest",
    )
    .map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?;
    let manifest_snapshot = local_file::read_regular_file_bounded(
        &publisher.canonical_path.join("execution-manifest.toml"),
        "compatibility generated execution manifest",
        MAX_RECIPE_BYTES,
    )
    .map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?;
    artifact.execution_manifest = Some(VersionedArtifactIdentity {
        schema_version: 1,
        canonical_path: artifact_path(
            "generated execution manifest",
            &manifest_snapshot.canonical_path,
        )
        .map_err(|_| ResolutionFailureCode::PublicationResourceFailed)?,
        sha256: manifest_snapshot.sha256,
    });
    Ok(())
}

async fn resolve_with_executor(
    mut request: ProviderFreeResolutionRequest,
    executor: &dyn ResolutionExecutor,
) -> Result<ResolutionArtifact, BoxError> {
    let (prepared, protected) = prepare_resolution_request(&mut request)?;
    let resolution_id = format!("r3c-{}", crate::implement::nonce(24));
    stable_id("generated resolution id", &resolution_id)
        .map_err(|error| format!("compatibility resolve: {error}"))?;
    let publisher = BundlePublisher::create_with_setup(&request.output, |bundle| {
        setup_resolution_artifact(&request, &resolution_id, bundle, &protected)
    })?;
    let mut artifact = publisher.setup_artifact.clone();
    let body_result =
        materialize_resolution_body(&request, &prepared, &publisher, executor, &mut artifact).await;
    let (protected_inputs, protected_changed) = protected_evidence(&protected, true);
    artifact.protected_inputs = protected_inputs;
    let failure = if protected_changed {
        Some(ResolutionFailureCode::ProtectedStateChanged)
    } else {
        body_result.err()
    };
    match failure {
        Some(code) => {
            artifact.state = ResolutionState::Failed;
            artifact.failure = Some(ResolutionFailure { code });
            publisher.publish_terminal(&artifact)?;
            Err(format!(
                "compatibility resolve: provider-free resolution failed with {code:?}; inspect {}/resolution.json",
                publisher.canonical_path.display()
            )
            .into())
        }
        None => {
            artifact.state = ResolutionState::Complete;
            validate_resolution(&artifact).map_err(|error| {
                format!("compatibility resolve: completed artifact invalid: {error}")
            })?;
            publisher.publish_terminal(&artifact)?;
            Ok(artifact)
        }
    }
}

pub(super) async fn resolve_provider_free(
    request: ProviderFreeResolutionRequest,
) -> Result<ResolutionArtifact, BoxError> {
    resolve_with_executor(request, &ProcessResolutionExecutor).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_recipes() -> String {
        r#"schema_version = 1
production_manifest = "manifest.toml"

[limits]
timeout_secs = 900
max_download_bytes = 536870912
max_unpacked_bytes = 1073741824
max_files = 100000

[artifact]
retention_days = 30
redaction = "strict"

[[package_sets]]
id = "codex-current"
ecosystem = "npm"
registry = "npmjs"
adapter = "@agentclientprotocol/codex-acp"
adapter_selector = "latest"
agent_cli = "@openai/codex"

[[images]]
id = "reader-current"
template = "node-acp-reader-v1"
base = "docker.io/library/node:24-slim"
package_sets = ["codex-current"]

[[cases]]
id = "codex-host-floating-current"
baseline_case = "codex-host-bridge-gpt56-sol"
package_set = "codex-current"
target = "host-package-tree"
config_template = "codex-host-read-only-v1"

[[cases]]
id = "codex-reader-floating-current"
baseline_case = "codex-reader-bridge-gpt56-sol"
package_set = "codex-current"
target = "container-ro-image"
config_template = "codex-reader-read-only-v1"
image = "reader-current"
"#
        .into()
    }

    fn parse_recipes(raw: &str) -> Result<FloatingRecipeManifest, String> {
        secret_free_raw("floating recipes", raw)?;
        let recipes: FloatingRecipeManifest =
            toml::from_str(raw).map_err(|error| error.to_string())?;
        validate_recipes(&recipes)?;
        Ok(recipes)
    }

    #[test]
    fn recipe_contract_accepts_only_closed_package_image_and_config_templates() {
        parse_recipes(&valid_recipes()).unwrap();

        for (from, to) in [
            ("registry = \"npmjs\"", "registry = \"attacker\""),
            ("adapter_selector = \"latest\"", "adapter_selector = \"^1\""),
            (
                "base = \"docker.io/library/node:24-slim\"",
                "base = \"attacker.example/node:latest\"",
            ),
            (
                "config_template = \"codex-host-read-only-v1\"",
                "config_template = \"codex-reader-read-only-v1\"",
            ),
        ] {
            let invalid = valid_recipes().replacen(from, to, 1);
            assert!(
                parse_recipes(&invalid).is_err(),
                "mutation {from:?} -> {to:?} must fail closed"
            );
        }
    }

    #[test]
    fn recipe_contract_rejects_unknown_fields_duplicate_baselines_and_path_escape() {
        let unknown = valid_recipes().replace(
            "adapter_selector = \"latest\"",
            "adapter_selector = \"latest\"\ncommand = \"sh -c anything\"",
        );
        assert!(parse_recipes(&unknown).is_err());

        let duplicate = valid_recipes().replace(
            "baseline_case = \"codex-reader-bridge-gpt56-sol\"",
            "baseline_case = \"codex-host-bridge-gpt56-sol\"",
        );
        assert!(parse_recipes(&duplicate)
            .unwrap_err()
            .contains("duplicate baseline"));

        let escaped = valid_recipes().replace(
            "production_manifest = \"manifest.toml\"",
            "production_manifest = \"../manifest.toml\"",
        );
        assert!(parse_recipes(&escaped)
            .unwrap_err()
            .contains("must not contain"));
    }

    #[test]
    fn recipe_contract_requires_bounded_strict_artifact_policy_and_secret_free_text() {
        let missing_policy = valid_recipes().replace(
            "[artifact]\nretention_days = 30\nredaction = \"strict\"\n\n",
            "",
        );
        assert!(parse_recipes(&missing_policy).is_err());

        let excessive_retention =
            valid_recipes().replace("retention_days = 30", "retention_days = 91");
        assert!(parse_recipes(&excessive_retention)
            .unwrap_err()
            .contains("retention_days"));

        let weak_redaction =
            valid_recipes().replace("redaction = \"strict\"", "redaction = \"weak\"");
        assert!(parse_recipes(&weak_redaction).is_err());

        for secret in [
            "# AKIA1234567890ABCDEF\n",
            "# eyJheader.payload.signature\n",
        ] {
            let raw = format!("{secret}{}", valid_recipes());
            assert!(
                parse_recipes(&raw).is_err(),
                "secret-shaped recipe text must fail closed"
            );
        }
    }

    fn binding(container: bool) -> ResolvedBinding {
        ResolvedBinding {
            resolution_id: "resolution-1".into(),
            recipe_sha256: "a".repeat(64),
            config_sha256: "b".repeat(64),
            adapter: "@agentclientprotocol/codex-acp=1.2.3".into(),
            agent_cli: "@openai/codex=0.150.0".into(),
            package_inventory_sha256: "c".repeat(64),
            package_tree_sha256: "d".repeat(64),
            image_digest: container.then(|| format!("sha256:{}", "e".repeat(64))),
            base_image_digest: container.then(|| format!("sha256:{}", "f".repeat(64))),
        }
    }

    #[test]
    fn resolved_binding_separates_exact_candidate_evidence_from_floating_requests() {
        validate_resolved_binding(&binding(false), false, None).unwrap();
        let container = binding(true);
        validate_resolved_binding(&container, true, container.image_digest.as_deref()).unwrap();

        let mut floating = binding(false);
        floating.adapter = "@agentclientprotocol/codex-acp=latest".into();
        assert!(validate_resolved_binding(&floating, false, None)
            .unwrap_err()
            .contains("semantic version"));

        let mut missing_base = binding(true);
        missing_base.base_image_digest = None;
        assert!(validate_resolved_binding(
            &missing_base,
            true,
            missing_base.image_digest.as_deref(),
        )
        .is_err());

        let mut host_with_image = binding(false);
        host_with_image.image_digest = Some(format!("sha256:{}", "e".repeat(64)));
        assert!(validate_resolved_binding(&host_with_image, false, None).is_err());
    }

    fn exact_package(name: &str, version: &str) -> ExactNpmPackage {
        ExactNpmPackage {
            name: name.into(),
            version: version.into(),
            integrity: format!("sha512-{}==", "A".repeat(86)),
        }
    }

    fn identity(path: &str, byte: char) -> ArtifactIdentity {
        ArtifactIdentity {
            canonical_path: path.into(),
            sha256: byte.to_string().repeat(64),
        }
    }

    fn versioned_identity(path: &str, byte: char) -> VersionedArtifactIdentity {
        VersionedArtifactIdentity {
            schema_version: 1,
            canonical_path: path.into(),
            sha256: byte.to_string().repeat(64),
        }
    }

    fn executable(path: &str, byte: char) -> ExecutableIdentity {
        ExecutableIdentity {
            canonical_path: path.into(),
            sha256: byte.to_string().repeat(64),
            byte_length: 42,
        }
    }

    fn valid_resolution() -> ResolutionArtifact {
        let binding = ResolvedBinding {
            resolution_id: "resolution-1".into(),
            recipe_sha256: "a".repeat(64),
            config_sha256: "f".repeat(64),
            adapter: "@agentclientprotocol/codex-acp=1.2.3".into(),
            agent_cli: "@openai/codex=0.150.0".into(),
            package_inventory_sha256: "4".repeat(64),
            package_tree_sha256: "5".repeat(64),
            image_digest: None,
            base_image_digest: None,
        };
        ResolutionArtifact {
            schema_version: 1,
            state: ResolutionState::Complete,
            resolution_id: "resolution-1".into(),
            recipes: versioned_identity("/tmp/bundle/floating-current.toml", 'a'),
            production_manifest: versioned_identity("/tmp/repo/compatibility/manifest.toml", 'b'),
            candidate: executable("/tmp/a2a-bridge", 'c'),
            environment: ResolutionEnvironment {
                environment_owner: "test-runner".into(),
                os: "macos".into(),
                architecture: "aarch64".into(),
                runtime: RuntimeKind::Docker,
                runtime_executable: executable("/usr/local/bin/docker", 'd'),
            },
            limits: ResolutionLimits {
                timeout_secs: 900,
                max_download_bytes: 536_870_912,
                max_unpacked_bytes: 1_073_741_824,
                max_files: 100_000,
            },
            execution_manifest: Some(versioned_identity(
                "/tmp/bundle/execution-manifest.toml",
                'e',
            )),
            packages: vec![ResolvedPackageSet {
                id: "codex-current".into(),
                requested: RequestedPackageSet {
                    adapter: "@agentclientprotocol/codex-acp".into(),
                    adapter_selector: "latest".into(),
                    agent_cli: "@openai/codex".into(),
                },
                adapter: exact_package("@agentclientprotocol/codex-acp", "1.2.3"),
                agent_cli: exact_package("@openai/codex", "0.150.0"),
                bundled_cli_version: None,
                resolution_lock_sha256: "3".repeat(64),
                inventory_sha256: "4".repeat(64),
                tree_sha256: "5".repeat(64),
                adapter_executable: identity(
                    "/tmp/bundle/packages/codex-current/tree/node_modules/@agentclientprotocol/codex-acp/dist/index.js",
                    '7',
                ),
                adapter_executable_relative:
                    "node_modules/@agentclientprotocol/codex-acp/dist/index.js".into(),
            }],
            images: Vec::new(),
            cases: vec![ResolvedCase {
                id: "codex-host-floating-current".into(),
                baseline_case: "codex-host-bridge-gpt56-sol".into(),
                package_set: "codex-current".into(),
                image: None,
                model: "gpt-5.6-sol".into(),
                effort: Some("xhigh".into()),
                mode: Some("read-only".into()),
                prerequisites: vec![NonSecretPrerequisite {
                    name: "host-authentication".into(),
                    destination: None,
                }],
                generated_config: identity("/tmp/bundle/configs/codex-host.toml", 'f'),
                binding,
            }],
            model_catalog: ModelCatalogResolution {
                state: CatalogResolutionState::DeferredToAuthorizedSmoke,
            },
            protected_inputs: vec![ProtectedInput {
                path: "/tmp/repo/Cargo.lock".into(),
                before_sha256: "6".repeat(64),
                after_sha256: "6".repeat(64),
            }],
            failure: None,
            owned_resources: vec![OwnedResource {
                kind: OwnedResourceKind::Bundle,
                identity: "/tmp/bundle".into(),
            }],
        }
    }

    #[test]
    fn completed_resolution_binds_every_case_to_exact_package_and_protected_state() {
        validate_resolution(&valid_resolution()).unwrap();

        let mut config_drift = valid_resolution();
        config_drift.cases[0].binding.config_sha256 = "1".repeat(64);
        assert!(validate_resolution(&config_drift)
            .unwrap_err()
            .contains("binding does not match"));

        let mut protected_drift = valid_resolution();
        protected_drift.protected_inputs[0].after_sha256 = "7".repeat(64);
        assert_eq!(
            validate_resolution(&protected_drift).unwrap_err(),
            "compatibility resolution: protected_state_changed"
        );

        let mut malformed_integrity = valid_resolution();
        malformed_integrity.packages[0].adapter.integrity = "sha512-é".into();
        assert!(validate_resolution(&malformed_integrity)
            .unwrap_err()
            .contains("canonical sha512"));

        let mut duplicate_proof = valid_resolution();
        duplicate_proof
            .protected_inputs
            .push(duplicate_proof.protected_inputs[0].clone());
        assert!(validate_resolution(&duplicate_proof)
            .unwrap_err()
            .contains("duplicate protected input"));

        let mut duplicate_resource = valid_resolution();
        duplicate_resource
            .owned_resources
            .push(duplicate_resource.owned_resources[0].clone());
        assert!(validate_resolution(&duplicate_resource)
            .unwrap_err()
            .contains("duplicate owned resource"));

        let mut duplicate_prerequisite = valid_resolution();
        let prerequisite = duplicate_prerequisite.cases[0].prerequisites[0].clone();
        duplicate_prerequisite.cases[0]
            .prerequisites
            .push(prerequisite);
        assert!(validate_resolution(&duplicate_prerequisite)
            .unwrap_err()
            .contains("repeats prerequisite"));

        let mut secret_model = valid_resolution();
        secret_model.cases[0].model = "sk-secret-model".into();
        assert!(validate_resolution(&secret_model)
            .unwrap_err()
            .contains("secret-shaped"));
    }

    #[test]
    fn completed_failed_and_floating_resolved_states_cannot_be_confused() {
        let mut failed_complete = valid_resolution();
        failed_complete.failure = Some(ResolutionFailure {
            code: ResolutionFailureCode::NpmNonzero,
        });
        assert!(validate_resolution(&failed_complete)
            .unwrap_err()
            .contains("complete evidence"));

        let mut missing_failure = valid_resolution();
        missing_failure.state = ResolutionState::Failed;
        missing_failure.execution_manifest = None;
        assert!(validate_resolution(&missing_failure)
            .unwrap_err()
            .contains("requires a typed failure"));

        let mut ranged = valid_resolution();
        ranged.packages[0].adapter.version = "latest".into();
        assert!(validate_resolution(&ranged)
            .unwrap_err()
            .contains("semantic version"));

        let mut wrong_recipe_schema = valid_resolution();
        wrong_recipe_schema.recipes.schema_version = 2;
        assert!(validate_resolution(&wrong_recipe_schema)
            .unwrap_err()
            .contains("schema_version"));

        let mut unbounded = valid_resolution();
        unbounded.limits.timeout_secs = 0;
        assert!(validate_resolution(&unbounded)
            .unwrap_err()
            .contains("timeout_secs"));

        let mut unused_package = valid_resolution();
        let mut extra = unused_package.packages[0].clone();
        extra.id = "unused-package".into();
        unused_package.packages.push(extra);
        assert!(validate_resolution(&unused_package)
            .unwrap_err()
            .contains("unreferenced"));

        let mut setup_with_failure = valid_resolution();
        setup_with_failure.state = ResolutionState::SetupIncomplete;
        setup_with_failure.failure = Some(ResolutionFailure {
            code: ResolutionFailureCode::NpmNonzero,
        });
        assert!(validate_resolution(&setup_with_failure)
            .unwrap_err()
            .contains("must not declare a failure"));
    }

    #[test]
    fn resolution_json_rejects_unknown_failure_and_resource_kinds() {
        let valid = serde_json::to_value(valid_resolution()).unwrap();

        let mut unknown_failure = valid.clone();
        unknown_failure["state"] = serde_json::json!("failed");
        unknown_failure["failure"] = serde_json::json!({"code": "dynamic_subprocess_text"});
        assert!(serde_json::from_value::<ResolutionArtifact>(unknown_failure).is_err());

        let mut unknown_resource = valid;
        unknown_resource["owned_resources"][0]["kind"] = serde_json::json!("shared_operator");
        assert!(serde_json::from_value::<ResolutionArtifact>(unknown_resource).is_err());
    }

    fn setup_resolution(bundle_path: &Path) -> ResolutionArtifact {
        let mut artifact = valid_resolution();
        artifact.state = ResolutionState::SetupIncomplete;
        artifact.execution_manifest = None;
        artifact.packages.clear();
        artifact.images.clear();
        artifact.cases.clear();
        artifact.failure = None;
        artifact.owned_resources = vec![OwnedResource {
            kind: OwnedResourceKind::Bundle,
            identity: bundle_path.to_string_lossy().into_owned(),
        }];
        artifact
    }

    #[cfg(unix)]
    #[test]
    fn bundle_publisher_keeps_setup_blocking_until_exact_terminal_replacement() {
        use std::os::unix::fs::PermissionsExt as _;

        let parent = tempfile::tempdir().unwrap();
        let output = parent.path().join("bundle");
        let publisher =
            BundlePublisher::create_with_setup(&output, |path| Ok(setup_resolution(path))).unwrap();
        assert_eq!(
            fs::metadata(&output).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(output.join("resolution.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            load_resolution(&output.join("resolution.json"))
                .unwrap()
                .artifact
                .state,
            ResolutionState::SetupIncomplete
        );

        let mut failed = setup_resolution(&output);
        failed.state = ResolutionState::Failed;
        failed.failure = Some(ResolutionFailure {
            code: ResolutionFailureCode::NpmNonzero,
        });
        publisher.publish_terminal(&failed).unwrap();
        let loaded = load_resolution(&output.join("resolution.json")).unwrap();
        assert_eq!(loaded.artifact.state, ResolutionState::Failed);
        assert_eq!(
            loaded.artifact.failure.unwrap().code,
            ResolutionFailureCode::NpmNonzero
        );
    }

    #[test]
    fn bundle_publisher_rejects_existing_repo_and_invalid_setup_without_residue() {
        let parent = tempfile::tempdir().unwrap();
        let existing = parent.path().join("existing");
        fs::create_dir(&existing).unwrap();
        assert!(BundlePublisher::create_with_setup(&existing, |path| {
            Ok(setup_resolution(path))
        })
        .is_err());

        let repo = parent.path().join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        assert!(
            BundlePublisher::create_with_setup(&repo.join("bundle"), |path| {
                Ok(setup_resolution(path))
            })
            .is_err()
        );

        let invalid = parent.path().join("invalid");
        assert!(BundlePublisher::create_with_setup(&invalid, |path| {
            let mut artifact = setup_resolution(path);
            artifact.state = ResolutionState::Complete;
            Ok(artifact)
        })
        .is_err());
        assert!(!invalid.exists());
    }

    #[test]
    fn bundle_terminal_publication_rejects_same_name_setup_replacement() {
        let parent = tempfile::tempdir().unwrap();
        let output = parent.path().join("bundle");
        let publisher =
            BundlePublisher::create_with_setup(&output, |path| Ok(setup_resolution(path))).unwrap();
        fs::remove_file(output.join("resolution.json")).unwrap();
        fs::write(output.join("resolution.json"), b"attacker replacement\n").unwrap();

        let mut failed = setup_resolution(&output);
        failed.state = ResolutionState::Failed;
        failed.failure = Some(ResolutionFailure {
            code: ResolutionFailureCode::NpmNonzero,
        });
        assert!(publisher
            .publish_terminal(&failed)
            .unwrap_err()
            .to_string()
            .contains("target identity changed"));
        assert_eq!(
            fs::read(output.join("resolution.json")).unwrap(),
            b"attacker replacement\n"
        );
    }

    fn canonical_integrity(byte: char) -> String {
        format!("sha512-{}==", byte.to_string().repeat(86))
    }

    fn valid_package_lock() -> Vec<u8> {
        format!(
            r#"{{
  "name": "a2a-r3c-resolution",
  "lockfileVersion": 3,
  "requires": true,
  "packages": {{
    "": {{
      "name": "a2a-r3c-resolution",
      "dependencies": {{
        "@agentclientprotocol/codex-acp": "latest"
      }}
    }},
    "node_modules/@agentclientprotocol/codex-acp": {{
      "version": "1.2.3",
      "resolved": "https://registry.npmjs.org/@agentclientprotocol/codex-acp/-/codex-acp-1.2.3.tgz",
      "integrity": {adapter_integrity:?},
      "dependencies": {{
        "@openai/codex": "^0.150.0"
      }}
    }},
    "node_modules/@openai/codex": {{
      "version": "0.150.1",
      "resolved": "https://registry.npmjs.org/@openai/codex/-/codex-0.150.1.tgz",
      "integrity": {cli_integrity:?}
    }}
  }}
}}"#,
            adapter_integrity = canonical_integrity('A'),
            cli_integrity = canonical_integrity('B'),
        )
        .into_bytes()
    }

    #[test]
    fn package_lock_requires_exact_registry_integrity_and_reviewed_pair() {
        let lock = parse_package_lock(
            &valid_package_lock(),
            "@agentclientprotocol/codex-acp",
            "@openai/codex",
            100,
        )
        .unwrap();
        assert_eq!(lock.adapter.version, "1.2.3");
        assert_eq!(lock.agent_cli.version, "0.150.1");
        assert_eq!(lock.sha256.len(), 64);

        for (from, to, expected) in [
            (
                "https://registry.npmjs.org/@openai/codex/",
                "https://attacker.example/@openai/codex/",
                "fixed npmjs registry",
            ),
            ("\"^0.150.0\"", "\"file:../escape\"", "forbidden external"),
            (
                "\"version\": \"1.2.3\",",
                "\"version\": \"1.2.3\", \"hasInstallScript\": true,",
                "lifecycle install script",
            ),
            (
                &canonical_integrity('B'),
                "sha512-not-base64",
                "canonical sha512",
            ),
        ] {
            let raw = String::from_utf8(valid_package_lock())
                .unwrap()
                .replacen(from, to, 1);
            let error = parse_package_lock(
                raw.as_bytes(),
                "@agentclientprotocol/codex-acp",
                "@openai/codex",
                100,
            )
            .unwrap_err();
            assert!(
                error.contains(expected),
                "mutation {from:?} -> {to:?}: {error}"
            );
        }
    }

    #[test]
    fn package_lock_rejects_secret_text_duplicates_and_package_count_overflow() {
        let secret = String::from_utf8(valid_package_lock()).unwrap().replace(
            "\"requires\": true",
            "\"note\": \"sk-secret-value\", \"requires\": true",
        );
        assert!(parse_package_lock(
            secret.as_bytes(),
            "@agentclientprotocol/codex-acp",
            "@openai/codex",
            100,
        )
        .unwrap_err()
        .contains("secret-shaped"));

        assert!(parse_package_lock(
            &valid_package_lock(),
            "@agentclientprotocol/codex-acp",
            "@openai/codex",
            2,
        )
        .unwrap_err()
        .contains("package count"));

        let duplicate = String::from_utf8(valid_package_lock())
            .unwrap()
            .replace(
                "\"node_modules/@openai/codex\": {",
                "\"node_modules/@agentclientprotocol/codex-acp/node_modules/@openai/codex\": {\n      \"version\": \"0.149.0\",\n      \"resolved\": \"https://registry.npmjs.org/@openai/codex/-/codex-0.149.0.tgz\",\n      \"integrity\": \"sha512-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==\"\n    },\n    \"node_modules/@openai/codex\": {",
            );
        assert!(parse_package_lock(
            duplicate.as_bytes(),
            "@agentclientprotocol/codex-acp",
            "@openai/codex",
            100,
        )
        .unwrap_err()
        .contains("multiple nested"));
    }

    #[cfg(unix)]
    struct PackageFakeExecutor {
        calls: std::sync::Mutex<Vec<ResolutionCommandKind>>,
        cache_payload_bytes: usize,
    }

    #[cfg(unix)]
    #[async_trait]
    impl ResolutionExecutor for PackageFakeExecutor {
        async fn execute(
            &self,
            command: &ResolutionCommandSpec,
        ) -> Result<Vec<u8>, ResolutionFailureCode> {
            self.calls.lock().unwrap().push(command.kind);
            match command.kind {
                ResolutionCommandKind::NpmLock => {
                    fs::write(command.cwd.join("package-lock.json"), valid_package_lock()).unwrap();
                    if self.cache_payload_bytes > 0 {
                        fs::write(
                            command.cwd.join("cache/download"),
                            vec![b'x'; self.cache_payload_bytes],
                        )
                        .unwrap();
                    }
                }
                ResolutionCommandKind::NpmMaterialize => {
                    use std::os::unix::fs::PermissionsExt as _;

                    let adapter = command
                        .cwd
                        .join("tree/node_modules/@agentclientprotocol/codex-acp");
                    let cli = command.cwd.join("tree/node_modules/@openai/codex");
                    fs::create_dir_all(adapter.join("dist")).unwrap();
                    fs::create_dir_all(&cli).unwrap();
                    let executable = adapter.join("dist/index.js");
                    fs::write(&executable, b"#!/usr/bin/env node\n").unwrap();
                    fs::set_permissions(&executable, fs::Permissions::from_mode(0o500)).unwrap();
                    fs::write(
                        adapter.join("package.json"),
                        br#"{"name":"@agentclientprotocol/codex-acp","version":"1.2.3","bin":{"codex-acp":"dist/index.js"}}"#,
                    )
                    .unwrap();
                    fs::write(
                        cli.join("package.json"),
                        br#"{"name":"@openai/codex","version":"0.150.1"}"#,
                    )
                    .unwrap();
                }
                _ => panic!("unexpected package fake command: {:?}", command.kind),
            }
            Ok(Vec::new())
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn package_materializer_uses_lock_then_ci_and_emits_sealed_exact_evidence() {
        use std::os::unix::fs::PermissionsExt as _;

        let parent = tempfile::tempdir().unwrap();
        let output = parent.path().join("bundle");
        let publisher =
            BundlePublisher::create_with_setup(&output, |path| Ok(setup_resolution(path))).unwrap();
        let packages = publisher
            .create_directory(OsStr::new("packages"), "compatibility packages")
            .unwrap();
        let recipe: FloatingRecipeManifest = toml::from_str(&valid_recipes()).unwrap();
        let executor = PackageFakeExecutor {
            calls: std::sync::Mutex::new(Vec::new()),
            cache_payload_bytes: 0,
        };
        let tooling = ResolverTooling {
            npm_executable: PathBuf::from("/unused/npm"),
            runtime_executable: PathBuf::from("/unused/docker"),
            safe_path: OsString::from("/usr/bin:/bin"),
        };
        let resolved = materialize_package_set(
            &packages,
            &recipe.package_sets[0],
            &recipe.limits,
            &tooling,
            &executor,
        )
        .await
        .unwrap();
        assert_eq!(
            &*executor.calls.lock().unwrap(),
            &[
                ResolutionCommandKind::NpmLock,
                ResolutionCommandKind::NpmMaterialize
            ]
        );
        assert_eq!(resolved.adapter.version, "1.2.3");
        assert_eq!(resolved.agent_cli.version, "0.150.1");
        assert!(resolved
            .adapter_executable_relative
            .ends_with("dist/index.js"));
        assert_eq!(resolved.inventory_sha256.len(), 64);
        assert_eq!(resolved.tree_sha256.len(), 64);
        assert!(!output.join("packages/codex-current/home").exists());
        assert!(!output.join("packages/codex-current/npmrc").exists());
        assert_eq!(
            fs::metadata(output.join("packages/codex-current/tree/package.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o400
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn package_materializer_stops_before_ci_when_download_budget_is_exceeded() {
        let parent = tempfile::tempdir().unwrap();
        let output = parent.path().join("bundle");
        let publisher =
            BundlePublisher::create_with_setup(&output, |path| Ok(setup_resolution(path))).unwrap();
        let packages = publisher
            .create_directory(OsStr::new("packages"), "compatibility packages")
            .unwrap();
        let recipe: FloatingRecipeManifest = toml::from_str(&valid_recipes()).unwrap();
        let executor = PackageFakeExecutor {
            calls: std::sync::Mutex::new(Vec::new()),
            cache_payload_bytes: 4,
        };
        let tooling = ResolverTooling {
            npm_executable: PathBuf::from("/unused/npm"),
            runtime_executable: PathBuf::from("/unused/docker"),
            safe_path: OsString::from("/usr/bin:/bin"),
        };
        let mut limits = recipe.limits.clone();
        limits.max_download_bytes = 3;

        assert_eq!(
            materialize_package_set(
                &packages,
                &recipe.package_sets[0],
                &limits,
                &tooling,
                &executor,
            )
            .await
            .unwrap_err(),
            ResolutionFailureCode::NpmDownloadBudgetExceeded
        );
        assert_eq!(
            &*executor.calls.lock().unwrap(),
            &[ResolutionCommandKind::NpmLock],
            "materialization must not begin after the download bound is exhausted"
        );
    }

    #[cfg(unix)]
    #[test]
    fn npm_download_budget_rejects_cache_symlinks() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let output = parent.path().join("bundle");
        let publisher =
            BundlePublisher::create_with_setup(&output, |path| Ok(setup_resolution(path))).unwrap();
        let cache = publisher
            .create_directory(OsStr::new("cache"), "compatibility npm cache")
            .unwrap();
        let outside = parent.path().join("outside");
        fs::write(&outside, b"secret-shaped external state").unwrap();
        symlink(&outside, output.join("cache/escape")).unwrap();

        assert_eq!(
            enforce_npm_download_budget(&cache, &tree_limits()).unwrap_err(),
            ResolutionFailureCode::WriteScopeEscape
        );
    }

    #[test]
    fn closed_templates_accept_all_supported_configs_without_reading_credentials() {
        struct Fixture<'a> {
            file: &'a str,
            template: ConfigTemplate,
            target: FloatingTarget,
            agent: &'a str,
            model: &'a str,
            effort: Option<&'a str>,
            mode: Option<&'a str>,
            auth_path: &'a str,
            package_set: &'a str,
        }

        let fixtures = [
            Fixture {
                file: "codex-host.toml",
                template: ConfigTemplate::CodexHostReadOnlyV1,
                target: FloatingTarget::HostPackageTree,
                agent: "codex-host",
                model: "gpt-5.6-sol",
                effort: Some("xhigh"),
                mode: Some("read-only"),
                auth_path: "pre_authenticated",
                package_set: "codex-current",
            },
            Fixture {
                file: "codex-reader.toml",
                template: ConfigTemplate::CodexReaderReadOnlyV1,
                target: FloatingTarget::ContainerRoImage,
                agent: "codex-reader",
                model: "gpt-5.6-sol",
                effort: Some("xhigh"),
                mode: None,
                auth_path: "pre_authenticated",
                package_set: "codex-current",
            },
            Fixture {
                file: "claude-host-044.toml",
                template: ConfigTemplate::ClaudeHostReadOnlyV1,
                target: FloatingTarget::HostPackageTree,
                agent: "claude-host",
                model: "claude-fable-5[1m]",
                effort: Some("xhigh"),
                mode: None,
                auth_path: "automatic",
                package_set: "claude-current",
            },
            Fixture {
                file: "claude-reader-055.toml",
                template: ConfigTemplate::ClaudeReaderReadOnlyV1,
                target: FloatingTarget::ContainerRoImage,
                agent: "claude-reader",
                model: "claude-fable-5[1m]",
                effort: Some("xhigh"),
                mode: None,
                auth_path: "pre_authenticated",
                package_set: "claude-current",
            },
        ];
        let parent = tempfile::tempdir().unwrap();
        let settings_path = parent.path().join("safe-fable-settings.json");
        let settings_bytes = br#"{"env":{"CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS":"1"}}"#;
        fs::write(&settings_path, settings_bytes).unwrap();
        let settings_sha256 = local_file::sha256_hex(settings_bytes);
        let configs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../compatibility/configs");

        for (index, fixture) in fixtures.into_iter().enumerate() {
            let source = fs::read_to_string(configs.join(fixture.file)).unwrap();
            let parsed = crate::config::RegistryConfig::parse(&source).unwrap();
            let mut raw = source;
            if let Some(sandbox) = parsed.agents[0].sandbox.as_ref() {
                for volume in &sandbox.volumes {
                    let declaration = bridge_core::sandbox::parse_sandbox_volume(volume).unwrap();
                    let bridge_core::sandbox::SandboxVolumeSource::Host(source) =
                        declaration.source()
                    else {
                        panic!("supported reader fixtures use host volume sources");
                    };
                    let replacement = if declaration.destination() == "/root/.claude/settings.json"
                    {
                        settings_path.to_string_lossy().into_owned()
                    } else {
                        format!("/definitely-missing-r3c-credential-{index}")
                    };
                    raw = raw.replace(source, &replacement);
                }
            }
            let config_path = parent.path().join(fixture.file);
            fs::write(&config_path, raw.as_bytes()).unwrap();
            let snapshot = local_file::read_regular_file_bounded(
                &config_path,
                "supported template fixture",
                MAX_RECIPE_BYTES,
            )
            .unwrap();
            let reader = template_is_reader(fixture.template);
            let prepared = prepare_case_source(ResolutionCaseInput {
                recipe: FloatingCaseRecipe {
                    id: format!("case-{index}"),
                    baseline_case: format!("baseline-{index}"),
                    package_set: fixture.package_set.into(),
                    target: fixture.target,
                    config_template: fixture.template,
                    image: reader.then(|| "reader-current".into()),
                },
                evidence_path: "bridge_smoke".into(),
                execution_mode: if reader { "container_ro" } else { "host" }.into(),
                os: std::env::consts::OS.into(),
                architecture: std::env::consts::ARCH.into(),
                environment_owner: "test-runner".into(),
                agent: fixture.agent.into(),
                model: fixture.model.into(),
                effort: fixture.effort.map(str::to_owned),
                mode: fixture.mode.map(str::to_owned),
                session_cwd: Some(parent.path().to_path_buf()),
                auth_path: fixture.auth_path.into(),
                credential_env: None,
                required_env: Vec::new(),
                probe: "minimal".into(),
                billable: true,
                timeout_secs: 30,
                max_tokens: 1_000,
                max_cost_usd: Some(1.0),
                retry_cap: 0,
                expected_status: "PASS".into(),
                artifact: ResolutionArtifactInput { retention_days: 1 },
                baseline_config: BaselineConfigInput {
                    canonical_path: snapshot.canonical_path,
                    sha256: snapshot.sha256,
                    bytes: snapshot.bytes,
                },
                component_pins: if fixture.template == ConfigTemplate::ClaudeReaderReadOnlyV1 {
                    BTreeMap::from([("fable-settings".into(), format!("sha256:{settings_sha256}"))])
                } else {
                    BTreeMap::new()
                },
            })
            .unwrap_or_else(|error| panic!("{}: {error}", fixture.file));
            assert_eq!(
                prepared.settings.is_some(),
                fixture.template == ConfigTemplate::ClaudeReaderReadOnlyV1
            );
            if fixture.template == ConfigTemplate::ClaudeReaderReadOnlyV1 {
                let mut package = valid_resolution().packages.remove(0);
                package.id = "claude-current".into();
                package.adapter_executable_relative =
                    "node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js".into();
                let image = ResolvedImage {
                    id: "reader-current".into(),
                    requested_base: NODE_READER_BASE.into(),
                    package_sets: vec!["claude-current".into()],
                    registry_index_digest: format!("sha256:{}", "a".repeat(64)),
                    platform_manifest_digest: format!("sha256:{}", "b".repeat(64)),
                    build_template_sha256: "c".repeat(64),
                    final_image_id: format!("sha256:{}", "d".repeat(64)),
                    owned_tag: "localhost/a2a-bridge-r3c:test-reader".into(),
                    labels: BTreeMap::new(),
                };
                let generated = render_generated_config(
                    &prepared,
                    &package,
                    Some(&image),
                    &parent.path().join("bundle"),
                    Path::new("/trusted/bin/docker"),
                    Some(&settings_path),
                )
                .unwrap();
                let generated = String::from_utf8(generated).unwrap();
                assert!(generated.contains("/definitely-missing-r3c-credential-3"));
                assert!(generated.contains(settings_path.to_str().unwrap()));
                assert!(generated.contains("/opt/a2a/packages/claude-current/"));
            }
        }
    }

    #[test]
    fn npm_commands_are_closed_direct_argv_with_an_isolated_environment() {
        let lock = npm_command(
            Path::new("/trusted/bin/npm"),
            OsString::from("/trusted/bin:/usr/bin"),
            PathBuf::from("/private/bundle/packages/codex"),
            Duration::from_secs(30),
            false,
        );
        assert_eq!(lock.kind, ResolutionCommandKind::NpmLock);
        assert_eq!(lock.program, Path::new("/trusted/bin/npm"));
        assert_eq!(lock.args[0], "install");
        assert!(lock.args.iter().any(|arg| arg == "--package-lock-only"));
        assert!(lock.args.iter().any(|arg| arg == "--ignore-scripts"));
        assert!(lock
            .args
            .iter()
            .any(|arg| arg == "--registry=https://registry.npmjs.org/"));
        assert!(lock.args.iter().all(|arg| {
            let arg = arg.to_string_lossy();
            !arg.contains(';') && !arg.contains("$()") && arg != "sh" && arg != "-c"
        }));
        let keys: BTreeSet<_> = lock
            .env
            .keys()
            .map(|key| key.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            keys,
            [
                "HOME",
                "PATH",
                "TMPDIR",
                "npm_config_audit",
                "npm_config_cache",
                "npm_config_fund",
                "npm_config_ignore_scripts",
                "npm_config_prefix",
                "npm_config_registry",
                "npm_config_userconfig",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect()
        );
        assert!(!keys.contains("OPENAI_API_KEY"));
        assert!(!keys.contains("ANTHROPIC_API_KEY"));
        assert_eq!(
            lock.env.get(OsStr::new("HOME")).unwrap(),
            "/private/bundle/packages/codex/home"
        );
        assert_eq!(
            lock.env.get(OsStr::new("npm_config_userconfig")).unwrap(),
            "/private/bundle/packages/codex/npmrc"
        );

        let materialize = npm_command(
            Path::new("/trusted/bin/npm"),
            OsString::from("/trusted/bin:/usr/bin"),
            PathBuf::from("/private/bundle/packages/codex"),
            Duration::from_secs(30),
            true,
        );
        assert_eq!(materialize.kind, ResolutionCommandKind::NpmMaterialize);
        assert_eq!(materialize.args[0], "ci");
        assert!(!materialize
            .args
            .iter()
            .any(|arg| arg == "--package-lock-only"));
    }

    fn base_manifest() -> Vec<u8> {
        format!(
            r#"{{"schemaVersion":2,"manifests":[
                {{"digest":"sha256:{amd}","platform":{{"os":"linux","architecture":"amd64"}}}},
                {{"digest":"sha256:{arm}","platform":{{"os":"linux","architecture":"arm64"}}}}
            ]}}"#,
            amd = "a".repeat(64),
            arm = "b".repeat(64),
        )
        .into_bytes()
    }

    #[test]
    fn base_manifest_resolution_binds_exact_index_and_platform_digests() {
        let resolved = parse_base_manifest(&base_manifest(), "linux", "arm64").unwrap();
        assert_eq!(
            resolved.platform_manifest_digest,
            format!("sha256:{}", "b".repeat(64))
        );
        assert_eq!(resolved.registry_index_digest.len(), 71);

        assert!(parse_base_manifest(&base_manifest(), "linux", "s390x")
            .unwrap_err()
            .contains("no exact"));
        let duplicate = String::from_utf8(base_manifest())
            .unwrap()
            .replace("\"amd64\"", "\"arm64\"");
        assert!(parse_base_manifest(duplicate.as_bytes(), "linux", "arm64")
            .unwrap_err()
            .contains("multiple matching"));
        let mutable = String::from_utf8(base_manifest())
            .unwrap()
            .replace(&format!("sha256:{}", "b".repeat(64)), "node:24-slim");
        assert!(parse_base_manifest(mutable.as_bytes(), "linux", "arm64")
            .unwrap_err()
            .contains("immutable"));
    }

    #[test]
    fn image_template_and_runtime_commands_are_closed_and_resolution_unique() {
        let digest = format!("sha256:{}", "c".repeat(64));
        let template =
            render_containerfile(&digest, &["codex-current".into(), "claude-current".into()])
                .unwrap();
        let template = String::from_utf8(template).unwrap();
        assert!(template.contains(&format!("FROM docker.io/library/node@{digest}")));
        assert!(!template.contains("node:24-slim"));
        assert!(!template.contains("RUN "));
        assert!(template.contains("COPY packages/codex-current/tree/"));

        let tag = owned_image_tag("resolution-123", "reader-current").unwrap();
        assert!(tag.contains("resolution-123-reader-current"));
        let packages = valid_resolution().packages;
        let labels = image_labels(
            "resolution-123",
            &"a".repeat(64),
            "reader-current",
            &packages,
        );
        let docker = image_build_command(ImageBuildCommand {
            runtime: RuntimeKind::Docker,
            executable: Path::new("/trusted/bin/docker"),
            safe_path: OsString::from("/trusted/bin:/usr/bin"),
            cwd: PathBuf::from("/private/bundle"),
            timeout: Duration::from_secs(30),
            platform: "linux/arm64",
            containerfile: "images/reader-current.Containerfile",
            tag: &tag,
            labels: &labels,
        });
        assert_eq!(docker.kind, ResolutionCommandKind::BuildImage);
        assert!(docker.args.iter().any(|arg| arg == "--pull=false"));
        assert!(docker.args.iter().any(|arg| arg == "--network=none"));
        assert!(docker.args.iter().all(|arg| arg != "sh" && arg != "-c"));
        assert!(!docker.env.contains_key(OsStr::new("OPENAI_API_KEY")));
        assert!(!docker.env.contains_key(OsStr::new("ANTHROPIC_API_KEY")));
        assert_eq!(
            docker.env.get(OsStr::new("DOCKER_CONFIG")).unwrap(),
            "/private/bundle/runtime-config"
        );

        let podman = image_build_command(ImageBuildCommand {
            runtime: RuntimeKind::Podman,
            executable: Path::new("/trusted/bin/podman"),
            safe_path: OsString::from("/trusted/bin:/usr/bin"),
            cwd: PathBuf::from("/private/bundle"),
            timeout: Duration::from_secs(30),
            platform: "linux/arm64",
            containerfile: "images/reader-current.Containerfile",
            tag: &tag,
            labels: &labels,
        });
        assert!(podman.args.iter().any(|arg| arg == "--pull=never"));

        let resolve = resolve_base_command(
            RuntimeKind::Docker,
            Path::new("/trusted/bin/docker"),
            OsString::from("/trusted/bin:/usr/bin"),
            PathBuf::from("/private/bundle"),
            Duration::from_secs(30),
        );
        assert_eq!(resolve.kind, ResolutionCommandKind::ResolveBase);
        assert_eq!(resolve.args.last().unwrap(), NODE_READER_BASE);
        let podman_resolve = resolve_base_command(
            RuntimeKind::Podman,
            Path::new("/trusted/bin/skopeo"),
            OsString::from("/trusted/bin:/usr/bin"),
            PathBuf::from("/private/bundle"),
            Duration::from_secs(30),
        );
        assert_eq!(podman_resolve.program, Path::new("/trusted/bin/skopeo"));
        assert_eq!(
            podman_resolve.args,
            [
                "inspect",
                "--raw",
                "docker://docker.io/library/node:24-slim"
            ]
            .into_iter()
            .map(OsString::from)
            .collect::<Vec<_>>()
        );
        let absent = image_tag_absence_command(
            RuntimeKind::Docker,
            Path::new("/trusted/bin/docker"),
            OsString::from("/trusted/bin:/usr/bin"),
            PathBuf::from("/private/bundle"),
            Duration::from_secs(30),
            &tag,
        );
        assert_eq!(absent.kind, ResolutionCommandKind::EnsureImageTagAbsent);
        assert!(absent
            .args
            .iter()
            .any(|argument| argument.to_string_lossy() == format!("reference={tag}")));
        confirm_image_tag_absent(b"\n").unwrap();
        assert_eq!(
            confirm_image_tag_absent(format!("sha256:{}\n", "e".repeat(64)).as_bytes())
                .unwrap_err(),
            ResolutionFailureCode::ImageTagAlreadyExists
        );
        let inspect = image_inspect_command(
            Path::new("/trusted/bin/docker"),
            OsString::from("/trusted/bin:/usr/bin"),
            PathBuf::from("/private/bundle"),
            Duration::from_secs(30),
            &tag,
        );
        assert_eq!(inspect.kind, ResolutionCommandKind::InspectImage);
    }

    #[test]
    fn image_inspect_requires_exact_id_and_label_set() {
        let packages = valid_resolution().packages;
        let labels = image_labels(
            "resolution-123",
            &"a".repeat(64),
            "reader-current",
            &packages,
        );
        let id = format!("sha256:{}", "d".repeat(64));
        let raw = serde_json::to_vec(&serde_json::json!([{
            "Id": id,
            "Config": {"Labels": labels}
        }]))
        .unwrap();
        assert_eq!(parse_image_inspect(&raw, &labels).unwrap(), id);

        let mut missing = labels.clone();
        missing.remove("io.a2a-bridge.r3c.recipe-sha256");
        assert!(parse_image_inspect(&raw, &missing)
            .unwrap_err()
            .contains("exact resolution provenance"));

        let mutable = String::from_utf8(raw)
            .unwrap()
            .replace(&id, "localhost/a2a-bridge-r3c:latest");
        assert!(parse_image_inspect(mutable.as_bytes(), &labels)
            .unwrap_err()
            .contains("immutable"));
    }

    fn tree_limits() -> ResolutionLimits {
        ResolutionLimits {
            timeout_secs: 30,
            max_download_bytes: 1024,
            max_unpacked_bytes: 1024 * 1024,
            max_files: 100,
        }
    }

    #[cfg(unix)]
    #[test]
    fn package_tree_inventory_is_deterministic_and_rejects_escape_and_hardlinks() {
        use std::os::unix::fs::{symlink, PermissionsExt as _};

        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        let package = tree.join("node_modules/example");
        fs::create_dir_all(package.join("bin")).unwrap();
        fs::write(package.join("package.json"), b"{\"name\":\"example\"}\n").unwrap();
        let executable = package.join("bin/example");
        fs::write(&executable, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let link = package.join("bin/package.json");
        symlink("../package.json", &link).unwrap();

        let first = inspect_package_tree(&tree, &tree_limits()).unwrap();
        let second = inspect_package_tree(&tree, &tree_limits()).unwrap();
        assert_eq!(first, second);
        assert!(first.file_count >= 5);
        assert!(first.byte_count > 0);
        assert_eq!(first.inventory_sha256.len(), 64);
        assert_eq!(first.tree_sha256.len(), 64);

        fs::remove_file(&link).unwrap();
        let outside = dir.path().join("outside");
        fs::write(&outside, b"outside").unwrap();
        symlink(&outside, &link).unwrap();
        assert!(inspect_package_tree(&tree, &tree_limits())
            .unwrap_err()
            .contains("escapes"));

        fs::remove_file(&link).unwrap();
        fs::hard_link(package.join("package.json"), &link).unwrap();
        assert!(inspect_package_tree(&tree, &tree_limits())
            .unwrap_err()
            .contains("exactly one link"));
    }

    #[test]
    fn package_tree_enforces_file_and_byte_bounds() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        fs::create_dir_all(&tree).unwrap();
        fs::write(tree.join("one"), b"12345").unwrap();

        let mut limits = tree_limits();
        limits.max_files = 0;
        assert!(inspect_package_tree(&tree, &limits)
            .unwrap_err()
            .contains("file-count"));

        limits.max_files = 10;
        limits.max_unpacked_bytes = 4;
        assert!(inspect_package_tree(&tree, &limits)
            .unwrap_err()
            .contains("unpacked-byte"));
    }

    struct ResolutionFakeExecutor {
        calls: std::sync::Mutex<Vec<ResolutionCommandKind>>,
        labels: std::sync::Mutex<BTreeMap<String, String>>,
        tag_exists: bool,
        tag_query_fails: bool,
        mutate_path: Option<PathBuf>,
    }

    #[async_trait]
    impl ResolutionExecutor for ResolutionFakeExecutor {
        async fn execute(
            &self,
            command: &ResolutionCommandSpec,
        ) -> Result<Vec<u8>, ResolutionFailureCode> {
            self.calls.lock().unwrap().push(command.kind);
            match command.kind {
                ResolutionCommandKind::NpmLock => {
                    fs::write(command.cwd.join("package-lock.json"), valid_package_lock()).unwrap();
                    if let Some(path) = &self.mutate_path {
                        fs::write(path, b"mutated protected input").unwrap();
                    }
                    Ok(Vec::new())
                }
                ResolutionCommandKind::NpmMaterialize => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt as _;

                        let adapter = command
                            .cwd
                            .join("tree/node_modules/@agentclientprotocol/codex-acp");
                        let cli = command.cwd.join("tree/node_modules/@openai/codex");
                        fs::create_dir_all(adapter.join("dist")).unwrap();
                        fs::create_dir_all(&cli).unwrap();
                        let executable = adapter.join("dist/index.js");
                        fs::write(&executable, b"#!/usr/bin/env node\n").unwrap();
                        fs::set_permissions(&executable, fs::Permissions::from_mode(0o500))
                            .unwrap();
                        fs::write(
                            adapter.join("package.json"),
                            br#"{"name":"@agentclientprotocol/codex-acp","version":"1.2.3","bin":{"codex-acp":"dist/index.js"}}"#,
                        )
                        .unwrap();
                        fs::write(
                            cli.join("package.json"),
                            br#"{"name":"@openai/codex","version":"0.150.1"}"#,
                        )
                        .unwrap();
                    }
                    Ok(Vec::new())
                }
                ResolutionCommandKind::ResolveBase => Ok(serde_json::to_vec(&serde_json::json!({
                    "schemaVersion": 2,
                    "manifests": [
                        {
                            "digest": format!("sha256:{}", "b".repeat(64)),
                            "platform": {"os": "linux", "architecture": "arm64"}
                        },
                        {
                            "digest": format!("sha256:{}", "c".repeat(64)),
                            "platform": {"os": "linux", "architecture": "amd64"}
                        }
                    ]
                }))
                .unwrap()),
                ResolutionCommandKind::EnsureImageTagAbsent if self.tag_query_fails => {
                    Err(ResolutionFailureCode::RuntimeNonzero)
                }
                ResolutionCommandKind::EnsureImageTagAbsent if self.tag_exists => {
                    Ok(format!("sha256:{}\n", "e".repeat(64)).into_bytes())
                }
                ResolutionCommandKind::EnsureImageTagAbsent => Ok(Vec::new()),
                ResolutionCommandKind::BuildImage => {
                    let mut labels = BTreeMap::new();
                    let mut arguments = command.args.iter();
                    while let Some(argument) = arguments.next() {
                        if argument == "--label" {
                            let value = arguments
                                .next()
                                .expect("every fake build label has a value")
                                .to_string_lossy();
                            let (key, value) = value
                                .split_once('=')
                                .expect("fake build labels use key=value");
                            labels.insert(key.to_owned(), value.to_owned());
                        }
                    }
                    *self.labels.lock().unwrap() = labels;
                    Ok(Vec::new())
                }
                ResolutionCommandKind::InspectImage => {
                    let labels = self.labels.lock().unwrap().clone();
                    Ok(serde_json::to_vec(&serde_json::json!([{
                        "Id": format!("sha256:{}", "d".repeat(64)),
                        "Config": {"Labels": labels}
                    }]))
                    .unwrap())
                }
            }
        }
    }

    fn test_executable_identity(path: &Path) -> ExecutableIdentity {
        let snapshot =
            local_file::read_regular_file_bounded(path, "test executable", 1024 * 1024).unwrap();
        ExecutableIdentity {
            canonical_path: snapshot.canonical_path.to_string_lossy().into_owned(),
            sha256: snapshot.sha256,
            byte_length: snapshot.bytes.len() as u64,
        }
    }

    fn codex_baseline_config(parent: &Path, reader: bool) -> BaselineConfigInput {
        let path = parent.join(if reader {
            "codex-reader.toml"
        } else {
            "codex-host.toml"
        });
        let raw = if reader {
            format!(
                r#"default = "codex-reader"
allowed_cwd_root = {root:?}

[[agents]]
id = "codex-reader"
cmd = "codex-acp"
pre_authenticated = true
model = "gpt-5.6-sol"
effort = "xhigh"
args = ["-c", "sandbox_mode=\"danger-full-access\"", "-c", "approval_policy=\"never\""]

[agents.sandbox]
image = "sha256:{image}"
mount = {root:?}
access = "ro"
egress = "locked"
network = "a2a-egress-internal"
proxy = "http://a2a-egress-proxy:8888"
volumes = ["/private/nonexistent/codex-auth.json:/root/.codex/auth.json"]

[registry]
allowed_cmds = ["docker"]

[server]
addr = "127.0.0.1:8080"
"#,
                root = parent.to_string_lossy(),
                image = "a".repeat(64),
            )
        } else {
            r#"default = "codex-host"

[[agents]]
id = "codex-host"
cmd = "codex-acp"
pre_authenticated = true
model = "gpt-5.6-sol"
effort = "xhigh"
mode = "read-only"
args = ["-c", "sandbox_mode=\"read-only\"", "-c", "approval_policy=\"never\""]

[registry]
allowed_cmds = ["codex-acp"]

[server]
addr = "127.0.0.1:8080"
"#
            .into()
        };
        fs::write(&path, raw.as_bytes()).unwrap();
        let snapshot =
            local_file::read_regular_file_bounded(&path, "test baseline config", 1024 * 1024)
                .unwrap();
        BaselineConfigInput {
            canonical_path: snapshot.canonical_path,
            sha256: snapshot.sha256,
            bytes: snapshot.bytes,
        }
    }

    fn provider_free_request(parent: &Path, include_reader: bool) -> ProviderFreeResolutionRequest {
        let recipes_path = parent.join("floating-current.toml");
        fs::write(&recipes_path, valid_recipes()).unwrap();
        let manifest_path = parent.join("manifest.toml");
        fs::write(&manifest_path, b"schema_version = 1\n").unwrap();
        let recipes = load_recipes(&recipes_path).unwrap();
        let manifest =
            local_file::read_regular_file_bounded(&manifest_path, "test manifest", 1024 * 1024)
                .unwrap();
        let candidate_path = parent.join("candidate");
        let runtime_path = parent.join("runtime");
        let npm_path = parent.join("npm");
        fs::write(&candidate_path, b"test candidate").unwrap();
        fs::write(&runtime_path, b"test runtime").unwrap();
        fs::write(&npm_path, b"test npm").unwrap();
        let npm_snapshot =
            local_file::read_regular_file_bounded(&npm_path, "test npm", 1024 * 1024).unwrap();
        let owner = "test-runner".to_owned();
        let os = std::env::consts::OS.to_owned();
        let architecture = std::env::consts::ARCH.to_owned();
        let mut cases = vec![ResolutionCaseInput {
            recipe: recipes
                .recipes
                .cases
                .iter()
                .find(|case| case.id == "codex-host-floating-current")
                .unwrap()
                .clone(),
            evidence_path: "bridge_smoke".into(),
            execution_mode: "host".into(),
            os: os.clone(),
            architecture: architecture.clone(),
            environment_owner: owner.clone(),
            agent: "codex-host".into(),
            model: "gpt-5.6-sol".into(),
            effort: Some("xhigh".into()),
            mode: Some("read-only".into()),
            session_cwd: Some(parent.to_path_buf()),
            auth_path: "pre_authenticated".into(),
            credential_env: None,
            required_env: Vec::new(),
            probe: "minimal".into(),
            billable: true,
            timeout_secs: 30,
            max_tokens: 10_000,
            max_cost_usd: Some(1.0),
            retry_cap: 0,
            expected_status: "PASS".into(),
            artifact: ResolutionArtifactInput { retention_days: 1 },
            baseline_config: codex_baseline_config(parent, false),
            component_pins: BTreeMap::new(),
        }];
        if include_reader {
            cases.push(ResolutionCaseInput {
                recipe: recipes
                    .recipes
                    .cases
                    .iter()
                    .find(|case| case.id == "codex-reader-floating-current")
                    .unwrap()
                    .clone(),
                evidence_path: "bridge_smoke".into(),
                execution_mode: "container_ro".into(),
                os: os.clone(),
                architecture: architecture.clone(),
                environment_owner: owner.clone(),
                agent: "codex-reader".into(),
                model: "gpt-5.6-sol".into(),
                effort: Some("xhigh".into()),
                mode: None,
                session_cwd: Some(parent.to_path_buf()),
                auth_path: "pre_authenticated".into(),
                credential_env: None,
                required_env: Vec::new(),
                probe: "minimal".into(),
                billable: true,
                timeout_secs: 30,
                max_tokens: 10_000,
                max_cost_usd: Some(1.0),
                retry_cap: 0,
                expected_status: "PASS".into(),
                artifact: ResolutionArtifactInput { retention_days: 1 },
                baseline_config: codex_baseline_config(parent, true),
                component_pins: BTreeMap::new(),
            });
        }
        let runtime_executable = test_executable_identity(&runtime_path);
        let base_resolver_executable = PathBuf::from(&runtime_executable.canonical_path);
        ProviderFreeResolutionRequest {
            output: parent.join("bundle"),
            recipes,
            production_manifest: VersionedArtifactIdentity {
                schema_version: 1,
                canonical_path: manifest.canonical_path.to_string_lossy().into_owned(),
                sha256: manifest.sha256,
            },
            candidate: test_executable_identity(&candidate_path),
            environment_owner: owner,
            os,
            architecture,
            runtime: RuntimeKind::Docker,
            runtime_executable,
            base_resolver_executable,
            npm_executable: npm_snapshot.canonical_path.clone(),
            safe_path: OsString::from("/usr/bin:/bin"),
            budget: ResolutionBudgetInput {
                timeout_secs: 60,
                max_tokens: 20_000,
                max_cost_usd: Some(2.0),
            },
            cases,
            protected_inputs: vec![ProtectedFileInput {
                canonical_path: npm_snapshot.canonical_path,
                sha256: npm_snapshot.sha256,
                max_bytes: npm_snapshot.bytes.len() as u64,
            }],
        }
    }

    #[tokio::test]
    async fn provider_free_resolver_publishes_exact_host_and_reader_bundle_with_fake_effects() {
        let parent = tempfile::tempdir().unwrap();
        let request = provider_free_request(parent.path(), true);
        let output = request.output.clone();
        let executor = ResolutionFakeExecutor {
            calls: std::sync::Mutex::new(Vec::new()),
            labels: std::sync::Mutex::new(BTreeMap::new()),
            tag_exists: false,
            tag_query_fails: false,
            mutate_path: None,
        };

        let artifact = resolve_with_executor(request, &executor).await.unwrap();
        assert_eq!(artifact.state, ResolutionState::Complete);
        assert_eq!(artifact.packages.len(), 1);
        assert_eq!(artifact.images.len(), 1);
        assert_eq!(artifact.cases.len(), 2);
        assert_eq!(
            &*executor.calls.lock().unwrap(),
            &[
                ResolutionCommandKind::NpmLock,
                ResolutionCommandKind::NpmMaterialize,
                ResolutionCommandKind::ResolveBase,
                ResolutionCommandKind::EnsureImageTagAbsent,
                ResolutionCommandKind::BuildImage,
                ResolutionCommandKind::InspectImage,
            ]
        );
        let loaded = load_resolution(&output.join("resolution.json")).unwrap();
        assert_eq!(loaded.artifact.state, ResolutionState::Complete);
        let manifest = fs::read_to_string(output.join("execution-manifest.toml")).unwrap();
        compatibility::validate_manifest_text(&manifest).unwrap();
        let host =
            fs::read_to_string(output.join("configs/codex-host-floating-current.toml")).unwrap();
        assert!(host.contains("packages/codex-current/tree/"));
        let reader =
            fs::read_to_string(output.join("configs/codex-reader-floating-current.toml")).unwrap();
        assert!(reader.contains("/opt/a2a/packages/codex-current/"));
        assert!(reader.contains(&format!("sha256:{}", "d".repeat(64))));
        assert!(!output.join("packages/codex-current/cache").exists());
    }

    struct ImageRevalidationExecutor {
        id: String,
        labels: BTreeMap<String, String>,
    }

    #[async_trait]
    impl ResolutionExecutor for ImageRevalidationExecutor {
        async fn execute(
            &self,
            command: &ResolutionCommandSpec,
        ) -> Result<Vec<u8>, ResolutionFailureCode> {
            assert_eq!(command.kind, ResolutionCommandKind::InspectImage);
            Ok(serde_json::to_vec(&serde_json::json!([{
                "Id": self.id,
                "Config": {"Labels": self.labels}
            }]))
            .unwrap())
        }
    }

    #[cfg(unix)]
    fn overwrite_test_file(path: &Path, bytes: &[u8], mode: u32) {
        use std::os::unix::fs::PermissionsExt as _;

        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bound_revalidation_detects_each_file_owner_and_image_drift_family() {
        use std::os::unix::fs::PermissionsExt as _;

        #[derive(Clone, Copy)]
        enum Drift {
            Resolution,
            Recipe,
            Candidate,
            Runtime,
            Config,
            Lock,
            Tree,
            Executable,
            Owner,
        }
        for (drift, expected) in [
            (
                Drift::Resolution,
                RevalidationFailure::ResolutionArtifactChanged,
            ),
            (Drift::Recipe, RevalidationFailure::RecipeChanged),
            (
                Drift::Candidate,
                RevalidationFailure::CandidateBinaryChanged,
            ),
            (
                Drift::Runtime,
                RevalidationFailure::RuntimeExecutableChanged,
            ),
            (Drift::Config, RevalidationFailure::GeneratedConfigChanged),
            (Drift::Lock, RevalidationFailure::PackageLockChanged),
            (Drift::Tree, RevalidationFailure::PackageTreeChanged),
            (
                Drift::Executable,
                RevalidationFailure::AdapterExecutableChanged,
            ),
            (Drift::Owner, RevalidationFailure::EnvironmentOwnerChanged),
        ] {
            let parent = tempfile::tempdir().unwrap();
            let request = provider_free_request(parent.path(), false);
            let output = request.output.clone();
            let executor = ResolutionFakeExecutor {
                calls: std::sync::Mutex::new(Vec::new()),
                labels: std::sync::Mutex::new(BTreeMap::new()),
                tag_exists: false,
                tag_query_fails: false,
                mutate_path: None,
            };
            resolve_with_executor(request, &executor).await.unwrap();
            let loaded = load_resolution(&output.join("resolution.json")).unwrap();
            revalidate_resolution_case_with_executor(
                &loaded,
                "test-runner",
                "codex-host-floating-current",
                &executor,
            )
            .await
            .unwrap();

            match drift {
                Drift::Resolution => overwrite_test_file(&loaded.canonical_path, b"{}\n", 0o600),
                Drift::Recipe => overwrite_test_file(
                    Path::new(&loaded.artifact.recipes.canonical_path),
                    b"schema_version = 1\n",
                    0o600,
                ),
                Drift::Candidate => overwrite_test_file(
                    Path::new(&loaded.artifact.candidate.canonical_path),
                    b"changed candidate",
                    0o700,
                ),
                Drift::Runtime => overwrite_test_file(
                    Path::new(
                        &loaded
                            .artifact
                            .environment
                            .runtime_executable
                            .canonical_path,
                    ),
                    b"changed runtime",
                    0o700,
                ),
                Drift::Config => overwrite_test_file(
                    Path::new(&loaded.artifact.cases[0].generated_config.canonical_path),
                    b"changed config",
                    0o600,
                ),
                Drift::Lock => overwrite_test_file(
                    &output.join("packages/codex-current/package-lock.json"),
                    b"{}\n",
                    0o600,
                ),
                Drift::Tree => overwrite_test_file(
                    &output.join("packages/codex-current/tree/package.json"),
                    br#"{"name":"changed-materialization-root"}"#,
                    0o400,
                ),
                Drift::Executable => overwrite_test_file(
                    Path::new(
                        &loaded.artifact.packages[0]
                            .adapter_executable
                            .canonical_path,
                    ),
                    b"#!/usr/bin/env node\n// changed\n",
                    0o500,
                ),
                Drift::Owner => {}
            }
            let owner = if matches!(drift, Drift::Owner) {
                "different-owner"
            } else {
                "test-runner"
            };
            let error = revalidate_resolution_case_with_executor(
                &loaded,
                owner,
                "codex-host-floating-current",
                &executor,
            )
            .await
            .unwrap_err();
            assert_eq!(
                error, expected,
                "unexpected classification for drift family"
            );

            // Keep TempDir cleanup reliable after deliberately making package files read-only.
            if output.exists() {
                let _ = fs::set_permissions(&output, fs::Permissions::from_mode(0o700));
            }
        }

        let parent = tempfile::tempdir().unwrap();
        let request = provider_free_request(parent.path(), true);
        let output = request.output.clone();
        let executor = ResolutionFakeExecutor {
            calls: std::sync::Mutex::new(Vec::new()),
            labels: std::sync::Mutex::new(BTreeMap::new()),
            tag_exists: false,
            tag_query_fails: false,
            mutate_path: None,
        };
        resolve_with_executor(request, &executor).await.unwrap();
        let loaded = load_resolution(&output.join("resolution.json")).unwrap();
        let image = &loaded.artifact.images[0];
        let wrong_image = ImageRevalidationExecutor {
            id: format!("sha256:{}", "e".repeat(64)),
            labels: image.labels.clone(),
        };
        assert_eq!(
            revalidate_resolution_case_with_executor(
                &loaded,
                "test-runner",
                "codex-reader-floating-current",
                &wrong_image,
            )
            .await
            .unwrap_err(),
            RevalidationFailure::ImageChanged
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn generated_prerequisite_is_hash_bound_and_drift_is_rejected() {
        use std::os::unix::fs::PermissionsExt as _;

        let parent = tempfile::tempdir().unwrap();
        let request = provider_free_request(parent.path(), false);
        let output = request.output.clone();
        let executor = ResolutionFakeExecutor {
            calls: std::sync::Mutex::new(Vec::new()),
            labels: std::sync::Mutex::new(BTreeMap::new()),
            tag_exists: false,
            tag_query_fails: false,
            mutate_path: None,
        };
        resolve_with_executor(request, &executor).await.unwrap();
        let mut loaded = load_resolution(&output.join("resolution.json")).unwrap();
        let directory = output.join("prerequisites");
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.join("fable-settings.json");
        fs::write(&path, b"{}\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let snapshot = local_file::read_regular_file_bounded(
            &path,
            "test generated prerequisite",
            MAX_SETTINGS_BYTES,
        )
        .unwrap();
        loaded.artifact.protected_inputs.push(ProtectedInput {
            path: snapshot.canonical_path.to_string_lossy().into_owned(),
            before_sha256: snapshot.sha256.clone(),
            after_sha256: snapshot.sha256,
        });
        let mut case = loaded.artifact.cases[0].clone();
        case.prerequisites.push(NonSecretPrerequisite {
            name: "fable-settings".into(),
            destination: Some("/root/.claude/settings.json".into()),
        });
        revalidate_generated_prerequisites(&loaded, &case).unwrap();

        fs::write(&path, b"{\"changed\":true}\n").unwrap();
        assert_eq!(
            revalidate_generated_prerequisites(&loaded, &case).unwrap_err(),
            RevalidationFailure::PrerequisiteChanged
        );
    }

    #[tokio::test]
    async fn provider_free_resolver_refuses_existing_tag_before_build_and_publishes_failure() {
        let parent = tempfile::tempdir().unwrap();
        let request = provider_free_request(parent.path(), true);
        let output = request.output.clone();
        let executor = ResolutionFakeExecutor {
            calls: std::sync::Mutex::new(Vec::new()),
            labels: std::sync::Mutex::new(BTreeMap::new()),
            tag_exists: true,
            tag_query_fails: false,
            mutate_path: None,
        };

        resolve_with_executor(request, &executor).await.unwrap_err();
        let loaded = load_resolution(&output.join("resolution.json")).unwrap();
        assert_eq!(loaded.artifact.state, ResolutionState::Failed);
        assert_eq!(
            loaded.artifact.failure.unwrap().code,
            ResolutionFailureCode::ImageTagAlreadyExists
        );
        assert!(!executor
            .calls
            .lock()
            .unwrap()
            .contains(&ResolutionCommandKind::BuildImage));
    }

    #[tokio::test]
    async fn provider_free_resolver_does_not_treat_failed_tag_query_as_absent() {
        let parent = tempfile::tempdir().unwrap();
        let request = provider_free_request(parent.path(), true);
        let output = request.output.clone();
        let executor = ResolutionFakeExecutor {
            calls: std::sync::Mutex::new(Vec::new()),
            labels: std::sync::Mutex::new(BTreeMap::new()),
            tag_exists: false,
            tag_query_fails: true,
            mutate_path: None,
        };

        resolve_with_executor(request, &executor).await.unwrap_err();
        let loaded = load_resolution(&output.join("resolution.json")).unwrap();
        assert_eq!(loaded.artifact.state, ResolutionState::Failed);
        assert_eq!(
            loaded.artifact.failure.unwrap().code,
            ResolutionFailureCode::RuntimeNonzero
        );
        assert!(!executor
            .calls
            .lock()
            .unwrap()
            .contains(&ResolutionCommandKind::BuildImage));
    }

    #[tokio::test]
    async fn provider_free_resolver_records_protected_mutation_as_terminal_failure() {
        let parent = tempfile::tempdir().unwrap();
        let sentinel = parent.path().join("protected-sentinel");
        fs::write(&sentinel, b"before").unwrap();
        let sentinel_snapshot =
            local_file::read_regular_file_bounded(&sentinel, "test protected sentinel", 1024)
                .unwrap();
        let mut request = provider_free_request(parent.path(), false);
        request.protected_inputs.push(ProtectedFileInput {
            canonical_path: sentinel_snapshot.canonical_path.clone(),
            sha256: sentinel_snapshot.sha256,
            max_bytes: 1024,
        });
        let output = request.output.clone();
        let executor = ResolutionFakeExecutor {
            calls: std::sync::Mutex::new(Vec::new()),
            labels: std::sync::Mutex::new(BTreeMap::new()),
            tag_exists: false,
            tag_query_fails: false,
            mutate_path: Some(sentinel_snapshot.canonical_path),
        };

        resolve_with_executor(request, &executor).await.unwrap_err();
        let loaded = load_resolution(&output.join("resolution.json")).unwrap();
        assert_eq!(loaded.artifact.state, ResolutionState::Failed);
        assert_eq!(
            loaded.artifact.failure.unwrap().code,
            ResolutionFailureCode::ProtectedStateChanged
        );
        assert!(loaded
            .artifact
            .protected_inputs
            .iter()
            .any(|input| input.before_sha256 != input.after_sha256));
    }

    #[tokio::test]
    async fn provider_free_resolver_rejects_unknown_baseline_config_fields_before_effects() {
        let parent = tempfile::tempdir().unwrap();
        let mut request = provider_free_request(parent.path(), false);
        let case = &mut request.cases[0];
        case.baseline_config
            .bytes
            .extend_from_slice(b"unknown = true\n");
        fs::write(
            &case.baseline_config.canonical_path,
            &case.baseline_config.bytes,
        )
        .unwrap();
        case.baseline_config.sha256 = local_file::sha256_hex(&case.baseline_config.bytes);
        let output = request.output.clone();
        let executor = ResolutionFakeExecutor {
            calls: std::sync::Mutex::new(Vec::new()),
            labels: std::sync::Mutex::new(BTreeMap::new()),
            tag_exists: false,
            tag_query_fails: false,
            mutate_path: None,
        };

        resolve_with_executor(request, &executor).await.unwrap_err();
        assert!(!output.exists());
        assert!(executor.calls.lock().unwrap().is_empty());
    }

    fn command_for_test(program: &Path, args: &[&str], timeout: Duration) -> ResolutionCommandSpec {
        ResolutionCommandSpec {
            family: ResolutionCommandFamily::Runtime,
            kind: ResolutionCommandKind::InspectImage,
            program: program.to_path_buf(),
            args: args.iter().map(OsString::from).collect(),
            cwd: PathBuf::from("/"),
            env: BTreeMap::new(),
            timeout,
            max_output_bytes: 1024,
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn process_executor_bounds_output_nonzero_and_timeout_without_dynamic_errors() {
        let executor = ProcessResolutionExecutor;
        let mut command = command_for_test(
            Path::new("/usr/bin/printf"),
            &["bounded"],
            Duration::from_secs(1),
        );
        assert_eq!(executor.execute(&command).await.unwrap(), b"bounded");

        command.max_output_bytes = 3;
        assert_eq!(
            executor.execute(&command).await.unwrap_err(),
            ResolutionFailureCode::RuntimeOutputTooLarge
        );

        command = command_for_test(Path::new("/usr/bin/false"), &[], Duration::from_secs(1));
        assert_eq!(
            executor.execute(&command).await.unwrap_err(),
            ResolutionFailureCode::RuntimeNonzero
        );

        command = command_for_test(Path::new("/bin/sleep"), &["2"], Duration::from_millis(20));
        assert_eq!(
            executor.execute(&command).await.unwrap_err(),
            ResolutionFailureCode::RuntimeTimeout
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn process_executor_kills_stdout_holding_descendant_after_parent_exit() {
        let directory = tempfile::tempdir().unwrap();
        let survivor_marker = directory.path().join("survivor");
        let survivor_marker_text = survivor_marker.to_str().unwrap();
        let command = command_for_test(
            Path::new("/bin/sh"),
            &[
                "-c",
                "(/bin/sleep 0.2; : > \"$1\") &",
                "resolver-descendant-test",
                survivor_marker_text,
            ],
            Duration::from_millis(20),
        );

        assert_eq!(
            ProcessResolutionExecutor
                .execute(&command)
                .await
                .unwrap_err(),
            ResolutionFailureCode::RuntimeTimeout
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            !survivor_marker.exists(),
            "the stdout-holding descendant survived the resolver deadline"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn process_executor_settles_detached_descendants_after_parent_completion() {
        let directory = tempfile::tempdir().unwrap();
        for (name, exit_code) in [("success-survivor", "0"), ("failure-survivor", "7")] {
            let marker = directory.path().join(name);
            let command = command_for_test(
                Path::new("/bin/sh"),
                &[
                    "-c",
                    "(/bin/sleep 0.2; : > \"$1\") >/dev/null 2>&1 & exit \"$2\"",
                    "resolver-detached-descendant-test",
                    marker.to_str().unwrap(),
                    exit_code,
                ],
                Duration::from_secs(1),
            );
            let result = ProcessResolutionExecutor.execute(&command).await;
            if exit_code == "0" {
                assert_eq!(result.unwrap(), Vec::<u8>::new());
            } else {
                assert_eq!(result.unwrap_err(), ResolutionFailureCode::RuntimeNonzero);
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
            assert!(
                !marker.exists(),
                "a detached descendant survived a completed resolver command"
            );
        }
    }
}
