//! Typed, provider-free provider-refresh planning and evidence checking.
//!
//! Slice A deliberately has no promotion executor. It compiles already-resolved identities and
//! validates captured non-prompt evidence. Runtime effects, operator restart, and billable turns
//! remain separate authorities.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{local_file, BoxError};

const MAX_INPUT_BYTES: u64 = 1024 * 1024;
const MAX_BOUND_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_COMPONENTS: usize = 64;
const MAX_MODELS: usize = 256;
const MAX_OPERATIONS: usize = 32;
const MAX_CHECKS: usize = 32;

pub(crate) const USAGE: &str = "\
usage: a2a-bridge provider-refresh plan --request <absolute-json> --out <absolute-new-json>
       a2a-bridge provider-refresh check --plan <absolute-json> --evidence <absolute-json>
                                           --out <absolute-new-json>

`plan` compiles already-resolved exact provider identities without a subprocess, registry lookup,
provider session, or production mutation. `check` validates captured provider-free artifacts and
does not spawn a provider. Slice A does not authorize a billable turn or operator restart.

Typed slice A does not implement `promote`. Its plan contains closed declarative promotion
operations for later independently reviewed slices; `provider-refresh promote` fails closed.";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Provider {
    Codex,
    Claude,
    Kiro,
    Opencode,
    Openrouter,
}

