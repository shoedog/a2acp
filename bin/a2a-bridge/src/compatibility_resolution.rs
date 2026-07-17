//! R3c floating-current recipe and exact resolution contracts.
//!
//! This module intentionally owns no subprocess or filesystem-write implementation yet. The contract
//! slice makes recipe and completed-resolution evidence strict before a later slice is allowed to add
//! registry, package-tree, or container-runtime effects.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{compatibility, local_file, BoxError};

pub(super) const DEFAULT_RECIPES: &str = "compatibility/floating-current.toml";

const MAX_RECIPE_BYTES: u64 = 1024 * 1024;
const MAX_RESOLUTION_BYTES: u64 = 16 * 1024 * 1024;
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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PackageSetRecipe {
    pub(super) id: String,
    pub(super) ecosystem: RecipeEcosystem,
    pub(super) registry: RecipeRegistry,
    pub(super) adapter: String,
    pub(super) adapter_selector: String,
    pub(super) agent_cli: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ImageRecipe {
    pub(super) id: String,
    pub(super) template: ImageTemplate,
    pub(super) base: String,
    pub(super) package_sets: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
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

#[derive(Clone, Debug, Deserialize, Serialize)]
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
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ArtifactIdentity {
    pub(super) canonical_path: String,
    pub(super) sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct VersionedArtifactIdentity {
    pub(super) schema_version: u16,
    pub(super) canonical_path: String,
    pub(super) sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExecutableIdentity {
    pub(super) canonical_path: String,
    pub(super) sha256: String,
    pub(super) byte_length: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResolutionEnvironment {
    pub(super) environment_owner: String,
    pub(super) os: String,
    pub(super) architecture: String,
    pub(super) runtime: RuntimeKind,
    pub(super) runtime_executable: ExecutableIdentity,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RequestedPackageSet {
    pub(super) adapter: String,
    pub(super) adapter_selector: String,
    pub(super) agent_cli: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExactNpmPackage {
    pub(super) name: String,
    pub(super) version: String,
    pub(super) integrity: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
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
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResolvedImage {
    pub(super) id: String,
    pub(super) requested_base: String,
    pub(super) registry_index_digest: String,
    pub(super) platform_manifest_digest: String,
    pub(super) build_template_sha256: String,
    pub(super) final_image_id: String,
    pub(super) owned_tag: String,
    pub(super) labels: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NonSecretPrerequisite {
    pub(super) name: String,
    #[serde(default)]
    pub(super) destination: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
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
    NpmTimeout,
    NpmNonzero,
    NpmOutputTooLarge,
    BaseDigestUnavailable,
    PackageIdentityMismatch,
    PackageTreeDrift,
    ImageLabelMismatch,
    ProtectedStateChanged,
    WriteScopeEscape,
    RuntimeTimeout,
    RuntimeNonzero,
    RuntimeOutputTooLarge,
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
    if identity.byte_length == 0 {
        return Err(format!("{label} byte_length must be positive"));
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
        if image.owned_tag.ends_with(":latest") {
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
        if input.before_sha256 != input.after_sha256 {
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
}
