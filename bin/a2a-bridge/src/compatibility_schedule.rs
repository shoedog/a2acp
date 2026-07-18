//! R3d0 default-off scheduling policy, schema, and canonical-identity foundation.
//!
//! This module is deliberately effect-free. It parses bounded local files, validates the complete
//! checked-in characterization inventory, and derives canonical hashes. It does not read credentials,
//! access a registry or container runtime, spawn an agent, publish a check, or mutate operator state.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::{compatibility, local_file, BoxError};

const MAX_FOUNDATION_FILE_BYTES: u64 = 1024 * 1024;
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_ID_BYTES: usize = 128;
const MAX_TEXT_BYTES: usize = 4096;
const MAX_CASES: usize = 64;
const MAX_REQUIRED_ENV: usize = 32;
const MAX_DEFERRED_PROFILES: usize = 32;
const MAX_TIMEOUT_SECS: u64 = 900;
const MAX_TOKENS: u64 = 1_000_000;
const MAX_COST_MICROUSD: u64 = 100_000_000;
const EXPECTED_SUPPORT_PROFILES: [&str; 4] = [
    "claude-host-acp-044-fable",
    "claude-reader-055-fable",
    "codex-host-bridge-gpt56-sol",
    "codex-reader-bridge-gpt56-sol",
];

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub(super) enum TriggerKindV1 {
    ManualCharacterization,
    ManualCompatibility,
    Daily,
    ScheduledMain,
    TestMerge,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub(super) enum EffectClassV1 {
    ProviderPrompt,
    RegistryRead,
    ImageInspect,
    ImageBuild,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ReplicationModeV1 {
    OwnerIcloud,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct EffectCapsV1 {
    pub(super) timeout_secs: u64,
    pub(super) max_tokens: u64,
    pub(super) max_cost_microusd: u64,
    pub(super) attempts: u8,
    pub(super) retry_cap: u8,
    pub(super) fallback_cap: u8,
}

impl EffectCapsV1 {
    pub(super) fn validate(&self, label: &str) -> Result<(), BoxError> {
        if self.timeout_secs == 0 || self.timeout_secs > MAX_TIMEOUT_SECS {
            return Err(format!(
                "schedule foundation: {label}.timeout_secs must be in 1..={MAX_TIMEOUT_SECS}"
            )
            .into());
        }
        if self.max_tokens == 0 || self.max_tokens > MAX_TOKENS {
            return Err(format!(
                "schedule foundation: {label}.max_tokens must be in 1..={MAX_TOKENS}"
            )
            .into());
        }
        if self.max_cost_microusd > MAX_COST_MICROUSD {
            return Err(format!(
                "schedule foundation: {label}.max_cost_microusd exceeds {MAX_COST_MICROUSD}"
            )
            .into());
        }
        if self.attempts != 1 || self.retry_cap != 0 || self.fallback_cap != 0 {
            return Err(format!(
                "schedule foundation: {label} must allow exactly one attempt with retry/fallback zero"
            )
            .into());
        }
        Ok(())
    }

    pub(super) fn within(&self, maximum: &Self, label: &str) -> Result<(), BoxError> {
        if self.timeout_secs > maximum.timeout_secs
            || self.max_tokens > maximum.max_tokens
            || self.max_cost_microusd > maximum.max_cost_microusd
            || self.attempts > maximum.attempts
            || self.retry_cap > maximum.retry_cap
            || self.fallback_cap > maximum.fallback_cap
        {
            return Err(format!(
                "schedule foundation: {label} exceeds the checked-in profile maximum"
            )
            .into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct StoragePolicyV1 {
    hot_root: String,
    hot_cap_bytes: u64,
    hot_index_cap_bytes: u64,
    hot_scratch_cap_bytes: u64,
    hot_sealed_cap_bytes: u64,
    cold_root: String,
    cold_cap_bytes: u64,
    replication_mode: ReplicationModeV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct SchedulePolicyV1 {
    schema_version: u16,
    environment_owner: String,
    repository: String,
    fixed_prompt_contract: String,
    artifact_template: String,
    price_ranking_authority: String,
    price_snapshot_max_age_secs: u64,
    scheduled_registry: PathBuf,
    characterization_inventory: PathBuf,
    production_manifest: PathBuf,
    floating_recipes: PathBuf,
    allowed_triggers: Vec<TriggerKindV1>,
    allowed_effects: Vec<EffectClassV1>,
    storage: StoragePolicyV1,
    profile_maxima: EffectCapsV1,
    #[serde(default)]
    deferred_profiles: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum CharacterizationStateV1 {
    CharacterizationRequired,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum ScheduledLaneV1 {
    FloatingCurrent,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ScheduledClassificationV1 {
    Canary,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum EvidencePurposeV1 {
    ProviderPathAdvisory,
    ClaimedSupportGate,
    Characterization,
    ManualDiagnostic,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ScheduleExecutionModeV1 {
    Host,
    ContainerRo,
    RemoteApi,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ScheduleAuthPathV1 {
    ApiKeyEnv,
    PreAuthenticated,
    Automatic,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ScheduleRedactionV1 {
    Strict,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
struct RequiredEnvironmentV1 {
    name: String,
    #[serde(default)]
    one_of: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ScheduleArtifactPolicyV1 {
    retention_days: u16,
    redaction: ScheduleRedactionV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ScheduledCaseV1 {
    id: String,
    state: CharacterizationStateV1,
    provider_family: String,
    capability: String,
    lane: ScheduledLaneV1,
    classification: ScheduledClassificationV1,
    evidence_purpose: EvidencePurposeV1,
    execution_mode: ScheduleExecutionModeV1,
    os: String,
    architecture: String,
    environment_owner: String,
    config: PathBuf,
    agent: String,
    model: String,
    #[serde(default)]
    effort: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    expected_effective_model: String,
    #[serde(default)]
    expected_effective_effort: Option<String>,
    #[serde(default)]
    expected_effective_mode: Option<String>,
    session_cwd: PathBuf,
    auth_path: ScheduleAuthPathV1,
    #[serde(default)]
    credential_env: Option<String>,
    #[serde(default)]
    required_env: Vec<RequiredEnvironmentV1>,
    #[serde(default)]
    resolution_case: Option<String>,
    config_template: String,
    adapter_family: String,
    agent_cli_family: String,
    #[serde(default)]
    image_family: Option<String>,
    allowed_effects: Vec<EffectClassV1>,
    caps: EffectCapsV1,
    artifact: ScheduleArtifactPolicyV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ScheduledCaseRegistryV1 {
    schema_version: u16,
    #[serde(default)]
    cases: Vec<ScheduledCaseV1>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum ProfileSourceKindV1 {
    ScheduledAdvisory,
    ClaimedSupportGate,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CharacterizationProfileReferenceV1 {
    id: String,
    source_kind: ProfileSourceKindV1,
    source_id: String,
    profile_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CharacterizationProfileInventoryV1 {
    schema_version: u16,
    #[serde(default)]
    profiles: Vec<CharacterizationProfileReferenceV1>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CanonicalProfileInputV1 {
    schema_version: u16,
    source_kind: ProfileSourceKindV1,
    source_id: String,
    repository: String,
    source_schema_version: u16,
    lane: String,
    classification: String,
    evidence_purpose: EvidencePurposeV1,
    execution_mode: String,
    provider_family: String,
    agent: String,
    capability: String,
    adapter_family: String,
    agent_cli_family: String,
    image_family: Option<String>,
    auth_path: String,
    credential_env_name: Option<String>,
    required_env: Vec<RequiredEnvironmentV1>,
    environment_owner: String,
    os: String,
    architecture: String,
    session_cwd: String,
    requested_model: String,
    requested_effort: Option<String>,
    requested_mode: Option<String>,
    expected_effective_model: String,
    expected_effective_effort: Option<String>,
    expected_effective_mode: Option<String>,
    config_template: String,
    config_template_sha256: String,
    resolution_constraint: Option<String>,
    fixed_prompt_contract: String,
    artifact_template: String,
    artifact_retention_days: u16,
    artifact_redaction: String,
    maximum_caps: EffectCapsV1,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CanonicalSandboxTemplateV1 {
    image_family: String,
    mount: String,
    access: String,
    egress: String,
    network: String,
    proxy: String,
    volumes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CanonicalConfigTemplateV1 {
    schema_version: u16,
    template_id: String,
    kind: String,
    command_family: Option<String>,
    base_url: Option<String>,
    api_key_env: Option<String>,
    pre_authenticated: bool,
    args: Vec<String>,
    allowed_cwd_root: Option<String>,
    sandbox: Option<CanonicalSandboxTemplateV1>,
    allowed_commands: Vec<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ProfilePolicyBundleInputV1 {
    schema_version: u16,
    policy_sha256: String,
    registry_sha256: String,
    inventory_sha256: String,
    floating_recipes_sha256: String,
    scheduled_profiles: BTreeMap<String, String>,
    claimed_support_profiles: BTreeMap<String, String>,
    config_templates: BTreeMap<String, String>,
    allowed_effects: Vec<EffectClassV1>,
    profile_maxima: EffectCapsV1,
    fixed_prompt_contract: String,
    artifact_template: String,
}

#[derive(Debug)]
struct LoadedToml<T> {
    value: T,
    sha256: String,
    canonical_path: PathBuf,
}

#[derive(Debug)]
pub(super) struct LoadedScheduleFoundation {
    pub(super) scheduled_profile_count: usize,
    pub(super) claimed_support_profile_count: usize,
    pub(super) profile_policy_bundle_sha256: String,
}

fn bounded_text(label: &str, value: &str) -> Result<(), BoxError> {
    if value.is_empty()
        || value.trim() != value
        || value.len() > MAX_TEXT_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(format!(
            "schedule foundation: {label} must be non-empty, unpadded, control-free UTF-8 of at most {MAX_TEXT_BYTES} bytes"
        )
        .into());
    }
    if compatibility::looks_like_secret(value) {
        return Err(format!("schedule foundation: {label} contains secret-shaped material").into());
    }
    Ok(())
}

fn stable_id(label: &str, value: &str) -> Result<(), BoxError> {
    bounded_text(label, value)?;
    if value.len() > MAX_ID_BYTES {
        return Err(format!("schedule foundation: {label} exceeds {MAX_ID_BYTES} bytes").into());
    }
    let mut bytes = value.bytes();
    if !matches!(bytes.next(), Some(b'a'..=b'z') | Some(b'0'..=b'9'))
        || !bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
    {
        return Err(format!("schedule foundation: {label} must be a lowercase stable id").into());
    }
    Ok(())
}

fn env_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(b'A'..=b'Z') | Some(b'_'))
        && bytes.all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        && value.len() <= MAX_ID_BYTES
}

fn validate_sha256(label: &str, value: &str) -> Result<(), BoxError> {
    if !local_file::valid_sha256(value) || value.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return Err(
            format!("schedule foundation: {label} must be one lowercase SHA-256 digest").into(),
        );
    }
    Ok(())
}

fn validate_relative_path(label: &str, path: &Path) -> Result<(), BoxError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "schedule foundation: {label} must be a non-empty relative path without traversal"
        )
        .into());
    }
    Ok(())
}

fn load_toml<T: DeserializeOwned>(path: &Path, label: &str) -> Result<LoadedToml<T>, BoxError> {
    let snapshot = local_file::read_regular_file_bounded(path, label, MAX_FOUNDATION_FILE_BYTES)?;
    let text = std::str::from_utf8(&snapshot.bytes)
        .map_err(|_| format!("schedule foundation: {label} must be UTF-8"))?;
    let value = toml::from_str(text)
        .map_err(|error| format!("schedule foundation: invalid {label}: {error}"))?;
    Ok(LoadedToml {
        value,
        sha256: snapshot.sha256,
        canonical_path: snapshot.canonical_path,
    })
}

fn ensure_snapshot_within_root(
    root: &Path,
    canonical_path: &Path,
    label: &str,
) -> Result<(), BoxError> {
    if !canonical_path.starts_with(root) {
        return Err(format!("schedule foundation: {label} escapes the foundation root").into());
    }
    Ok(())
}

fn load_foundation_toml<T: DeserializeOwned>(
    root: &Path,
    path: &Path,
    label: &str,
) -> Result<LoadedToml<T>, BoxError> {
    let loaded = load_toml(path, label)?;
    ensure_snapshot_within_root(root, &loaded.canonical_path, label)?;
    Ok(loaded)
}

fn canonical_hash<T: Serialize>(label: &str, value: &T) -> Result<String, BoxError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| format!("schedule foundation: cannot canonicalize {label}: {error}"))?;
    Ok(local_file::sha256_hex(&bytes))
}

fn exact_toml_keys(
    table: &toml::map::Map<String, toml::Value>,
    allowed: &[&str],
    label: &str,
) -> Result<(), BoxError> {
    let allowed = allowed.iter().copied().collect::<BTreeSet<_>>();
    let unknown = table
        .keys()
        .filter(|key| !allowed.contains(key.as_str()))
        .map(String::as_str)
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        return Err(
            format!("schedule foundation: {label} contains unknown fields {unknown:?}").into(),
        );
    }
    Ok(())
}

fn validate_runtime_config(text: &str, label: &str) -> Result<(), BoxError> {
    crate::config::RegistryConfig::parse(text)
        .and_then(crate::config::RegistryConfig::into_snapshot)
        .map(|_| ())
        .map_err(|error| format!("schedule foundation: invalid {label}: {error}").into())
}

fn optional_toml_string(
    table: &toml::map::Map<String, toml::Value>,
    field: &str,
    label: &str,
) -> Result<Option<String>, BoxError> {
    table
        .get(field)
        .map(|value| {
            value.as_str().map(str::to_owned).ok_or_else(|| {
                format!("schedule foundation: {label}.{field} must be a string").into()
            })
        })
        .transpose()
}

fn toml_string_array(
    table: &toml::map::Map<String, toml::Value>,
    field: &str,
    label: &str,
) -> Result<Vec<String>, BoxError> {
    let Some(value) = table.get(field) else {
        return Ok(Vec::new());
    };
    let values = value
        .as_array()
        .ok_or_else(|| format!("schedule foundation: {label}.{field} must be an array"))?;
    values
        .iter()
        .map(|value| {
            value.as_str().map(str::to_owned).ok_or_else(|| {
                format!("schedule foundation: {label}.{field} entries must be strings").into()
            })
        })
        .collect()
}

fn canonical_config_template_hash(
    template_id: &str,
    image_family: Option<&str>,
    value: &toml::Value,
) -> Result<String, BoxError> {
    stable_id("config template id", template_id)?;
    let root = value
        .as_table()
        .ok_or("schedule foundation: config template must be a TOML table")?;
    exact_toml_keys(
        root,
        &[
            "default",
            "allowed_cwd_root",
            "agents",
            "registry",
            "server",
        ],
        "config root",
    )?;
    let agents = root
        .get("agents")
        .and_then(toml::Value::as_array)
        .ok_or("schedule foundation: config template must contain one agent")?;
    if agents.len() != 1 {
        return Err("schedule foundation: config template must contain exactly one agent".into());
    }
    let agent = agents[0]
        .as_table()
        .ok_or("schedule foundation: config template agent must be a table")?;
    exact_toml_keys(
        agent,
        &[
            "id",
            "kind",
            "cmd",
            "pre_authenticated",
            "model",
            "effort",
            "mode",
            "args",
            "base_url",
            "api_key_env",
            "sandbox",
        ],
        "agent",
    )?;
    let kind = optional_toml_string(agent, "kind", "agent")?.unwrap_or_else(|| "acp".into());
    let command_family = optional_toml_string(agent, "cmd", "agent")?;
    let base_url = optional_toml_string(agent, "base_url", "agent")?;
    let api_key_env = optional_toml_string(agent, "api_key_env", "agent")?;
    if api_key_env.as_deref().is_some_and(|name| !env_name(name)) {
        return Err("schedule foundation: config template api_key_env is invalid".into());
    }
    let pre_authenticated = agent
        .get("pre_authenticated")
        .map(|value| {
            value.as_bool().ok_or_else(|| -> BoxError {
                "schedule foundation: agent.pre_authenticated must be a boolean".into()
            })
        })
        .transpose()?
        .unwrap_or(false);
    let args = toml_string_array(agent, "args", "agent")?;
    let allowed_cwd_root = optional_toml_string(root, "allowed_cwd_root", "config")?;
    let sandbox = agent
        .get("sandbox")
        .map(|value| -> Result<CanonicalSandboxTemplateV1, BoxError> {
            let table = value
                .as_table()
                .ok_or("schedule foundation: agent.sandbox must be a table")?;
            exact_toml_keys(
                table,
                &[
                    "image", "mount", "access", "egress", "network", "proxy", "volumes",
                ],
                "agent.sandbox",
            )?;
            let required = |field: &str| -> Result<String, BoxError> {
                optional_toml_string(table, field, "agent.sandbox")?.ok_or_else(|| {
                    format!("schedule foundation: agent.sandbox.{field} is required").into()
                })
            };
            let image_family = image_family
                .ok_or("schedule foundation: sandbox config requires an image-family binding")?;
            let image = required("image")?;
            if !image.starts_with("sha256:") || !local_file::valid_sha256(&image[7..]) {
                return Err(
                    "schedule foundation: sandbox image must be an immutable digest".into(),
                );
            }
            let mut volumes = toml_string_array(table, "volumes", "agent.sandbox")?;
            volumes.sort();
            if volumes.windows(2).any(|pair| pair[0] == pair[1]) {
                return Err("schedule foundation: sandbox volumes must be unique".into());
            }
            Ok(CanonicalSandboxTemplateV1 {
                image_family: image_family.to_owned(),
                mount: required("mount")?,
                access: required("access")?,
                egress: required("egress")?,
                network: required("network")?,
                proxy: required("proxy")?,
                volumes,
            })
        })
        .transpose()?;
    if sandbox.is_none() && image_family.is_some() {
        return Err("schedule foundation: image-family binding requires a sandbox config".into());
    }
    let mut allowed_commands = root
        .get("registry")
        .and_then(toml::Value::as_table)
        .map(|table| {
            exact_toml_keys(table, &["allowed_cmds"], "registry")?;
            toml_string_array(table, "allowed_cmds", "registry")
        })
        .transpose()?
        .unwrap_or_default();
    if let Some(server) = root.get("server").and_then(toml::Value::as_table) {
        exact_toml_keys(server, &["addr"], "server")?;
    }
    allowed_commands.sort();
    if allowed_commands.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("schedule foundation: registry allowed commands must be unique".into());
    }
    let projection = CanonicalConfigTemplateV1 {
        schema_version: 1,
        template_id: template_id.to_owned(),
        kind,
        command_family,
        base_url,
        api_key_env,
        pre_authenticated,
        args,
        allowed_cwd_root,
        sandbox,
        allowed_commands,
    };
    canonical_hash("config template", &projection)
}

fn validate_policy(policy: &SchedulePolicyV1) -> Result<(), BoxError> {
    if policy.schema_version != 1 {
        return Err("schedule foundation: policy schema_version must be 1".into());
    }
    stable_id("policy.environment_owner", &policy.environment_owner)?;
    bounded_text("policy.repository", &policy.repository)?;
    stable_id(
        "policy.fixed_prompt_contract",
        &policy.fixed_prompt_contract,
    )?;
    stable_id("policy.artifact_template", &policy.artifact_template)?;
    stable_id(
        "policy.price_ranking_authority",
        &policy.price_ranking_authority,
    )?;
    if policy.price_snapshot_max_age_secs == 0
        || policy.price_snapshot_max_age_secs > 31 * 24 * 60 * 60
    {
        return Err(
            "schedule foundation: price_snapshot_max_age_secs must be in 1..=2678400".into(),
        );
    }
    for (label, path) in [
        ("scheduled_registry", &policy.scheduled_registry),
        (
            "characterization_inventory",
            &policy.characterization_inventory,
        ),
        ("production_manifest", &policy.production_manifest),
        ("floating_recipes", &policy.floating_recipes),
    ] {
        validate_relative_path(label, path)?;
    }
    let triggers = policy
        .allowed_triggers
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if triggers.len() != policy.allowed_triggers.len()
        || triggers
            != BTreeSet::from([
                TriggerKindV1::ManualCharacterization,
                TriggerKindV1::ManualCompatibility,
                TriggerKindV1::Daily,
                TriggerKindV1::ScheduledMain,
                TriggerKindV1::TestMerge,
            ])
    {
        return Err(
            "schedule foundation: policy must name each approved trigger exactly once".into(),
        );
    }
    let effects = policy
        .allowed_effects
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if effects.len() != policy.allowed_effects.len()
        || effects
            != BTreeSet::from([
                EffectClassV1::ProviderPrompt,
                EffectClassV1::RegistryRead,
                EffectClassV1::ImageInspect,
                EffectClassV1::ImageBuild,
            ])
    {
        return Err(
            "schedule foundation: policy must name each approved effect exactly once".into(),
        );
    }
    policy.profile_maxima.validate("policy.profile_maxima")?;
    if policy.deferred_profiles.len() > MAX_DEFERRED_PROFILES {
        return Err("schedule foundation: too many deferred profile records".into());
    }
    for (index, value) in policy.deferred_profiles.iter().enumerate() {
        bounded_text(&format!("deferred_profiles[{index}]"), value)?;
    }
    validate_storage_policy(&policy.storage)?;
    Ok(())
}

fn validate_storage_policy(storage: &StoragePolicyV1) -> Result<(), BoxError> {
    if storage.hot_root != "~/Library/Application Support/a2a-bridge/operator/evidence"
        || storage.cold_root != "~/Documents/a2a-bridge/evidence-archive"
    {
        return Err(
            "schedule foundation: storage roots do not match the owner-approved paths".into(),
        );
    }
    if storage.hot_cap_bytes != 10 * 1024 * 1024 * 1024
        || storage.hot_index_cap_bytes != 1024 * 1024 * 1024
        || storage.hot_scratch_cap_bytes != 4 * 1024 * 1024 * 1024
        || storage.hot_sealed_cap_bytes != 5 * 1024 * 1024 * 1024
        || storage.hot_index_cap_bytes
            + storage.hot_scratch_cap_bytes
            + storage.hot_sealed_cap_bytes
            != storage.hot_cap_bytes
        || storage.cold_cap_bytes != 25 * 1024 * 1024 * 1024
    {
        return Err(
            "schedule foundation: storage caps do not match the approved 10/25 GiB allocation"
                .into(),
        );
    }
    Ok(())
}

fn validate_registry(
    registry: &ScheduledCaseRegistryV1,
    policy: &SchedulePolicyV1,
) -> Result<(), BoxError> {
    if registry.schema_version != 1 {
        return Err("schedule foundation: scheduled registry schema_version must be 1".into());
    }
    if registry.cases.is_empty() || registry.cases.len() > MAX_CASES {
        return Err(format!(
            "schedule foundation: scheduled registry must contain 1..={MAX_CASES} cases"
        )
        .into());
    }
    let mut ids = BTreeSet::new();
    for case in &registry.cases {
        validate_scheduled_case(case, policy)?;
        if !ids.insert(case.id.as_str()) {
            return Err(format!(
                "schedule foundation: duplicate scheduled case id {:?}",
                case.id
            )
            .into());
        }
    }
    Ok(())
}

fn validate_scheduled_case(
    case: &ScheduledCaseV1,
    policy: &SchedulePolicyV1,
) -> Result<(), BoxError> {
    stable_id("scheduled case id", &case.id)?;
    stable_id("scheduled provider_family", &case.provider_family)?;
    stable_id("scheduled capability", &case.capability)?;
    stable_id("scheduled agent", &case.agent)?;
    bounded_text("scheduled model", &case.model)?;
    bounded_text(
        "scheduled expected_effective_model",
        &case.expected_effective_model,
    )?;
    if case.model != case.expected_effective_model
        || case.effort != case.expected_effective_effort
        || case.mode != case.expected_effective_mode
    {
        return Err(format!(
            "schedule foundation: case {:?} proposed and expected-effective identities must initially match",
            case.id
        )
        .into());
    }
    if case.evidence_purpose != EvidencePurposeV1::ProviderPathAdvisory {
        return Err(format!(
            "schedule foundation: case {:?} is not a provider-path advisory profile",
            case.id
        )
        .into());
    }
    if case.environment_owner != policy.environment_owner {
        return Err(format!(
            "schedule foundation: case {:?} has the wrong environment owner",
            case.id
        )
        .into());
    }
    if case.os != "macos" || case.architecture != "aarch64" || !case.session_cwd.is_absolute() {
        return Err(format!(
            "schedule foundation: case {:?} has an unsupported environment or relative session cwd",
            case.id
        )
        .into());
    }
    validate_relative_path("scheduled case config", &case.config)?;
    stable_id("scheduled config_template", &case.config_template)?;
    bounded_text("scheduled adapter_family", &case.adapter_family)?;
    bounded_text("scheduled agent_cli_family", &case.agent_cli_family)?;
    if let Some(image_family) = &case.image_family {
        stable_id("scheduled image_family", image_family)?;
    }
    if case.required_env.len() > MAX_REQUIRED_ENV {
        return Err(format!(
            "schedule foundation: case {:?} has too many required environment entries",
            case.id
        )
        .into());
    }
    let mut envs = BTreeSet::new();
    for required in &case.required_env {
        if !env_name(&required.name) || !envs.insert(required.name.as_str()) {
            return Err(format!(
                "schedule foundation: case {:?} has an invalid or duplicate required environment name",
                case.id
            )
            .into());
        }
        let values = required.one_of.iter().collect::<BTreeSet<_>>();
        if values.len() != required.one_of.len() {
            return Err(format!(
                "schedule foundation: case {:?} repeats a required environment value",
                case.id
            )
            .into());
        }
        for value in &required.one_of {
            bounded_text("required environment value", value)?;
        }
    }
    match (&case.auth_path, &case.credential_env) {
        (ScheduleAuthPathV1::ApiKeyEnv, Some(name)) if env_name(name) => {}
        (ScheduleAuthPathV1::ApiKeyEnv, _) => {
            return Err(format!(
                "schedule foundation: API-key case {:?} requires one credential environment name",
                case.id
            )
            .into())
        }
        (_, None) => {}
        _ => {
            return Err(format!(
                "schedule foundation: non-API-key case {:?} must not declare credential_env",
                case.id
            )
            .into())
        }
    }
    let effects = case
        .allowed_effects
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if effects.len() != case.allowed_effects.len()
        || !effects.contains(&EffectClassV1::ProviderPrompt)
        || !effects.is_subset(&policy.allowed_effects.iter().copied().collect())
    {
        return Err(format!(
            "schedule foundation: case {:?} has invalid or duplicate effect classes",
            case.id
        )
        .into());
    }
    match case.execution_mode {
        ScheduleExecutionModeV1::ContainerRo if case.image_family.is_none() => {
            return Err(format!(
                "schedule foundation: reader case {:?} requires an image family",
                case.id
            )
            .into())
        }
        ScheduleExecutionModeV1::Host | ScheduleExecutionModeV1::RemoteApi
            if case.image_family.is_some() =>
        {
            return Err(format!(
                "schedule foundation: non-reader case {:?} must not declare an image family",
                case.id
            )
            .into())
        }
        _ => {}
    }
    if case.execution_mode == ScheduleExecutionModeV1::RemoteApi && case.resolution_case.is_some() {
        return Err(format!(
            "schedule foundation: static API case {:?} must not claim an R3c resolution case",
            case.id
        )
        .into());
    }
    if let Some(resolution_case) = &case.resolution_case {
        stable_id("scheduled resolution_case", resolution_case)?;
    }
    case.caps.validate(&format!("case {:?} caps", case.id))?;
    case.caps
        .within(&policy.profile_maxima, &format!("case {:?} caps", case.id))?;
    if case.artifact.retention_days == 0 || case.artifact.retention_days > 365 {
        return Err(format!(
            "schedule foundation: case {:?} retention_days must be in 1..=365",
            case.id
        )
        .into());
    }
    Ok(())
}

fn validate_config(root: &Path, case: &ScheduledCaseV1) -> Result<(String, toml::Value), BoxError> {
    let path = root.join(&case.config);
    let snapshot = local_file::read_regular_file_bounded(
        &path,
        &format!("scheduled config {:?}", case.id),
        MAX_CONFIG_BYTES,
    )?;
    ensure_snapshot_within_root(
        root,
        &snapshot.canonical_path,
        &format!("scheduled config {:?}", case.id),
    )?;
    let text = std::str::from_utf8(&snapshot.bytes)
        .map_err(|_| format!("schedule foundation: config {:?} must be UTF-8", case.id))?;
    validate_runtime_config(text, &format!("config {:?}", case.id))?;
    let value: toml::Value = toml::from_str(text)
        .map_err(|error| format!("schedule foundation: invalid config {:?}: {error}", case.id))?;
    let table = value.as_table().ok_or_else(|| {
        format!(
            "schedule foundation: config {:?} must be a TOML table",
            case.id
        )
    })?;
    if table.get("default").and_then(toml::Value::as_str) != Some(case.agent.as_str()) {
        return Err(format!(
            "schedule foundation: config {:?} default agent does not match the registry",
            case.id
        )
        .into());
    }
    let agents = table
        .get("agents")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| {
            format!(
                "schedule foundation: config {:?} must have one agent",
                case.id
            )
        })?;
    if agents.len() != 1 {
        return Err(format!(
            "schedule foundation: config {:?} must have exactly one agent",
            case.id
        )
        .into());
    }
    let agent = agents[0].as_table().ok_or_else(|| {
        format!(
            "schedule foundation: config {:?} agent must be a table",
            case.id
        )
    })?;
    let field = |name: &str| agent.get(name).and_then(toml::Value::as_str);
    if field("id") != Some(case.agent.as_str())
        || field("model") != Some(case.model.as_str())
        || field("effort") != case.effort.as_deref()
        || field("mode") != case.mode.as_deref()
    {
        return Err(format!(
            "schedule foundation: config {:?} agent/model/effort/mode differs from its registry row",
            case.id
        )
        .into());
    }
    let kind = field("kind").unwrap_or("acp");
    match case.execution_mode {
        ScheduleExecutionModeV1::RemoteApi if kind != "api" || agent.contains_key("sandbox") => {
            return Err(format!(
                "schedule foundation: remote API config {:?} must be an unsandboxed API agent",
                case.id
            )
            .into())
        }
        ScheduleExecutionModeV1::ContainerRo => {
            let sandbox = agent
                .get("sandbox")
                .and_then(toml::Value::as_table)
                .ok_or_else(|| {
                    format!(
                        "schedule foundation: reader config {:?} requires a sandbox",
                        case.id
                    )
                })?;
            if kind != "acp"
                || sandbox.get("access").and_then(toml::Value::as_str) != Some("ro")
                || !sandbox
                    .get("image")
                    .and_then(toml::Value::as_str)
                    .is_some_and(|image| {
                        image.starts_with("sha256:")
                            && image.len() == 71
                            && image[7..].bytes().all(|byte| byte.is_ascii_hexdigit())
                    })
            {
                return Err(format!(
                    "schedule foundation: reader config {:?} must use ACP, read-only access, and an immutable image",
                    case.id
                )
                .into());
            }
        }
        ScheduleExecutionModeV1::Host if kind != "acp" || agent.contains_key("sandbox") => {
            return Err(format!(
                "schedule foundation: host config {:?} must be an unsandboxed ACP agent",
                case.id
            )
            .into())
        }
        _ => {}
    }
    let template_sha256 = canonical_config_template_hash(
        &case.config_template,
        case.image_family.as_deref(),
        &value,
    )?;
    Ok((template_sha256, value))
}

fn scheduled_profile(
    policy: &SchedulePolicyV1,
    case: &ScheduledCaseV1,
    config_sha256: String,
) -> CanonicalProfileInputV1 {
    CanonicalProfileInputV1 {
        schema_version: 1,
        source_kind: ProfileSourceKindV1::ScheduledAdvisory,
        source_id: case.id.clone(),
        repository: policy.repository.clone(),
        source_schema_version: 1,
        lane: "floating-current".into(),
        classification: "canary".into(),
        evidence_purpose: case.evidence_purpose,
        execution_mode: match case.execution_mode {
            ScheduleExecutionModeV1::Host => "host",
            ScheduleExecutionModeV1::ContainerRo => "container_ro",
            ScheduleExecutionModeV1::RemoteApi => "remote_api",
        }
        .into(),
        provider_family: case.provider_family.clone(),
        agent: case.agent.clone(),
        capability: case.capability.clone(),
        adapter_family: case.adapter_family.clone(),
        agent_cli_family: case.agent_cli_family.clone(),
        image_family: case.image_family.clone(),
        auth_path: match case.auth_path {
            ScheduleAuthPathV1::ApiKeyEnv => "api_key_env",
            ScheduleAuthPathV1::PreAuthenticated => "pre_authenticated",
            ScheduleAuthPathV1::Automatic => "automatic",
        }
        .into(),
        credential_env_name: case.credential_env.clone(),
        required_env: case.required_env.clone(),
        environment_owner: case.environment_owner.clone(),
        os: case.os.clone(),
        architecture: case.architecture.clone(),
        session_cwd: case.session_cwd.to_string_lossy().into_owned(),
        requested_model: case.model.clone(),
        requested_effort: case.effort.clone(),
        requested_mode: case.mode.clone(),
        expected_effective_model: case.expected_effective_model.clone(),
        expected_effective_effort: case.expected_effective_effort.clone(),
        expected_effective_mode: case.expected_effective_mode.clone(),
        config_template: case.config_template.clone(),
        config_template_sha256: config_sha256,
        resolution_constraint: case.resolution_case.clone(),
        fixed_prompt_contract: policy.fixed_prompt_contract.clone(),
        artifact_template: policy.artifact_template.clone(),
        artifact_retention_days: case.artifact.retention_days,
        artifact_redaction: "strict".into(),
        maximum_caps: case.caps.clone(),
    }
}

fn support_profiles(
    policy: &SchedulePolicyV1,
    root: &Path,
    manifest_bytes: &[u8],
) -> Result<Vec<CanonicalProfileInputV1>, BoxError> {
    let text = std::str::from_utf8(manifest_bytes)
        .map_err(|_| "schedule foundation: production manifest must be UTF-8")?;
    let manifest: toml::Value = toml::from_str(text)
        .map_err(|error| format!("schedule foundation: invalid production manifest: {error}"))?;
    let cases = manifest
        .get("cases")
        .and_then(toml::Value::as_array)
        .ok_or("schedule foundation: production manifest has no cases")?;
    let default_max_cost_usd = manifest
        .get("budget")
        .and_then(toml::Value::as_table)
        .and_then(|budget| budget.get("max_cost_usd"))
        .and_then(|value| {
            value
                .as_float()
                .or_else(|| value.as_integer().map(|value| value as f64))
        })
        .ok_or("schedule foundation: production manifest has no cost ceiling")?;
    let mut result = Vec::new();
    for case in cases {
        let table = case
            .as_table()
            .ok_or("schedule foundation: production case must be a table")?;
        if table.get("classification").and_then(toml::Value::as_str) != Some("support") {
            continue;
        }
        result.push(support_profile(policy, root, table, default_max_cost_usd)?);
    }
    Ok(result)
}

fn support_profile(
    policy: &SchedulePolicyV1,
    root: &Path,
    case: &toml::map::Map<String, toml::Value>,
    default_max_cost_usd: f64,
) -> Result<CanonicalProfileInputV1, BoxError> {
    let required = |name: &str| -> Result<String, BoxError> {
        case.get(name)
            .and_then(toml::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("schedule foundation: support case missing {name}").into())
    };
    let optional = |name: &str| {
        case.get(name)
            .and_then(toml::Value::as_str)
            .map(str::to_owned)
    };
    let id = required("id")?;
    let config = PathBuf::from(required("config")?);
    validate_relative_path("support config", &config)?;
    let config_snapshot = local_file::read_regular_file_bounded(
        &root.join(&config),
        &format!("support config {id:?}"),
        MAX_CONFIG_BYTES,
    )?;
    ensure_snapshot_within_root(
        root,
        &config_snapshot.canonical_path,
        &format!("support config {id:?}"),
    )?;
    let config_text = std::str::from_utf8(&config_snapshot.bytes)
        .map_err(|_| format!("schedule foundation: support config {id:?} must be UTF-8"))?;
    validate_runtime_config(config_text, &format!("support config {id:?}"))?;
    let config_value: toml::Value = toml::from_str(config_text)
        .map_err(|error| format!("schedule foundation: invalid support config {id:?}: {error}"))?;
    let pins = case
        .get("pins")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("schedule foundation: support case {id:?} has no pins"))?;
    let package_family = |name: &str| -> Result<String, BoxError> {
        let value = pins
            .get(name)
            .and_then(toml::Value::as_str)
            .ok_or_else(|| format!("schedule foundation: support case {id:?} lacks {name}"))?;
        value
            .split_once('=')
            .map(|(package, _)| package.to_owned())
            .ok_or_else(|| {
                format!("schedule foundation: support case {id:?} has invalid {name}").into()
            })
    };
    let execution_mode = required("execution_mode")?;
    let agent = required("agent")?;
    let provider_family = if agent.starts_with("codex") {
        "openai-codex"
    } else if agent.starts_with("claude") {
        "anthropic-claude"
    } else {
        return Err(
            format!("schedule foundation: unsupported claimed-support agent {agent:?}").into(),
        );
    };
    let required_env = case
        .get("required_env")
        .cloned()
        .map(|value| value.try_into::<Vec<RequiredEnvironmentV1>>())
        .transpose()
        .map_err(|error| format!("schedule foundation: invalid support required_env: {error}"))?
        .unwrap_or_default();
    let artifact = case
        .get("artifact")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| {
            format!("schedule foundation: support case {id:?} has no artifact policy")
        })?;
    let retention_days = artifact
        .get("retention_days")
        .and_then(toml::Value::as_integer)
        .and_then(|value| u16::try_from(value).ok())
        .ok_or_else(|| format!("schedule foundation: support case {id:?} has invalid retention"))?;
    let cost_microusd = case
        .get("max_cost_usd")
        .and_then(|value| {
            value
                .as_float()
                .or_else(|| value.as_integer().map(|value| value as f64))
        })
        .or(Some(default_max_cost_usd))
        .map(|cost| (cost * 1_000_000.0).round() as u64)
        .expect("default production-manifest cost is required");
    let caps = EffectCapsV1 {
        timeout_secs: case
            .get("timeout_secs")
            .and_then(toml::Value::as_integer)
            .and_then(|value| u64::try_from(value).ok())
            .ok_or_else(|| {
                format!("schedule foundation: support case {id:?} has invalid timeout")
            })?,
        max_tokens: case
            .get("max_tokens")
            .and_then(toml::Value::as_integer)
            .and_then(|value| u64::try_from(value).ok())
            .ok_or_else(|| {
                format!("schedule foundation: support case {id:?} has invalid token cap")
            })?,
        max_cost_microusd: cost_microusd,
        attempts: 1,
        retry_cap: case
            .get("retry_cap")
            .and_then(toml::Value::as_integer)
            .and_then(|value| u8::try_from(value).ok())
            .ok_or_else(|| {
                format!("schedule foundation: support case {id:?} has invalid retry cap")
            })?,
        fallback_cap: 0,
    };
    caps.validate(&format!("support case {id:?} caps"))?;
    caps.within(&policy.profile_maxima, &format!("support case {id:?} caps"))?;
    let model = required("model")?;
    let adapter_family = package_family("adapter")?;
    let agent_cli_family = package_family("agent_cli")?;
    let config_template = match (provider_family, execution_mode.as_str()) {
        ("openai-codex", "host") => "codex-host-read-only-v1",
        ("openai-codex", "container_ro") => "codex-reader-read-only-v1",
        ("anthropic-claude", "host") => "claude-host-read-only-v1",
        ("anthropic-claude", "container_ro") => "claude-reader-read-only-v1",
        _ => {
            return Err(format!(
                "schedule foundation: support case {id:?} has no approved config template"
            )
            .into())
        }
    };
    let image_family = (execution_mode == "container_ro").then(|| "node-acp-reader-v1".into());
    let config_template_sha256 =
        canonical_config_template_hash(config_template, image_family.as_deref(), &config_value)?;
    Ok(CanonicalProfileInputV1 {
        schema_version: 1,
        source_kind: ProfileSourceKindV1::ClaimedSupportGate,
        source_id: id,
        repository: policy.repository.clone(),
        source_schema_version: 1,
        lane: required("lane")?,
        classification: "support".into(),
        evidence_purpose: EvidencePurposeV1::ClaimedSupportGate,
        execution_mode: execution_mode.clone(),
        provider_family: provider_family.into(),
        agent,
        capability: "claimed-support".into(),
        adapter_family,
        agent_cli_family,
        image_family,
        auth_path: required("auth_path")?,
        credential_env_name: optional("credential_env"),
        required_env,
        environment_owner: required("environment_owner")?,
        os: required("os")?,
        architecture: required("architecture")?,
        session_cwd: optional("session_cwd").unwrap_or_else(|| "not_applicable".into()),
        requested_model: model.clone(),
        requested_effort: optional("effort"),
        requested_mode: optional("mode"),
        expected_effective_model: model,
        expected_effective_effort: optional("effort"),
        expected_effective_mode: optional("mode"),
        config_template: config_template.into(),
        config_template_sha256,
        resolution_constraint: Some("pinned-exact-within-profile".into()),
        fixed_prompt_contract: policy.fixed_prompt_contract.clone(),
        artifact_template: policy.artifact_template.clone(),
        artifact_retention_days: retention_days,
        artifact_redaction: artifact
            .get("redaction")
            .and_then(toml::Value::as_str)
            .unwrap_or("invalid")
            .to_owned(),
        maximum_caps: caps,
    })
}

fn validate_inventory(
    inventory: &CharacterizationProfileInventoryV1,
    expected: &BTreeMap<(ProfileSourceKindV1, String), String>,
) -> Result<(), BoxError> {
    if inventory.schema_version != 1 {
        return Err(
            "schedule foundation: characterization inventory schema_version must be 1".into(),
        );
    }
    let mut observed = BTreeMap::new();
    let mut ids = BTreeSet::new();
    for profile in &inventory.profiles {
        stable_id("inventory profile id", &profile.id)?;
        stable_id("inventory source id", &profile.source_id)?;
        validate_sha256("inventory profile_sha256", &profile.profile_sha256)?;
        if profile.id != profile.source_id {
            return Err(format!(
                "schedule foundation: inventory profile {:?} must retain its exact source id",
                profile.id
            )
            .into());
        }
        if !ids.insert(profile.id.as_str()) {
            return Err(format!(
                "schedule foundation: duplicate inventory id {:?}",
                profile.id
            )
            .into());
        }
        let key = (profile.source_kind, profile.source_id.clone());
        if observed
            .insert(key, profile.profile_sha256.clone())
            .is_some()
        {
            return Err("schedule foundation: duplicate inventory source reference".into());
        }
    }
    if observed.keys().collect::<BTreeSet<_>>() != expected.keys().collect::<BTreeSet<_>>() {
        let missing = expected
            .keys()
            .filter(|key| !observed.contains_key(*key))
            .map(|(_, id)| id.as_str())
            .collect::<Vec<_>>();
        let extra = observed
            .keys()
            .filter(|key| !expected.contains_key(*key))
            .map(|(_, id)| id.as_str())
            .collect::<Vec<_>>();
        return Err(format!(
            "schedule foundation: characterization inventory mismatch (missing={missing:?}, extra={extra:?})"
        )
        .into());
    }
    let mismatches = expected
        .iter()
        .filter_map(|(key, expected_hash)| {
            let actual = observed.get(key)?;
            (actual != expected_hash).then(|| {
                format!(
                    "{}:{} expected {} but found {}",
                    match key.0 {
                        ProfileSourceKindV1::ScheduledAdvisory => "scheduled_advisory",
                        ProfileSourceKindV1::ClaimedSupportGate => "claimed_support_gate",
                    },
                    key.1,
                    expected_hash,
                    actual
                )
            })
        })
        .collect::<Vec<_>>();
    if !mismatches.is_empty() {
        return Err(format!(
            "schedule foundation: characterization fingerprint mismatch: {}",
            mismatches.join("; ")
        )
        .into());
    }
    Ok(())
}

pub(super) fn load_schedule_foundation(root: &Path) -> Result<LoadedScheduleFoundation, BoxError> {
    if root.as_os_str().is_empty() {
        return Err("schedule foundation: root must be non-empty".into());
    }
    let root = std::fs::canonicalize(root)
        .map_err(|error| format!("schedule foundation: cannot canonicalize root: {error}"))?;
    if !root.is_dir() {
        return Err("schedule foundation: root must be a directory".into());
    }
    let policy = load_foundation_toml::<SchedulePolicyV1>(
        &root,
        &root.join("scheduling-policy.toml"),
        "scheduling policy",
    )?;
    validate_policy(&policy.value)?;
    let registry = load_foundation_toml::<ScheduledCaseRegistryV1>(
        &root,
        &root.join(&policy.value.scheduled_registry),
        "scheduled case registry",
    )?;
    validate_registry(&registry.value, &policy.value)?;
    let inventory = load_foundation_toml::<CharacterizationProfileInventoryV1>(
        &root,
        &root.join(&policy.value.characterization_inventory),
        "characterization profile inventory",
    )?;

    let (
        production_manifest_bytes,
        _production_manifest_sha256,
        production_manifest_canonical_path,
    ) = compatibility::validated_manifest_snapshot(&root.join(&policy.value.production_manifest))?;
    ensure_snapshot_within_root(
        &root,
        &production_manifest_canonical_path,
        "production manifest",
    )?;
    let floating = local_file::read_regular_file_bounded(
        &root.join(&policy.value.floating_recipes),
        "floating recipes",
        MAX_FOUNDATION_FILE_BYTES,
    )?;
    ensure_snapshot_within_root(&root, &floating.canonical_path, "floating recipes")?;

    let mut expected = BTreeMap::new();
    let mut scheduled_hashes = BTreeMap::new();
    let mut config_hashes = BTreeMap::new();
    for case in &registry.value.cases {
        let (config_sha256, _) = validate_config(&root, case)?;
        config_hashes.insert(
            case.config.to_string_lossy().into_owned(),
            config_sha256.clone(),
        );
        let profile = scheduled_profile(&policy.value, case, config_sha256);
        let hash = canonical_hash("scheduled characterization profile", &profile)?;
        expected.insert(
            (ProfileSourceKindV1::ScheduledAdvisory, case.id.clone()),
            hash.clone(),
        );
        scheduled_hashes.insert(case.id.clone(), hash);
    }

    let support = support_profiles(&policy.value, &root, &production_manifest_bytes)?;
    let support_ids = support
        .iter()
        .map(|profile| profile.source_id.as_str())
        .collect::<BTreeSet<_>>();
    if support_ids != EXPECTED_SUPPORT_PROFILES.into_iter().collect() {
        return Err(format!(
            "schedule foundation: claimed-support inventory changed: {support_ids:?}"
        )
        .into());
    }
    let mut support_hashes = BTreeMap::new();
    for profile in support {
        config_hashes.insert(
            profile.config_template.clone(),
            profile.config_template_sha256.clone(),
        );
        let hash = canonical_hash("claimed-support characterization profile", &profile)?;
        expected.insert(
            (
                ProfileSourceKindV1::ClaimedSupportGate,
                profile.source_id.clone(),
            ),
            hash.clone(),
        );
        support_hashes.insert(profile.source_id, hash);
    }
    validate_inventory(&inventory.value, &expected)?;

    let bundle = ProfilePolicyBundleInputV1 {
        schema_version: 1,
        policy_sha256: policy.sha256,
        registry_sha256: registry.sha256,
        inventory_sha256: inventory.sha256,
        floating_recipes_sha256: floating.sha256,
        scheduled_profiles: scheduled_hashes.clone(),
        claimed_support_profiles: support_hashes.clone(),
        config_templates: config_hashes,
        allowed_effects: policy.value.allowed_effects,
        profile_maxima: policy.value.profile_maxima,
        fixed_prompt_contract: policy.value.fixed_prompt_contract,
        artifact_template: policy.value.artifact_template,
    };
    let bundle_sha256 = canonical_hash("profile policy bundle", &bundle)?;
    Ok(LoadedScheduleFoundation {
        scheduled_profile_count: scheduled_hashes.len(),
        claimed_support_profile_count: support_hashes.len(),
        profile_policy_bundle_sha256: bundle_sha256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../compatibility")
    }

    #[test]
    fn checked_in_foundation_has_no_effecting_entrypoint_and_exact_profile_sets() {
        let loaded = load_schedule_foundation(&fixture_root()).unwrap();
        assert_eq!(loaded.scheduled_profile_count, 6);
        assert_eq!(loaded.claimed_support_profile_count, 4);
        assert!(local_file::valid_sha256(
            &loaded.profile_policy_bundle_sha256
        ));
    }

    #[test]
    fn caps_reject_retry_fallback_and_profile_maximum_overrun() {
        let maximum = EffectCapsV1 {
            timeout_secs: 100,
            max_tokens: 100,
            max_cost_microusd: 100,
            attempts: 1,
            retry_cap: 0,
            fallback_cap: 0,
        };
        let retry = EffectCapsV1 {
            retry_cap: 1,
            ..maximum.clone()
        };
        assert!(retry
            .validate("retry")
            .unwrap_err()
            .to_string()
            .contains("retry/fallback zero"));
        let widened = EffectCapsV1 {
            timeout_secs: 101,
            ..maximum.clone()
        };
        assert!(widened.within(&maximum, "widened").is_err());
    }
}
