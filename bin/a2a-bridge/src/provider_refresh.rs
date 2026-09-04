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

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Component {
    provider: Provider,
    component: String,
    version: String,
    source: String,
    size_bytes: u64,
    integrity: String,
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

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum BindingRole {
    Candidate,
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
        candidate_binding: String,
        production_binding: String,
        rollback_binding: String,
    },
    OperatorRestartRequired {
        id: String,
        service_id: String,
    },
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

fn valid_integrity(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(local_file::valid_sha256)
        || value
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

fn sort_unique_strings(values: &mut Vec<String>, label: &str) -> Result<(), BoxError> {
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

fn checked_snapshot(path: &Path, expected: &str, label: &str) -> Result<PathBuf, BoxError> {
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
    Ok(snapshot.canonical_path)
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

fn validate_components(components: &mut Vec<Component>) -> Result<(), BoxError> {
    if components.is_empty() || components.len() > MAX_COMPONENTS {
        return Err("provider-refresh: one to 64 exact components are required".into());
    }
    let mut keys = BTreeSet::new();
    let mut providers = BTreeSet::new();
    for component in components.iter() {
        if component.provider == Provider::Openrouter {
            return Err("provider-refresh: OpenRouter has no local component".into());
        }
        require_id(&component.component, "component")?;
        exact_version(
            &component.version,
            &format!("component {} version", component.component),
        )?;
        if !valid_bounded_string(&component.source, 2048)
            || !(1..=MAX_BOUND_FILE_BYTES).contains(&component.size_bytes)
            || !valid_integrity(&component.integrity)
            || !keys.insert((component.provider, component.component.clone()))
        {
            return Err(
                "provider-refresh: component source, size, integrity, or identity is invalid"
                    .into(),
            );
        }
        if component.provider == Provider::Kiro
            && (component.source.contains("/latest/")
                || !component.source.contains(&component.version))
        {
            return Err(
                "provider-refresh: Kiro source must be a versioned archive matching its exact version"
                    .into(),
            );
        }
        providers.insert(component.provider);
    }
    let expected = BTreeSet::from([
        Provider::Codex,
        Provider::Claude,
        Provider::Kiro,
        Provider::Opencode,
    ]);
    if providers != expected {
        return Err(
            "provider-refresh: components must cover Codex, Claude, Kiro, and OpenCode".into(),
        );
    }
    components.sort_by(|left, right| {
        (left.provider, left.component.as_str()).cmp(&(right.provider, right.component.as_str()))
    });
    Ok(())
}

fn validate_targets(
    targets: &mut Vec<ProviderTarget>,
    opencode_subscription_models: &mut Vec<String>,
    openrouter_models: &mut Vec<OpenRouterModel>,
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

fn validate_bindings(bindings: &mut Vec<FileBinding>) -> Result<(), BoxError> {
    if bindings.is_empty() || bindings.len() > 128 {
        return Err("provider-refresh: one to 128 exact file bindings are required".into());
    }
    let mut ids = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for binding in bindings.iter_mut() {
        require_id(&binding.id, "binding id")?;
        if !ids.insert(binding.id.clone()) {
            return Err("provider-refresh: duplicate binding id".into());
        }
        binding.path = checked_snapshot(
            &binding.path,
            &binding.sha256,
            &format!("binding {}", binding.id),
        )?;
        if !paths.insert(binding.path.clone()) {
            return Err(
                "provider-refresh: binding paths must be unique across authority roles".into(),
            );
        }
    }
    for role in [
        BindingRole::Candidate,
        BindingRole::Production,
        BindingRole::Rollback,
    ] {
        if !bindings.iter().any(|binding| binding.role == role) {
            return Err(format!("provider-refresh: missing required {role:?} binding").into());
        }
    }
    bindings.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(())
}

fn binding_has_role(bindings: &[FileBinding], id: &str, role: BindingRole) -> bool {
    bindings
        .iter()
        .any(|binding| binding.id == id && binding.role == role)
}

fn validate_operations(
    operations: &[PromotionOperation],
    bindings: &[FileBinding],
) -> Result<(), BoxError> {
    if operations.is_empty() || operations.len() > MAX_OPERATIONS {
        return Err("provider-refresh: one to 32 typed promotion operations are required".into());
    }
    let mut ids = BTreeSet::new();
    let mut restart_markers = 0_usize;
    for operation in operations {
        require_id(operation.id(), "promotion operation id")?;
        if !ids.insert(operation.id()) {
            return Err("provider-refresh: duplicate promotion operation id".into());
        }
        match operation {
            PromotionOperation::AtomicFileReplace {
                candidate_binding,
                production_binding,
                rollback_binding,
                ..
            } => {
                if !binding_has_role(bindings, candidate_binding, BindingRole::Candidate)
                    || !binding_has_role(bindings, production_binding, BindingRole::Production)
                    || !binding_has_role(bindings, rollback_binding, BindingRole::Rollback)
                {
                    return Err(
                        "provider-refresh: atomic file replacement must bind candidate, production, and rollback roles"
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

fn compile_plan(
    mut request: RefreshRequest,
    request_sha256: String,
) -> Result<RefreshPlan, BoxError> {
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
    validate_bindings(&mut request.bindings)?;
    for target in &request.targets {
        if !binding_has_role(
            &request.bindings,
            &target.source_binding,
            BindingRole::Candidate,
        ) {
            return Err(
                "provider-refresh: every provider target must bind an exact candidate or catalog source"
                    .into(),
            );
        }
    }
    validate_operations(&request.promotion_operations, &request.bindings)?;
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
    };
    plan.plan_id = canonical_plan_id(&plan)?;
    Ok(plan)
}

fn validate_plan(plan: &RefreshPlan) -> Result<(), BoxError> {
    if plan.schema_version != 2
        || plan.authority != "resolution_and_verification_plan_only"
        || plan.promotion_ready
        || !local_file::valid_sha256(&plan.request_sha256)
        || !local_file::valid_sha256(&plan.plan_id)
        || canonical_plan_id(plan)? != plan.plan_id
    {
        return Err("provider-refresh: plan identity or authority mismatch".into());
    }
    let rebuilt = compile_plan(
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
    Ok(())
}

fn target<'a>(plan: &'a RefreshPlan, provider: Provider) -> Result<&'a ProviderTarget, BoxError> {
    plan.targets
        .iter()
        .find(|target| target.provider == provider)
        .ok_or_else(|| "provider-refresh: plan target is missing".into())
}

fn validate_raw_acp(agent: &str, value: &Value) -> Result<(), BoxError> {
    let protocol = value
        .get("protocol_version")
        .or_else(|| value.get("protocolVersion"))
        .and_then(Value::as_u64);
    if protocol != Some(1)
        || value.get("agent").and_then(Value::as_str) != Some(agent)
        || value.get("initialized").and_then(Value::as_bool) != Some(true)
        || value.get("session_created").and_then(Value::as_bool) != Some(false)
        || value.get("prompt_calls").and_then(Value::as_u64) != Some(0)
    {
        return Err("provider-refresh: raw ACP evidence is not initialize-only protocol v1".into());
    }
    Ok(())
}

fn validate_doctor(agent: &str, value: &Value) -> Result<(), BoxError> {
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
    let prefix = format!("provenance:{agent}:");
    let provenance: Vec<_> = rows
        .iter()
        .filter(|row| {
            row.get("check")
                .and_then(Value::as_str)
                .is_some_and(|check| check.starts_with(&prefix))
        })
        .collect();
    if provenance.is_empty()
        || provenance
            .iter()
            .any(|row| row.get("status").and_then(Value::as_str) != Some("ok"))
    {
        return Err("provider-refresh: doctor evidence lacks green bound-agent provenance".into());
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

fn validate_evidence(
    plan: &RefreshPlan,
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
    match requirement.kind {
        CheckKind::RawAcpInitialize => validate_raw_acp(
            requirement.agent.as_deref().unwrap_or(""),
            &envelope.payload,
        ),
        CheckKind::Doctor => validate_doctor(
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
    validate_plan(plan)?;
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
        let canonical = checked_snapshot(
            &item.artifact,
            &item.sha256,
            &format!("check artifact {}", item.id),
        )?;
        let snapshot = local_file::read_regular_file_bounded(
            &canonical,
            &format!("check artifact {}", item.id),
            MAX_INPUT_BYTES,
        )?;
        let value: Value = serde_json::from_slice(&snapshot.bytes).map_err(|error| {
            format!("provider-refresh: check artifact is invalid JSON: {error}")
        })?;
        validate_evidence(plan, requirement, value)?;
        checked.push(CheckedEvidence {
            id: item.id,
            provider: requirement.provider,
            kind: item.kind,
            agent: requirement.agent.clone(),
            artifact: canonical,
            sha256: item.sha256,
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
        status: "pass".into(),
        checks: checked,
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
}