impl Provider {
    fn label(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Kiro => "kiro",
            Self::Opencode => "opencode",
            Self::Openrouter => "openrouter",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum ComponentKind {
    CodexAcpAdapter,
    CodexNestedCli,
    CodexStandaloneCli,
    ClaudeAcpAdapter,
    ClaudeAgentSdk,
    ClaudeBundledCli,
    ClaudeStandaloneCli,
    OpencodeCli,
    KiroCli,
}

impl ComponentKind {
    fn provider(self) -> Provider {
        match self {
            Self::CodexAcpAdapter | Self::CodexNestedCli | Self::CodexStandaloneCli => {
                Provider::Codex
            }
            Self::ClaudeAcpAdapter
            | Self::ClaudeAgentSdk
            | Self::ClaudeBundledCli
            | Self::ClaudeStandaloneCli => Provider::Claude,
            Self::OpencodeCli => Provider::Opencode,
            Self::KiroCli => Provider::Kiro,
        }
    }

    fn npm_package(self) -> Option<&'static str> {
        match self {
            Self::CodexAcpAdapter => Some("@agentclientprotocol/codex-acp"),
            Self::CodexNestedCli => Some("@openai/codex"),
            Self::ClaudeAcpAdapter => Some("@agentclientprotocol/claude-agent-acp"),
            Self::ClaudeAgentSdk => Some("@anthropic-ai/claude-agent-sdk"),
            Self::OpencodeCli => Some("opencode-ai"),
            Self::CodexStandaloneCli
            | Self::ClaudeBundledCli
            | Self::ClaudeStandaloneCli
            | Self::KiroCli => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ManagedInstaller {
    Homebrew,
    Mise,
    NativeUpdater,
    Npm,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum KiroArchitecture {
    Aarch64AppleDarwin,
    X86_64AppleDarwin,
    Aarch64LinuxMusl,
    X86_64LinuxMusl,
}

impl KiroArchitecture {
    fn archive_name(self) -> &'static str {
        match self {
            Self::Aarch64AppleDarwin => "kirocli-aarch64-apple-darwin.zip",
            Self::X86_64AppleDarwin => "kirocli-x86_64-apple-darwin.zip",
            Self::Aarch64LinuxMusl => "kirocli-aarch64-linux-musl.zip",
            Self::X86_64LinuxMusl => "kirocli-x86_64-linux-musl.zip",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ComponentSource {
    Npm {
        package: String,
        tarball_url: String,
        size_bytes: u64,
        integrity: String,
    },
    KiroStableArchive {
        architecture: KiroArchitecture,
        url: String,
        size_bytes: u64,
        sha256: String,
    },
    ManagedExecutable {
        manager: ManagedInstaller,
        package: String,
        path: PathBuf,
        size_bytes: u64,
        sha256: String,
    },
    BundledCli {
        parent: ComponentKind,
        parent_version: String,
        manifest_sha256: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Component {
    kind: ComponentKind,
    version: String,
    source: ComponentSource,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum TargetMode {
    Acp,
    DeferredCatalog,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ProviderTarget {
    provider: Provider,
    mode: TargetMode,
    source_binding: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default_model: Option<String>,
    selected_models: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
struct OpenRouterModel {
    model: String,
    prompt_price: String,
    completion_price: String,
    supports_tools: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum BindingRole {
    CandidateManifest,
    CatalogResolution,
    PromotionPayload,
    Production,
    Rollback,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct FileBinding {
    id: String,
    role: BindingRole,
    path: PathBuf,
    sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum PromotionOperation {
    AtomicFileReplace {
        id: String,
        owner_source_binding: String,
        candidate_binding: String,
        production_binding: String,
        rollback_binding: String,
    },
    OperatorRestartRequired {
        id: String,
        service_id: String,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum ManifestArtifactKind {
    Executable,
    PackageTreeManifest,
    Config,
    ImageReceipt,
    CatalogSnapshot,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ManifestArtifact {
    id: String,
    kind: ManifestArtifactKind,
    path: PathBuf,
    size_bytes: u64,
    sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ExecutionIdentity {
    Host {
        executable_artifact: String,
    },
    Container {
        image_artifact: String,
        immutable_id: String,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum SourceManifestKind {
    CandidateManifest,
    CatalogResolution,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct SourceManifest {
    schema_version: u32,
    kind: SourceManifestKind,
    provider: Provider,
    components: Vec<Component>,
    artifacts: Vec<ManifestArtifact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    execution: Option<ExecutionIdentity>,
    #[serde(default)]
    promotion_payload_bindings: Vec<String>,
}

impl PromotionOperation {
    fn id(&self) -> &str {
        match self {
            Self::AtomicFileReplace { id, .. } | Self::OperatorRestartRequired { id, .. } => id,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RefreshRequest {
    schema_version: u32,
    refresh_id: String,
    components: Vec<Component>,
    targets: Vec<ProviderTarget>,
    opencode_subscription_models: Vec<String>,
    openrouter_models: Vec<OpenRouterModel>,
    bindings: Vec<FileBinding>,
    promotion_operations: Vec<PromotionOperation>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum CheckKind {
    RawAcpInitialize,
    Doctor,
    Models,
    OpencodeCatalog,
    OpenrouterCatalog,
}

impl CheckKind {
    fn label(self) -> &'static str {
        match self {
            Self::RawAcpInitialize => "raw_acp_initialize",
            Self::Doctor => "doctor",
            Self::Models => "models",
            Self::OpencodeCatalog => "opencode_catalog",
            Self::OpenrouterCatalog => "openrouter_catalog",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RequiredCheck {
    id: String,
    provider: Provider,
    kind: CheckKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RefreshPlan {
    schema_version: u32,
    authority: String,
    promotion_ready: bool,
    plan_id: String,
    request_sha256: String,
    refresh_id: String,
    components: Vec<Component>,
    targets: Vec<ProviderTarget>,
    opencode_subscription_models: Vec<String>,
    openrouter_models: Vec<OpenRouterModel>,
    bindings: Vec<FileBinding>,
    promotion_operations: Vec<PromotionOperation>,
    required_checks: Vec<RequiredCheck>,
    deferred_components: Vec<ComponentKind>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckEvidence {
    schema_version: u32,
    plan_id: String,
    checks: Vec<EvidenceItem>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceItem {
    id: String,
    kind: CheckKind,
    artifact: PathBuf,
    sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CheckedEvidence {
    id: String,
    provider: Provider,
    kind: CheckKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent: Option<String>,
    artifact: PathBuf,
    sha256: String,
    status: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CheckReceipt {
    schema_version: u32,
    authority: String,
    promotion_ready: bool,
    check_id: String,
    plan_id: String,
    evidence_request_sha256: String,
    status: String,
    checks: Vec<CheckedEvidence>,
    deferred_components: Vec<ComponentKind>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceEnvelope {
    schema_version: u32,
    plan_id: String,
    provider: Provider,
    source_binding: String,
    source_sha256: String,
    kind: CheckKind,
    #[serde(default)]
    agent: Option<String>,
    prompt_calls: u64,
    session_created: bool,
    payload: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenCodeCatalogEvidence {
    provider: Provider,
    prompt_calls: u64,
    models: Vec<OpenCodeCatalogModel>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenCodeCatalogModel {
    id: String,
    subscription_included: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenRouterCatalogEvidence {
    provider: Provider,
    prompt_calls: u64,
    models: Vec<OpenRouterCatalogModel>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenRouterCatalogModel {
    id: String,
    prompt_price: String,
    completion_price: String,
    supports_tools: bool,
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn require_id(value: &str, label: &str) -> Result<(), BoxError> {
    if valid_id(value) {
        Ok(())
    } else {
        Err(format!("provider-refresh: {label} must be a bounded portable id").into())
    }
}

fn valid_bounded_string(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.as_bytes().contains(&0)
}

fn valid_npm_sri(value: &str) -> bool {
    value
        .strip_prefix("sha512-")
        .and_then(|encoded| {
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .ok()
        })
        .is_some_and(|decoded| decoded.len() == 64)
}

fn exact_version(value: &str, label: &str) -> Result<(), BoxError> {
    semver::Version::parse(value).map(|_| ()).map_err(|error| {
        format!("provider-refresh: {label} must be an exact semantic version: {error}").into()
    })
}

fn sort_unique_strings(values: &mut [String], label: &str) -> Result<(), BoxError> {
    if values.is_empty()
        || values.len() > MAX_MODELS
        || values.iter().any(|value| !valid_bounded_string(value, 256))
    {
        return Err(format!("provider-refresh: {label} must be a non-empty bounded set").into());
    }
    values.sort();
    if values.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(format!("provider-refresh: {label} contains a duplicate").into());
    }
    Ok(())
}

fn checked_snapshot(
    path: &Path,
    expected: &str,
    label: &str,
) -> Result<local_file::LocalFileSnapshot, BoxError> {
    if !path.is_absolute() || !local_file::valid_sha256(expected) {
        return Err(
            format!("provider-refresh: {label} requires an absolute path and SHA-256").into(),
        );
    }
    let snapshot = local_file::read_regular_file_bounded(path, label, MAX_BOUND_FILE_BYTES)?;
    if snapshot.sha256 != expected {
        return Err(format!(
            "provider-refresh: {label} binding drift: expected {expected}, observed {}",
            snapshot.sha256
        )
        .into());
    }
    Ok(snapshot)
}

fn checked_json_snapshot_with<T, F>(
    path: &Path,
    expected: &str,
    label: &str,
    after_snapshot: F,
) -> Result<(T, local_file::LocalFileSnapshot), BoxError>
where
    T: for<'de> Deserialize<'de>,
    F: FnOnce(),
{
    let snapshot = checked_snapshot(path, expected, label)?;
    if snapshot.bytes.len() as u64 > MAX_INPUT_BYTES {
        return Err(format!("provider-refresh: {label} exceeds the JSON input limit").into());
    }
    after_snapshot();
    let parsed = serde_json::from_slice(&snapshot.bytes)
        .map_err(|error| format!("provider-refresh: {label} is invalid JSON: {error}"))?;
    Ok((parsed, snapshot))
}

fn read_json<T: for<'de> Deserialize<'de>>(
    path: &Path,
    label: &str,
) -> Result<(T, String), BoxError> {
    if !path.is_absolute() {
        return Err(format!("provider-refresh: {label} path must be absolute").into());
    }
    let snapshot = local_file::read_regular_file_bounded(path, label, MAX_INPUT_BYTES)?;
    let parsed = serde_json::from_slice(&snapshot.bytes)
        .map_err(|error| format!("provider-refresh: invalid {label} JSON: {error}"))?;
    Ok((parsed, snapshot.sha256))
}

fn owner_private_output(path: &Path, bytes: &[u8], label: &str) -> Result<File, BoxError> {
    if !path.is_absolute() {
        return Err(format!("provider-refresh: {label} must be absolute").into());
    }
    let parent = path
        .parent()
        .ok_or_else(|| format!("provider-refresh: {label} has no parent"))?;
    let name = path
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| format!("provider-refresh: {label} has no file name"))?;
    let metadata = std::fs::metadata(parent)
        .map_err(|error| format!("provider-refresh: cannot inspect {label} parent: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            return Err(format!("provider-refresh: {label} parent must be owner-private").into());
        }
    }
    let snapshot = local_file::snapshot_directory(parent, label)?;
    let directory = local_file::PinnedDirectory::open(
        parent,
        &snapshot.canonical_cwd,
        &snapshot.identity,
        label,
    )?;
    let mut file = directory.create_new_file(name, 0o600, label)?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("provider-refresh: cannot write {label}: {error}"))?;
    directory.sync()?;
    Ok(file)
}

fn expected_component_kinds() -> BTreeSet<ComponentKind> {
    BTreeSet::from([
        ComponentKind::CodexAcpAdapter,
        ComponentKind::CodexNestedCli,
        ComponentKind::CodexStandaloneCli,
        ComponentKind::ClaudeAcpAdapter,
        ComponentKind::ClaudeAgentSdk,
        ComponentKind::ClaudeBundledCli,
        ComponentKind::ClaudeStandaloneCli,
        ComponentKind::OpencodeCli,
        ComponentKind::KiroCli,
    ])
}

fn expected_package(kind: ComponentKind) -> Option<&'static str> {
    match kind {
        ComponentKind::CodexStandaloneCli => Some("@openai/codex"),
        ComponentKind::ClaudeStandaloneCli => Some("claude"),
        _ => kind.npm_package(),
    }
}

fn validate_component(component: &mut Component) -> Result<(), BoxError> {
    exact_version(
        &component.version,
        &format!("component {:?} version", component.kind),
    )?;
    match &mut component.source {
        ComponentSource::Npm {
            package,
            tarball_url,
            size_bytes,
            integrity,
        } => {
            let expected = component
                .kind
                .npm_package()
                .ok_or("provider-refresh: component kind cannot use an npm source")?;
            let basename = expected.rsplit('/').next().unwrap_or(expected);
            let expected_url = format!(
                "https://registry.npmjs.org/{expected}/-/{basename}-{}.tgz",
                component.version
            );
            if package != expected
                || tarball_url != &expected_url
                || !(1..=MAX_BOUND_FILE_BYTES).contains(size_bytes)
                || !valid_npm_sri(integrity)
            {
                return Err(
                    "provider-refresh: npm component requires its canonical package tarball, exact size, and SHA-512 SRI"
                        .into(),
                );
            }
        }
        ComponentSource::KiroStableArchive {
            architecture,
            url,
            size_bytes,
            sha256,
        } => {
            let expected_url = format!(
                "https://prod.download.cli.kiro.dev/stable/{}/{}",
                component.version,
                architecture.archive_name()
            );
            if component.kind != ComponentKind::KiroCli
                || url != &expected_url
                || !(1..=MAX_BOUND_FILE_BYTES).contains(size_bytes)
                || !local_file::valid_sha256(sha256)
            {
                return Err(
                    "provider-refresh: Kiro requires a canonical architecture-specific stable archive and SHA-256"
                        .into(),
                );
            }
        }
        ComponentSource::ManagedExecutable {
            manager: _,
            package,
            path,
            size_bytes,
            sha256,
        } => {
            let expected = match component.kind {
                ComponentKind::CodexStandaloneCli | ComponentKind::ClaudeStandaloneCli => {
                    expected_package(component.kind).unwrap_or("")
                }
                _ => return Err(
                    "provider-refresh: only standalone CLIs may use managed-executable provenance"
                        .into(),
                ),
            };
            let snapshot = checked_snapshot(
                path,
                sha256,
                &format!("managed {:?} executable", component.kind),
            )?;
            if package != expected || snapshot.bytes.len() as u64 != *size_bytes {
                return Err(
                    "provider-refresh: managed executable package, size, or SHA-256 is invalid"
                        .into(),
                );
            }
            *path = snapshot.canonical_path;
        }
        ComponentSource::BundledCli {
            parent,
            parent_version,
            manifest_sha256,
        } => {
            if component.kind != ComponentKind::ClaudeBundledCli
                || *parent != ComponentKind::ClaudeAgentSdk
                || !local_file::valid_sha256(manifest_sha256)
            {
                return Err(
                    "provider-refresh: bundled CLI must bind the exact Claude Agent SDK manifest"
                        .into(),
                );
            }
            exact_version(parent_version, "bundled CLI parent version")?;
        }
    }
    Ok(())
}

fn validate_component_set(
    components: &mut [Component],
    expected: &BTreeSet<ComponentKind>,
    label: &str,
) -> Result<(), BoxError> {
    if components.len() > MAX_COMPONENTS {
        return Err("provider-refresh: component set exceeds 64 entries".into());
    }
    let mut observed = BTreeSet::new();
    for component in components.iter_mut() {
        validate_component(component)?;
        if !observed.insert(component.kind) {
            return Err(
                format!("provider-refresh: {label} contains a duplicate component kind").into(),
            );
        }
    }
    if &observed != expected {
        return Err(
            format!("provider-refresh: {label} does not match the closed component graph").into(),
        );
    }
    if observed.contains(&ComponentKind::ClaudeBundledCli) {
        let sdk_version = components
            .iter()
            .find(|component| component.kind == ComponentKind::ClaudeAgentSdk)
            .map(|component| component.version.as_str())
            .ok_or("provider-refresh: Claude bundled CLI requires the Agent SDK component")?;
        let bundled_parent = components
            .iter()
            .find(|component| component.kind == ComponentKind::ClaudeBundledCli)
            .and_then(|component| match &component.source {
                ComponentSource::BundledCli { parent_version, .. } => Some(parent_version.as_str()),
                _ => None,
            })
            .ok_or("provider-refresh: Claude bundled CLI source is invalid")?;
        if bundled_parent != sdk_version {
            return Err(
                "provider-refresh: Claude bundled CLI parent version must equal the Agent SDK version"
                    .into(),
            );
        }
    }
    components.sort_by_key(|component| component.kind);
    Ok(())
}

fn validate_components(components: &mut [Component]) -> Result<(), BoxError> {
    validate_component_set(
        components,
        &expected_component_kinds(),
        "request components",
    )
}

fn validate_targets(
    targets: &mut [ProviderTarget],
    opencode_subscription_models: &mut [String],
    openrouter_models: &mut [OpenRouterModel],
) -> Result<(), BoxError> {
    sort_unique_strings(
        opencode_subscription_models,
        "operator-asserted OpenCode subscription models",
    )?;
    if openrouter_models.is_empty() || openrouter_models.len() > MAX_MODELS {
        return Err("provider-refresh: bounded OpenRouter models are required".into());
    }
    openrouter_models.sort();
    if openrouter_models
        .windows(2)
        .any(|pair| pair[0].model == pair[1].model)
        || openrouter_models.iter().any(|model| {
            !valid_bounded_string(&model.model, 256)
                || !valid_bounded_string(&model.prompt_price, 64)
                || !valid_bounded_string(&model.completion_price, 64)
        })
    {
        return Err(
            "provider-refresh: OpenRouter catalog claims must be unique and bounded".into(),
        );
    }
    if targets.len() != 5 {
        return Err("provider-refresh: exactly one target for each provider is required".into());
    }
    let mut providers = BTreeSet::new();
    for target in targets.iter_mut() {
        if !providers.insert(target.provider) {
            return Err(
                "provider-refresh: exactly one target for each provider is required".into(),
            );
        }
        sort_unique_strings(
            &mut target.selected_models,
            &format!("{} selected models", target.provider.label()),
        )?;
        require_id(&target.source_binding, "provider source binding")?;
        match target.provider {
            Provider::Codex | Provider::Claude | Provider::Kiro => {
                if target.mode != TargetMode::Acp
                    || target.default_model.is_some()
                    || !target.agent.as_deref().is_some_and(valid_id)
                {
                    return Err(
                        "provider-refresh: current ACP targets require one bound agent and no deferred default"
                            .into(),
                    );
                }
            }
            Provider::Opencode => {
                if target.mode != TargetMode::DeferredCatalog
                    || target.agent.is_some()
                    || target.default_model.is_some()
                    || target.selected_models.iter().any(|selected| {
                        opencode_subscription_models
                            .binary_search(selected)
                            .is_err()
                    })
                {
                    return Err(
                        "provider-refresh: OpenCode selections must be in the operator-asserted OpenCode subscription set"
                            .into(),
                    );
                }
            }
            Provider::Openrouter => {
                if target.mode != TargetMode::DeferredCatalog
                    || target.agent.is_some()
                    || target.default_model.as_deref() != Some("openrouter/free")
                    || target.selected_models.iter().any(|selected| {
                        openrouter_models
                            .binary_search_by(|model| model.model.as_str().cmp(selected))
                            .is_err()
                    })
                {
                    return Err(
                        "provider-refresh: OpenRouter target must be deferred, default to openrouter/free, and select only exact free tool-capable models"
                            .into(),
                    );
                }
            }
        }
    }
    let expected = BTreeSet::from([
        Provider::Codex,
        Provider::Claude,
        Provider::Kiro,
        Provider::Opencode,
        Provider::Openrouter,
    ]);
    if providers != expected {
        return Err("provider-refresh: exactly one target for each provider is required".into());
    }
    targets.sort_by_key(|target| target.provider);
    Ok(())
}

fn validate_bindings(
    bindings: &mut [FileBinding],
) -> Result<BTreeMap<String, SourceManifest>, BoxError> {
    if bindings.is_empty() || bindings.len() > 128 {
        return Err("provider-refresh: one to 128 exact file bindings are required".into());
    }
    let mut ids = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let mut manifests = BTreeMap::new();
    for binding in bindings.iter_mut() {
        require_id(&binding.id, "binding id")?;
        if !ids.insert(binding.id.clone()) {
            return Err("provider-refresh: duplicate binding id".into());
        }
        let snapshot = checked_snapshot(
            &binding.path,
            &binding.sha256,
            &format!("binding {}", binding.id),
        )?;
        binding.path = snapshot.canonical_path.clone();
        if !paths.insert(binding.path.clone()) {
            return Err(
                "provider-refresh: binding paths must be unique across authority roles".into(),
            );
        }
        if matches!(
            binding.role,
            BindingRole::CandidateManifest | BindingRole::CatalogResolution
        ) {
            if snapshot.bytes.len() as u64 > MAX_INPUT_BYTES {
                return Err(
                    "provider-refresh: source manifest exceeds the JSON input limit".into(),
                );
            }
            let manifest: SourceManifest =
                serde_json::from_slice(&snapshot.bytes).map_err(|error| {
                    format!("provider-refresh: invalid typed source manifest JSON: {error}")
                })?;
            let expected_kind = match binding.role {
                BindingRole::CandidateManifest => SourceManifestKind::CandidateManifest,
                BindingRole::CatalogResolution => SourceManifestKind::CatalogResolution,
                _ => unreachable!("source roles selected above"),
            };
            if manifest.schema_version != 1 || manifest.kind != expected_kind {
                return Err(
                    "provider-refresh: binding role does not match the typed source manifest kind"
                        .into(),
                );
            }
            manifests.insert(binding.id.clone(), manifest);
        }
    }
    for role in [
        BindingRole::CandidateManifest,
        BindingRole::CatalogResolution,
        BindingRole::PromotionPayload,
        BindingRole::Production,
        BindingRole::Rollback,
    ] {
        if !bindings.iter().any(|binding| binding.role == role) {
            return Err(format!("provider-refresh: missing required {role:?} binding").into());
        }
    }
    bindings.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(manifests)
}

fn binding_has_role(bindings: &[FileBinding], id: &str, role: BindingRole) -> bool {
    bindings
        .iter()
        .any(|binding| binding.id == id && binding.role == role)
}

fn artifact<'a>(manifest: &'a SourceManifest, id: &str) -> Option<&'a ManifestArtifact> {
    manifest.artifacts.iter().find(|artifact| artifact.id == id)
}

fn validate_source_manifests(
    components: &[Component],
    targets: &[ProviderTarget],
    bindings: &[FileBinding],
    manifests: &mut BTreeMap<String, SourceManifest>,
) -> Result<(), BoxError> {
    let source_bindings: BTreeSet<_> = targets
        .iter()
        .map(|target| target.source_binding.clone())
        .collect();
    if source_bindings.len() != targets.len() {
        return Err("provider-refresh: provider targets require distinct source bindings".into());
    }
    if manifests.keys().cloned().collect::<BTreeSet<_>>() != source_bindings {
        return Err(
            "provider-refresh: typed manifests must be exactly the five target source bindings"
                .into(),
        );
    }
    let authority_paths: BTreeSet<_> = bindings
        .iter()
        .map(|binding| binding.path.clone())
        .collect();
    let mut managed_paths = BTreeSet::new();
    for component in components {
        if let ComponentSource::ManagedExecutable { path, .. } = &component.source {
            if authority_paths.contains(path) || !managed_paths.insert(path.clone()) {
                return Err(
                    "provider-refresh: managed executable authority path alias is forbidden".into(),
                );
            }
        }
    }
    let mut shared_candidate_paths = BTreeMap::new();
    for target in targets {
        let expected_role = match target.mode {
            TargetMode::Acp => BindingRole::CandidateManifest,
            TargetMode::DeferredCatalog => BindingRole::CatalogResolution,
        };
        if !binding_has_role(bindings, &target.source_binding, expected_role) {
            return Err(
                "provider-refresh: target source binding has the wrong typed manifest role".into(),
            );
        }
        let manifest = manifests
            .get_mut(&target.source_binding)
            .ok_or("provider-refresh: typed target source manifest is missing")?;
        if manifest.provider != target.provider {
            return Err("provider-refresh: target and source manifest provider mismatch".into());
        }

        let expected_components: BTreeSet<_> = components
            .iter()
            .filter(|component| component.kind.provider() == target.provider)
            .map(|component| component.kind)
            .collect();
        validate_component_set(
            &mut manifest.components,
            &expected_components,
            &format!("{} source manifest components", target.provider.label()),
        )?;
        let planned_components: Vec<_> = components
            .iter()
            .filter(|component| component.kind.provider() == target.provider)
            .cloned()
            .collect();
        if manifest.components != planned_components {
            return Err(
                "provider-refresh: source manifest component identities do not match the plan"
                    .into(),
            );
        }

        let mut artifact_ids = BTreeSet::new();
        let mut artifact_paths = BTreeSet::new();
        let mut artifact_kinds = BTreeSet::new();
        for item in &mut manifest.artifacts {
            require_id(&item.id, "manifest artifact id")?;
            let snapshot = checked_snapshot(
                &item.path,
                &item.sha256,
                &format!("manifest artifact {}", item.id),
            )?;
            if authority_paths.contains(&snapshot.canonical_path)
                || managed_paths.contains(&snapshot.canonical_path)
            {
                return Err(
                    "provider-refresh: manifest artifact authority path alias is forbidden".into(),
                );
            }
            let shared_identity = (item.kind, item.size_bytes, item.sha256.clone());
            if shared_candidate_paths
                .get(&snapshot.canonical_path)
                .is_some_and(|existing| existing != &shared_identity)
            {
                return Err(
                    "provider-refresh: shared candidate artifact path has conflicting identities"
                        .into(),
                );
            }
            shared_candidate_paths.insert(snapshot.canonical_path.clone(), shared_identity);
            if snapshot.bytes.len() as u64 != item.size_bytes
                || !artifact_ids.insert(item.id.clone())
                || !artifact_paths.insert(snapshot.canonical_path.clone())
                || !artifact_kinds.insert(item.kind)
            {
                return Err(
                    "provider-refresh: manifest artifacts require unique ids, kinds, paths, exact sizes, and SHA-256"
                        .into(),
                );
            }
            item.path = snapshot.canonical_path;
        }
        manifest
            .artifacts
            .sort_by(|left, right| left.id.cmp(&right.id));
        manifest.promotion_payload_bindings.sort();
        if manifest
            .promotion_payload_bindings
            .iter()
            .any(|id| require_id(id, "manifest promotion payload binding").is_err())
            || manifest
                .promotion_payload_bindings
                .iter()
                .any(|id| !binding_has_role(bindings, id, BindingRole::PromotionPayload))
            || manifest
                .promotion_payload_bindings
                .windows(2)
                .any(|pair| pair[0] == pair[1])
        {
            return Err(
                "provider-refresh: manifest promotion-payload binding is invalid or unbound".into(),
            );
        }

        match target.mode {
            TargetMode::Acp => {
                if manifest.kind != SourceManifestKind::CandidateManifest
                    || !artifact_kinds.contains(&ManifestArtifactKind::Config)
                    || matches!(target.provider, Provider::Codex | Provider::Claude)
                        && !artifact_kinds.contains(&ManifestArtifactKind::PackageTreeManifest)
                    || artifact_kinds.contains(&ManifestArtifactKind::CatalogSnapshot)
                {
                    return Err(
                        "provider-refresh: ACP candidate manifest lacks its closed config/tree artifacts"
                            .into(),
                    );
                }
                match manifest.execution.as_ref().ok_or(
                    "provider-refresh: ACP candidate manifest requires an execution identity",
                )? {
                    ExecutionIdentity::Host {
                        executable_artifact,
                    } => {
                        if artifact(manifest, executable_artifact)
                            .is_none_or(|item| item.kind != ManifestArtifactKind::Executable)
                        {
                            return Err(
                                "provider-refresh: host execution must reference its exact executable artifact"
                                    .into(),
                            );
                        }
                    }
                    ExecutionIdentity::Container {
                        image_artifact,
                        immutable_id,
                    } => {
                        if artifact(manifest, image_artifact)
                            .is_none_or(|item| item.kind != ManifestArtifactKind::ImageReceipt)
                            || !immutable_id
                                .strip_prefix("sha256:")
                                .is_some_and(local_file::valid_sha256)
                        {
                            return Err(
                                "provider-refresh: container execution must bind one immutable image receipt"
                                    .into(),
                            );
                        }
                    }
                }
            }
            TargetMode::DeferredCatalog => {
                if manifest.kind != SourceManifestKind::CatalogResolution
                    || manifest.execution.is_some()
                    || !manifest.promotion_payload_bindings.is_empty()
                    || manifest.artifacts.len() != 1
                    || manifest.artifacts[0].kind != ManifestArtifactKind::CatalogSnapshot
                {
                    return Err(
                        "provider-refresh: deferred target requires one inert catalog-resolution artifact"
                            .into(),
                    );
                }
            }
        }
    }
    Ok(())
}

fn validate_operations(
    operations: &[PromotionOperation],
    bindings: &[FileBinding],
    manifests: &BTreeMap<String, SourceManifest>,
) -> Result<(), BoxError> {
    if operations.is_empty() || operations.len() > MAX_OPERATIONS {
        return Err("provider-refresh: one to 32 typed promotion operations are required".into());
    }
    let mut ids = BTreeSet::new();
    let mut restart_markers = 0_usize;
    let mut atomic_replacements = 0_usize;
    let mut used_payloads = BTreeSet::new();
    let mut used_production = BTreeSet::new();
    let mut used_rollbacks = BTreeSet::new();
    let mut owned_payloads = BTreeMap::new();
    for (source_binding, manifest) in manifests {
        if manifest.kind == SourceManifestKind::CandidateManifest {
            for payload in &manifest.promotion_payload_bindings {
                if owned_payloads
                    .insert(payload.as_str(), source_binding.as_str())
                    .is_some()
                {
                    return Err(
                        "provider-refresh: promotion-payload binding has multiple manifest owners"
                            .into(),
                    );
                }
            }
        }
    }
    for operation in operations {
        require_id(operation.id(), "promotion operation id")?;
        if !ids.insert(operation.id()) {
            return Err("provider-refresh: duplicate promotion operation id".into());
        }
        match operation {
            PromotionOperation::AtomicFileReplace {
                owner_source_binding,
                candidate_binding,
                production_binding,
                rollback_binding,
                ..
            } => {
                atomic_replacements += 1;
                if !binding_has_role(bindings, candidate_binding, BindingRole::PromotionPayload)
                    || !binding_has_role(bindings, production_binding, BindingRole::Production)
                    || !binding_has_role(bindings, rollback_binding, BindingRole::Rollback)
                    || owned_payloads.get(candidate_binding.as_str())
                        != Some(&owner_source_binding.as_str())
                    || !used_payloads.insert(candidate_binding.as_str())
                    || !used_production.insert(production_binding.as_str())
                    || !used_rollbacks.insert(rollback_binding.as_str())
                {
                    return Err(
                        "provider-refresh: atomic replacement payload must be owned by its candidate manifest and bind production/rollback roles"
                            .into(),
                    );
                }
            }
            PromotionOperation::OperatorRestartRequired { service_id, .. } => {
                require_id(service_id, "operator service id")?;
                restart_markers += 1;
            }
        }
    }
    if restart_markers != 1 {
        return Err(
            "provider-refresh: exactly one operator restart-required marker is required".into(),
        );
    }
    if atomic_replacements == 0 {
        return Err("provider-refresh: at least one atomic replacement is required".into());
    }
    let role_bindings = |role| {
        bindings
            .iter()
            .filter(|binding| binding.role == role)
            .map(|binding| binding.id.as_str())
            .collect::<BTreeSet<_>>()
    };
    if used_payloads != role_bindings(BindingRole::PromotionPayload)
        || used_payloads != owned_payloads.keys().copied().collect()
        || used_production != role_bindings(BindingRole::Production)
        || used_rollbacks != role_bindings(BindingRole::Rollback)
    {
        return Err("provider-refresh: orphaned operation-role binding is forbidden".into());
    }
    Ok(())
}

fn derive_required_checks(targets: &[ProviderTarget]) -> Vec<RequiredCheck> {
    let mut checks = Vec::new();
    for target in targets {
        match target.provider {
            Provider::Codex | Provider::Claude | Provider::Kiro => {
                for kind in [
                    CheckKind::RawAcpInitialize,
                    CheckKind::Doctor,
                    CheckKind::Models,
                ] {
                    checks.push(RequiredCheck {
                        id: format!("{}.{}", target.provider.label(), kind.label()),
                        provider: target.provider,
                        kind,
                        agent: target.agent.clone(),
                    });
                }
            }
            Provider::Opencode => checks.push(RequiredCheck {
                id: "opencode.opencode_catalog".into(),
                provider: target.provider,
                kind: CheckKind::OpencodeCatalog,
                agent: None,
            }),
            Provider::Openrouter => checks.push(RequiredCheck {
                id: "openrouter.openrouter_catalog".into(),
                provider: target.provider,
                kind: CheckKind::OpenrouterCatalog,
                agent: None,
            }),
        }
    }
    checks.sort_by(|left, right| left.id.cmp(&right.id));
    checks
}

fn canonical_plan_id(plan: &RefreshPlan) -> Result<String, BoxError> {
    let mut material = plan.clone();
    material.plan_id.clear();
    material.request_sha256.clear();
    Ok(local_file::sha256_hex(&serde_json::to_vec(&material)?))
}

fn canonical_check_id(receipt: &CheckReceipt) -> Result<String, BoxError> {
    let mut material = receipt.clone();
    material.check_id.clear();
    Ok(local_file::sha256_hex(&serde_json::to_vec(&material)?))
}

fn derived_deferred_components() -> Vec<ComponentKind> {
    vec![
        ComponentKind::CodexStandaloneCli,
        ComponentKind::ClaudeStandaloneCli,
        ComponentKind::OpencodeCli,
    ]
}

fn compile_plan_with_sources(
    mut request: RefreshRequest,
    request_sha256: String,
) -> Result<(RefreshPlan, BTreeMap<String, SourceManifest>), BoxError> {
    if request.schema_version != 2 {
        return Err("provider-refresh: request schema_version must be 2".into());
    }
    require_id(&request.refresh_id, "refresh_id")?;
    validate_components(&mut request.components)?;
    validate_targets(
        &mut request.targets,
        &mut request.opencode_subscription_models,
        &mut request.openrouter_models,
    )?;
    let mut manifests = validate_bindings(&mut request.bindings)?;
    validate_source_manifests(
        &request.components,
        &request.targets,
        &request.bindings,
        &mut manifests,
    )?;
    validate_operations(&request.promotion_operations, &request.bindings, &manifests)?;
    let required_checks = derive_required_checks(&request.targets);
    let mut plan = RefreshPlan {
        schema_version: 2,
        authority: "resolution_and_verification_plan_only".into(),
        promotion_ready: false,
        plan_id: String::new(),
        request_sha256,
        refresh_id: request.refresh_id,
        components: request.components,
        targets: request.targets,
        opencode_subscription_models: request.opencode_subscription_models,
        openrouter_models: request.openrouter_models,
        bindings: request.bindings,
        promotion_operations: request.promotion_operations,
        required_checks,
        deferred_components: derived_deferred_components(),
    };
    plan.plan_id = canonical_plan_id(&plan)?;
    Ok((plan, manifests))
}

fn compile_plan(request: RefreshRequest, request_sha256: String) -> Result<RefreshPlan, BoxError> {
    Ok(compile_plan_with_sources(request, request_sha256)?.0)
}

fn validate_plan(plan: &RefreshPlan) -> Result<BTreeMap<String, SourceManifest>, BoxError> {
    if plan.schema_version != 2
        || plan.authority != "resolution_and_verification_plan_only"
        || plan.promotion_ready
        || !local_file::valid_sha256(&plan.request_sha256)
        || !local_file::valid_sha256(&plan.plan_id)
        || canonical_plan_id(plan)? != plan.plan_id
    {
        return Err("provider-refresh: plan identity or authority mismatch".into());
    }
    let (rebuilt, manifests) = compile_plan_with_sources(
        RefreshRequest {
            schema_version: plan.schema_version,
            refresh_id: plan.refresh_id.clone(),
            components: plan.components.clone(),
            targets: plan.targets.clone(),
            opencode_subscription_models: plan.opencode_subscription_models.clone(),
            openrouter_models: plan.openrouter_models.clone(),
            bindings: plan.bindings.clone(),
            promotion_operations: plan.promotion_operations.clone(),
        },
        plan.request_sha256.clone(),
    )?;
    if &rebuilt != plan {
        return Err("provider-refresh: plan semantics are not canonical".into());
    }
    Ok(manifests)
}

fn target(plan: &RefreshPlan, provider: Provider) -> Result<&ProviderTarget, BoxError> {
    plan.targets
        .iter()
        .find(|target| target.provider == provider)
        .ok_or_else(|| "provider-refresh: plan target is missing".into())
}

fn component(plan: &RefreshPlan, kind: ComponentKind) -> Result<&Component, BoxError> {
    plan.components
        .iter()
        .find(|component| component.kind == kind)
        .ok_or_else(|| "provider-refresh: required closed component is missing".into())
}

fn acp_version_component(provider: Provider) -> Result<ComponentKind, BoxError> {
    match provider {
        Provider::Codex => Ok(ComponentKind::CodexAcpAdapter),
        Provider::Claude => Ok(ComponentKind::ClaudeAcpAdapter),
        Provider::Kiro => Ok(ComponentKind::KiroCli),
        Provider::Opencode | Provider::Openrouter => {
            Err("provider-refresh: deferred provider has no ACP version component".into())
        }
    }
}

fn validate_raw_acp(
    plan: &RefreshPlan,
    provider: Provider,
    agent: &str,
    value: &Value,
) -> Result<(), BoxError> {
    let exactly_one_alias = |snake, camel| match (value.get(snake), value.get(camel)) {
        (Some(field), None) | (None, Some(field)) => Some(field),
        _ => None,
    };
    let protocol = exactly_one_alias("protocol_version", "protocolVersion")
        .ok_or(
            "provider-refresh: raw ACP evidence requires exactly one spelling per aliased field",
        )?
        .as_u64();
    let agent_info = exactly_one_alias("agent_info", "agentInfo").ok_or(
        "provider-refresh: raw ACP evidence requires exactly one spelling per aliased field",
    )?;
    let expected_version = &component(plan, acp_version_component(provider)?)?.version;
    if protocol != Some(1)
        || value.get("agent").and_then(Value::as_str) != Some(agent)
        || value.get("initialized").and_then(Value::as_bool) != Some(true)
        || value.get("session_created").and_then(Value::as_bool) != Some(false)
        || value.get("prompt_calls").and_then(Value::as_u64) != Some(0)
        || agent_info.get("version").and_then(Value::as_str) != Some(expected_version.as_str())
    {
        return Err(
            "provider-refresh: raw ACP evidence is not initialize-only protocol v1 for the exact candidate version"
                .into(),
        );
    }
    Ok(())
}

fn unique_detail_field<'a>(detail: &'a str, field: &str) -> Option<&'a str> {
    let mut matches = detail.split_ascii_whitespace().filter_map(|part| {
        let (name, value) = part.split_once('=')?;
        (name == field && !value.is_empty()).then_some(value)
    });
    let value = matches.next()?;
    matches.next().is_none().then_some(value)
}

fn doctor_row<'a>(rows: &'a [Value], agent: &str, surface: &str) -> Result<&'a Value, BoxError> {
    let check = format!("provenance:{agent}:{surface}");
    let mut matches = rows
        .iter()
        .filter(|row| row.get("check").and_then(Value::as_str) == Some(check.as_str()));
    let row = matches
        .next()
        .ok_or_else(|| format!("provider-refresh: doctor evidence lacks {check}"))?;
    if matches.next().is_some() || row.get("status").and_then(Value::as_str) != Some("ok") {
        return Err(
            format!("provider-refresh: doctor evidence does not have one green {check}").into(),
        );
    }
    Ok(row)
}

fn doctor_package_matches(row: &Value, package: &str, version: &str) -> bool {
    let Some(detail) = row.get("detail").and_then(Value::as_str) else {
        return false;
    };
    unique_detail_field(detail, "package") == Some(package)
        && unique_detail_field(detail, "version") == Some(version)
}

fn validate_doctor(
    plan: &RefreshPlan,
    manifests: &BTreeMap<String, SourceManifest>,
    provider: Provider,
    agent: &str,
    value: &Value,
) -> Result<(), BoxError> {
    let rows = value
        .as_array()
        .filter(|rows| !rows.is_empty())
        .ok_or("provider-refresh: doctor evidence must be a non-empty JSON array")?;
    if rows.iter().any(|row| {
        !matches!(
            row.get("status").and_then(Value::as_str),
            Some("ok" | "warn")
        )
    }) {
        return Err("provider-refresh: doctor evidence contains a failing check".into());
    }
    let target = target(plan, provider)?;
    let manifest = manifests
        .get(&target.source_binding)
        .ok_or("provider-refresh: candidate manifest is missing during doctor validation")?;
    let execution = doctor_row(rows, agent, "execution")?;
    let execution_detail = execution
        .get("detail")
        .and_then(Value::as_str)
        .ok_or("provider-refresh: doctor execution provenance lacks detail")?;
    match manifest
        .execution
        .as_ref()
        .ok_or("provider-refresh: candidate execution identity is missing")?
    {
        ExecutionIdentity::Host {
            executable_artifact,
        } => {
            let executable = artifact(manifest, executable_artifact)
                .ok_or("provider-refresh: candidate executable artifact is missing")?;
            let exact_path = format!(" executable={:?}", executable.path);
            if unique_detail_field(execution_detail, "execution") != Some("host")
                || execution_detail.matches(" executable=").count() != 1
                || !execution_detail.ends_with(&exact_path)
            {
                return Err(
                    "provider-refresh: doctor execution does not match the exact host executable"
                        .into(),
                );
            }
        }
        ExecutionIdentity::Container { immutable_id, .. } => {
            let image = doctor_row(rows, agent, "image")?;
            let image_detail = image
                .get("detail")
                .and_then(Value::as_str)
                .ok_or("provider-refresh: doctor image provenance lacks detail")?;
            if unique_detail_field(execution_detail, "execution") != Some("container")
                || unique_detail_field(image_detail, "immutable_id") != Some(immutable_id.as_str())
            {
                return Err(
                    "provider-refresh: doctor execution does not match the immutable candidate image"
                        .into(),
                );
            }
        }
    }
    match provider {
        Provider::Codex => {
            let adapter = component(plan, ComponentKind::CodexAcpAdapter)?;
            let cli = component(plan, ComponentKind::CodexNestedCli)?;
            if !doctor_package_matches(
                doctor_row(rows, agent, "adapter")?,
                adapter.kind.npm_package().unwrap_or(""),
                &adapter.version,
            ) || !doctor_package_matches(
                doctor_row(rows, agent, "agent-cli")?,
                cli.kind.npm_package().unwrap_or(""),
                &cli.version,
            ) {
                return Err(
                    "provider-refresh: doctor package provenance does not match the Codex candidate"
                        .into(),
                );
            }
        }
        Provider::Claude => {
            let adapter = component(plan, ComponentKind::ClaudeAcpAdapter)?;
            let sdk = component(plan, ComponentKind::ClaudeAgentSdk)?;
            let bundled = component(plan, ComponentKind::ClaudeBundledCli)?;
            let cli_row = doctor_row(rows, agent, "agent-cli")?;
            let cli_detail = cli_row.get("detail").and_then(Value::as_str).unwrap_or("");
            if !doctor_package_matches(
                doctor_row(rows, agent, "adapter")?,
                adapter.kind.npm_package().unwrap_or(""),
                &adapter.version,
            ) || !doctor_package_matches(
                cli_row,
                sdk.kind.npm_package().unwrap_or(""),
                &sdk.version,
            ) || unique_detail_field(cli_detail, "bundled_cli_version")
                != Some(bundled.version.as_str())
            {
                return Err(
                    "provider-refresh: doctor package provenance does not match the Claude candidate"
                        .into(),
                );
            }
        }
        Provider::Kiro => {}
        Provider::Opencode | Provider::Openrouter => {
            return Err("provider-refresh: deferred provider cannot carry doctor evidence".into())
        }
    }
    Ok(())
}

fn validate_models(agent: &str, selected: &[String], value: &Value) -> Result<(), BoxError> {
    let caps = value
        .get(agent)
        .and_then(Value::as_object)
        .ok_or("provider-refresh: models evidence lacks the bound agent")?;
    let models: BTreeSet<_> = caps
        .get("models")
        .and_then(Value::as_array)
        .filter(|models| !models.is_empty())
        .ok_or("provider-refresh: models evidence is unavailable or empty")?
        .iter()
        .map(|model| {
            model
                .as_str()
                .filter(|model| !model.is_empty())
                .ok_or("provider-refresh: models evidence is unavailable or empty")
        })
        .collect::<Result<_, _>>()?;
    if caps.get("available").and_then(Value::as_bool) == Some(false) {
        return Err("provider-refresh: models evidence is unavailable or empty".into());
    }
    if selected
        .iter()
        .any(|model| !models.contains(model.as_str()))
    {
        return Err("provider-refresh: models evidence omits a selected model".into());
    }
    Ok(())
}

fn validate_opencode_catalog(plan: &RefreshPlan, value: Value) -> Result<(), BoxError> {
    let evidence: OpenCodeCatalogEvidence = serde_json::from_value(value)
        .map_err(|error| format!("provider-refresh: invalid OpenCode catalog evidence: {error}"))?;
    if evidence.provider != Provider::Opencode
        || evidence.prompt_calls != 0
        || evidence.models.is_empty()
    {
        return Err("provider-refresh: OpenCode catalog evidence is not provider-free".into());
    }
    let allowed: BTreeSet<_> = plan
        .opencode_subscription_models
        .iter()
        .map(String::as_str)
        .collect();
    let mut observed = BTreeSet::new();
    for model in evidence.models {
        if !model.subscription_included
            || !allowed.contains(model.id.as_str())
            || !observed.insert(model.id)
        {
            return Err(
                "provider-refresh: OpenCode catalog evidence is outside the operator-asserted subscription set"
                    .into(),
            );
        }
    }
    let selected = &target(plan, Provider::Opencode)?.selected_models;
    if selected.iter().any(|model| !observed.contains(model)) {
        return Err(
            "provider-refresh: OpenCode catalog evidence omits a selected subscription model"
                .into(),
        );
    }
    Ok(())
}

fn validate_openrouter_catalog(plan: &RefreshPlan, value: Value) -> Result<(), BoxError> {
    let evidence: OpenRouterCatalogEvidence = serde_json::from_value(value).map_err(|error| {
        format!("provider-refresh: invalid OpenRouter catalog evidence: {error}")
    })?;
    if evidence.provider != Provider::Openrouter
        || evidence.prompt_calls != 0
        || evidence.models.is_empty()
    {
        return Err("provider-refresh: OpenRouter catalog evidence is not provider-free".into());
    }
    let expected: BTreeMap<_, _> = plan
        .openrouter_models
        .iter()
        .map(|model| (model.model.as_str(), model))
        .collect();
    let mut observed = BTreeSet::new();
    for model in evidence.models {
        let Some(planned) = expected.get(model.id.as_str()) else {
            return Err(
                "provider-refresh: OpenRouter catalog evidence does not match the free-only plan"
                    .into(),
            );
        };
        if model.prompt_price != "0"
            || model.completion_price != "0"
            || !model.supports_tools
            || planned.prompt_price != model.prompt_price
            || planned.completion_price != model.completion_price
            || planned.supports_tools != model.supports_tools
            || !observed.insert(model.id)
        {
            return Err(
                "provider-refresh: OpenRouter catalog evidence does not match the free-only plan"
                    .into(),
            );
        }
    }
    let selected = &target(plan, Provider::Openrouter)?.selected_models;
    if selected.iter().any(|model| !observed.contains(model)) {
        return Err(
            "provider-refresh: OpenRouter catalog evidence does not match the free-only plan"
                .into(),
        );
    }
    Ok(())
}

fn validate_bound_catalog_snapshot(
    manifests: &BTreeMap<String, SourceManifest>,
    target: &ProviderTarget,
    payload: &Value,
) -> Result<(), BoxError> {
    let manifest = manifests
        .get(&target.source_binding)
        .ok_or("provider-refresh: catalog-resolution manifest is missing")?;
    let snapshot_artifact = manifest
        .artifacts
        .iter()
        .find(|artifact| artifact.kind == ManifestArtifactKind::CatalogSnapshot)
        .ok_or("provider-refresh: catalog-resolution snapshot is missing")?;
    let (bound_payload, _): (Value, _) = checked_json_snapshot_with(
        &snapshot_artifact.path,
        &snapshot_artifact.sha256,
        "bound catalog snapshot",
        || {},
    )?;
    if &bound_payload != payload {
        return Err(
            "provider-refresh: evidence catalog payload does not match the bound resolution snapshot"
                .into(),
        );
    }
    Ok(())
}

fn validate_evidence(
    plan: &RefreshPlan,
    manifests: &BTreeMap<String, SourceManifest>,
    requirement: &RequiredCheck,
    value: Value,
) -> Result<(), BoxError> {
    let envelope: EvidenceEnvelope = serde_json::from_value(value)
        .map_err(|error| format!("provider-refresh: invalid evidence envelope: {error}"))?;
    let target = target(plan, requirement.provider)?;
    let source = plan
        .bindings
        .iter()
        .find(|binding| binding.id == target.source_binding)
        .ok_or("provider-refresh: provider source binding is missing")?;
    if envelope.schema_version != 1
        || envelope.plan_id != plan.plan_id
        || envelope.provider != requirement.provider
        || envelope.source_binding != target.source_binding
        || envelope.source_sha256 != source.sha256
        || envelope.kind != requirement.kind
        || envelope.agent != requirement.agent
        || envelope.prompt_calls != 0
        || envelope.session_created
    {
        return Err(
            "provider-refresh: evidence envelope does not bind the exact plan, candidate, probe, and zero-prompt state"
                .into(),
        );
    }
    if matches!(
        requirement.kind,
        CheckKind::OpencodeCatalog | CheckKind::OpenrouterCatalog
    ) {
        validate_bound_catalog_snapshot(manifests, target, &envelope.payload)?;
    }
    match requirement.kind {
        CheckKind::RawAcpInitialize => validate_raw_acp(
            plan,
            requirement.provider,
            requirement.agent.as_deref().unwrap_or(""),
            &envelope.payload,
        ),
        CheckKind::Doctor => validate_doctor(
            plan,
            manifests,
            requirement.provider,
            requirement.agent.as_deref().unwrap_or(""),
            &envelope.payload,
        ),
        CheckKind::Models => validate_models(
            requirement.agent.as_deref().unwrap_or(""),
            &target.selected_models,
            &envelope.payload,
        ),
        CheckKind::OpencodeCatalog => validate_opencode_catalog(plan, envelope.payload),
        CheckKind::OpenrouterCatalog => validate_openrouter_catalog(plan, envelope.payload),
    }
}

fn check_plan(
    plan: &RefreshPlan,
    evidence: CheckEvidence,
    evidence_sha256: String,
) -> Result<CheckReceipt, BoxError> {
    let manifests = validate_plan(plan)?;
    if evidence.schema_version != 2 || evidence.plan_id != plan.plan_id {
        return Err("provider-refresh: evidence is not bound to this exact plan".into());
    }
    if evidence.checks.len() != plan.required_checks.len() || evidence.checks.len() > MAX_CHECKS {
        return Err(
            "provider-refresh: evidence does not contain the exact derived provider check set"
                .into(),
        );
    }
    let requirements: BTreeMap<_, _> = plan
        .required_checks
        .iter()
        .map(|check| (check.id.as_str(), check))
        .collect();
    let mut seen = BTreeSet::new();
    let mut checked = Vec::with_capacity(evidence.checks.len());
    for item in evidence.checks {
        let requirement = requirements
            .get(item.id.as_str())
            .ok_or("provider-refresh: evidence contains an unexpected check id")?;
        if !seen.insert(item.id.clone()) || item.kind != requirement.kind {
            return Err("provider-refresh: evidence check id or kind mismatch".into());
        }
        let (value, snapshot): (Value, _) = checked_json_snapshot_with(
            &item.artifact,
            &item.sha256,
            &format!("check artifact {}", item.id),
            || {},
        )?;
        validate_evidence(plan, &manifests, requirement, value)?;
        checked.push(CheckedEvidence {
            id: item.id,
            provider: requirement.provider,
            kind: item.kind,
            agent: requirement.agent.clone(),
            artifact: snapshot.canonical_path,
            sha256: snapshot.sha256,
            status: "pass".into(),
        });
    }
    checked.sort_by(|left, right| left.id.cmp(&right.id));
    let mut receipt = CheckReceipt {
        schema_version: 2,
        authority: "provider_free_verification_only".into(),
        promotion_ready: false,
        check_id: String::new(),
        plan_id: plan.plan_id.clone(),
        evidence_request_sha256: evidence_sha256,
        status: "pass_with_deferred_components".into(),
        checks: checked,
        deferred_components: plan.deferred_components.clone(),
    };
    receipt.check_id = canonical_check_id(&receipt)?;
    Ok(receipt)
}

#[derive(Default)]
struct Args {
    request: Option<PathBuf>,
    plan: Option<PathBuf>,
    evidence: Option<PathBuf>,
    out: Option<PathBuf>,
}

fn parse_args(args: &[String]) -> Result<(&str, Args), BoxError> {
    let Some(subcommand) = args.first().map(String::as_str) else {
        return Err(format!("provider-refresh: missing subcommand\n{USAGE}").into());
    };
    if subcommand == "--help" || subcommand == "-h" {
        return Ok((subcommand, Args::default()));
    }
    if subcommand == "promote" {
        return Err("provider-refresh: promote is not implemented in typed slice A".into());
    }
    if !matches!(subcommand, "plan" | "check") {
        return Err(format!("provider-refresh: unknown subcommand {subcommand:?}\n{USAGE}").into());
    }
    let mut parsed = Args::default();
    let mut index = 1;
    while index < args.len() {
        let flag = args[index].as_str();
        index += 1;
        let value = args.get(index).ok_or_else(|| {
            format!("provider-refresh {subcommand}: {flag} requires a value\n{USAGE}")
        })?;
        index += 1;
        let slot = match flag {
            "--request" => &mut parsed.request,
            "--plan" => &mut parsed.plan,
            "--evidence" => &mut parsed.evidence,
            "--out" => &mut parsed.out,
            _ => {
                return Err(format!(
                    "provider-refresh {subcommand}: unknown argument {flag:?}\n{USAGE}"
                )
                .into());
            }
        };
        if slot.replace(PathBuf::from(value)).is_some() {
            return Err(format!("provider-refresh {subcommand}: duplicate {flag}").into());
        }
    }
    Ok((subcommand, parsed))
}

fn required_path(
    value: Option<PathBuf>,
    flag: &str,
    subcommand: &str,
) -> Result<PathBuf, BoxError> {
    value
        .ok_or_else(|| format!("provider-refresh {subcommand}: {flag} is required\n{USAGE}").into())
}

pub(crate) fn provider_refresh_cmd(args: &[String]) -> Result<(), BoxError> {
    let (subcommand, parsed) = parse_args(args)?;
    if matches!(subcommand, "--help" | "-h") {
        println!("{USAGE}");
        return Ok(());
    }
    match subcommand {
        "plan" => {
            let request_path = required_path(parsed.request, "--request", subcommand)?;
            let output = required_path(parsed.out, "--out", subcommand)?;
            if parsed.plan.is_some() || parsed.evidence.is_some() {
                return Err(
                    format!("provider-refresh plan: incompatible arguments\n{USAGE}").into(),
                );
            }
            let (request, request_sha256) = read_json(&request_path, "request")?;
            let plan = compile_plan(request, request_sha256)?;
            let mut bytes = serde_json::to_vec_pretty(&plan)?;
            bytes.push(b'\n');
            owner_private_output(&output, &bytes, "provider-refresh plan")?;
            println!("planned provider refresh {}", plan.plan_id);
            Ok(())
        }
        "check" => {
            let plan_path = required_path(parsed.plan, "--plan", subcommand)?;
            let evidence_path = required_path(parsed.evidence, "--evidence", subcommand)?;
            let output = required_path(parsed.out, "--out", subcommand)?;
            if parsed.request.is_some() {
                return Err(
                    format!("provider-refresh check: incompatible arguments\n{USAGE}").into(),
                );
            }
            let (plan, _): (RefreshPlan, _) = read_json(&plan_path, "plan")?;
            let (evidence, evidence_sha256) = read_json(&evidence_path, "evidence")?;
            let receipt = check_plan(&plan, evidence, evidence_sha256)?;
            let mut bytes = serde_json::to_vec_pretty(&receipt)?;
            bytes.push(b'\n');
            owner_private_output(&output, &bytes, "provider-refresh check receipt")?;
            println!("checked provider refresh {}", receipt.check_id);
            Ok(())
        }
        _ => unreachable!("subcommand validated by parser"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_ids_reject_paths_and_whitespace() {
        assert!(valid_id("refresh-1.ok"));
        assert!(!valid_id("../refresh"));
        assert!(!valid_id("refresh one"));
    }

    #[test]
    fn checked_json_parses_the_same_snapshot_that_was_hashed() {
        let directory = tempfile::tempdir().unwrap();
        let artifact = directory.path().join("evidence.json");
        let replacement = directory.path().join("replacement.json");
        let original = br#"{"identity":"artifact-a"}"#;
        std::fs::write(&artifact, original).unwrap();
        std::fs::write(&replacement, br#"{"identity":"artifact-b"}"#).unwrap();
        let expected = local_file::sha256_hex(original);

        let (parsed, snapshot): (Value, _) =
            checked_json_snapshot_with(&artifact, &expected, "race regression", || {
                std::fs::rename(&replacement, &artifact).unwrap()
            })
            .unwrap();

        assert_eq!(parsed["identity"], "artifact-a");
        assert_eq!(snapshot.sha256, expected);
        let current: Value = serde_json::from_slice(&std::fs::read(&artifact).unwrap()).unwrap();
        assert_eq!(current["identity"], "artifact-b");
    }

    #[test]
    fn kiro_architectures_have_closed_versioned_archive_names() {
        assert_eq!(
            KiroArchitecture::Aarch64AppleDarwin.archive_name(),
            "kirocli-aarch64-apple-darwin.zip"
        );
        assert_eq!(
            KiroArchitecture::X86_64AppleDarwin.archive_name(),
            "kirocli-x86_64-apple-darwin.zip"
        );
        assert_eq!(
            KiroArchitecture::Aarch64LinuxMusl.archive_name(),
            "kirocli-aarch64-linux-musl.zip"
        );
        assert_eq!(
            KiroArchitecture::X86_64LinuxMusl.archive_name(),
            "kirocli-x86_64-linux-musl.zip"
        );
    }
}
