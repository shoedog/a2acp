//! Versioned compatibility manifests and bounded canary execution (R3a).
//!
//! The runner deliberately shells back into this exact candidate binary's R2c `smoke` command. It
//! does not own a second prompt path, retry policy, or provider fallback. Selected eligible cases get
//! one fixed-PONG smoke process; every other selected case remains an explicit aggregate row.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bridge_core::diagnostics::diagnostic_timestamp_ms;
use bridge_core::domain::Effort;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::compatibility_resolution::{
    self, FloatingTarget, LoadedRecipes, ResolutionState, ResolvedBinding, RuntimeKind,
};
use crate::{local_file, BoxError};

const DEFAULT_MANIFEST: &str = "compatibility/manifest.toml";
const DEFAULT_BASELINE: &str = "compatibility/baselines/pinned.json";
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_AGGREGATE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_EMBEDDED_SMOKE_BYTES: usize = 8 * 1024 * 1024;
const MAX_EXECUTABLE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_CASES: usize = 128;
const MAX_ID_BYTES: usize = 128;
const MAX_TEXT_BYTES: usize = 4096;
const MAX_TOTAL_TIMEOUT_SECS: u64 = 24 * 60 * 60;
const MAX_CASE_TIMEOUT_SECS: u64 = 900;
const MAX_TOTAL_TOKENS: u64 = 10_000_000;
const MAX_RETENTION_DAYS: u16 = 90;
#[cfg(target_os = "linux")]
const INHERITED_EXECUTABLE_FD_ENV: &str = "_A2A_BRIDGE_INTERNAL_COMPAT_EXECUTABLE_FD";
#[cfg(target_os = "linux")]
const INHERITED_SCRATCH_FD_ENV: &str = "_A2A_BRIDGE_INTERNAL_COMPAT_SCRATCH_FD";

pub(crate) const USAGE: &str = "\
usage: a2a-bridge compatibility validate
              [--manifest <path> | --recipes <path>]
       a2a-bridge compatibility resolve [--recipes <path>]
              (--case <id>... | --all)
              --environment-owner <id> --runtime docker|podman
              --acknowledge-resolution-effects --out <new-directory>
       a2a-bridge compatibility run [--manifest <path>]
              (--lane pinned|floating-current | --case <id>... | --all)
              --environment-owner <id> --acknowledge-billable --out <path>
       a2a-bridge compatibility run --resolution <resolution.json>
              (--case <id>... | --all-resolved)
              --environment-owner <id> --acknowledge-billable --out <path>
       a2a-bridge compatibility compare --current <aggregate.json>
              [--baseline <pinned.json>] [--mode pinned|floating-to-pinned]

`validate` is local and non-billable. `resolve` is non-billable but requires explicit acknowledgement
before registry/image effects. `run` requires both an explicit selection and billing acknowledgement before
it reads a manifest or resolution. Direct unresolved floating execution is refused. Every eligible selected
case invokes this exact binary's fixed-prompt R2c smoke once, with no retry or fallback. Child stdout/stderr
is discarded; one owner-only aggregate JSON artifact is written to --out. `compare` reports pinned
case/aggregate outcome, provenance, capability, auth, phase, terminal, and diagnostic drift independently.";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
enum Lane {
    Pinned,
    FloatingCurrent,
}

impl Lane {
    fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "pinned" => Ok(Self::Pinned),
            "floating-current" => Ok(Self::FloatingCurrent),
            _ => Err("compatibility: --lane must be pinned or floating-current".into()),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ExecutionMode {
    Host,
    ContainerRo,
    ContainerRw,
    RemoteApi,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum EvidencePath {
    DirectCli,
    DirectAcp,
    BridgeSmoke,
    BridgeWorkflow,
}

impl ExecutionMode {
    fn wire(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::ContainerRo => "container_ro",
            Self::ContainerRw => "container_rw",
            Self::RemoteApi => "remote_api",
        }
    }

    fn is_container(self) -> bool {
        matches!(self, Self::ContainerRo | Self::ContainerRw)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum AuthPath {
    ApiKeyEnv,
    PreAuthenticated,
    ConfiguredMethod,
    Automatic,
    NotApplicable,
}

impl AuthPath {
    fn wire(self) -> &'static str {
        match self {
            Self::ApiKeyEnv => "api_key_env",
            Self::PreAuthenticated => "pre_authenticated",
            Self::ConfiguredMethod => "configured_method",
            Self::Automatic => "automatic",
            Self::NotApplicable => "not_applicable",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ProbeType {
    Minimal,
    Representative,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum EvidenceStatus {
    Pass,
    Fail,
    Unknown,
    Stale,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Classification {
    Support,
    NonGoal,
    Canary,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RedactionPolicy {
    Strict,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ArtifactPolicy {
    retention_days: u16,
    redaction: RedactionPolicy,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestBudget {
    timeout_secs: u64,
    max_tokens: u64,
    #[serde(default)]
    max_cost_usd: Option<f64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PinSet {
    config_sha256: String,
    model: String,
    #[serde(default)]
    adapter: Option<String>,
    #[serde(default)]
    agent_cli: Option<String>,
    #[serde(default)]
    image_digest: Option<String>,
    #[serde(default)]
    components: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequiredEnvironment {
    name: String,
    #[serde(default)]
    one_of: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompatibilityCase {
    id: String,
    lane: Lane,
    evidence_path: EvidencePath,
    execution_mode: ExecutionMode,
    os: String,
    architecture: String,
    environment_owner: String,
    #[serde(default)]
    expected_image_digest: Option<String>,
    config: PathBuf,
    agent: String,
    model: String,
    #[serde(default)]
    effort: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    session_cwd: Option<PathBuf>,
    auth_path: AuthPath,
    #[serde(default)]
    credential_env: Option<String>,
    #[serde(default)]
    required_env: Vec<RequiredEnvironment>,
    probe: ProbeType,
    billable: bool,
    timeout_secs: u64,
    max_tokens: u64,
    #[serde(default)]
    max_cost_usd: Option<f64>,
    retry_cap: u8,
    expected_status: EvidenceStatus,
    classification: Classification,
    #[serde(default)]
    baseline_case: Option<String>,
    artifact: ArtifactPolicy,
    #[serde(default)]
    pins: Option<PinSet>,
    #[serde(default)]
    resolved: Option<ResolvedBinding>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompatibilityManifest {
    schema_version: u16,
    budget: ManifestBudget,
    #[serde(default)]
    cases: Vec<CompatibilityCase>,
}

struct LoadedManifest {
    manifest: CompatibilityManifest,
    canonical_path: PathBuf,
    canonical_path_text: String,
    sha256: String,
}

fn bounded_text(label: &str, value: &str, max: usize) -> Result<(), BoxError> {
    if value.is_empty()
        || value.trim() != value
        || value.len() > max
        || value.chars().any(char::is_control)
    {
        return Err(format!(
            "compatibility manifest: {label} must be non-empty, unpadded, control-free, and at most {max} bytes"
        )
        .into());
    }
    Ok(())
}

fn stable_id(label: &str, value: &str) -> Result<(), BoxError> {
    bounded_text(label, value, MAX_ID_BYTES)?;
    reject_secret_text(label, value)?;
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return Err(format!("compatibility manifest: {label} is empty").into());
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(format!(
            "compatibility manifest: {label} must start with a lowercase ASCII letter or digit"
        )
        .into());
    }
    if !bytes.all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
    }) {
        return Err(format!(
            "compatibility manifest: {label} must contain only lowercase ASCII letters, digits, '.', '_', or '-'"
        )
        .into());
    }
    Ok(())
}

fn env_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(b'A'..=b'Z') | Some(b'_'))
        && bytes.all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        && value.len() <= MAX_ID_BYTES
}

fn credential_shaped_env_name(value: &str) -> bool {
    value.split('_').any(|part| {
        matches!(
            part,
            "AUTH"
                | "AUTHORIZATION"
                | "BEARER"
                | "COOKIE"
                | "CRED"
                | "CREDS"
                | "CREDENTIAL"
                | "CREDENTIALS"
                | "KEY"
                | "APIKEY"
                | "PASS"
                | "PASSWD"
                | "PASSWORD"
                | "PAT"
                | "SECRET"
                | "SESSION"
                | "TOKEN"
        )
    })
}

pub(super) fn looks_like_secret(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    secret_shaped_tokens(value)
        || lower.contains("bearer ")
        || lower.contains("basic ")
        || lower.contains("api_key=")
        || lower.contains("apikey=")
        || lower.contains("token=")
        || lower.contains("password=")
        || lower.contains("secret=")
        || value.contains("-----BEGIN PRIVATE KEY-----")
}

fn secret_shaped_tokens(value: &str) -> bool {
    value
        .split(|character: char| {
            character.is_whitespace()
                || matches!(
                    character,
                    '"' | '\'' | '=' | ':' | ',' | '[' | ']' | '{' | '}' | '(' | ')' | '#'
                )
        })
        .filter(|token| !token.is_empty())
        .any(|token| {
            token.starts_with("sk-")
                || token.starts_with("sk_")
                || token.starts_with("ghp_")
                || token.starts_with("github_pat_")
                || token.starts_with("xoxb-")
                || token.starts_with("xoxp-")
                || (token.starts_with("AKIA") && token.len() >= 16)
                || looks_like_jwt(token)
        })
}

fn looks_like_jwt(value: &str) -> bool {
    let mut parts = value.split('.');
    let Some(header) = parts.next() else {
        return false;
    };
    let Some(payload) = parts.next() else {
        return false;
    };
    let Some(signature) = parts.next() else {
        return false;
    };
    parts.next().is_none()
        && header.starts_with("eyJ")
        && !payload.is_empty()
        && !signature.is_empty()
}

fn reject_secret_text(label: &str, value: &str) -> Result<(), BoxError> {
    if looks_like_secret(value) {
        return Err(format!(
            "compatibility manifest: {label} contains secret-shaped material; record only an environment-variable name or non-secret identity"
        )
        .into());
    }
    Ok(())
}

fn valid_positive_cost(label: &str, value: Option<f64>) -> Result<(), BoxError> {
    if value.is_some_and(|value| !value.is_finite() || value <= 0.0 || value > 10_000.0) {
        return Err(format!(
            "compatibility manifest: {label} must be a finite value in (0, 10000]"
        )
        .into());
    }
    Ok(())
}

fn exact_component(label: &str, value: &str) -> Result<(), BoxError> {
    bounded_text(label, value, MAX_TEXT_BYTES)?;
    reject_secret_text(label, value)?;
    let lower = value.to_ascii_lowercase();
    let contains_floating_word = lower
        .split(|character: char| !character.is_ascii_alphanumeric())
        .any(|part| {
            matches!(
                part,
                "auto" | "default" | "latest" | "current" | "next" | "nightly" | "canary"
            )
        });
    if lower == "latest"
        || lower == "unknown"
        || lower == "stable"
        || contains_floating_word
        || lower.contains(":latest")
        || lower
            .split(['.', '-', '_'])
            .any(|part| part.eq_ignore_ascii_case("x"))
        || value.contains('*')
        || value.contains('^')
        || value.contains('~')
        || value.contains('>')
        || value.contains('<')
        || value.contains('|')
        || value.contains(',')
        || value.contains(" - ")
    {
        return Err(format!(
            "compatibility manifest: {label} must be an exact identity, not a floating tag or range"
        )
        .into());
    }
    Ok(())
}

fn exact_model(label: &str, value: &str) -> Result<(), BoxError> {
    exact_component(label, value)
}

fn exact_remote_component(label: &str, value: &str) -> Result<(), BoxError> {
    exact_component(label, value)?;
    if stable_id(label, value).is_err() {
        return Err(format!(
            "compatibility manifest: {label} must be one exact identity, not a compound or ranged expression"
        )
        .into());
    }
    Ok(())
}

fn exact_package_pin(label: &str, value: &str) -> Result<(), BoxError> {
    bounded_text(label, value, MAX_TEXT_BYTES)?;
    reject_secret_text(label, value)?;
    let mut pieces = value.split('=');
    let package = pieces.next().unwrap_or_default();
    let version = pieces.next().unwrap_or_default();
    if package.is_empty()
        || version.is_empty()
        || pieces.next().is_some()
        || package.chars().any(char::is_whitespace)
        || version.chars().any(char::is_whitespace)
    {
        return Err(format!(
            "compatibility manifest: {label} must use exact <package>=<version> form"
        )
        .into());
    }
    if package
        .chars()
        .any(|character| character.is_control() || character == '\\')
    {
        return Err(format!(
            "compatibility manifest: {label} package name contains an invalid character"
        )
        .into());
    }
    semver::Version::parse(version).map_err(|_| -> BoxError {
        format!(
            "compatibility manifest: {label} must be an exact identity with one complete immutable semantic version"
        )
        .into()
    })?;
    Ok(())
}

fn artifact_safe_path(label: &str, path: &Path) -> Result<String, BoxError> {
    let value = path
        .to_str()
        .ok_or_else(|| format!("{label}: canonical path must be UTF-8"))?;
    if value.is_empty()
        || value.len() > MAX_TEXT_BYTES
        || value.chars().any(char::is_control)
        || looks_like_secret(value)
    {
        return Err(format!(
            "{label}: canonical path must be non-empty, secret-free, control-free UTF-8 of at most {MAX_TEXT_BYTES} bytes"
        )
        .into());
    }
    Ok(value.to_owned())
}

fn validate_case(case: &CompatibilityCase, budget: &ManifestBudget) -> Result<(), BoxError> {
    stable_id("case id", &case.id)?;
    stable_id("environment owner", &case.environment_owner)?;
    stable_id("operating system", &case.os)?;
    stable_id("architecture", &case.architecture)?;
    bounded_text("agent id", &case.agent, MAX_ID_BYTES)?;
    bounded_text("model id", &case.model, MAX_TEXT_BYTES)?;
    reject_secret_text("agent id", &case.agent)?;
    reject_secret_text("model id", &case.model)?;
    if let Some(effort) = &case.effort {
        bounded_text("effort", effort, MAX_ID_BYTES)?;
        Effort::from_str(effort)
            .map_err(|error| format!("compatibility manifest: invalid effort: {error}"))?;
    }
    if let Some(mode) = &case.mode {
        bounded_text("mode", mode, MAX_ID_BYTES)?;
        reject_secret_text("mode", mode)?;
    }
    if case.execution_mode == ExecutionMode::RemoteApi
        && matches!(
            case.evidence_path,
            EvidencePath::DirectCli | EvidencePath::DirectAcp
        )
    {
        return Err(format!(
            "compatibility manifest: case {:?} remote API execution mode requires bridge smoke/workflow evidence, not a direct CLI or ACP control",
            case.id
        )
        .into());
    }
    for (label, path) in [
        ("config path", Some(&case.config)),
        ("session cwd", case.session_cwd.as_ref()),
    ] {
        if let Some(path) = path {
            bounded_text(label, &path.to_string_lossy(), MAX_TEXT_BYTES)?;
            reject_secret_text(label, &path.to_string_lossy())?;
        }
    }
    let mut env_seen = BTreeSet::new();
    match (case.auth_path, &case.credential_env) {
        (AuthPath::ApiKeyEnv, Some(name)) if env_name(name) => {}
        (AuthPath::ApiKeyEnv, _) => {
            return Err(format!(
                "compatibility manifest: API-key case {:?} requires a valid credential_env name",
                case.id
            )
            .into())
        }
        (_, Some(_)) => {
            return Err(format!(
                "compatibility manifest: non-API-key case {:?} must not declare credential_env",
                case.id
            )
            .into())
        }
        (_, None) => {}
    }
    for requirement in &case.required_env {
        let name = &requirement.name;
        if !env_name(name) {
            return Err(format!(
                "compatibility manifest: case {:?} has invalid required_env name {:?}",
                case.id, name
            )
            .into());
        }
        if !env_seen.insert(name) {
            return Err(format!(
                "compatibility manifest: case {:?} repeats required_env {:?}",
                case.id, name
            )
            .into());
        }
        if credential_shaped_env_name(name) {
            return Err(format!(
                "compatibility manifest: case {:?} must declare credential-shaped environment name {:?} as credential_env, not required_env",
                case.id, name
            )
            .into());
        }
        if case.credential_env.as_deref() == Some(name) {
            return Err(format!(
                "compatibility manifest: case {:?} must not repeat credential_env in required_env",
                case.id
            )
            .into());
        }
        let mut values = BTreeSet::new();
        for expected in &requirement.one_of {
            bounded_text("required_env expected value", expected, MAX_ID_BYTES)?;
            reject_secret_text("required_env expected value", expected)?;
            if !values.insert(expected) {
                return Err(format!(
                    "compatibility manifest: case {:?} repeats a required_env expected value",
                    case.id
                )
                .into());
            }
        }
    }
    match (case.execution_mode.is_container(), &case.expected_image_digest) {
        (true, Some(digest)) if valid_image_digest(digest) => {}
        (true, _) => {
            return Err(format!(
                "compatibility manifest: container case {:?} requires an immutable expected_image_digest",
                case.id
            )
            .into())
        }
        (false, Some(_)) => {
            return Err(format!(
                "compatibility manifest: non-container case {:?} must not declare expected_image_digest",
                case.id
            )
            .into())
        }
        (false, None) => {}
    }
    if !case.billable {
        return Err(format!(
            "compatibility manifest: case {:?} must set billable=true because R3a invokes the potentially billable R2c smoke path",
            case.id
        )
        .into());
    }
    if !(1..=MAX_CASE_TIMEOUT_SECS).contains(&case.timeout_secs) {
        return Err(format!(
            "compatibility manifest: case {:?} timeout_secs must be in 1..={MAX_CASE_TIMEOUT_SECS}",
            case.id
        )
        .into());
    }
    if case.timeout_secs > budget.timeout_secs {
        return Err(format!(
            "compatibility manifest: case {:?} timeout exceeds the total timeout budget",
            case.id
        )
        .into());
    }
    if case.max_tokens == 0 || case.max_tokens > budget.max_tokens {
        return Err(format!(
            "compatibility manifest: case {:?} max_tokens must be positive and no greater than the total token budget",
            case.id
        )
        .into());
    }
    valid_positive_cost("case max_cost_usd", case.max_cost_usd)?;
    if let Some(case_cost) = case.max_cost_usd {
        let Some(total_cost) = budget.max_cost_usd else {
            return Err(format!(
                "compatibility manifest: case {:?} has a cost cap but the total budget does not",
                case.id
            )
            .into());
        };
        if case_cost > total_cost {
            return Err(format!(
                "compatibility manifest: case {:?} cost cap exceeds the total cost budget",
                case.id
            )
            .into());
        }
    }
    if case.retry_cap != 0 {
        return Err(format!(
            "compatibility manifest: case {:?} retry_cap must be exactly zero",
            case.id
        )
        .into());
    }
    if case.artifact.retention_days == 0 || case.artifact.retention_days > MAX_RETENTION_DAYS {
        return Err(format!(
            "compatibility manifest: case {:?} artifact retention_days must be in 1..={MAX_RETENTION_DAYS}",
            case.id
        )
        .into());
    }
    match (case.lane, case.baseline_case.as_deref()) {
        (Lane::Pinned, None) => {}
        (Lane::Pinned, Some(_)) => {
            return Err(format!(
                "compatibility manifest: pinned case {:?} must not declare baseline_case",
                case.id
            )
            .into())
        }
        (Lane::FloatingCurrent, Some(baseline)) => {
            stable_id("floating baseline case id", baseline)?;
        }
        (Lane::FloatingCurrent, None) => {
            return Err(format!(
                "compatibility manifest: floating-current case {:?} requires baseline_case",
                case.id
            )
            .into())
        }
    }

    match (case.lane, &case.pins, &case.resolved) {
        (Lane::Pinned, None, _) => {
            return Err(format!(
                "compatibility manifest: pinned case {:?} is missing exact pins",
                case.id
            )
            .into())
        }
        (Lane::Pinned, Some(_), Some(_)) => {
            return Err(format!(
                "compatibility manifest: pinned case {:?} must not declare candidate resolution evidence",
                case.id
            )
            .into())
        }
        (Lane::FloatingCurrent, Some(_), _) => {
            return Err(format!(
            "compatibility manifest: floating-current case {:?} must not declare production pins",
            case.id
        )
            .into())
        }
        (Lane::FloatingCurrent, None, None) => {
            return Err(format!(
                "compatibility manifest: floating-current case {:?} requires exact candidate resolution evidence",
                case.id
            )
            .into())
        }
        (Lane::FloatingCurrent, None, Some(resolved)) => {
            if case.classification != Classification::Canary {
                return Err(format!(
                    "compatibility manifest: floating-current case {:?} classification must be canary",
                    case.id
                )
                .into());
            }
            if case.evidence_path != EvidencePath::BridgeSmoke
                || case.probe != ProbeType::Minimal
                || !matches!(
                    case.execution_mode,
                    ExecutionMode::Host | ExecutionMode::ContainerRo
                )
                || case.expected_status != EvidenceStatus::Pass
            {
                return Err(format!(
                    "compatibility manifest: floating-current case {:?} must be a minimal host/container-ro bridge-smoke canary expecting PASS",
                    case.id
                )
                .into());
            }
            compatibility_resolution::validate_resolved_binding(
                resolved,
                case.execution_mode.is_container(),
                case.expected_image_digest.as_deref(),
            )
            .map_err(|error| format!("compatibility manifest: case {:?}: {error}", case.id))?;
        }
        (Lane::Pinned, Some(pins), None) => {
            if case.classification == Classification::Canary {
                return Err(format!(
                    "compatibility manifest: pinned case {:?} must not use canary classification",
                    case.id
                )
                .into());
            }
            if !local_file::valid_sha256(&pins.config_sha256)
                || pins.config_sha256 != pins.config_sha256.to_ascii_lowercase()
            {
                return Err(format!(
                    "compatibility manifest: pinned case {:?} config_sha256 must be 64 lowercase hexadecimal characters",
                    case.id
                )
                .into());
            }
            if pins.model != case.model {
                return Err(format!(
                    "compatibility manifest: pinned case {:?} model pin must equal the raw case model",
                    case.id
                )
                .into());
            }
            exact_model("pinned model", &pins.model)?;
            if let Some(adapter) = &pins.adapter {
                exact_package_pin("adapter pin", adapter)?;
            }
            if let Some(agent_cli) = &pins.agent_cli {
                exact_package_pin("agent CLI pin", agent_cli)?;
            }
            for (name, value) in &pins.components {
                stable_id("component pin name", name)?;
                if ["key", "token", "secret", "password", "credential"]
                    .iter()
                    .any(|marker| name.contains(marker))
                {
                    return Err(format!(
                        "compatibility manifest: component pin name {name:?} is secret-shaped"
                    )
                    .into());
                }
                exact_component("component pin", value)?;
            }
            match (case.execution_mode, case.evidence_path) {
                (ExecutionMode::RemoteApi, _) => {
                    if pins.adapter.is_some() || pins.agent_cli.is_some() {
                        return Err(format!(
                            "compatibility manifest: pinned remote-API case {:?} must express provider dependencies through component pins, not adapter or agent CLI pins",
                            case.id
                        )
                        .into());
                    }
                    if !["provider", "api", "api_version"]
                        .iter()
                        .all(|name| pins.components.contains_key(*name))
                    {
                        return Err(format!(
                            "compatibility manifest: pinned remote-API case {:?} requires exact provider identity, API identity, and API version component pins",
                            case.id
                        )
                        .into());
                    }
                    for name in ["provider", "api", "api_version"] {
                        exact_remote_component(
                            "remote component pin",
                            pins.components.get(name).expect("presence checked above"),
                        )?;
                    }
                }
                (_, EvidencePath::DirectCli) => {
                    if pins.adapter.is_some() {
                        return Err(format!(
                            "compatibility manifest: pinned direct-CLI case {:?} must not declare an adapter pin",
                            case.id
                        )
                        .into());
                    }
                    if pins.agent_cli.is_none() {
                        return Err(format!(
                            "compatibility manifest: pinned direct-CLI case {:?} requires an exact agent CLI pin",
                            case.id
                        )
                        .into());
                    }
                }
                (
                    _,
                    EvidencePath::DirectAcp
                    | EvidencePath::BridgeSmoke
                    | EvidencePath::BridgeWorkflow,
                ) => {
                    if pins.adapter.is_none() {
                        return Err(format!(
                            "compatibility manifest: pinned ACP/bridge case {:?} requires an exact adapter pin",
                            case.id
                        )
                        .into());
                    }
                    if pins.agent_cli.is_none() {
                        return Err(format!(
                            "compatibility manifest: pinned ACP/bridge case {:?} requires an exact agent CLI pin",
                            case.id
                        )
                        .into());
                    }
                }
            }
            match (case.execution_mode.is_container(), &pins.image_digest) {
                (true, Some(digest))
                    if valid_image_digest(digest)
                        && case.expected_image_digest.as_deref() == Some(digest.as_str()) =>
                {
                }
                (true, _) => {
                    return Err(format!(
                        "compatibility manifest: pinned container case {:?} requires an immutable image_digest equal to expected_image_digest",
                        case.id
                    )
                    .into())
                }
                (false, Some(_)) => {
                    return Err(format!(
                        "compatibility manifest: non-container case {:?} must not declare image_digest",
                        case.id
                    )
                    .into())
                }
                (false, None) => {}
            }
        }
    }
    Ok(())
}

fn valid_image_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        local_file::valid_sha256(digest) && digest == digest.to_ascii_lowercase()
    })
}

fn validate_manifest(manifest: &CompatibilityManifest) -> Result<(), BoxError> {
    if manifest.schema_version != 1 {
        return Err("compatibility manifest: schema_version must be 1".into());
    }
    if !(1..=MAX_TOTAL_TIMEOUT_SECS).contains(&manifest.budget.timeout_secs) {
        return Err(format!(
            "compatibility manifest: budget.timeout_secs must be in 1..={MAX_TOTAL_TIMEOUT_SECS}"
        )
        .into());
    }
    if manifest.budget.max_tokens == 0 || manifest.budget.max_tokens > MAX_TOTAL_TOKENS {
        return Err(format!(
            "compatibility manifest: budget.max_tokens must be in 1..={MAX_TOTAL_TOKENS}"
        )
        .into());
    }
    valid_positive_cost("budget.max_cost_usd", manifest.budget.max_cost_usd)?;
    if manifest.cases.len() > MAX_CASES {
        return Err(
            format!("compatibility manifest: at most {MAX_CASES} cases are allowed").into(),
        );
    }
    let mut ids = BTreeSet::new();
    let mut floating_baselines = BTreeSet::new();
    for case in &manifest.cases {
        validate_case(case, &manifest.budget)?;
        if !ids.insert(&case.id) {
            return Err(format!("compatibility manifest: duplicate case id {:?}", case.id).into());
        }
        if case.lane == Lane::FloatingCurrent
            && !floating_baselines.insert(
                case.baseline_case
                    .as_deref()
                    .expect("floating validation requires baseline_case"),
            )
        {
            return Err(format!(
                "compatibility manifest: duplicate floating baseline mapping {:?}",
                case.baseline_case.as_deref().unwrap_or_default()
            )
            .into());
        }
    }
    Ok(())
}

fn load_manifest(path: &Path) -> Result<LoadedManifest, BoxError> {
    let snapshot =
        local_file::read_regular_file_bounded(path, "compatibility manifest", MAX_MANIFEST_BYTES)?;
    let canonical_path_text =
        artifact_safe_path("compatibility manifest", &snapshot.canonical_path)?;
    let raw = std::str::from_utf8(&snapshot.bytes)
        .map_err(|_| "compatibility manifest: file must be UTF-8")?;
    let manifest = parse_manifest_text(raw)?;
    Ok(LoadedManifest {
        manifest,
        canonical_path: snapshot.canonical_path,
        canonical_path_text,
        sha256: snapshot.sha256,
    })
}

fn parse_manifest_text(raw: &str) -> Result<CompatibilityManifest, BoxError> {
    // Scan before TOML parsing so comments and parse-error source snippets cannot carry a credential
    // into a checked-in manifest or diagnostic.
    reject_secret_text("manifest text", raw)?;
    let manifest: CompatibilityManifest = toml::from_str(raw)
        .map_err(|error| format!("compatibility manifest: invalid TOML: {error}"))?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

fn load_recipes_with_pinned_manifest(
    path: &Path,
) -> Result<(LoadedRecipes, LoadedManifest), BoxError> {
    let recipes = compatibility_resolution::load_recipes(path)?;
    let manifest = load_manifest(&compatibility_resolution::production_manifest_path(
        &recipes,
    ))?;
    let package_sets: BTreeMap<_, _> = recipes
        .recipes
        .package_sets
        .iter()
        .map(|package| (package.id.as_str(), package))
        .collect();
    for recipe_case in &recipes.recipes.cases {
        let baseline = manifest
            .manifest
            .cases
            .iter()
            .find(|case| case.id == recipe_case.baseline_case)
            .ok_or_else(|| {
                format!(
                    "floating recipes: case {:?} maps to missing pinned case {:?}",
                    recipe_case.id, recipe_case.baseline_case
                )
            })?;
        if baseline.lane != Lane::Pinned
            || baseline.classification != Classification::Support
            || baseline.evidence_path != EvidencePath::BridgeSmoke
            || baseline.probe != ProbeType::Minimal
        {
            return Err(format!(
                "floating recipes: baseline {:?} must be a pinned minimal bridge-smoke support case",
                baseline.id
            )
            .into());
        }
        let execution_matches = matches!(
            (recipe_case.target, baseline.execution_mode),
            (FloatingTarget::HostPackageTree, ExecutionMode::Host)
                | (FloatingTarget::ContainerRoImage, ExecutionMode::ContainerRo)
        );
        if !execution_matches {
            return Err(format!(
                "floating recipes: case {:?} target does not match baseline execution mode",
                recipe_case.id
            )
            .into());
        }
        let package = package_sets
            .get(recipe_case.package_set.as_str())
            .expect("recipe validator checked package-set references");
        let pins = baseline
            .pins
            .as_ref()
            .expect("pinned manifest validation requires pins");
        let adapter_matches = pins
            .adapter
            .as_deref()
            .and_then(|value| value.split_once('='))
            .is_some_and(|(name, _)| name == package.adapter);
        let cli_matches = pins
            .agent_cli
            .as_deref()
            .and_then(|value| value.split_once('='))
            .is_some_and(|(name, _)| name == package.agent_cli);
        if !adapter_matches || !cli_matches {
            return Err(format!(
                "floating recipes: case {:?} package pair does not match its pinned baseline",
                recipe_case.id
            )
            .into());
        }
    }
    Ok((recipes, manifest))
}

#[derive(Debug)]
enum ValidateSource {
    Manifest(PathBuf),
    Recipes(PathBuf),
}

#[derive(Debug)]
struct ValidateArgs {
    source: ValidateSource,
}

fn parse_validate_args(args: &[String]) -> Result<ValidateArgs, BoxError> {
    let mut manifest = None;
    let mut recipes = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--manifest" if manifest.is_some() => {
                return Err("compatibility validate: duplicate --manifest".into())
            }
            "--manifest" => {
                manifest = Some(PathBuf::from(
                    it.next()
                        .ok_or("compatibility validate: --manifest requires a path")?,
                ));
            }
            "--recipes" if recipes.is_some() => {
                return Err("compatibility validate: duplicate --recipes".into())
            }
            "--recipes" => {
                recipes = Some(PathBuf::from(
                    it.next()
                        .ok_or("compatibility validate: --recipes requires a path")?,
                ));
            }
            other => {
                return Err(
                    format!("compatibility validate: unknown argument {other:?}\n{USAGE}").into(),
                )
            }
        }
    }
    if manifest.is_some() && recipes.is_some() {
        return Err(
            "compatibility validate: --manifest and --recipes are mutually exclusive".into(),
        );
    }
    if let Some(recipes) = recipes {
        if recipes.as_os_str().is_empty() {
            return Err("compatibility validate: --recipes must be non-empty".into());
        }
        return Ok(ValidateArgs {
            source: ValidateSource::Recipes(recipes),
        });
    }
    let manifest = manifest.unwrap_or_else(|| PathBuf::from(DEFAULT_MANIFEST));
    if manifest.as_os_str().is_empty() {
        return Err("compatibility validate: --manifest must be non-empty".into());
    }
    Ok(ValidateArgs {
        source: ValidateSource::Manifest(manifest),
    })
}

#[derive(Debug)]
struct ResolveArgs {
    recipes: PathBuf,
    cases: Vec<String>,
    all: bool,
    environment_owner: String,
    runtime: RuntimeKind,
    out: PathBuf,
}

fn parse_resolve_args(args: &[String]) -> Result<ResolveArgs, BoxError> {
    // Resolution is non-billable but it has registry, package-tree, and image-cache effects. The
    // acknowledgement therefore wins before recipe reads, output effects, or runtime lookup.
    if !args
        .iter()
        .any(|arg| arg == "--acknowledge-resolution-effects")
    {
        return Err(format!(
            "compatibility resolve: refusing registry/image effects without --acknowledge-resolution-effects\n{USAGE}"
        )
        .into());
    }

    let mut recipes = None;
    let mut cases = Vec::new();
    let mut all = false;
    let mut environment_owner = None;
    let mut runtime = None;
    let mut out = None;
    let mut acknowledged = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let value = |it: &mut std::slice::Iter<'_, String>, flag: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("compatibility resolve: {flag} requires a value"))
        };
        match arg.as_str() {
            "--acknowledge-resolution-effects" if acknowledged => {
                return Err(
                    "compatibility resolve: duplicate --acknowledge-resolution-effects".into(),
                )
            }
            "--acknowledge-resolution-effects" => acknowledged = true,
            "--recipes" if recipes.is_some() => {
                return Err("compatibility resolve: duplicate --recipes".into())
            }
            "--recipes" => recipes = Some(PathBuf::from(value(&mut it, "--recipes")?)),
            "--case" => cases.push(value(&mut it, "--case")?),
            "--all" if all => return Err("compatibility resolve: duplicate --all".into()),
            "--all" => all = true,
            "--environment-owner" if environment_owner.is_some() => {
                return Err("compatibility resolve: duplicate --environment-owner".into())
            }
            "--environment-owner" => {
                environment_owner = Some(value(&mut it, "--environment-owner")?)
            }
            "--runtime" if runtime.is_some() => {
                return Err("compatibility resolve: duplicate --runtime".into())
            }
            "--runtime" => runtime = Some(RuntimeKind::parse(&value(&mut it, "--runtime")?)?),
            "--out" if out.is_some() => return Err("compatibility resolve: duplicate --out".into()),
            "--out" => out = Some(PathBuf::from(value(&mut it, "--out")?)),
            other => {
                return Err(
                    format!("compatibility resolve: unknown argument {other:?}\n{USAGE}").into(),
                )
            }
        }
    }
    if !acknowledged {
        return Err(format!(
            "compatibility resolve: refusing registry/image effects without --acknowledge-resolution-effects\n{USAGE}"
        )
        .into());
    }
    if all && !cases.is_empty() {
        return Err("compatibility resolve: --all cannot be combined with --case".into());
    }
    if !all && cases.is_empty() {
        return Err(
            "compatibility resolve: explicit selection is required (--case or --all)".into(),
        );
    }
    let mut seen = BTreeSet::new();
    for case in &cases {
        stable_id("selected floating case id", case)?;
        if !seen.insert(case) {
            return Err(format!("compatibility resolve: duplicate --case {case:?}").into());
        }
    }
    let recipes =
        recipes.unwrap_or_else(|| PathBuf::from(compatibility_resolution::DEFAULT_RECIPES));
    if recipes.as_os_str().is_empty() {
        return Err("compatibility resolve: --recipes must be non-empty".into());
    }
    let environment_owner =
        environment_owner.ok_or("compatibility resolve: --environment-owner is required")?;
    stable_id("environment owner", &environment_owner)?;
    let runtime = runtime.ok_or("compatibility resolve: --runtime is required")?;
    let out = out.ok_or("compatibility resolve: --out is required")?;
    if out.as_os_str().is_empty() || out == Path::new("-") {
        return Err(
            "compatibility resolve: --out requires an explicit non-empty directory path".into(),
        );
    }
    Ok(ResolveArgs {
        recipes,
        cases,
        all,
        environment_owner,
        runtime,
        out,
    })
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SelectionRecord {
    #[serde(skip_serializing_if = "Option::is_none")]
    lane: Option<Lane>,
    cases: Vec<String>,
    all: bool,
}

#[derive(Debug)]
enum RunSource {
    Manifest(PathBuf),
    Resolution(PathBuf),
}

#[derive(Debug)]
struct RunArgs {
    source: RunSource,
    selection: SelectionRecord,
    environment_owner: String,
    out: PathBuf,
}

fn parse_run_args(args: &[String]) -> Result<RunArgs, BoxError> {
    // This barrier deliberately wins before manifest lookup, output creation, config resolution, or
    // environment probing. A token consumed as another flag's value is caught by `acknowledged` below.
    if !args.iter().any(|arg| arg == "--acknowledge-billable") {
        return Err(format!(
            "compatibility run: refusing potentially billable cases without --acknowledge-billable\n{USAGE}"
        )
        .into());
    }

    let mut manifest = None;
    let mut resolution = None;
    let mut lane = None;
    let mut cases = Vec::new();
    let mut all = false;
    let mut all_resolved = false;
    let mut environment_owner = None;
    let mut out = None;
    let mut acknowledged = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let value = |it: &mut std::slice::Iter<'_, String>, flag: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("compatibility run: {flag} requires a value"))
        };
        match arg.as_str() {
            "--acknowledge-billable" if acknowledged => {
                return Err("compatibility run: duplicate --acknowledge-billable".into())
            }
            "--acknowledge-billable" => acknowledged = true,
            "--manifest" if manifest.is_some() => {
                return Err("compatibility run: duplicate --manifest".into())
            }
            "--manifest" => manifest = Some(PathBuf::from(value(&mut it, "--manifest")?)),
            "--resolution" if resolution.is_some() => {
                return Err("compatibility run: duplicate --resolution".into())
            }
            "--resolution" => resolution = Some(PathBuf::from(value(&mut it, "--resolution")?)),
            "--lane" if lane.is_some() => return Err("compatibility run: duplicate --lane".into()),
            "--lane" => lane = Some(Lane::parse(&value(&mut it, "--lane")?)?),
            "--case" => cases.push(value(&mut it, "--case")?),
            "--all" if all => return Err("compatibility run: duplicate --all".into()),
            "--all" => all = true,
            "--all-resolved" if all_resolved => {
                return Err("compatibility run: duplicate --all-resolved".into())
            }
            "--all-resolved" => all_resolved = true,
            "--environment-owner" if environment_owner.is_some() => {
                return Err("compatibility run: duplicate --environment-owner".into())
            }
            "--environment-owner" => {
                environment_owner = Some(value(&mut it, "--environment-owner")?)
            }
            "--out" if out.is_some() => return Err("compatibility run: duplicate --out".into()),
            "--out" => out = Some(PathBuf::from(value(&mut it, "--out")?)),
            other => {
                return Err(
                    format!("compatibility run: unknown argument {other:?}\n{USAGE}").into(),
                )
            }
        }
    }
    if !acknowledged {
        return Err(format!(
            "compatibility run: refusing potentially billable cases without --acknowledge-billable\n{USAGE}"
        )
        .into());
    }
    if manifest.is_some() && resolution.is_some() {
        return Err("compatibility run: --manifest and --resolution are mutually exclusive".into());
    }
    if all && (lane.is_some() || !cases.is_empty() || all_resolved) {
        return Err("compatibility run: --all cannot be combined with --lane or --case".into());
    }
    if lane.is_some() && !cases.is_empty() {
        return Err("compatibility run: --lane cannot be combined with --case".into());
    }
    if resolution.is_some() {
        if manifest.is_some() || lane.is_some() || all {
            return Err(
                "compatibility run: --resolution cannot be combined with --manifest, --lane, or --all"
                    .into(),
            );
        }
        if all_resolved && !cases.is_empty() {
            return Err("compatibility run: --all-resolved cannot be combined with --case".into());
        }
        if !all_resolved && cases.is_empty() {
            return Err(
                "compatibility run: explicit resolved selection is required (--case or --all-resolved)"
                    .into(),
            );
        }
    } else {
        if all_resolved {
            return Err("compatibility run: --all-resolved requires --resolution".into());
        }
        if !all && lane.is_none() && cases.is_empty() {
            return Err(
                "compatibility run: explicit selection is required (--lane, --case, or --all)"
                    .into(),
            );
        }
        if lane == Some(Lane::FloatingCurrent) {
            return Err("compatibility run: floating_resolution_required; use --resolution".into());
        }
    }
    let mut seen = BTreeSet::new();
    for case in &cases {
        stable_id("selected case id", case)?;
        if !seen.insert(case) {
            return Err(format!("compatibility run: duplicate --case {case:?}").into());
        }
    }
    let environment_owner =
        environment_owner.ok_or("compatibility run: --environment-owner is required")?;
    stable_id("environment owner", &environment_owner)?;
    let out = out.ok_or("compatibility run: --out is required")?;
    if out.as_os_str().is_empty() || out == Path::new("-") {
        return Err("compatibility run: --out requires an explicit non-empty file path".into());
    }
    let source = match resolution {
        Some(path) => {
            if path.as_os_str().is_empty() {
                return Err("compatibility run: --resolution must be non-empty".into());
            }
            RunSource::Resolution(path)
        }
        None => {
            let manifest = manifest.unwrap_or_else(|| PathBuf::from(DEFAULT_MANIFEST));
            if manifest.as_os_str().is_empty() {
                return Err("compatibility run: --manifest must be non-empty".into());
            }
            RunSource::Manifest(manifest)
        }
    };
    Ok(RunArgs {
        source,
        selection: SelectionRecord {
            lane,
            cases,
            all: all || all_resolved,
        },
        environment_owner,
        out,
    })
}

#[derive(Debug)]
enum ComparisonMode {
    Pinned,
    FloatingToPinned,
}

#[derive(Debug)]
struct CompareArgs {
    current: PathBuf,
    baseline: PathBuf,
    mode: ComparisonMode,
}

fn parse_compare_args(args: &[String]) -> Result<CompareArgs, BoxError> {
    let mut current = None;
    let mut baseline = None;
    let mut mode = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--current" if current.is_some() => {
                return Err("compatibility compare: duplicate --current".into())
            }
            "--current" => {
                current = Some(PathBuf::from(
                    it.next()
                        .ok_or("compatibility compare: --current requires a path")?,
                ));
            }
            "--baseline" if baseline.is_some() => {
                return Err("compatibility compare: duplicate --baseline".into())
            }
            "--baseline" => {
                baseline = Some(PathBuf::from(
                    it.next()
                        .ok_or("compatibility compare: --baseline requires a path")?,
                ));
            }
            "--mode" if mode.is_some() => {
                return Err("compatibility compare: duplicate --mode".into())
            }
            "--mode" => {
                mode = Some(
                    match it
                        .next()
                        .ok_or("compatibility compare: --mode requires a value")?
                        .as_str()
                    {
                        "pinned" => ComparisonMode::Pinned,
                        "floating-to-pinned" => ComparisonMode::FloatingToPinned,
                        _ => return Err(
                            "compatibility compare: --mode must be pinned or floating-to-pinned"
                                .into(),
                        ),
                    },
                );
            }
            other => {
                return Err(
                    format!("compatibility compare: unknown argument {other:?}\n{USAGE}").into(),
                )
            }
        }
    }
    let current = current.ok_or("compatibility compare: --current is required")?;
    if current.as_os_str().is_empty() {
        return Err("compatibility compare: --current must be non-empty".into());
    }
    let baseline = baseline.unwrap_or_else(|| PathBuf::from(DEFAULT_BASELINE));
    if baseline.as_os_str().is_empty() {
        return Err("compatibility compare: --baseline must be non-empty".into());
    }
    Ok(CompareArgs {
        current,
        baseline,
        mode: mode.unwrap_or(ComparisonMode::Pinned),
    })
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ManifestIdentity {
    schema_version: u16,
    canonical_path: String,
    sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CandidateIdentity {
    canonical_path: String,
    sha256: String,
    byte_length: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BudgetOutcome {
    timeout_secs: u64,
    max_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_cost_usd: Option<f64>,
    observed_tokens: u64,
    observed_cost_usd: f64,
    token_observation_missing_cases: u32,
    cost_observation_missing_cases: u32,
    exhausted: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ExecutionState {
    Completed,
    NotRun,
    RunnerFailure,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
enum CandidateOutcome {
    #[serde(rename = "candidate_pass")]
    Pass,
    #[serde(rename = "candidate_fail")]
    Fail,
    #[serde(rename = "candidate_unknown")]
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct FloatingSummary {
    candidate_pass: u32,
    candidate_fail: u32,
    candidate_unknown: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CaseResult {
    case_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    baseline_case_id: Option<String>,
    lane: Lane,
    evidence_path: EvidencePath,
    probe: ProbeType,
    billable: bool,
    execution: ExecutionState,
    expected_status: EvidenceStatus,
    actual_status: EvidenceStatus,
    expectation_met: bool,
    classification: Classification,
    #[serde(skip_serializing_if = "Option::is_none")]
    candidate_outcome: Option<CandidateOutcome>,
    artifact_policy: ArtifactPolicy,
    duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    not_run_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runner_error_code: Option<String>,
    drift: Vec<String>,
    budget_violations: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    smoke: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AggregateArtifact {
    schema_version: u16,
    candidate: CandidateIdentity,
    manifest: ManifestIdentity,
    selection: SelectionRecord,
    environment_owner: String,
    started_at_ms: i64,
    ended_at_ms: i64,
    cancelled: bool,
    success: bool,
    budget: BudgetOutcome,
    results: Vec<CaseResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    floating_summary: Option<FloatingSummary>,
}

fn floating_summary(results: &[CaseResult]) -> Option<FloatingSummary> {
    let mut summary = FloatingSummary {
        candidate_pass: 0,
        candidate_fail: 0,
        candidate_unknown: 0,
    };
    let mut any = false;
    for result in results {
        match result.candidate_outcome {
            Some(CandidateOutcome::Pass) => {
                any = true;
                summary.candidate_pass += 1;
            }
            Some(CandidateOutcome::Fail) => {
                any = true;
                summary.candidate_fail += 1;
            }
            Some(CandidateOutcome::Unknown) => {
                any = true;
                summary.candidate_unknown += 1;
            }
            None => {}
        }
    }
    any.then_some(summary)
}

#[derive(Clone, Debug)]
struct SmokeRequest {
    agent: String,
    config: PathBuf,
    model: String,
    effort: Option<String>,
    mode: Option<String>,
    session_cwd: Option<PathBuf>,
    timeout_secs: u64,
    artifact_path: PathBuf,
}

#[derive(Debug)]
struct InvocationResult {
    artifact: Option<Value>,
    process_success: bool,
    runner_error_code: Option<&'static str>,
    not_run_reason: Option<&'static str>,
}

impl InvocationResult {
    fn admission_rejected(reason: &'static str) -> Self {
        Self {
            artifact: None,
            process_success: false,
            runner_error_code: None,
            not_run_reason: Some(reason),
        }
    }
}

struct SpawnAdmission<'a> {
    cancellation_requested: &'a std::sync::atomic::AtomicBool,
    started: Instant,
    total_timeout: Duration,
    case_timeout: Duration,
}

impl SpawnAdmission<'_> {
    fn reason(&self) -> Option<&'static str> {
        if self
            .cancellation_requested
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Some("cancellation_requested");
        }
        let remaining = self
            .total_timeout
            .checked_sub(Instant::now().saturating_duration_since(self.started));
        if remaining.is_none_or(|remaining| remaining < self.case_timeout) {
            return Some("total_budget_insufficient_for_case");
        }
        None
    }
}

#[async_trait]
trait SmokeInvoker: Send + Sync {
    async fn invoke(
        &self,
        request: &SmokeRequest,
        admission: &SpawnAdmission<'_>,
    ) -> InvocationResult;
}

struct StagedExecutable {
    file: Arc<File>,
    object_path: PathBuf,
    #[cfg(test)]
    staged_path: PathBuf,
    retain_executable_after_exec: bool,
}

struct ProcessSmokeInvoker<'a> {
    executable: StagedExecutable,
    artifact_directory: &'a local_file::PinnedDirectory,
    expected_sha256: String,
}

#[async_trait]
impl SmokeInvoker for ProcessSmokeInvoker<'_> {
    async fn invoke(
        &self,
        request: &SmokeRequest,
        admission: &SpawnAdmission<'_>,
    ) -> InvocationResult {
        self.invoke_after_candidate_check(request, admission, || {})
            .await
    }
}

impl ProcessSmokeInvoker<'_> {
    async fn invoke_after_candidate_check<F>(
        &self,
        request: &SmokeRequest,
        admission: &SpawnAdmission<'_>,
        after_candidate_check: F,
    ) -> InvocationResult
    where
        F: FnOnce(),
    {
        let candidate_sha256 = match local_file::sha256_regular_file_bounded(
            &self.executable.file,
            "compatibility staged candidate",
            MAX_EXECUTABLE_BYTES,
        ) {
            Ok(sha256) if sha256 == self.expected_sha256 => sha256,
            _ => {
                return InvocationResult {
                    artifact: None,
                    process_success: false,
                    runner_error_code: Some("candidate_binary_changed"),
                    not_run_reason: None,
                }
            }
        };
        after_candidate_check();
        if let Some(reason) = admission.reason() {
            return InvocationResult::admission_rejected(reason);
        }
        #[cfg(unix)]
        let mut command = {
            use std::os::fd::AsRawFd as _;
            use std::os::unix::process::CommandExt as _;

            let executable_fd = self.executable.file.as_raw_fd();
            let retained_directory = self.artifact_directory.file_handle();
            let retained_directory_fd = retained_directory.as_raw_fd();
            let executable_path = self.executable.object_path.clone();
            let retain_executable = self.executable.retain_executable_after_exec;
            let retain_directory = self.artifact_directory.retain_descriptor_after_exec();
            let mut command = tokio::process::Command::new(executable_path);
            command.arg("smoke");
            #[cfg(target_os = "linux")]
            {
                command
                    .env_remove(INHERITED_EXECUTABLE_FD_ENV)
                    .env_remove(INHERITED_SCRATCH_FD_ENV);
                if retain_executable {
                    command
                        .arg("--internal-compat-executable-fd")
                        .arg(executable_fd.to_string());
                }
                if retain_directory {
                    command
                        .arg("--internal-compat-scratch-fd")
                        .arg(retained_directory_fd.to_string());
                }
            }
            // SAFETY: this callback runs after fork and before exec. It performs only async-signal-
            // safe fcntl calls. The bridge parent's descriptors stay FD_CLOEXEC; only the forked
            // smoke child retains the verified executable and, on Linux, its descriptor-backed
            // scratch path.
            unsafe {
                command.as_std_mut().pre_exec(move || {
                    if retain_executable {
                        clear_close_on_exec(executable_fd)?;
                    }
                    if retain_directory && retained_directory_fd != executable_fd {
                        clear_close_on_exec(retained_directory_fd)?;
                    }
                    Ok(())
                });
            }
            command
        };
        #[cfg(not(unix))]
        {
            let _ = candidate_sha256;
            return InvocationResult {
                artifact: None,
                process_success: false,
                runner_error_code: Some("smoke_process_launch_failed"),
                not_run_reason: None,
            };
        }
        #[cfg(unix)]
        command
            .arg("--agent")
            .arg(&request.agent)
            .arg("--config")
            .arg(&request.config)
            .arg("--model")
            .arg(&request.model)
            .arg("--timeout-secs")
            .arg(request.timeout_secs.to_string())
            .arg("--acknowledge-billable")
            .arg("--out")
            .arg(&request.artifact_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        drop(candidate_sha256);
        if let Some(effort) = &request.effort {
            command.arg("--effort").arg(effort);
        }
        if let Some(mode) = &request.mode {
            command.arg("--mode").arg(mode);
        }
        if let Some(session_cwd) = &request.session_cwd {
            command.arg("--session-cwd").arg(session_cwd);
        }

        let status = match command.status().await {
            Ok(status) => status,
            Err(_) => {
                return InvocationResult {
                    artifact: None,
                    process_success: false,
                    runner_error_code: Some("smoke_process_launch_failed"),
                    not_run_reason: None,
                }
            }
        };
        let process_success = status.success();
        let Some(artifact_name) = request.artifact_path.file_name() else {
            return InvocationResult {
                artifact: None,
                process_success,
                runner_error_code: Some("smoke_artifact_missing_or_invalid_file"),
                not_run_reason: None,
            };
        };
        let snapshot = match self
            .artifact_directory
            .open_regular_file(artifact_name, "compatibility smoke artifact")
            .and_then(|file| {
                local_file::read_open_regular_file_bounded(
                    &file,
                    "compatibility smoke artifact",
                    MAX_AGGREGATE_BYTES,
                )
            }) {
            Ok(snapshot) => snapshot,
            Err(_) => {
                let _ = self.artifact_directory.remove_child(
                    artifact_name,
                    false,
                    "compatibility smoke artifact cleanup",
                );
                return InvocationResult {
                    artifact: None,
                    process_success,
                    runner_error_code: Some("smoke_artifact_missing_or_invalid_file"),
                    not_run_reason: None,
                };
            }
        };
        let _ = self.artifact_directory.remove_child(
            artifact_name,
            false,
            "compatibility smoke artifact cleanup",
        );
        match serde_json::from_slice(&snapshot.bytes) {
            Ok(artifact) => InvocationResult {
                artifact: Some(artifact),
                process_success,
                runner_error_code: None,
                not_run_reason: None,
            },
            Err(_) => InvocationResult {
                artifact: None,
                process_success,
                runner_error_code: Some("smoke_artifact_invalid_json"),
                not_run_reason: None,
            },
        }
    }
}

#[cfg(unix)]
fn clear_close_on_exec(fd: std::os::fd::RawFd) -> std::io::Result<()> {
    // SAFETY: the caller supplies a live descriptor in the forked child. fcntl is async-signal-safe.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn close_raw_descriptor(fd: std::os::fd::RawFd, label: &str) -> Result<(), BoxError> {
    if fd <= libc::STDERR_FILENO {
        return Err(format!("{label}: inherited descriptor must be greater than stderr").into());
    }
    // SAFETY: the compatibility parent passed this live descriptor specifically for the staged
    // smoke child. Closing it transfers no ownership and occurs before any provider child exists.
    if unsafe { libc::close(fd) } == -1 {
        return Err(format!(
            "{label}: cannot close inherited descriptor: {}",
            std::io::Error::last_os_error()
        )
        .into());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn descriptor_metadata(fd: i32, label: &str) -> Result<std::fs::Metadata, BoxError> {
    if fd <= libc::STDERR_FILENO {
        return Err(format!("{label}: inherited descriptor must be greater than stderr").into());
    }
    std::fs::metadata(format!("/proc/self/fd/{fd}"))
        .map_err(|error| format!("{label}: cannot inspect inherited descriptor: {error}").into())
}

#[cfg(target_os = "linux")]
fn same_linux_object(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    left.dev() == right.dev() && left.ino() == right.ino()
}

pub(crate) fn close_inherited_compatibility_executable(fd: Option<i32>) -> Result<(), BoxError> {
    #[cfg(target_os = "linux")]
    if let Some(fd) = fd {
        let descriptor = descriptor_metadata(fd, "compatibility staged executable")?;
        let running = std::fs::metadata("/proc/self/exe").map_err(|error| {
            format!("compatibility staged executable: cannot inspect running executable: {error}")
        })?;
        if !descriptor.is_file() || !same_linux_object(&descriptor, &running) {
            return Err(
                "compatibility staged executable: inherited descriptor does not identify the running executable"
                    .into(),
            );
        }
        close_raw_descriptor(fd, "compatibility staged executable")?;
    }
    #[cfg(not(target_os = "linux"))]
    if fd.is_some() {
        return Err("compatibility staged executable: inherited descriptor is Linux-only".into());
    }
    Ok(())
}

pub(crate) fn close_inherited_compatibility_scratch(
    fd: Option<i32>,
    artifact_path: Option<&Path>,
) -> Result<(), BoxError> {
    #[cfg(target_os = "linux")]
    if let Some(fd) = fd {
        let artifact_path = artifact_path
            .ok_or("compatibility staged scratch directory: inherited descriptor requires --out")?;
        let parent = artifact_path.parent().unwrap_or_else(|| Path::new("."));
        let descriptor = descriptor_metadata(fd, "compatibility staged scratch directory")?;
        let output_parent = std::fs::metadata(parent).map_err(|error| {
            format!("compatibility staged scratch directory: cannot inspect --out parent: {error}")
        })?;
        if !descriptor.is_dir() || !same_linux_object(&descriptor, &output_parent) {
            return Err(
                "compatibility staged scratch directory: inherited descriptor does not identify the --out parent"
                    .into(),
            );
        }
        close_raw_descriptor(fd, "compatibility staged scratch directory")?;
    }
    #[cfg(not(target_os = "linux"))]
    if fd.is_some() {
        let _ = artifact_path;
        return Err(
            "compatibility staged scratch directory: inherited descriptor is Linux-only".into(),
        );
    }
    Ok(())
}

struct ScratchDir {
    path: PathBuf,
    pin: local_file::PinnedDirectory,
    parent: Arc<File>,
    name: OsString,
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        if let Ok(entries) = std::fs::read_dir(&self.path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    let _ = std::fs::remove_dir_all(path);
                } else {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd as _;
            use std::os::unix::ffi::OsStrExt as _;

            if let Ok(name) = std::ffi::CString::new(self.name.as_os_str().as_bytes()) {
                // SAFETY: `parent` retains the directory descriptor through Drop and `name` is the
                // single component created beneath it. Cleanup is best effort.
                unsafe {
                    libc::unlinkat(self.parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR);
                }
            }
        }
    }
}

fn create_scratch_dir(parent: &local_file::PinnedDirectory) -> Result<ScratchDir, BoxError> {
    let name = OsString::from(format!(
        ".a2a-compat-{}-{}",
        std::process::id(),
        crate::implement::nonce(20)
    ));
    let pin =
        parent.create_child_directory(&name, 0o700, "compatibility private scratch directory")?;
    let path = pin.acp_session_cwd();
    Ok(ScratchDir {
        path,
        pin,
        parent: parent.file_handle(),
        name,
    })
}

fn stage_candidate(
    snapshot: &local_file::LocalFileSnapshot,
    scratch: &ScratchDir,
) -> Result<StagedExecutable, BoxError> {
    let name = OsStr::new("a2a-bridge-candidate");
    #[cfg(test)]
    let staged_path = scratch.path.join(name);
    // The creating descriptor is writable even though the directory entry is mode 0500. Publishing
    // the inode without owner-write permission prevents another ordinary same-owner opener from
    // retaining a writable handle across the digest-to-exec boundary.
    let mut file = scratch
        .pin
        .create_new_file(name, 0o500, "compatibility staged candidate")?;
    file.write_all(&snapshot.bytes)?;
    file.flush()?;
    file.sync_all()?;
    drop(file);
    let file = scratch
        .pin
        .open_regular_file(name, "compatibility staged candidate")?;
    let staged_sha256 = local_file::sha256_regular_file_bounded(
        &file,
        "compatibility staged candidate",
        MAX_EXECUTABLE_BYTES,
    )?;
    if staged_sha256 != snapshot.sha256 {
        return Err("compatibility run: staged candidate digest mismatch".into());
    }
    let (object_path, retain_executable_after_exec) =
        local_file::stable_regular_file_path(&file, "compatibility staged candidate")?;
    Ok(StagedExecutable {
        file: Arc::new(file),
        object_path,
        #[cfg(test)]
        staged_path,
        retain_executable_after_exec,
    })
}

fn repository_root(path: &Path) -> Option<PathBuf> {
    let start = if path.is_dir() { path } else { path.parent()? };
    start
        .ancestors()
        .find(|ancestor| {
            std::fs::symlink_metadata(ancestor.join(".git")).is_ok()
                || looks_like_bare_git_repository(ancestor)
        })
        .map(Path::to_path_buf)
}

fn looks_like_bare_git_repository(path: &Path) -> bool {
    std::fs::metadata(path.join("HEAD")).is_ok_and(|metadata| metadata.is_file())
        && std::fs::metadata(path.join("objects")).is_ok_and(|metadata| metadata.is_dir())
        && (std::fs::metadata(path.join("refs")).is_ok_and(|metadata| metadata.is_dir())
            || std::fs::metadata(path.join("packed-refs")).is_ok_and(|metadata| metadata.is_file()))
}

struct PinnedOutputDirectory {
    pin: local_file::PinnedDirectory,
    output_name: OsString,
}

impl PinnedOutputDirectory {
    fn prepare_output(&self) -> Result<File, BoxError> {
        self.prepare_output_after_guard(|| {})
    }

    fn prepare_output_after_guard<F>(&self, after_guard: F) -> Result<File, BoxError>
    where
        F: FnOnce(),
    {
        if !self.pin.current_path_matches() {
            return Err(
                "compatibility run: aggregate parent identity changed before output creation"
                    .into(),
            );
        }
        after_guard();
        let file =
            self.pin
                .create_new_file(&self.output_name, 0o600, "compatibility aggregate output")?;
        if !self.pin.current_path_matches() {
            drop(file);
            let _ = self.pin.remove_child(
                &self.output_name,
                false,
                "compatibility aggregate output cleanup",
            );
            return Err(
                "compatibility run: aggregate parent identity changed during output creation"
                    .into(),
            );
        }
        Ok(file)
    }

    fn prepare_output_with_setup_evidence(
        &self,
        aggregate: &AggregateArtifact,
    ) -> Result<File, BoxError> {
        let mut file = self.prepare_output()?;
        if let Err(error) = write_aggregate(&mut file, aggregate) {
            drop(file);
            let _ = self.pin.remove_child(
                &self.output_name,
                false,
                "compatibility aggregate output cleanup",
            );
            return Err(error);
        }
        Ok(file)
    }

    fn replace_setup_with_final(
        &self,
        setup_file: &File,
        setup_aggregate: &AggregateArtifact,
        aggregate: &AggregateArtifact,
    ) -> Result<(), BoxError> {
        let replacement_name = OsString::from(format!(
            ".a2a-compat-final-{}-{}",
            std::process::id(),
            crate::implement::nonce(20)
        ));
        let rollback_name = OsString::from(format!(
            ".a2a-compat-setup-{}-{}",
            std::process::id(),
            crate::implement::nonce(20)
        ));
        let mut replacement =
            self.pin
                .create_new_file(&replacement_name, 0o600, "compatibility final aggregate")?;
        let mut rollback = match self.pin.create_new_file(
            &rollback_name,
            0o600,
            "compatibility setup rollback aggregate",
        ) {
            Ok(rollback) => rollback,
            Err(error) => {
                drop(replacement);
                let _ = self.pin.remove_child(
                    &replacement_name,
                    false,
                    "compatibility final aggregate cleanup",
                );
                return Err(error);
            }
        };
        if let Err(error) = write_aggregate(&mut replacement, aggregate)
            .and_then(|()| write_aggregate(&mut rollback, setup_aggregate))
        {
            drop(replacement);
            drop(rollback);
            let _ = self.pin.remove_child(
                &replacement_name,
                false,
                "compatibility final aggregate cleanup",
            );
            let _ = self.pin.remove_child(
                &rollback_name,
                false,
                "compatibility setup rollback cleanup",
            );
            return Err(error);
        }
        let published = self.pin.replace_regular_child(
            local_file::RegularChildRef::new(&self.output_name, setup_file),
            local_file::RegularChildRef::new(&replacement_name, &replacement),
            local_file::RegularChildRef::new(&rollback_name, &rollback),
            "compatibility final aggregate",
        );
        if let Err(error) = published {
            drop(replacement);
            drop(rollback);
            let _ = self.pin.remove_child(
                &replacement_name,
                false,
                "compatibility final aggregate cleanup",
            );
            return Err(error);
        }
        drop(rollback);
        Ok(())
    }

    fn create_scratch(&self) -> Result<ScratchDir, BoxError> {
        if !self.pin.current_path_matches() {
            return Err(
                "compatibility run: aggregate parent identity changed before scratch creation"
                    .into(),
            );
        }
        let scratch = create_scratch_dir(&self.pin)?;
        if !self.pin.current_path_matches() {
            return Err(
                "compatibility run: aggregate parent identity changed during scratch creation"
                    .into(),
            );
        }
        Ok(scratch)
    }
}

fn ensure_output_outside_repositories(output: &Path) -> Result<PinnedOutputDirectory, BoxError> {
    let file_name = output
        .file_name()
        .ok_or("compatibility run: --out must name a file")?;
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    let snapshot = local_file::snapshot_directory(parent, "compatibility aggregate parent")?;
    let canonical_parent = PathBuf::from(snapshot.canonical_cwd.as_str());
    if repository_root(&canonical_parent).is_some() {
        return Err("compatibility run: --out must be outside any repository".into());
    }
    let pin = local_file::PinnedDirectory::open(
        parent,
        &snapshot.canonical_cwd,
        &snapshot.identity,
        "compatibility aggregate parent",
    )?;
    Ok(PinnedOutputDirectory {
        pin,
        output_name: file_name.to_os_string(),
    })
}

fn aggregate_bytes(aggregate: &AggregateArtifact) -> Result<Vec<u8>, BoxError> {
    let mut bytes = serde_json::to_vec(aggregate)?;
    bytes.push(b'\n');
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_AGGREGATE_BYTES {
        return Err(
            format!("compatibility run: aggregate exceeds {MAX_AGGREGATE_BYTES} bytes").into(),
        );
    }
    Ok(bytes)
}

fn write_aggregate(file: &mut File, aggregate: &AggregateArtifact) -> Result<(), BoxError> {
    let bytes = aggregate_bytes(aggregate)?;
    file.write_all(&bytes)?;
    file.flush()?;
    file.sync_all()?;
    Ok(())
}

fn select_case_indices(
    manifest: &CompatibilityManifest,
    selection: &SelectionRecord,
) -> Result<Vec<usize>, BoxError> {
    let ids: BTreeSet<&str> = manifest.cases.iter().map(|case| case.id.as_str()).collect();
    for requested in &selection.cases {
        if !ids.contains(requested.as_str()) {
            return Err(format!(
                "compatibility run: selected case {requested:?} is not in the manifest"
            )
            .into());
        }
    }
    let selected: Vec<_> = manifest
        .cases
        .iter()
        .enumerate()
        .filter(|(_, case)| {
            if selection.all {
                return true;
            }
            let lane_matches = selection.lane.is_none_or(|lane| case.lane == lane);
            let case_matches =
                selection.cases.is_empty() || selection.cases.iter().any(|id| id == &case.id);
            lane_matches && case_matches
        })
        .map(|(index, _)| index)
        .collect();
    if selected.is_empty() {
        return Err("compatibility run: explicit selection resolved to zero cases".into());
    }
    Ok(selected)
}

fn resolve_case_path(manifest_path: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(path)
    }
}

fn value_contains_secret(value: &Value, known_secrets: &[String]) -> bool {
    match value {
        Value::String(text) => {
            looks_like_secret(text) || known_secrets.iter().any(|secret| text.contains(secret))
        }
        Value::Array(values) => values
            .iter()
            .any(|value| value_contains_secret(value, known_secrets)),
        Value::Object(values) => values.iter().any(|(key, value)| {
            sensitive_json_key(key)
                || looks_like_secret(key)
                || value_contains_secret(value, known_secrets)
        }),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

fn sensitive_json_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "api_key"
            | "apikey"
            | "token"
            | "access_token"
            | "refresh_token"
            | "password"
            | "secret"
            | "credential"
            | "authorization"
    ) || lower.ends_with("_api_key")
        || lower.ends_with("_access_token")
        || lower.ends_with("_refresh_token")
        || lower.ends_with("_password")
        || lower.ends_with("_secret")
}

fn valid_smoke_shape(value: &Value, case: &CompatibilityCase) -> bool {
    value.get("schema_version").and_then(Value::as_u64) == Some(2)
        && value.get("success").and_then(Value::as_bool).is_some()
        && value
            .pointer("/attempt/id")
            .and_then(Value::as_str)
            .is_some()
        && value.pointer("/request/agent").and_then(Value::as_str) == Some(case.agent.as_str())
        && value.pointer("/turn/prompt").and_then(Value::as_str) == Some(crate::smoke::FIXED_PROMPT)
        && value.get("diagnostics").is_some_and(Value::is_object)
        && value.get("cleanup").is_some_and(Value::is_object)
}

fn provenance_detail<'a>(smoke: &'a Value, agent: &str, component: &str) -> Option<&'a str> {
    let expected_check = format!("provenance:{agent}:{component}");
    let mut matches = smoke
        .pointer("/target/provenance")?
        .as_array()?
        .iter()
        .filter(|row| row.get("check").and_then(Value::as_str) == Some(expected_check.as_str()));
    let row = matches.next()?;
    if matches.next().is_some() || row.get("status").and_then(Value::as_str) != Some("ok") {
        return None;
    }
    row.get("detail").and_then(Value::as_str)
}

fn unique_detail_field<'a>(detail: &'a str, field: &str) -> Option<&'a str> {
    let mut matches = detail.split_ascii_whitespace().filter_map(|part| {
        let (name, value) = part.split_once('=')?;
        (name == field && !value.is_empty()).then_some(value)
    });
    let value = matches.next()?;
    matches.next().is_none().then_some(value)
}

fn provenance_package_matches(smoke: &Value, agent: &str, component: &str, expected: &str) -> bool {
    let Some(detail) = provenance_detail(smoke, agent, component) else {
        return false;
    };
    let Some(package) = unique_detail_field(detail, "package") else {
        return false;
    };
    let Some(version) = unique_detail_field(detail, "version") else {
        return false;
    };
    format!("{package}={version}") == expected
}

fn provenance_field_matches(
    smoke: &Value,
    agent: &str,
    component: &str,
    field: &str,
    expected: &str,
) -> bool {
    provenance_detail(smoke, agent, component).and_then(|detail| unique_detail_field(detail, field))
        == Some(expected)
}

fn optional_request_field_matches(smoke: &Value, field: &str, expected: Option<&str>) -> bool {
    let actual = smoke.get("request").and_then(|request| request.get(field));
    match expected {
        Some(expected) => actual.and_then(Value::as_str) == Some(expected),
        None => actual.is_none_or(Value::is_null),
    }
}

fn effective_field_matches(smoke: &Value, field: &str, expected: &str) -> bool {
    smoke
        .pointer(&format!("/session/effective_request/{field}"))
        .and_then(Value::as_str)
        == Some(expected)
}

fn api_key_authentication_matches(smoke: &Value, expected_env: &str) -> bool {
    smoke
        .pointer("/target/authentication/name/state")
        .and_then(Value::as_str)
        == Some("value")
        && smoke
            .pointer("/target/authentication/name/value")
            .and_then(Value::as_str)
            == Some(expected_env)
        && smoke
            .pointer("/target/authentication/present")
            .and_then(Value::as_bool)
            == Some(true)
}

fn drift_for(case: &CompatibilityCase, smoke: &Value) -> Vec<String> {
    let mut drift = Vec::new();
    let success = smoke
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if smoke.pointer("/request/model").and_then(Value::as_str) != Some(case.model.as_str()) {
        drift.push("capability.model".into());
    }
    if !optional_request_field_matches(smoke, "effort", case.effort.as_deref()) {
        drift.push("capability.effort".into());
    }
    if !optional_request_field_matches(smoke, "mode", case.mode.as_deref()) {
        drift.push("capability.mode".into());
    }
    if success {
        if !effective_field_matches(smoke, "model", &case.model) {
            drift.push("capability.effective_model".into());
        }
        if case
            .effort
            .as_deref()
            .is_some_and(|expected| !effective_field_matches(smoke, "effort", expected))
        {
            drift.push("capability.effective_effort".into());
        }
        if case
            .mode
            .as_deref()
            .is_some_and(|expected| !effective_field_matches(smoke, "mode", expected))
        {
            drift.push("capability.effective_mode".into());
        }
    }
    if success || smoke.get("target").is_some_and(|value| !value.is_null()) {
        if smoke
            .pointer("/target/execution_mode")
            .and_then(Value::as_str)
            != Some(case.execution_mode.wire())
        {
            drift.push("provenance.execution_mode".into());
        }
        if smoke
            .pointer("/target/authentication/path")
            .and_then(Value::as_str)
            != Some(case.auth_path.wire())
        {
            drift.push("authentication.path".into());
        } else if case.auth_path == AuthPath::ApiKeyEnv
            && case
                .credential_env
                .as_deref()
                .is_none_or(|expected| !api_key_authentication_matches(smoke, expected))
        {
            drift.push("authentication.credential_env".into());
        }
        if let Some(image) = &case.expected_image_digest {
            if !provenance_field_matches(smoke, &case.agent, "image", "immutable_id", image) {
                drift.push("provenance.image".into());
            }
        }
    }
    if let Some(pins) = &case.pins {
        if smoke
            .pointer("/request/config_sha256")
            .and_then(Value::as_str)
            != Some(pins.config_sha256.as_str())
        {
            drift.push("provenance.config_sha256".into());
        }
        if pins.adapter.as_ref().is_some_and(|expected| {
            !provenance_package_matches(smoke, &case.agent, "adapter", expected)
        }) {
            drift.push("provenance.adapter".into());
        }
        if pins.agent_cli.as_ref().is_some_and(|expected| {
            !provenance_package_matches(smoke, &case.agent, "agent-cli", expected)
        }) {
            drift.push("provenance.agent_cli".into());
        }
        for (component, expected) in &pins.components {
            if provenance_detail(smoke, &case.agent, component) != Some(expected) {
                drift.push(format!("provenance.component.{component}"));
            }
        }
    }
    drift
}

fn observed_tokens(smoke: &Value) -> Option<u64> {
    smoke
        .pointer("/turn/usage/terminal/totalTokens")
        .and_then(Value::as_u64)
        .or_else(|| smoke.pointer("/turn/usage/used").and_then(Value::as_u64))
}

enum CostObservation {
    Missing,
    Invalid,
    Usd(f64),
}

fn observed_cost_usd(smoke: &Value) -> CostObservation {
    let usage = smoke.pointer("/turn/usage");
    if usage
        .and_then(|usage| usage.get("cost_rejected"))
        .is_some_and(|value| !value.is_null())
    {
        return CostObservation::Invalid;
    }
    let Some(cost) = usage.and_then(|usage| usage.get("cost")) else {
        return CostObservation::Missing;
    };
    if cost.is_null() {
        return CostObservation::Missing;
    }
    let amount = cost.get("amount").and_then(Value::as_f64);
    let currency = cost.get("currency").and_then(Value::as_str);
    match (amount, currency) {
        (Some(amount), Some("USD")) if amount.is_finite() && amount >= 0.0 => {
            CostObservation::Usd(amount)
        }
        _ => CostObservation::Invalid,
    }
}

fn not_run_result(case: &CompatibilityCase, reason: &str) -> CaseResult {
    let actual_status = if case.expected_status == EvidenceStatus::Stale {
        EvidenceStatus::Stale
    } else {
        EvidenceStatus::Unknown
    };
    CaseResult {
        case_id: case.id.clone(),
        baseline_case_id: case.baseline_case.clone(),
        lane: case.lane,
        evidence_path: case.evidence_path,
        probe: case.probe,
        billable: case.billable,
        execution: ExecutionState::NotRun,
        expected_status: case.expected_status,
        actual_status,
        expectation_met: actual_status == case.expected_status,
        classification: case.classification,
        candidate_outcome: (case.lane == Lane::FloatingCurrent)
            .then_some(CandidateOutcome::Unknown),
        artifact_policy: case.artifact.clone(),
        duration_ms: 0,
        not_run_reason: Some(reason.into()),
        runner_error_code: None,
        drift: Vec::new(),
        budget_violations: Vec::new(),
        smoke: None,
    }
}

fn runner_failure_result(case: &CompatibilityCase, duration: Duration, code: &str) -> CaseResult {
    CaseResult {
        case_id: case.id.clone(),
        baseline_case_id: case.baseline_case.clone(),
        lane: case.lane,
        evidence_path: case.evidence_path,
        probe: case.probe,
        billable: case.billable,
        execution: ExecutionState::RunnerFailure,
        expected_status: case.expected_status,
        actual_status: EvidenceStatus::Unknown,
        expectation_met: false,
        classification: case.classification,
        candidate_outcome: (case.lane == Lane::FloatingCurrent)
            .then_some(CandidateOutcome::Unknown),
        artifact_policy: case.artifact.clone(),
        duration_ms: duration.as_millis().try_into().unwrap_or(u64::MAX),
        not_run_reason: None,
        runner_error_code: Some(code.into()),
        drift: Vec::new(),
        budget_violations: Vec::new(),
        smoke: None,
    }
}

fn case_environment_ready(case: &CompatibilityCase, owner: &str) -> Option<&'static str> {
    if case.os != std::env::consts::OS || case.architecture != std::env::consts::ARCH {
        return Some("environment_platform_mismatch");
    }
    if case.environment_owner != owner {
        return Some("environment_owner_mismatch");
    }
    for requirement in &case.required_env {
        let Some(value) = std::env::var_os(&requirement.name) else {
            return Some("required_environment_missing");
        };
        if !requirement.one_of.is_empty()
            && value
                .to_str()
                .is_none_or(|value| !requirement.one_of.iter().any(|expected| expected == value))
        {
            return Some("required_environment_value_mismatch");
        }
    }
    if let Some(name) = &case.credential_env {
        match std::env::var(name) {
            Ok(value) if value.len() >= 8 => {}
            Ok(_) => return Some("credential_value_too_short_for_safe_redaction"),
            Err(_) => return Some("credential_environment_missing_or_non_unicode"),
        }
    }
    if case.evidence_path != EvidencePath::BridgeSmoke {
        return Some("evidence_path_not_implemented_in_r3a");
    }
    if case.probe == ProbeType::Representative {
        return Some("representative_probe_not_implemented_in_r3a");
    }
    None
}

fn pinned_config_ready(
    loaded: &LoadedManifest,
    case: &CompatibilityCase,
) -> Result<(), &'static str> {
    let Some(pins) = &case.pins else {
        return Ok(());
    };
    let path = resolve_case_path(&loaded.canonical_path, &case.config);
    match local_file::read_regular_file_bounded(
        &path,
        "compatibility pinned config",
        MAX_MANIFEST_BYTES,
    ) {
        Ok(snapshot) if snapshot.sha256 == pins.config_sha256 => Ok(()),
        Ok(_) => Err("config_pin_mismatch"),
        Err(_) => Err("config_pin_unavailable"),
    }
}

fn known_secret_values(case: &CompatibilityCase) -> Vec<String> {
    case.credential_env
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .collect()
}

struct AggregateInputs<'a> {
    loaded: &'a LoadedManifest,
    candidate: &'a CandidateIdentity,
    selection: &'a SelectionRecord,
    selected_indices: &'a [usize],
    environment_owner: &'a str,
    scratch: &'a Path,
    cancellation_requested: &'a std::sync::atomic::AtomicBool,
}

fn initial_budget(config: &ManifestBudget) -> BudgetOutcome {
    BudgetOutcome {
        timeout_secs: config.timeout_secs,
        max_tokens: config.max_tokens,
        max_cost_usd: config.max_cost_usd,
        observed_tokens: 0,
        observed_cost_usd: 0.0,
        token_observation_missing_cases: 0,
        cost_observation_missing_cases: 0,
        exhausted: false,
    }
}

fn setup_incomplete_aggregate(
    loaded: &LoadedManifest,
    candidate: &CandidateIdentity,
    selection: &SelectionRecord,
    selected_indices: &[usize],
    environment_owner: &str,
) -> AggregateArtifact {
    let at_ms = diagnostic_timestamp_ms();
    let mut results = Vec::with_capacity(selected_indices.len());
    for (ordinal, index) in selected_indices.iter().enumerate() {
        let case = &loaded.manifest.cases[*index];
        if ordinal == 0 {
            results.push(runner_failure_result(
                case,
                Duration::ZERO,
                "compatibility_setup_incomplete",
            ));
        } else {
            results.push(not_run_result(case, "prior_runner_failure"));
        }
    }
    AggregateArtifact {
        schema_version: 1,
        candidate: candidate.clone(),
        manifest: ManifestIdentity {
            schema_version: loaded.manifest.schema_version,
            canonical_path: loaded.canonical_path_text.clone(),
            sha256: loaded.sha256.clone(),
        },
        selection: selection.clone(),
        environment_owner: environment_owner.into(),
        started_at_ms: at_ms,
        ended_at_ms: at_ms,
        cancelled: false,
        success: false,
        budget: initial_budget(&loaded.manifest.budget),
        floating_summary: floating_summary(&results),
        results,
    }
}

async fn build_aggregate<I: SmokeInvoker>(
    inputs: AggregateInputs<'_>,
    invoker: &I,
) -> AggregateArtifact {
    let AggregateInputs {
        loaded,
        candidate,
        selection,
        selected_indices,
        environment_owner,
        scratch,
        cancellation_requested,
    } = inputs;
    let started_at_ms = diagnostic_timestamp_ms();
    let started = Instant::now();
    let budget_config = &loaded.manifest.budget;
    let mut budget = initial_budget(budget_config);
    let mut results = Vec::with_capacity(selected_indices.len());
    let mut prior_runner_failure = false;
    let mut embedded_smoke_bytes = 0usize;

    for (ordinal, index) in selected_indices.iter().enumerate() {
        let case = &loaded.manifest.cases[*index];
        if cancellation_requested.load(std::sync::atomic::Ordering::Acquire) {
            results.push(not_run_result(case, "cancellation_requested"));
            continue;
        }
        if prior_runner_failure {
            results.push(not_run_result(case, "prior_runner_failure"));
            continue;
        }
        if let Some(reason) = case_environment_ready(case, environment_owner) {
            results.push(not_run_result(case, reason));
            continue;
        }
        if let Err(code) = pinned_config_ready(loaded, case) {
            results.push(runner_failure_result(case, Duration::ZERO, code));
            prior_runner_failure = true;
            continue;
        }
        let remaining =
            Duration::from_secs(budget_config.timeout_secs).checked_sub(started.elapsed());
        let total_cost_exhausted = budget_config
            .max_cost_usd
            .is_some_and(|cap| budget.observed_cost_usd >= cap);
        if budget.exhausted
            || budget.observed_tokens >= budget_config.max_tokens
            || total_cost_exhausted
        {
            budget.exhausted = true;
            results.push(not_run_result(case, "total_budget_exhausted"));
            continue;
        }
        let token_headroom_insufficient = budget_config
            .max_tokens
            .saturating_sub(budget.observed_tokens)
            < case.max_tokens;
        let cost_headroom_insufficient = match (budget_config.max_cost_usd, case.max_cost_usd) {
            (Some(total), Some(case_cap)) => total - budget.observed_cost_usd < case_cap,
            _ => false,
        };
        if token_headroom_insufficient
            || cost_headroom_insufficient
            || remaining.is_none_or(|remaining| remaining < Duration::from_secs(case.timeout_secs))
        {
            budget.exhausted = true;
            results.push(not_run_result(case, "total_budget_insufficient_for_case"));
            continue;
        }
        let artifact_path = scratch.join(format!("case-{ordinal:03}.json"));
        let request = SmokeRequest {
            agent: case.agent.clone(),
            config: resolve_case_path(&loaded.canonical_path, &case.config),
            model: case.model.clone(),
            effort: case.effort.clone(),
            mode: case.mode.clone(),
            session_cwd: case
                .session_cwd
                .as_ref()
                .map(|path| resolve_case_path(&loaded.canonical_path, path)),
            timeout_secs: case.timeout_secs,
            artifact_path,
        };
        let case_started = Instant::now();
        let admission = SpawnAdmission {
            cancellation_requested,
            started,
            total_timeout: Duration::from_secs(budget_config.timeout_secs),
            case_timeout: Duration::from_secs(case.timeout_secs),
        };
        let invocation = invoker.invoke(&request, &admission).await;
        let duration = case_started.elapsed();
        if let Some(reason) = invocation.not_run_reason {
            if reason == "total_budget_insufficient_for_case" {
                budget.exhausted = true;
            }
            results.push(not_run_result(case, reason));
            continue;
        }
        let Some(smoke) = invocation.artifact else {
            results.push(runner_failure_result(
                case,
                duration,
                invocation
                    .runner_error_code
                    .unwrap_or("smoke_artifact_unavailable"),
            ));
            prior_runner_failure = true;
            continue;
        };
        if !valid_smoke_shape(&smoke, case) {
            results.push(runner_failure_result(
                case,
                duration,
                "smoke_artifact_schema_mismatch",
            ));
            prior_runner_failure = true;
            continue;
        }
        if value_contains_secret(&smoke, &known_secret_values(case)) {
            results.push(runner_failure_result(
                case,
                duration,
                "smoke_artifact_secret_rejected",
            ));
            prior_runner_failure = true;
            continue;
        }
        let smoke_bytes = match serde_json::to_vec(&smoke) {
            Ok(bytes) => bytes.len(),
            Err(_) => {
                results.push(runner_failure_result(
                    case,
                    duration,
                    "smoke_artifact_serialization_failed",
                ));
                prior_runner_failure = true;
                continue;
            }
        };
        if embedded_smoke_bytes
            .checked_add(smoke_bytes)
            .is_none_or(|total| total > MAX_EMBEDDED_SMOKE_BYTES)
        {
            results.push(runner_failure_result(
                case,
                duration,
                "aggregate_smoke_evidence_limit_exceeded",
            ));
            prior_runner_failure = true;
            continue;
        }
        embedded_smoke_bytes += smoke_bytes;
        let smoke_success = smoke
            .get("success")
            .and_then(Value::as_bool)
            .expect("validated above");
        if invocation.process_success != smoke_success {
            results.push(runner_failure_result(
                case,
                duration,
                "smoke_exit_artifact_mismatch",
            ));
            prior_runner_failure = true;
            continue;
        }

        let actual_status = if smoke_success {
            EvidenceStatus::Pass
        } else {
            EvidenceStatus::Fail
        };
        let drift = drift_for(case, &smoke);
        let mut budget_violations = Vec::new();
        match observed_tokens(&smoke) {
            Some(tokens) => {
                budget.observed_tokens = budget.observed_tokens.saturating_add(tokens);
                if tokens > case.max_tokens {
                    budget_violations.push("case_token_cap_exceeded".into());
                }
            }
            None => budget.token_observation_missing_cases += 1,
        }
        match observed_cost_usd(&smoke) {
            CostObservation::Usd(cost) => {
                let accumulated = budget.observed_cost_usd + cost;
                if accumulated.is_finite() {
                    budget.observed_cost_usd = accumulated;
                } else {
                    budget.observed_cost_usd = f64::MAX;
                    budget_violations.push("total_cost_overflow".into());
                }
                if case.max_cost_usd.is_some_and(|cap| cost > cap) {
                    budget_violations.push("case_cost_cap_exceeded".into());
                }
            }
            CostObservation::Missing => budget.cost_observation_missing_cases += 1,
            CostObservation::Invalid => {
                budget_violations.push("case_cost_observation_invalid".into());
            }
        }
        if started.elapsed() > Duration::from_secs(budget_config.timeout_secs) {
            budget_violations.push("total_timeout_exceeded".into());
            budget.exhausted = true;
        }
        if budget.observed_tokens > budget.max_tokens
            || budget
                .max_cost_usd
                .is_some_and(|cap| budget.observed_cost_usd > cap)
        {
            budget.exhausted = true;
        }
        if !budget_violations.is_empty() {
            budget.exhausted = true;
        }
        let expectation_met = actual_status == case.expected_status
            && drift.is_empty()
            && budget_violations.is_empty();
        let candidate_outcome = (case.lane == Lane::FloatingCurrent).then(|| {
            if !drift.is_empty() || !budget_violations.is_empty() {
                CandidateOutcome::Unknown
            } else if actual_status == EvidenceStatus::Pass {
                CandidateOutcome::Pass
            } else if actual_status == EvidenceStatus::Fail {
                CandidateOutcome::Fail
            } else {
                CandidateOutcome::Unknown
            }
        });
        results.push(CaseResult {
            case_id: case.id.clone(),
            baseline_case_id: case.baseline_case.clone(),
            lane: case.lane,
            evidence_path: case.evidence_path,
            probe: case.probe,
            billable: case.billable,
            execution: ExecutionState::Completed,
            expected_status: case.expected_status,
            actual_status,
            expectation_met,
            classification: case.classification,
            candidate_outcome,
            artifact_policy: case.artifact.clone(),
            duration_ms: duration.as_millis().try_into().unwrap_or(u64::MAX),
            not_run_reason: None,
            runner_error_code: None,
            drift,
            budget_violations,
            smoke: Some(smoke),
        });
    }

    if started.elapsed() > Duration::from_secs(budget_config.timeout_secs) {
        budget.exhausted = true;
    }

    let runner_failed = results
        .iter()
        .any(|result| result.execution == ExecutionState::RunnerFailure);
    let pinned_failed = results.iter().any(|result| {
        result.lane == Lane::Pinned
            && result.classification == Classification::Support
            && (result.execution != ExecutionState::Completed || !result.expectation_met)
    });
    let floating_not_pass = results.iter().any(|result| {
        result.lane == Lane::FloatingCurrent
            && result.candidate_outcome != Some(CandidateOutcome::Pass)
    });
    AggregateArtifact {
        schema_version: 1,
        candidate: candidate.clone(),
        manifest: ManifestIdentity {
            schema_version: loaded.manifest.schema_version,
            canonical_path: loaded.canonical_path_text.clone(),
            sha256: loaded.sha256.clone(),
        },
        selection: selection.clone(),
        environment_owner: environment_owner.into(),
        started_at_ms,
        ended_at_ms: diagnostic_timestamp_ms(),
        cancelled: cancellation_requested.load(std::sync::atomic::Ordering::Acquire),
        success: !runner_failed
            && !pinned_failed
            && !floating_not_pass
            && !budget.exhausted
            && !cancellation_requested.load(std::sync::atomic::Ordering::Acquire),
        budget,
        floating_summary: floating_summary(&results),
        results,
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct CaseBaseline {
    case_id: String,
    outcome: Value,
    status: EvidenceStatus,
    execution_mode: Value,
    provenance: Value,
    capability: Value,
    authentication: Value,
    phase: Value,
    terminal: Value,
    diagnostic: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct AggregateBaseline {
    success: bool,
    cancelled: bool,
    budget_exhausted: bool,
    token_observation_missing_cases: u32,
    cost_observation_missing_cases: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BaselineArtifact {
    schema_version: u16,
    manifest_schema_version: u16,
    manifest_sha256: String,
    aggregate: AggregateBaseline,
    cases: Vec<CaseBaseline>,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct ComparisonChange {
    case_id: String,
    dimensions: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct ComparisonReport {
    schema_version: u16,
    equal: bool,
    changes: Vec<ComparisonChange>,
}

fn object_subset(value: &Value, fields: &[&str]) -> Value {
    let mut out = serde_json::Map::new();
    for field in fields {
        out.insert(
            (*field).into(),
            value.get(*field).cloned().unwrap_or(Value::Null),
        );
    }
    Value::Object(out)
}

fn diagnostic_projection(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(diagnostic_projection).collect()),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .filter(|(key, _)| key.as_str() != "at_ms")
                .map(|(key, value)| (key.clone(), diagnostic_projection(value)))
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn baseline_from_result(result: &CaseResult) -> CaseBaseline {
    let smoke = result.smoke.as_ref().unwrap_or(&Value::Null);
    let request = smoke.get("request").unwrap_or(&Value::Null);
    let session = smoke.get("session").unwrap_or(&Value::Null);
    let target = smoke.get("target").unwrap_or(&Value::Null);
    let attempt = smoke.get("attempt").unwrap_or(&Value::Null);
    let diagnostics = smoke.get("diagnostics").unwrap_or(&Value::Null);
    let failure = smoke
        .pointer("/diagnostics/failure")
        .unwrap_or(&Value::Null);
    let turn = smoke.get("turn").unwrap_or(&Value::Null);
    let cleanup = smoke.get("cleanup").unwrap_or(&Value::Null);
    CaseBaseline {
        case_id: result.case_id.clone(),
        outcome: json!({
            "execution": result.execution,
            "expectation_met": result.expectation_met,
            "not_run_reason": result.not_run_reason,
            "runner_error_code": result.runner_error_code,
            "drift": result.drift,
            "budget_violations": result.budget_violations,
        }),
        status: result.actual_status,
        execution_mode: target.get("execution_mode").cloned().unwrap_or(Value::Null),
        provenance: target.get("provenance").cloned().unwrap_or(Value::Null),
        capability: json!({
            "request": object_subset(request, &["model", "effort", "mode"]),
            "effective": session.get("effective_request").cloned().unwrap_or(Value::Null),
        }),
        authentication: target.get("authentication").cloned().unwrap_or(Value::Null),
        phase: failure.get("failed_phase").cloned().unwrap_or(Value::Null),
        terminal: json!({
            "attempt": object_subset(
                attempt,
                &["timeout_secs", "timed_out", "prompt_may_have_been_accepted"],
            ),
            "turn": object_subset(
                turn,
                &[
                    "prompt",
                    "prompt_calls",
                    "terminal_state",
                    "stop_reason",
                    "exact_pong",
                    "text_bytes",
                    "tool_event_count",
                    "permission_update_count",
                ],
            ),
            "cleanup": cleanup,
        }),
        diagnostic: json!({
            "lifecycle": diagnostic_projection(
                diagnostics.get("lifecycle").unwrap_or(&Value::Null),
            ),
            "dropped_events": diagnostics
                .get("dropped_events")
                .cloned()
                .unwrap_or(Value::Null),
            "failure": diagnostic_projection(failure),
            "stderr_text": diagnostics
                .get("stderr_text")
                .cloned()
                .unwrap_or(Value::Null),
        }),
    }
}

fn baseline_from_aggregate(aggregate: &AggregateArtifact) -> AggregateBaseline {
    AggregateBaseline {
        success: aggregate.success,
        cancelled: aggregate.cancelled,
        budget_exhausted: aggregate.budget.exhausted,
        token_observation_missing_cases: aggregate.budget.token_observation_missing_cases,
        cost_observation_missing_cases: aggregate.budget.cost_observation_missing_cases,
    }
}

fn load_json<T: for<'de> Deserialize<'de>>(path: &Path, label: &str) -> Result<T, BoxError> {
    let snapshot = local_file::read_regular_file_bounded(path, label, MAX_AGGREGATE_BYTES)?;
    let value: Value = serde_json::from_slice(&snapshot.bytes)
        .map_err(|error| format!("{label}: invalid JSON: {error}"))?;
    if value_contains_secret(&value, &[]) {
        return Err(format!("{label}: secret-shaped material is not allowed").into());
    }
    serde_json::from_value(value)
        .map_err(|error| format!("{label}: invalid schema: {error}").into())
}

fn compare_artifacts(
    current: &AggregateArtifact,
    baseline: &BaselineArtifact,
) -> Result<ComparisonReport, BoxError> {
    if current.schema_version != 1
        || !local_file::valid_sha256(&current.candidate.sha256)
        || current.candidate.sha256 != current.candidate.sha256.to_ascii_lowercase()
        || current.candidate.byte_length == 0
        || current.manifest.schema_version != 1
        || !local_file::valid_sha256(&current.manifest.sha256)
        || current.manifest.sha256 != current.manifest.sha256.to_ascii_lowercase()
    {
        return Err("compatibility aggregate: invalid schema or manifest identity".into());
    }
    if baseline.schema_version != 1 {
        return Err("compatibility baseline: schema_version must be 1".into());
    }
    if baseline.manifest_schema_version != 1
        || !local_file::valid_sha256(&baseline.manifest_sha256)
        || baseline.manifest_sha256 != baseline.manifest_sha256.to_ascii_lowercase()
    {
        return Err("compatibility baseline: invalid manifest identity".into());
    }
    let mut changes = Vec::new();
    if baseline.manifest_schema_version != current.manifest.schema_version
        || baseline.manifest_sha256 != current.manifest.sha256
    {
        changes.push(ComparisonChange {
            case_id: "__manifest__".into(),
            dimensions: vec!["manifest".into()],
        });
    }
    let current_aggregate = baseline_from_aggregate(current);
    let mut aggregate_dimensions = Vec::new();
    if baseline.aggregate.success != current_aggregate.success {
        aggregate_dimensions.push("success".into());
    }
    if baseline.aggregate.cancelled != current_aggregate.cancelled {
        aggregate_dimensions.push("cancelled".into());
    }
    if baseline.aggregate.budget_exhausted != current_aggregate.budget_exhausted
        || baseline.aggregate.token_observation_missing_cases
            != current_aggregate.token_observation_missing_cases
        || baseline.aggregate.cost_observation_missing_cases
            != current_aggregate.cost_observation_missing_cases
    {
        aggregate_dimensions.push("budget".into());
    }
    if !aggregate_dimensions.is_empty() {
        changes.push(ComparisonChange {
            case_id: "__aggregate__".into(),
            dimensions: aggregate_dimensions,
        });
    }
    let mut baseline_cases = BTreeMap::new();
    for case in &baseline.cases {
        if baseline_cases.insert(&case.case_id, case).is_some() {
            return Err(format!(
                "compatibility baseline: duplicate case id {:?}",
                case.case_id
            )
            .into());
        }
    }
    let mut current_cases = BTreeMap::new();
    for result in current
        .results
        .iter()
        .filter(|result| result.lane == Lane::Pinned)
    {
        if current_cases
            .insert(&result.case_id, baseline_from_result(result))
            .is_some()
        {
            return Err(format!(
                "compatibility aggregate: duplicate pinned case id {:?}",
                result.case_id
            )
            .into());
        }
    }
    for id in baseline_cases
        .keys()
        .chain(current_cases.keys())
        .copied()
        .collect::<BTreeSet<_>>()
    {
        let Some(before) = baseline_cases.get(id) else {
            changes.push(ComparisonChange {
                case_id: id.clone(),
                dimensions: vec!["case_added".into()],
            });
            continue;
        };
        let Some(after) = current_cases.get(id) else {
            changes.push(ComparisonChange {
                case_id: id.clone(),
                dimensions: vec!["case_missing".into()],
            });
            continue;
        };
        let mut dimensions = Vec::new();
        if before.outcome != after.outcome {
            dimensions.push("outcome".into());
        }
        if before.status != after.status {
            dimensions.push("status".into());
        }
        if before.execution_mode != after.execution_mode {
            dimensions.push("execution_mode".into());
        }
        if before.provenance != after.provenance {
            dimensions.push("provenance".into());
        }
        if before.capability != after.capability {
            dimensions.push("capability".into());
        }
        if before.authentication != after.authentication {
            dimensions.push("authentication".into());
        }
        if before.phase != after.phase {
            dimensions.push("phase".into());
        }
        if before.terminal != after.terminal {
            dimensions.push("terminal".into());
        }
        if before.diagnostic != after.diagnostic {
            dimensions.push("diagnostic".into());
        }
        if !dimensions.is_empty() {
            changes.push(ComparisonChange {
                case_id: id.clone(),
                dimensions,
            });
        }
    }
    Ok(ComparisonReport {
        schema_version: 1,
        equal: changes.is_empty(),
        changes,
    })
}

async fn run_command(args: RunArgs) -> Result<(), BoxError> {
    let loaded = match &args.source {
        RunSource::Manifest(path) => load_manifest(path)?,
        RunSource::Resolution(path) => {
            let resolution = compatibility_resolution::load_resolution(path)?;
            let _resolution_identity = (
                &resolution.canonical_path,
                &resolution.canonical_path_text,
                &resolution.sha256,
            );
            if resolution.artifact.state != ResolutionState::Complete {
                return Err(
                    "compatibility run: resolution must have state complete before execution"
                        .into(),
                );
            }
            if resolution.artifact.environment.environment_owner != args.environment_owner {
                return Err("compatibility run: resolution environment owner mismatch".into());
            }
            let available: BTreeSet<_> = resolution
                .artifact
                .cases
                .iter()
                .map(|case| case.id.as_str())
                .collect();
            for requested in &args.selection.cases {
                if !available.contains(requested.as_str()) {
                    return Err(format!(
                        "compatibility run: selected case {requested:?} is not in the resolution"
                    )
                    .into());
                }
            }
            if resolution.artifact.cases.is_empty() {
                return Err("compatibility run: completed resolution contains no cases".into());
            }
            return Err(
                "compatibility run: resolved execution is not implemented in the R3c contract slice"
                    .into(),
            );
        }
    };
    let selected = select_case_indices(&loaded.manifest, &args.selection)?;
    if selected
        .iter()
        .any(|index| loaded.manifest.cases[*index].lane == Lane::FloatingCurrent)
    {
        return Err("compatibility run: floating_resolution_required; use --resolution".into());
    }
    let output_directory = ensure_output_outside_repositories(&args.out)?;
    let executable = std::env::current_exe()
        .map_err(|error| format!("compatibility run: cannot resolve candidate binary: {error}"))?;
    let executable = local_file::read_regular_file_bounded(
        &executable,
        "compatibility candidate binary",
        MAX_EXECUTABLE_BYTES,
    )?;
    let candidate = CandidateIdentity {
        canonical_path: artifact_safe_path(
            "compatibility candidate binary",
            &executable.canonical_path,
        )?,
        sha256: executable.sha256.clone(),
        byte_length: u64::try_from(executable.bytes.len())
            .map_err(|_| "compatibility run: candidate binary length does not fit u64")?,
    };
    let setup_evidence = setup_incomplete_aggregate(
        &loaded,
        &candidate,
        &args.selection,
        &selected,
        &args.environment_owner,
    );
    let output = output_directory.prepare_output_with_setup_evidence(&setup_evidence)?;
    let scratch = output_directory.create_scratch()?;
    let staged_executable = stage_candidate(&executable, &scratch)?;
    drop(executable);
    let invoker = ProcessSmokeInvoker {
        executable: staged_executable,
        artifact_directory: &scratch.pin,
        expected_sha256: candidate.sha256.clone(),
    };
    let cancellation_requested = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cancellation_for_signal = std::sync::Arc::clone(&cancellation_requested);
    let signal_task = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            // Finish the already-started one-attempt smoke so its R2c cleanup and artifact contract
            // remain intact, then refuse every subsequent case.
            cancellation_for_signal.store(true, std::sync::atomic::Ordering::Release);
        }
    });
    let aggregate = build_aggregate(
        AggregateInputs {
            loaded: &loaded,
            candidate: &candidate,
            selection: &args.selection,
            selected_indices: &selected,
            environment_owner: &args.environment_owner,
            scratch: &scratch.path,
            cancellation_requested: &cancellation_requested,
        },
        &invoker,
    )
    .await;
    signal_task.abort();
    let success = aggregate.success;
    output_directory.replace_setup_with_final(&output, &setup_evidence, &aggregate)?;
    if success {
        Ok(())
    } else {
        Err("compatibility run: aggregate contains a blocking failure; inspect --out".into())
    }
}

fn resolve_command(args: ResolveArgs) -> Result<(), BoxError> {
    let (recipes, _pinned) = load_recipes_with_pinned_manifest(&args.recipes)?;
    let available: BTreeSet<_> = recipes
        .recipes
        .cases
        .iter()
        .map(|case| case.id.as_str())
        .collect();
    for requested in &args.cases {
        if !available.contains(requested.as_str()) {
            return Err(format!(
                "compatibility resolve: selected case {requested:?} is not in the recipes"
            )
            .into());
        }
    }
    if args.all && available.is_empty() {
        return Err("compatibility resolve: explicit selection resolved to zero cases".into());
    }
    // Contract slice: prove the complete authority barrier and schemas before any writable executor
    // exists. These values are parsed now so later effect code cannot widen the CLI unnoticed.
    let _contract_binding = (
        &args.environment_owner,
        args.runtime,
        &args.out,
        &recipes.canonical_path_text,
        &recipes.sha256,
    );
    Err(
        "compatibility resolve: materialization is not implemented in the R3c contract slice"
            .into(),
    )
}

fn compare_command(args: CompareArgs) -> Result<(), BoxError> {
    if matches!(args.mode, ComparisonMode::FloatingToPinned) {
        return Err(
            "compatibility compare: floating-to-pinned is not implemented in the R3c contract slice"
                .into(),
        );
    }
    let current: AggregateArtifact = load_json(&args.current, "compatibility current aggregate")?;
    let baseline: BaselineArtifact = load_json(&args.baseline, "compatibility pinned baseline")?;
    let report = compare_artifacts(&current, &baseline)?;
    let equal = report.equal;
    serde_json::to_writer_pretty(std::io::stdout().lock(), &report)?;
    println!();
    if equal {
        Ok(())
    } else {
        Err("compatibility compare: drift detected".into())
    }
}

pub(crate) async fn compatibility_cmd(args: &[String]) -> Result<(), BoxError> {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
    {
        println!("{USAGE}");
        return Ok(());
    }
    let Some(subcommand) = args.first().map(String::as_str) else {
        return Err(format!("compatibility: missing subcommand\n{USAGE}").into());
    };
    match subcommand {
        "validate" => {
            let args = parse_validate_args(&args[1..])?;
            match args.source {
                ValidateSource::Manifest(path) => {
                    let loaded = load_manifest(&path)?;
                    println!(
                        "compatibility manifest valid: {} case{} (sha256 {})",
                        loaded.manifest.cases.len(),
                        if loaded.manifest.cases.len() == 1 {
                            ""
                        } else {
                            "s"
                        },
                        loaded.sha256
                    );
                }
                ValidateSource::Recipes(path) => {
                    let (loaded, _) = load_recipes_with_pinned_manifest(&path)?;
                    println!(
                        "floating recipes valid: {} case{} (sha256 {})",
                        loaded.recipes.cases.len(),
                        if loaded.recipes.cases.len() == 1 {
                            ""
                        } else {
                            "s"
                        },
                        loaded.sha256
                    );
                }
            }
            Ok(())
        }
        "resolve" => resolve_command(parse_resolve_args(&args[1..])?),
        "run" => run_command(parse_run_args(&args[1..])?).await,
        "compare" => compare_command(parse_compare_args(&args[1..])?),
        other => Err(format!(
            "compatibility: unknown subcommand {other:?} (expected validate | resolve | run | compare)\n{USAGE}"
        )
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::io::{Seek, SeekFrom};
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt as _;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};

    #[cfg(unix)]
    fn running_as_root() -> bool {
        // SAFETY: geteuid has no preconditions and only reads the process credential.
        unsafe { libc::geteuid() == 0 }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn compatibility_descriptor_handoff_validates_objects_before_close() {
        use std::os::fd::{AsRawFd as _, IntoRawFd as _};

        let unrelated = File::open("/dev/null").unwrap();
        let error =
            close_inherited_compatibility_executable(Some(unrelated.as_raw_fd())).unwrap_err();
        assert!(error.to_string().contains("does not identify"));
        // SAFETY: the rejected descriptor remains owned by `unrelated` and must still be live.
        assert_ne!(
            unsafe { libc::fcntl(unrelated.as_raw_fd(), libc::F_GETFD) },
            -1
        );

        let executable = File::open("/proc/self/exe").unwrap().into_raw_fd();
        close_inherited_compatibility_executable(Some(executable)).unwrap();
        // SAFETY: querying a descriptor transferred to and closed by the helper is defined.
        assert_eq!(unsafe { libc::fcntl(executable, libc::F_GETFD) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::EBADF)
        );

        let dir = tempfile::tempdir().unwrap();
        let scratch = File::open(dir.path()).unwrap().into_raw_fd();
        let artifact = dir.path().join("artifact.json");
        close_inherited_compatibility_scratch(Some(scratch), Some(&artifact)).unwrap();
        // SAFETY: querying a descriptor transferred to and closed by the helper is defined.
        assert_eq!(unsafe { libc::fcntl(scratch, libc::F_GETFD) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::EBADF)
        );
    }

    fn scratch_in(parent: &Path) -> ScratchDir {
        let snapshot = local_file::snapshot_directory(parent, "test scratch parent").unwrap();
        let pin = local_file::PinnedDirectory::open(
            parent,
            &snapshot.canonical_cwd,
            &snapshot.identity,
            "test scratch parent",
        )
        .unwrap();
        create_scratch_dir(&pin).unwrap()
    }

    fn case(id: &str, expected_status: EvidenceStatus) -> CompatibilityCase {
        CompatibilityCase {
            id: id.into(),
            lane: Lane::FloatingCurrent,
            evidence_path: EvidencePath::BridgeSmoke,
            execution_mode: ExecutionMode::Host,
            os: std::env::consts::OS.into(),
            architecture: std::env::consts::ARCH.into(),
            environment_owner: "test-runner".into(),
            expected_image_digest: None,
            config: PathBuf::from("missing.toml"),
            agent: id.into(),
            model: "test-model".into(),
            effort: None,
            mode: None,
            session_cwd: None,
            auth_path: AuthPath::Automatic,
            credential_env: None,
            required_env: Vec::new(),
            probe: ProbeType::Minimal,
            billable: true,
            timeout_secs: 1,
            max_tokens: 10,
            max_cost_usd: Some(0.5),
            retry_cap: 0,
            expected_status,
            classification: Classification::Canary,
            baseline_case: Some(format!("baseline-{id}")),
            artifact: ArtifactPolicy {
                retention_days: 1,
                redaction: RedactionPolicy::Strict,
            },
            pins: None,
            resolved: Some(ResolvedBinding {
                resolution_id: "resolution-1".into(),
                recipe_sha256: "e".repeat(64),
                config_sha256: "f".repeat(64),
                adapter: "@agentclientprotocol/codex-acp=1.2.3".into(),
                agent_cli: "@openai/codex=0.150.0".into(),
                package_inventory_sha256: "a".repeat(64),
                package_tree_sha256: "b".repeat(64),
                image_digest: None,
                base_image_digest: None,
            }),
        }
    }

    fn manifest(cases: Vec<CompatibilityCase>) -> CompatibilityManifest {
        CompatibilityManifest {
            schema_version: 1,
            budget: ManifestBudget {
                timeout_secs: 30,
                max_tokens: 100,
                max_cost_usd: Some(1.0),
            },
            cases,
        }
    }

    fn loaded(dir: &Path, cases: Vec<CompatibilityCase>) -> LoadedManifest {
        let canonical_path = dir.join("manifest.toml");
        LoadedManifest {
            manifest: manifest(cases),
            canonical_path_text: canonical_path.to_str().unwrap().into(),
            canonical_path,
            sha256: "a".repeat(64),
        }
    }

    fn smoke(case: &CompatibilityCase, success: bool, tokens: Option<u64>) -> Value {
        let target = success.then(|| {
            json!({
                "execution_mode": "host",
                "provenance": [],
                "authentication": {"path": "automatic"}
            })
        });
        let failure = (!success).then(|| {
            json!({
                "failed_phase": "resolve",
                "class": "config",
                "disposition": "fatal",
                "code": "fixture.failure",
                "prompt_may_have_been_accepted": false
            })
        });
        json!({
            "schema_version": 2,
            "success": success,
            "bridge": {"package_version": "0.2.1"},
            "attempt": {"id": format!("attempt-{}", case.id)},
            "request": {
                "agent": case.agent,
                "model": case.model,
                "config_sha256": Value::Null
            },
            "target": target,
            "session": {"effective_request": {"model": case.model}},
            "turn": {
                "prompt": crate::smoke::FIXED_PROMPT,
                "terminal_state": if success { "completed" } else { "not_started" },
                "stop_reason": if success { Value::String("end_turn".into()) } else { Value::Null },
                "exact_pong": success,
                "usage": tokens.map(|used| json!({"used": used}))
            },
            "diagnostics": {"failure": failure},
            "cleanup": {}
        })
    }

    struct FakeInvoker {
        calls: Mutex<Vec<String>>,
        results: Mutex<VecDeque<InvocationResult>>,
    }

    impl FakeInvoker {
        fn new(results: Vec<InvocationResult>) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                results: Mutex::new(results.into()),
            }
        }
    }

    struct DelayedInvoker {
        delay: Duration,
        calls: Mutex<Vec<String>>,
        result: Mutex<Option<InvocationResult>>,
    }

    #[async_trait]
    impl SmokeInvoker for DelayedInvoker {
        async fn invoke(
            &self,
            request: &SmokeRequest,
            admission: &SpawnAdmission<'_>,
        ) -> InvocationResult {
            if let Some(reason) = admission.reason() {
                return InvocationResult::admission_rejected(reason);
            }
            self.calls.lock().unwrap().push(request.agent.clone());
            tokio::time::sleep(self.delay).await;
            self.result.lock().unwrap().take().unwrap()
        }
    }

    struct PreSpawnDelayedInvoker {
        delay: Duration,
        calls: Mutex<Vec<String>>,
        result: Mutex<Option<InvocationResult>>,
    }

    #[async_trait]
    impl SmokeInvoker for PreSpawnDelayedInvoker {
        async fn invoke(
            &self,
            request: &SmokeRequest,
            admission: &SpawnAdmission<'_>,
        ) -> InvocationResult {
            tokio::time::sleep(self.delay).await;
            if let Some(reason) = admission.reason() {
                return InvocationResult::admission_rejected(reason);
            }
            self.calls.lock().unwrap().push(request.agent.clone());
            self.result.lock().unwrap().take().unwrap()
        }
    }

    #[async_trait]
    impl SmokeInvoker for FakeInvoker {
        async fn invoke(
            &self,
            request: &SmokeRequest,
            admission: &SpawnAdmission<'_>,
        ) -> InvocationResult {
            if let Some(reason) = admission.reason() {
                return InvocationResult::admission_rejected(reason);
            }
            self.calls.lock().unwrap().push(request.agent.clone());
            self.results
                .lock()
                .unwrap()
                .pop_front()
                .expect("one fake result per expected invocation")
        }
    }

    struct CancelAfterOneInvoker {
        cancellation: Arc<AtomicBool>,
        calls: Mutex<Vec<String>>,
        result: Mutex<Option<InvocationResult>>,
    }

    #[async_trait]
    impl SmokeInvoker for CancelAfterOneInvoker {
        async fn invoke(
            &self,
            request: &SmokeRequest,
            admission: &SpawnAdmission<'_>,
        ) -> InvocationResult {
            if let Some(reason) = admission.reason() {
                return InvocationResult::admission_rejected(reason);
            }
            self.calls.lock().unwrap().push(request.agent.clone());
            let result = self.result.lock().unwrap().take().unwrap();
            self.cancellation
                .store(true, std::sync::atomic::Ordering::Release);
            result
        }
    }

    struct CancelBeforeSpawnInvoker {
        cancellation: Arc<AtomicBool>,
        calls: Mutex<Vec<String>>,
        result: Mutex<Option<InvocationResult>>,
    }

    #[async_trait]
    impl SmokeInvoker for CancelBeforeSpawnInvoker {
        async fn invoke(
            &self,
            request: &SmokeRequest,
            admission: &SpawnAdmission<'_>,
        ) -> InvocationResult {
            self.cancellation
                .store(true, std::sync::atomic::Ordering::Release);
            if let Some(reason) = admission.reason() {
                return InvocationResult::admission_rejected(reason);
            }
            self.calls.lock().unwrap().push(request.agent.clone());
            self.result.lock().unwrap().take().unwrap()
        }
    }

    fn invocation(artifact: Value) -> InvocationResult {
        InvocationResult {
            process_success: artifact["success"].as_bool().unwrap(),
            artifact: Some(artifact),
            runner_error_code: None,
            not_run_reason: None,
        }
    }

    fn selection() -> SelectionRecord {
        SelectionRecord {
            lane: None,
            cases: Vec::new(),
            all: true,
        }
    }

    fn candidate_identity() -> CandidateIdentity {
        CandidateIdentity {
            canonical_path: "/tmp/a2a-bridge".into(),
            sha256: "c".repeat(64),
            byte_length: 42,
        }
    }

    fn aggregate_artifact(results: Vec<CaseResult>) -> AggregateArtifact {
        AggregateArtifact {
            schema_version: 1,
            candidate: candidate_identity(),
            manifest: ManifestIdentity {
                schema_version: 1,
                canonical_path: "/tmp/manifest.toml".into(),
                sha256: "a".repeat(64),
            },
            selection: selection(),
            environment_owner: "test-runner".into(),
            started_at_ms: 1,
            ended_at_ms: 2,
            cancelled: false,
            success: true,
            budget: BudgetOutcome {
                timeout_secs: 30,
                max_tokens: 100,
                max_cost_usd: Some(1.0),
                observed_tokens: 1,
                observed_cost_usd: 0.0,
                token_observation_missing_cases: 0,
                cost_observation_missing_cases: 0,
                exhausted: false,
            },
            floating_summary: floating_summary(&results),
            results,
        }
    }

    fn test_spawn_admission(cancellation: &AtomicBool) -> SpawnAdmission<'_> {
        SpawnAdmission {
            cancellation_requested: cancellation,
            started: Instant::now(),
            total_timeout: Duration::from_secs(30),
            case_timeout: Duration::from_secs(1),
        }
    }

    fn valid_pinned_toml() -> String {
        format!(
            r#"schema_version = 1
[budget]
timeout_secs = 30
max_tokens = 100
max_cost_usd = 1.0

[[cases]]
id = "host-case"
lane = "pinned"
evidence_path = "bridge_smoke"
execution_mode = "host"
os = "macos"
architecture = "aarch64"
environment_owner = "operator-host"
config = "config.toml"
agent = "codex"
model = "gpt-5.6-sol"
effort = "xhigh"
auth_path = "pre_authenticated"
required_env = []
probe = "minimal"
billable = true
timeout_secs = 10
max_tokens = 50
max_cost_usd = 0.5
retry_cap = 0
expected_status = "PASS"
classification = "support"

[cases.artifact]
retention_days = 30
redaction = "strict"

[cases.pins]
config_sha256 = {digest:?}
model = "gpt-5.6-sol"
adapter = "@agentclientprotocol/codex-acp=1.1.2"
agent_cli = "@openai/codex=0.144.1"
"#,
            digest = "a".repeat(64)
        )
    }

    fn parse_and_validate(raw: &str) -> Result<(), String> {
        parse_manifest_text(raw)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    #[test]
    fn manifest_schema_is_strict_and_accepts_an_exact_pinned_host_case() {
        parse_and_validate(&valid_pinned_toml()).unwrap();

        let unknown = valid_pinned_toml().replace(
            "retry_cap = 0",
            "retry_cap = 0\nprompt = \"ignore the fixed prompt\"",
        );
        assert!(parse_and_validate(&unknown)
            .unwrap_err()
            .contains("unknown field"));

        let bad_lane = valid_pinned_toml().replace("lane = \"pinned\"", "lane = \"nightly\"");
        assert!(parse_and_validate(&bad_lane).is_err());
    }

    #[test]
    fn manifest_rejects_duplicates_missing_pins_secrets_and_unbounded_cases() {
        let duplicate = format!(
            "{}\n{}",
            valid_pinned_toml(),
            valid_pinned_toml()
                .split("[[cases]]")
                .nth(1)
                .map(|tail| format!("[[cases]]{tail}"))
                .unwrap()
        );
        assert!(parse_and_validate(&duplicate)
            .unwrap_err()
            .contains("duplicate case id"));

        let missing_pins = valid_pinned_toml()
            .split("[cases.pins]")
            .next()
            .unwrap()
            .to_owned();
        assert!(parse_and_validate(&missing_pins)
            .unwrap_err()
            .contains("missing exact pins"));

        let secret = valid_pinned_toml().replace(
            "@agentclientprotocol/codex-acp=1.1.2",
            "sk-ant-secret-value",
        );
        assert!(parse_and_validate(&secret)
            .unwrap_err()
            .contains("secret-shaped"));

        let secret_id = valid_pinned_toml().replace("id = \"host-case\"", "id = \"sk-ant-secret\"");
        assert!(parse_and_validate(&secret_id)
            .unwrap_err()
            .contains("secret-shaped"));

        let secret_comment = format!("{}\n# token=opaque-comment-secret\n", valid_pinned_toml());
        assert!(parse_and_validate(&secret_comment)
            .unwrap_err()
            .contains("secret-shaped"));

        for embedded_secret in [
            "# AKIA1234567890ABCDEF\n",
            "# eyJheader.payload.signature\n",
        ] {
            let secret_comment = format!("{embedded_secret}{}", valid_pinned_toml());
            assert!(parse_and_validate(&secret_comment)
                .unwrap_err()
                .contains("secret-shaped"));
        }

        let floating_model = valid_pinned_toml().replace("gpt-5.6-sol", "latest");
        assert!(parse_and_validate(&floating_model)
            .unwrap_err()
            .contains("exact identity"));

        for selector in ["default", "gpt-5-chat-latest"] {
            let aliased_model = valid_pinned_toml().replace("gpt-5.6-sol", selector);
            assert!(
                parse_and_validate(&aliased_model)
                    .unwrap_err()
                    .contains("exact identity"),
                "pinned model selector {selector:?} must not remain floating"
            );
        }

        for floating_version in ["next", "1.x", "1.2", "beta", "1.2.3||2.0.0"] {
            let floating_adapter = valid_pinned_toml().replace("1.1.2", floating_version);
            assert!(
                parse_and_validate(&floating_adapter)
                    .unwrap_err()
                    .contains("exact identity"),
                "pinned package version {floating_version:?} must be immutable"
            );
        }

        let immutable_prerelease = valid_pinned_toml().replace(
            "@agentclientprotocol/codex-acp=1.1.2",
            "@agentclientprotocol/next-adapter=1.2.3-canary.1",
        );
        parse_and_validate(&immutable_prerelease).unwrap();

        let missing_adapter = valid_pinned_toml()
            .lines()
            .filter(|line| !line.starts_with("adapter = "))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(parse_and_validate(&missing_adapter)
            .unwrap_err()
            .contains("adapter pin"));

        for (from, to, expected) in [
            ("retry_cap = 0", "retry_cap = 1", "exactly zero"),
            ("timeout_secs = 10", "timeout_secs = 0", "timeout_secs"),
            ("billable = true", "billable = false", "billable=true"),
            ("max_tokens = 50", "max_tokens = 101", "token budget"),
            (
                "@openai/codex=0.144.1",
                "@openai/codex@0.144.1",
                "<package>=<version>",
            ),
        ] {
            let invalid = valid_pinned_toml().replace(from, to);
            assert!(
                parse_and_validate(&invalid).unwrap_err().contains(expected),
                "missing {expected:?} rejection"
            );
        }
    }

    #[test]
    fn advertised_raw_model_ids_that_are_also_fallback_aliases_remain_pinnable() {
        for raw_model in ["opus", "gpt-5-6-sol"] {
            let manifest = valid_pinned_toml().replace("gpt-5.6-sol", raw_model);
            parse_and_validate(&manifest)
                .unwrap_or_else(|error| panic!("raw advertised model {raw_model:?}: {error}"));
        }

        let mut case = case("raw-opus", EvidenceStatus::Pass);
        case.model = "opus".into();
        let mut artifact = smoke(&case, true, Some(1));
        artifact["session"]["effective_request"]["model"] = Value::String("default".into());
        assert!(drift_for(&case, &artifact).contains(&"capability.effective_model".into()));
    }

    #[test]
    fn pinned_container_requires_an_immutable_image_digest() {
        let container = valid_pinned_toml().replace(
            "execution_mode = \"host\"",
            "execution_mode = \"container_ro\"",
        );
        assert!(parse_and_validate(&container)
            .unwrap_err()
            .contains("immutable expected_image_digest"));

        let with_latest = container.replace(
            "config = \"config.toml\"",
            "expected_image_digest = \"reader:latest\"\nconfig = \"config.toml\"",
        );
        assert!(parse_and_validate(&with_latest)
            .unwrap_err()
            .contains("immutable expected_image_digest"));

        let digest = format!("sha256:{}", "b".repeat(64));
        let with_expected = container.replace(
            "config = \"config.toml\"",
            &format!("expected_image_digest = {digest:?}\nconfig = \"config.toml\""),
        );
        let with_digest = format!("{with_expected}image_digest = {digest:?}\n");
        parse_and_validate(&with_digest).unwrap();
    }

    #[test]
    fn pinned_dependencies_are_explicit_for_cli_acp_and_remote_api_paths() {
        let direct_with_adapter = valid_pinned_toml().replace(
            "evidence_path = \"bridge_smoke\"",
            "evidence_path = \"direct_cli\"",
        );
        assert!(parse_and_validate(&direct_with_adapter)
            .unwrap_err()
            .contains("must not declare an adapter pin"));
        let direct = direct_with_adapter
            .lines()
            .filter(|line| !line.starts_with("adapter = "))
            .collect::<Vec<_>>()
            .join("\n");
        parse_and_validate(&direct).unwrap();

        let remote = valid_pinned_toml()
            .replace(
                "execution_mode = \"host\"",
                "execution_mode = \"remote_api\"",
            )
            .lines()
            .filter(|line| !line.starts_with("adapter = ") && !line.starts_with("agent_cli = "))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(parse_and_validate(&remote)
            .unwrap_err()
            .contains("component pin"));
        let generic_execution =
            format!("{remote}\ncomponents = {{ execution = \"kind=api execution=remote\" }}\n");
        assert!(parse_and_validate(&generic_execution)
            .unwrap_err()
            .contains("provider identity"));

        let remote = format!(
            "{remote}\ncomponents = {{ provider = \"openai\", api = \"responses\", api_version = \"v1\" }}\n"
        );
        parse_and_validate(&remote).unwrap();

        for ranged_version in ["v1 || v2", "v1 - v2", "v1 or v2", "v1 v2", "v1/v2"] {
            let ranged = remote.replace(
                "api_version = \"v1\"",
                &format!("api_version = {ranged_version:?}"),
            );
            assert!(
                parse_and_validate(&ranged)
                    .unwrap_err()
                    .contains("exact identity"),
                "remote API version range {ranged_version:?} must not satisfy an exact pin"
            );
        }

        for evidence_path in ["direct_cli", "direct_acp"] {
            let contradictory = remote.replace("bridge_smoke", evidence_path);
            assert!(
                parse_and_validate(&contradictory)
                    .unwrap_err()
                    .contains("remote API execution mode"),
                "remote API mode must not bypass {evidence_path:?} dependency requirements"
            );
        }
    }

    #[test]
    fn api_key_auth_records_only_a_valid_credential_environment_name() {
        let api = valid_pinned_toml().replace(
            "auth_path = \"pre_authenticated\"",
            "auth_path = \"api_key_env\"",
        );
        assert!(parse_and_validate(&api)
            .unwrap_err()
            .contains("requires a valid credential_env name"));

        let invalid = api.replace(
            "required_env = []",
            "credential_env = \"not-a-valid-env-name\"\nrequired_env = []",
        );
        assert!(parse_and_validate(&invalid)
            .unwrap_err()
            .contains("requires a valid credential_env name"));

        let secret = api.replace(
            "required_env = []",
            "credential_env = \"sk-ant-not-a-name\"\nrequired_env = []",
        );
        assert!(parse_and_validate(&secret)
            .unwrap_err()
            .contains("secret-shaped"));

        let valid = api.replace(
            "required_env = []",
            "credential_env = \"OPENROUTER_API_KEY\"\nrequired_env = []",
        );
        parse_and_validate(&valid).unwrap();

        let wrong_path = valid.replace(
            "auth_path = \"api_key_env\"",
            "auth_path = \"pre_authenticated\"",
        );
        assert!(parse_and_validate(&wrong_path)
            .unwrap_err()
            .contains("must not declare credential_env"));

        let misclassified = valid_pinned_toml().replace(
            "required_env = []",
            "required_env = [{ name = \"OPENAI_API_KEY\" }]",
        );
        assert!(parse_and_validate(&misclassified)
            .unwrap_err()
            .contains("credential_env, not required_env"));

        let non_secret_prerequisite = valid_pinned_toml().replace(
            "required_env = []",
            "required_env = [{ name = \"AWS_PROFILE\" }, { name = \"PATH\" }]",
        );
        parse_and_validate(&non_secret_prerequisite).unwrap();

        let value_bound_prerequisite = valid_pinned_toml().replace(
            "required_env = []",
            "required_env = [{ name = \"A2A_BRIDGE_ALLOW_FABLE\", one_of = [\"1\", \"true\"] }]",
        );
        parse_and_validate(&value_bound_prerequisite).unwrap();

        for credential_name in [
            "CLAUDE_AUTH",
            "HTTP_AUTHORIZATION",
            "SERVICE_BEARER",
            "SESSION_COOKIE",
            "DB_PASS",
            "DB_PASSWD",
            "AWS_CREDS",
            "AWS_CREDENTIALS",
        ] {
            let misclassified = valid_pinned_toml().replace(
                "required_env = []",
                &format!("required_env = [{{ name = {credential_name:?} }}]"),
            );
            assert!(
                parse_and_validate(&misclassified)
                    .unwrap_err()
                    .contains("credential_env, not required_env"),
                "credential-shaped prerequisite {credential_name:?} must fail closed"
            );
        }
    }

    #[test]
    fn environment_prerequisites_can_require_an_exact_non_secret_value() {
        let path = std::env::var("PATH").expect("test process has PATH");
        let mut case = case("env", EvidenceStatus::Pass);
        case.required_env = vec![RequiredEnvironment {
            name: "PATH".into(),
            one_of: vec![path],
        }];
        assert_eq!(case_environment_ready(&case, "test-runner"), None);

        case.required_env[0].one_of = vec!["definitely-not-the-process-path".into()];
        assert_eq!(
            case_environment_ready(&case, "test-runner"),
            Some("required_environment_value_mismatch")
        );
    }

    #[tokio::test]
    async fn floating_failure_is_blocking_while_selected_cases_still_invoke_once_without_retry() {
        let dir = tempfile::tempdir().unwrap();
        let first = case("first", EvidenceStatus::Fail);
        let second = case("second", EvidenceStatus::Pass);
        let invoker = FakeInvoker::new(vec![
            invocation(smoke(&first, false, Some(1))),
            invocation(smoke(&second, true, Some(2))),
        ]);
        let loaded = loaded(dir.path(), vec![first, second]);
        let cancelled = AtomicBool::new(false);
        let candidate = candidate_identity();
        let selection = selection();
        let selected_indices = [0, 1];

        let aggregate = build_aggregate(
            AggregateInputs {
                loaded: &loaded,
                candidate: &candidate,
                selection: &selection,
                selected_indices: &selected_indices,
                environment_owner: "test-runner",
                scratch: dir.path(),
                cancellation_requested: &cancelled,
            },
            &invoker,
        )
        .await;

        assert_eq!(&*invoker.calls.lock().unwrap(), &["first", "second"]);
        assert_eq!(aggregate.results.len(), 2);
        assert!(aggregate
            .results
            .iter()
            .all(|result| result.execution == ExecutionState::Completed));
        assert!(aggregate
            .results
            .iter()
            .all(|result| result.expectation_met));
        assert_eq!(
            aggregate.results[0].candidate_outcome,
            Some(CandidateOutcome::Fail)
        );
        assert_eq!(
            aggregate.results[1].candidate_outcome,
            Some(CandidateOutcome::Pass)
        );
        assert_eq!(
            aggregate.floating_summary,
            Some(FloatingSummary {
                candidate_pass: 1,
                candidate_fail: 1,
                candidate_unknown: 0,
            })
        );
        assert!(!aggregate.success);
    }

    #[test]
    fn lane_case_and_all_selection_never_collapse_to_an_implicit_all() {
        let mut pinned = case("pinned", EvidenceStatus::Pass);
        pinned.lane = Lane::Pinned;
        let floating = case("floating", EvidenceStatus::Pass);
        let manifest = manifest(vec![pinned, floating]);

        assert_eq!(
            select_case_indices(
                &manifest,
                &SelectionRecord {
                    lane: Some(Lane::Pinned),
                    cases: Vec::new(),
                    all: false,
                },
            )
            .unwrap(),
            [0]
        );
        assert_eq!(
            select_case_indices(
                &manifest,
                &SelectionRecord {
                    lane: None,
                    cases: vec!["floating".into()],
                    all: false,
                },
            )
            .unwrap(),
            [1]
        );
        assert_eq!(
            select_case_indices(
                &manifest,
                &SelectionRecord {
                    lane: None,
                    cases: Vec::new(),
                    all: true,
                },
            )
            .unwrap(),
            [0, 1]
        );
        assert!(select_case_indices(
            &manifest,
            &SelectionRecord {
                lane: None,
                cases: vec!["missing".into()],
                all: false,
            },
        )
        .is_err());
    }

    #[tokio::test]
    async fn runner_failure_stops_before_any_later_billable_case() {
        let dir = tempfile::tempdir().unwrap();
        let first = case("first", EvidenceStatus::Pass);
        let second = case("second", EvidenceStatus::Pass);
        let invoker = FakeInvoker::new(vec![InvocationResult {
            artifact: None,
            process_success: false,
            runner_error_code: Some("smoke_artifact_missing_or_invalid_file"),
            not_run_reason: None,
        }]);
        let loaded = loaded(dir.path(), vec![first, second]);
        let cancelled = AtomicBool::new(false);
        let candidate = candidate_identity();
        let selection = selection();
        let selected_indices = [0, 1];

        let aggregate = build_aggregate(
            AggregateInputs {
                loaded: &loaded,
                candidate: &candidate,
                selection: &selection,
                selected_indices: &selected_indices,
                environment_owner: "test-runner",
                scratch: dir.path(),
                cancellation_requested: &cancelled,
            },
            &invoker,
        )
        .await;

        assert_eq!(&*invoker.calls.lock().unwrap(), &["first"]);
        assert_eq!(
            aggregate.results[0].execution,
            ExecutionState::RunnerFailure
        );
        assert_eq!(aggregate.results[1].execution, ExecutionState::NotRun);
        assert_eq!(
            aggregate.results[1].not_run_reason.as_deref(),
            Some("prior_runner_failure")
        );
        assert!(aggregate
            .results
            .iter()
            .all(|result| { result.candidate_outcome == Some(CandidateOutcome::Unknown) }));
        assert_eq!(
            aggregate.floating_summary,
            Some(FloatingSummary {
                candidate_pass: 0,
                candidate_fail: 0,
                candidate_unknown: 2,
            })
        );
        assert!(!aggregate.success);
    }

    #[tokio::test]
    async fn schema_and_exit_inconsistency_are_unaccounted_runner_failures() {
        let dir = tempfile::tempdir().unwrap();
        let case = case("first", EvidenceStatus::Pass);
        let candidate = candidate_identity();
        let selection = selection();
        let selected_indices = [0];

        let mut wrong_schema = smoke(&case, true, Some(1));
        wrong_schema["request"]["agent"] = Value::String("different-agent".into());
        let schema_invoker = FakeInvoker::new(vec![invocation(wrong_schema)]);
        let schema_loaded = loaded(dir.path(), vec![case.clone()]);
        let cancelled = AtomicBool::new(false);
        let schema = build_aggregate(
            AggregateInputs {
                loaded: &schema_loaded,
                candidate: &candidate,
                selection: &selection,
                selected_indices: &selected_indices,
                environment_owner: "test-runner",
                scratch: dir.path(),
                cancellation_requested: &cancelled,
            },
            &schema_invoker,
        )
        .await;
        assert_eq!(
            schema.results[0].runner_error_code.as_deref(),
            Some("smoke_artifact_schema_mismatch")
        );
        assert!(!schema.success);

        let exit_invoker = FakeInvoker::new(vec![InvocationResult {
            artifact: Some(smoke(&case, true, Some(1))),
            process_success: false,
            runner_error_code: None,
            not_run_reason: None,
        }]);
        let exit_loaded = loaded(dir.path(), vec![case]);
        let exit = build_aggregate(
            AggregateInputs {
                loaded: &exit_loaded,
                candidate: &candidate,
                selection: &selection,
                selected_indices: &selected_indices,
                environment_owner: "test-runner",
                scratch: dir.path(),
                cancellation_requested: &cancelled,
            },
            &exit_invoker,
        )
        .await;
        assert_eq!(
            exit.results[0].runner_error_code.as_deref(),
            Some("smoke_exit_artifact_mismatch")
        );
        assert!(!exit.success);
    }

    #[tokio::test]
    async fn observed_budget_exhaustion_stops_before_the_next_case() {
        let dir = tempfile::tempdir().unwrap();
        let first = case("first", EvidenceStatus::Pass);
        let second = case("second", EvidenceStatus::Pass);
        let mut loaded = loaded(dir.path(), vec![first.clone(), second]);
        loaded.manifest.budget.max_tokens = 10;
        let mut first_smoke = smoke(&first, true, Some(1));
        first_smoke["turn"]["usage"]["terminal"] = json!({
            "totalTokens": 11,
            "inputTokens": 7,
            "outputTokens": 4
        });
        let invoker = FakeInvoker::new(vec![invocation(first_smoke)]);
        let cancelled = AtomicBool::new(false);
        let candidate = candidate_identity();
        let selection = selection();
        let selected_indices = [0, 1];

        let aggregate = build_aggregate(
            AggregateInputs {
                loaded: &loaded,
                candidate: &candidate,
                selection: &selection,
                selected_indices: &selected_indices,
                environment_owner: "test-runner",
                scratch: dir.path(),
                cancellation_requested: &cancelled,
            },
            &invoker,
        )
        .await;

        assert_eq!(&*invoker.calls.lock().unwrap(), &["first"]);
        assert!(aggregate.budget.exhausted);
        assert_eq!(
            aggregate.results[0].budget_violations,
            ["case_token_cap_exceeded"]
        );
        assert_eq!(
            aggregate.results[0].candidate_outcome,
            Some(CandidateOutcome::Unknown)
        );
        assert_eq!(aggregate.results[1].execution, ExecutionState::NotRun);
        assert_eq!(
            aggregate.results[1].not_run_reason.as_deref(),
            Some("total_budget_exhausted")
        );
        assert!(!aggregate.success);
    }

    #[tokio::test]
    async fn floating_case_cap_violation_is_blocking_and_stops_before_next_case() {
        let dir = tempfile::tempdir().unwrap();
        let mut first = case("first", EvidenceStatus::Pass);
        first.max_tokens = 1;
        let second = case("second", EvidenceStatus::Pass);
        let invoker = FakeInvoker::new(vec![invocation(smoke(&first, true, Some(2)))]);
        let loaded = loaded(dir.path(), vec![first, second]);
        let cancelled = AtomicBool::new(false);
        let candidate = candidate_identity();
        let selection = selection();
        let selected_indices = [0, 1];

        let aggregate = build_aggregate(
            AggregateInputs {
                loaded: &loaded,
                candidate: &candidate,
                selection: &selection,
                selected_indices: &selected_indices,
                environment_owner: "test-runner",
                scratch: dir.path(),
                cancellation_requested: &cancelled,
            },
            &invoker,
        )
        .await;

        assert_eq!(&*invoker.calls.lock().unwrap(), &["first"]);
        assert_eq!(
            aggregate.results[0].budget_violations,
            ["case_token_cap_exceeded"]
        );
        assert!(aggregate.budget.exhausted);
        assert_eq!(
            aggregate.results[1].not_run_reason.as_deref(),
            Some("total_budget_exhausted")
        );
        assert!(!aggregate.success);
    }

    #[tokio::test]
    async fn insufficient_token_headroom_stops_before_the_next_billable_case() {
        let dir = tempfile::tempdir().unwrap();
        let mut first = case("first", EvidenceStatus::Pass);
        first.max_tokens = 5;
        let mut second = case("second", EvidenceStatus::Pass);
        second.max_tokens = 7;
        let invoker = FakeInvoker::new(vec![
            invocation(smoke(&first, true, Some(4))),
            invocation(smoke(&second, true, Some(1))),
        ]);
        let mut loaded = loaded(dir.path(), vec![first, second]);
        loaded.manifest.budget.max_tokens = 10;
        let cancelled = AtomicBool::new(false);
        let candidate = candidate_identity();
        let selection = selection();
        let selected_indices = [0, 1];

        let aggregate = build_aggregate(
            AggregateInputs {
                loaded: &loaded,
                candidate: &candidate,
                selection: &selection,
                selected_indices: &selected_indices,
                environment_owner: "test-runner",
                scratch: dir.path(),
                cancellation_requested: &cancelled,
            },
            &invoker,
        )
        .await;

        assert_eq!(&*invoker.calls.lock().unwrap(), &["first"]);
        assert_eq!(
            aggregate.results[1].not_run_reason.as_deref(),
            Some("total_budget_insufficient_for_case")
        );
        assert!(aggregate.budget.exhausted);
        assert!(!aggregate.success);
    }

    #[tokio::test]
    async fn insufficient_observable_cost_headroom_stops_before_the_next_case() {
        let dir = tempfile::tempdir().unwrap();
        let mut first = case("first", EvidenceStatus::Pass);
        first.max_cost_usd = Some(0.6);
        let mut second = case("second", EvidenceStatus::Pass);
        second.max_cost_usd = Some(0.6);
        let mut first_smoke = smoke(&first, true, Some(1));
        first_smoke["turn"]["usage"]["cost"] = json!({"amount": 0.5, "currency": "USD"});
        let invoker = FakeInvoker::new(vec![
            invocation(first_smoke),
            invocation(smoke(&second, true, Some(1))),
        ]);
        let mut loaded = loaded(dir.path(), vec![first, second]);
        loaded.manifest.budget.max_cost_usd = Some(1.0);
        let cancelled = AtomicBool::new(false);
        let candidate = candidate_identity();
        let selection = selection();
        let selected_indices = [0, 1];

        let aggregate = build_aggregate(
            AggregateInputs {
                loaded: &loaded,
                candidate: &candidate,
                selection: &selection,
                selected_indices: &selected_indices,
                environment_owner: "test-runner",
                scratch: dir.path(),
                cancellation_requested: &cancelled,
            },
            &invoker,
        )
        .await;

        assert_eq!(&*invoker.calls.lock().unwrap(), &["first"]);
        assert_eq!(
            aggregate.results[1].not_run_reason.as_deref(),
            Some("total_budget_insufficient_for_case")
        );
        assert!(aggregate.budget.exhausted);
        assert!(!aggregate.success);
    }

    #[tokio::test]
    async fn invalid_reported_cost_is_explicit_and_blocks_later_cases() {
        for invalid_cost in [
            json!({"amount": -0.01, "currency": "USD"}),
            json!({"amount": "NaN", "currency": "USD"}),
            json!({"amount": "Infinity", "currency": "USD"}),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let first = case("first", EvidenceStatus::Pass);
            let second = case("second", EvidenceStatus::Pass);
            let mut first_smoke = smoke(&first, true, Some(1));
            first_smoke["turn"]["usage"]["cost"] = invalid_cost;
            let invoker = FakeInvoker::new(vec![
                invocation(first_smoke),
                invocation(smoke(&second, true, Some(1))),
            ]);
            let loaded = loaded(dir.path(), vec![first, second]);
            let cancelled = AtomicBool::new(false);
            let candidate = candidate_identity();
            let selection = selection();
            let selected_indices = [0, 1];

            let aggregate = build_aggregate(
                AggregateInputs {
                    loaded: &loaded,
                    candidate: &candidate,
                    selection: &selection,
                    selected_indices: &selected_indices,
                    environment_owner: "test-runner",
                    scratch: dir.path(),
                    cancellation_requested: &cancelled,
                },
                &invoker,
            )
            .await;

            assert_eq!(&*invoker.calls.lock().unwrap(), &["first"]);
            assert_eq!(
                aggregate.results[0].budget_violations,
                ["case_cost_observation_invalid"]
            );
            assert_eq!(aggregate.budget.cost_observation_missing_cases, 0);
            assert!(aggregate.budget.exhausted);
            assert_eq!(
                aggregate.results[1].not_run_reason.as_deref(),
                Some("total_budget_exhausted")
            );
            assert!(!aggregate.success);
        }
    }

    #[tokio::test]
    async fn smoke_rejected_nonfinite_cost_is_explicit_and_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let first = case("first", EvidenceStatus::Pass);
        let second = case("second", EvidenceStatus::Pass);
        let mut first_smoke = smoke(&first, true, Some(1));
        first_smoke["turn"]["usage"]["cost_rejected"] =
            Value::String("negative_or_non_finite".into());
        let invoker = FakeInvoker::new(vec![
            invocation(first_smoke),
            invocation(smoke(&second, true, Some(1))),
        ]);
        let loaded = loaded(dir.path(), vec![first, second]);
        let cancelled = AtomicBool::new(false);
        let candidate = candidate_identity();
        let selection = selection();
        let selected_indices = [0, 1];

        let aggregate = build_aggregate(
            AggregateInputs {
                loaded: &loaded,
                candidate: &candidate,
                selection: &selection,
                selected_indices: &selected_indices,
                environment_owner: "test-runner",
                scratch: dir.path(),
                cancellation_requested: &cancelled,
            },
            &invoker,
        )
        .await;

        assert_eq!(
            aggregate.results[0].budget_violations,
            ["case_cost_observation_invalid"]
        );
        assert_eq!(aggregate.budget.cost_observation_missing_cases, 0);
        assert_eq!(&*invoker.calls.lock().unwrap(), &["first"]);
        assert_eq!(
            aggregate.results[1].not_run_reason.as_deref(),
            Some("total_budget_exhausted")
        );
        assert!(!aggregate.success);
    }

    #[tokio::test]
    async fn unsupported_advisory_case_is_classified_before_budget_admission() {
        let dir = tempfile::tempdir().unwrap();
        let mut first = case("first", EvidenceStatus::Pass);
        first.max_tokens = 5;
        let mut unsupported = case("unsupported", EvidenceStatus::Stale);
        unsupported.lane = Lane::Pinned;
        unsupported.evidence_path = EvidencePath::DirectCli;
        unsupported.classification = Classification::NonGoal;
        unsupported.baseline_case = None;
        unsupported.resolved = None;
        unsupported.max_tokens = 7;
        let invoker = FakeInvoker::new(vec![invocation(smoke(&first, true, Some(4)))]);
        let mut loaded = loaded(dir.path(), vec![first, unsupported]);
        loaded.manifest.budget.max_tokens = 10;
        let cancelled = AtomicBool::new(false);
        let candidate = candidate_identity();
        let selection = selection();
        let selected_indices = [0, 1];

        let aggregate = build_aggregate(
            AggregateInputs {
                loaded: &loaded,
                candidate: &candidate,
                selection: &selection,
                selected_indices: &selected_indices,
                environment_owner: "test-runner",
                scratch: dir.path(),
                cancellation_requested: &cancelled,
            },
            &invoker,
        )
        .await;

        assert_eq!(&*invoker.calls.lock().unwrap(), &["first"]);
        assert_eq!(
            aggregate.results[1].not_run_reason.as_deref(),
            Some("evidence_path_not_implemented_in_r3a")
        );
        assert!(!aggregate.budget.exhausted);
        assert!(aggregate.success);
    }

    #[tokio::test]
    async fn final_case_elapsed_time_exhaustion_is_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let only = case("only", EvidenceStatus::Pass);
        let invoker = DelayedInvoker {
            delay: Duration::from_millis(2_100),
            calls: Mutex::new(Vec::new()),
            result: Mutex::new(Some(invocation(smoke(&only, true, Some(1))))),
        };
        let mut loaded = loaded(dir.path(), vec![only]);
        loaded.manifest.budget.timeout_secs = 2;
        let cancelled = AtomicBool::new(false);
        let candidate = candidate_identity();
        let selection = selection();
        let selected_indices = [0];

        let aggregate = build_aggregate(
            AggregateInputs {
                loaded: &loaded,
                candidate: &candidate,
                selection: &selection,
                selected_indices: &selected_indices,
                environment_owner: "test-runner",
                scratch: dir.path(),
                cancellation_requested: &cancelled,
            },
            &invoker,
        )
        .await;

        assert_eq!(&*invoker.calls.lock().unwrap(), &["only"]);
        assert!(aggregate.budget.exhausted);
        assert_eq!(
            aggregate.results[0].budget_violations,
            ["total_timeout_exceeded"]
        );
        assert!(!aggregate.success);
    }

    #[tokio::test]
    async fn ineligible_and_representative_cases_are_explicit_and_never_invoked() {
        let dir = tempfile::tempdir().unwrap();
        let mut wrong_platform = case("wrong-platform", EvidenceStatus::Unknown);
        wrong_platform.os = "other-os".into();
        let mut representative = case("representative", EvidenceStatus::Unknown);
        representative.probe = ProbeType::Representative;
        let mut direct = case("direct", EvidenceStatus::Unknown);
        direct.evidence_path = EvidencePath::DirectAcp;
        let invoker = FakeInvoker::new(Vec::new());
        let loaded = loaded(dir.path(), vec![wrong_platform, representative, direct]);
        let cancelled = AtomicBool::new(false);
        let candidate = candidate_identity();
        let selection = selection();
        let selected_indices = [0, 1, 2];

        let aggregate = build_aggregate(
            AggregateInputs {
                loaded: &loaded,
                candidate: &candidate,
                selection: &selection,
                selected_indices: &selected_indices,
                environment_owner: "test-runner",
                scratch: dir.path(),
                cancellation_requested: &cancelled,
            },
            &invoker,
        )
        .await;

        assert!(invoker.calls.lock().unwrap().is_empty());
        assert_eq!(aggregate.results[0].execution, ExecutionState::NotRun);
        assert_eq!(
            aggregate.results[0].not_run_reason.as_deref(),
            Some("environment_platform_mismatch")
        );
        assert_eq!(
            aggregate.results[1].not_run_reason.as_deref(),
            Some("representative_probe_not_implemented_in_r3a")
        );
        assert_eq!(
            aggregate.results[2].not_run_reason.as_deref(),
            Some("evidence_path_not_implemented_in_r3a")
        );
    }

    #[tokio::test]
    async fn pinned_config_digest_is_an_admission_gate_before_provider_spawn() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("case.toml");
        std::fs::write(&config_path, b"exact config bytes").unwrap();
        let actual_digest = local_file::sha256_hex(b"exact config bytes");

        let pinned_case = |digest: String| {
            let mut pinned = case("pinned", EvidenceStatus::Pass);
            pinned.lane = Lane::Pinned;
            pinned.classification = Classification::Support;
            pinned.config = PathBuf::from("case.toml");
            pinned.pins = Some(PinSet {
                config_sha256: digest,
                model: pinned.model.clone(),
                adapter: Some("test-adapter=1.2.3".into()),
                agent_cli: Some("test-cli=4.5.6".into()),
                image_digest: None,
                components: BTreeMap::new(),
            });
            pinned
        };
        let cancelled = AtomicBool::new(false);
        let candidate = candidate_identity();
        let selection = selection();
        let selected_indices = [0];

        let changed = pinned_case("b".repeat(64));
        let changed_invoker = FakeInvoker::new(vec![invocation(smoke(&changed, true, Some(1)))]);
        let changed_loaded = loaded(dir.path(), vec![changed]);
        let changed_aggregate = build_aggregate(
            AggregateInputs {
                loaded: &changed_loaded,
                candidate: &candidate,
                selection: &selection,
                selected_indices: &selected_indices,
                environment_owner: "test-runner",
                scratch: dir.path(),
                cancellation_requested: &cancelled,
            },
            &changed_invoker,
        )
        .await;
        assert!(changed_invoker.calls.lock().unwrap().is_empty());
        assert_eq!(
            changed_aggregate.results[0].runner_error_code.as_deref(),
            Some("config_pin_mismatch")
        );

        let exact = pinned_case(actual_digest);
        let exact_invoker = FakeInvoker::new(vec![invocation(smoke(&exact, true, Some(1)))]);
        let exact_loaded = loaded(dir.path(), vec![exact]);
        let exact_aggregate = build_aggregate(
            AggregateInputs {
                loaded: &exact_loaded,
                candidate: &candidate,
                selection: &selection,
                selected_indices: &selected_indices,
                environment_owner: "test-runner",
                scratch: dir.path(),
                cancellation_requested: &cancelled,
            },
            &exact_invoker,
        )
        .await;
        assert_eq!(&*exact_invoker.calls.lock().unwrap(), &["pinned"]);
        assert_ne!(
            exact_aggregate.results[0].runner_error_code.as_deref(),
            Some("config_pin_mismatch")
        );

        std::fs::remove_file(&config_path).unwrap();
        let missing = pinned_case("c".repeat(64));
        let missing_invoker = FakeInvoker::new(vec![invocation(smoke(&missing, true, Some(1)))]);
        let missing_loaded = loaded(dir.path(), vec![missing]);
        let missing_aggregate = build_aggregate(
            AggregateInputs {
                loaded: &missing_loaded,
                candidate: &candidate,
                selection: &selection,
                selected_indices: &selected_indices,
                environment_owner: "test-runner",
                scratch: dir.path(),
                cancellation_requested: &cancelled,
            },
            &missing_invoker,
        )
        .await;
        assert!(missing_invoker.calls.lock().unwrap().is_empty());
        assert_eq!(
            missing_aggregate.results[0].runner_error_code.as_deref(),
            Some("config_pin_unavailable")
        );
    }

    #[tokio::test]
    async fn pinned_non_goal_is_advisory_but_pinned_support_remains_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let mut non_goal = case("historic-control", EvidenceStatus::Pass);
        non_goal.lane = Lane::Pinned;
        non_goal.classification = Classification::NonGoal;
        non_goal.evidence_path = EvidencePath::DirectCli;
        let invoker = FakeInvoker::new(Vec::new());
        let cancelled = AtomicBool::new(false);
        let candidate = candidate_identity();
        let selection = selection();
        let selected_indices = [0];

        let non_goal_loaded = loaded(dir.path(), vec![non_goal.clone()]);
        let advisory = build_aggregate(
            AggregateInputs {
                loaded: &non_goal_loaded,
                candidate: &candidate,
                selection: &selection,
                selected_indices: &selected_indices,
                environment_owner: "test-runner",
                scratch: dir.path(),
                cancellation_requested: &cancelled,
            },
            &invoker,
        )
        .await;
        assert!(!advisory.results[0].expectation_met);
        assert!(advisory.success);

        let mut support = non_goal;
        support.classification = Classification::Support;
        let support_loaded = loaded(dir.path(), vec![support]);
        let blocking = build_aggregate(
            AggregateInputs {
                loaded: &support_loaded,
                candidate: &candidate,
                selection: &selection,
                selected_indices: &selected_indices,
                environment_owner: "test-runner",
                scratch: dir.path(),
                cancellation_requested: &cancelled,
            },
            &invoker,
        )
        .await;
        assert!(!blocking.success);
    }

    #[tokio::test]
    async fn cancellation_finishes_the_active_smoke_and_starts_no_later_case() {
        let dir = tempfile::tempdir().unwrap();
        let first = case("first", EvidenceStatus::Pass);
        let second = case("second", EvidenceStatus::Pass);
        let cancellation = Arc::new(AtomicBool::new(false));
        let invoker = CancelAfterOneInvoker {
            cancellation: Arc::clone(&cancellation),
            calls: Mutex::new(Vec::new()),
            result: Mutex::new(Some(invocation(smoke(&first, true, Some(1))))),
        };
        let loaded = loaded(dir.path(), vec![first, second]);
        let candidate = candidate_identity();
        let selection = selection();
        let selected_indices = [0, 1];

        let aggregate = build_aggregate(
            AggregateInputs {
                loaded: &loaded,
                candidate: &candidate,
                selection: &selection,
                selected_indices: &selected_indices,
                environment_owner: "test-runner",
                scratch: dir.path(),
                cancellation_requested: cancellation.as_ref(),
            },
            &invoker,
        )
        .await;

        assert_eq!(&*invoker.calls.lock().unwrap(), &["first"]);
        assert_eq!(aggregate.results[0].execution, ExecutionState::Completed);
        assert_eq!(aggregate.results[1].execution, ExecutionState::NotRun);
        assert_eq!(
            aggregate.results[1].not_run_reason.as_deref(),
            Some("cancellation_requested")
        );
        assert!(aggregate.cancelled);
        assert!(!aggregate.success);
    }

    #[tokio::test]
    async fn cancellation_is_rechecked_after_pre_spawn_work() {
        let dir = tempfile::tempdir().unwrap();
        let first = case("first", EvidenceStatus::Pass);
        let candidate = candidate_identity();
        let selection = selection();
        let selected_indices = [0];

        let cancellation = Arc::new(AtomicBool::new(false));
        let cancel_invoker = CancelBeforeSpawnInvoker {
            cancellation: Arc::clone(&cancellation),
            calls: Mutex::new(Vec::new()),
            result: Mutex::new(Some(invocation(smoke(&first, true, Some(1))))),
        };
        let cancel_loaded = loaded(dir.path(), vec![first.clone()]);
        let cancelled = build_aggregate(
            AggregateInputs {
                loaded: &cancel_loaded,
                candidate: &candidate,
                selection: &selection,
                selected_indices: &selected_indices,
                environment_owner: "test-runner",
                scratch: dir.path(),
                cancellation_requested: cancellation.as_ref(),
            },
            &cancel_invoker,
        )
        .await;
        assert!(cancel_invoker.calls.lock().unwrap().is_empty());
        assert_eq!(cancelled.results[0].execution, ExecutionState::NotRun);
        assert_eq!(
            cancelled.results[0].not_run_reason.as_deref(),
            Some("cancellation_requested")
        );
    }

    #[tokio::test]
    async fn elapsed_budget_is_rechecked_after_pre_spawn_work() {
        let dir = tempfile::tempdir().unwrap();
        let first = case("first", EvidenceStatus::Pass);
        let candidate = candidate_identity();
        let selection = selection();
        let selected_indices = [0];
        let mut timeout_loaded = loaded(dir.path(), vec![first.clone()]);
        timeout_loaded.manifest.budget.timeout_secs = 2;
        let timeout_invoker = PreSpawnDelayedInvoker {
            delay: Duration::from_millis(1_100),
            calls: Mutex::new(Vec::new()),
            result: Mutex::new(Some(invocation(smoke(&first, true, Some(1))))),
        };
        let not_cancelled = AtomicBool::new(false);
        let timed_out = build_aggregate(
            AggregateInputs {
                loaded: &timeout_loaded,
                candidate: &candidate,
                selection: &selection,
                selected_indices: &selected_indices,
                environment_owner: "test-runner",
                scratch: dir.path(),
                cancellation_requested: &not_cancelled,
            },
            &timeout_invoker,
        )
        .await;
        assert!(timeout_invoker.calls.lock().unwrap().is_empty());
        assert_eq!(timed_out.results[0].execution, ExecutionState::NotRun);
        assert_eq!(
            timed_out.results[0].not_run_reason.as_deref(),
            Some("total_budget_insufficient_for_case")
        );
        assert!(timed_out.budget.exhausted);
        assert!(!timed_out.success);
    }

    #[tokio::test]
    async fn secret_shaped_smoke_artifact_is_omitted_and_fails_the_runner() {
        assert!(value_contains_secret(
            &json!({"detail": "prefix opaque-credential-value suffix"}),
            &["opaque-credential-value".into()]
        ));
        assert!(value_contains_secret(
            &json!({"access_token": "otherwise-unrecognizable-value"}),
            &[]
        ));
        let dir = tempfile::tempdir().unwrap();
        let case = case("secret-control", EvidenceStatus::Pass);
        let mut artifact = smoke(&case, true, Some(1));
        artifact["target"]["provenance"] = json!([{
            "check": "provenance:secret-control:adapter",
            "detail": "sk-ant-secret-value"
        }]);
        let invoker = FakeInvoker::new(vec![invocation(artifact)]);
        let loaded = loaded(dir.path(), vec![case]);
        let cancelled = AtomicBool::new(false);
        let candidate = candidate_identity();
        let selection = selection();
        let selected_indices = [0];

        let aggregate = build_aggregate(
            AggregateInputs {
                loaded: &loaded,
                candidate: &candidate,
                selection: &selection,
                selected_indices: &selected_indices,
                environment_owner: "test-runner",
                scratch: dir.path(),
                cancellation_requested: &cancelled,
            },
            &invoker,
        )
        .await;

        assert_eq!(
            aggregate.results[0].execution,
            ExecutionState::RunnerFailure
        );
        assert_eq!(
            aggregate.results[0].runner_error_code.as_deref(),
            Some("smoke_artifact_secret_rejected")
        );
        assert!(aggregate.results[0].smoke.is_none());
        assert!(!aggregate.success);
    }

    #[tokio::test]
    async fn oversized_smoke_evidence_is_omitted_and_stops_later_cases() {
        let dir = tempfile::tempdir().unwrap();
        let first = case("first", EvidenceStatus::Pass);
        let second = case("second", EvidenceStatus::Pass);
        let mut oversized = smoke(&first, true, Some(1));
        oversized["padding"] = Value::String("x".repeat(MAX_EMBEDDED_SMOKE_BYTES));
        let invoker = FakeInvoker::new(vec![invocation(oversized)]);
        let loaded = loaded(dir.path(), vec![first, second]);
        let cancelled = AtomicBool::new(false);
        let candidate = candidate_identity();
        let selection = selection();
        let selected_indices = [0, 1];

        let aggregate = build_aggregate(
            AggregateInputs {
                loaded: &loaded,
                candidate: &candidate,
                selection: &selection,
                selected_indices: &selected_indices,
                environment_owner: "test-runner",
                scratch: dir.path(),
                cancellation_requested: &cancelled,
            },
            &invoker,
        )
        .await;

        assert_eq!(&*invoker.calls.lock().unwrap(), &["first"]);
        assert_eq!(
            aggregate.results[0].runner_error_code.as_deref(),
            Some("aggregate_smoke_evidence_limit_exceeded")
        );
        assert!(aggregate.results[0].smoke.is_none());
        assert_eq!(
            aggregate.results[1].not_run_reason.as_deref(),
            Some("prior_runner_failure")
        );
        assert!(!aggregate.success);
    }

    #[tokio::test]
    async fn cumulative_smoke_evidence_limit_rejects_individually_bounded_cases() {
        let dir = tempfile::tempdir().unwrap();
        let first = case("first", EvidenceStatus::Pass);
        let second = case("second", EvidenceStatus::Pass);
        let mut first_smoke = smoke(&first, true, Some(1));
        let mut second_smoke = smoke(&second, true, Some(1));
        let padding = "x".repeat(MAX_EMBEDDED_SMOKE_BYTES / 2);
        first_smoke["padding"] = Value::String(padding.clone());
        second_smoke["padding"] = Value::String(padding);
        let invoker = FakeInvoker::new(vec![invocation(first_smoke), invocation(second_smoke)]);
        let loaded = loaded(dir.path(), vec![first, second]);
        let cancelled = AtomicBool::new(false);
        let candidate = candidate_identity();
        let selection = selection();
        let selected_indices = [0, 1];

        let aggregate = build_aggregate(
            AggregateInputs {
                loaded: &loaded,
                candidate: &candidate,
                selection: &selection,
                selected_indices: &selected_indices,
                environment_owner: "test-runner",
                scratch: dir.path(),
                cancellation_requested: &cancelled,
            },
            &invoker,
        )
        .await;

        assert_eq!(&*invoker.calls.lock().unwrap(), &["first", "second"]);
        assert_eq!(aggregate.results[0].execution, ExecutionState::Completed);
        assert_eq!(
            aggregate.results[1].runner_error_code.as_deref(),
            Some("aggregate_smoke_evidence_limit_exceeded")
        );
        assert!(aggregate.results[1].smoke.is_none());
        assert!(!aggregate.success);
    }

    #[test]
    fn pinned_drift_requires_exact_agent_rows_and_all_requested_capabilities() {
        let mut case = case("codex", EvidenceStatus::Pass);
        case.effort = Some("xhigh".into());
        case.mode = Some("read-only".into());
        case.pins = Some(PinSet {
            config_sha256: "a".repeat(64),
            model: case.model.clone(),
            adapter: Some("@agentclientprotocol/codex-acp=1.1.2".into()),
            agent_cli: Some("@openai/codex=0.144.1".into()),
            image_digest: None,
            components: BTreeMap::new(),
        });
        let mut artifact = smoke(&case, true, Some(1));
        artifact["request"]["config_sha256"] = Value::String("a".repeat(64));
        artifact["request"]["effort"] = Value::String("xhigh".into());
        artifact["request"]["mode"] = Value::String("read-only".into());
        artifact["session"]["effective_request"]["effort"] = Value::String("xhigh".into());
        artifact["session"]["effective_request"]["mode"] = Value::String("read-only".into());
        artifact["target"]["provenance"] = json!([
            {
                "check": "provenance:codex:adapter",
                "status": "ok",
                "detail": "executable=\"/usr/local/bin/codex-acp\" package=@agentclientprotocol/codex-acp version=1.1.2 manifest=\"/usr/local/lib/package.json\"",
                "remedy": ""
            },
            {
                "check": "provenance:codex:agent-cli",
                "status": "ok",
                "detail": "package=@openai/codex version=0.144.1 manifest=\"/usr/local/lib/codex/package.json\"",
                "remedy": ""
            }
        ]);
        assert!(drift_for(&case, &artifact).is_empty());

        artifact["target"]["provenance"][0]["detail"] =
            Value::String("package=@agentclientprotocol/codex-acp version=1.1.20".into());
        artifact["request"]["effort"] = Value::String("high".into());
        let drift = drift_for(&case, &artifact);
        assert!(drift.contains(&"provenance.adapter".into()));
        assert!(drift.contains(&"capability.effort".into()));

        artifact["target"]["provenance"][0]["detail"] =
            Value::String("package=@agentclientprotocol/codex-acp version=1.1.2".into());
        artifact["target"]["provenance"][0]["status"] = Value::String("warn".into());
        assert!(drift_for(&case, &artifact).contains(&"provenance.adapter".into()));
    }

    #[test]
    fn claude_pins_require_exact_ok_adapter_and_sdk_rows() {
        let mut case = case("claude", EvidenceStatus::Pass);
        case.pins = Some(PinSet {
            config_sha256: "a".repeat(64),
            model: case.model.clone(),
            adapter: Some("@agentclientprotocol/claude-agent-acp=0.55.0".into()),
            agent_cli: Some("@anthropic-ai/claude-agent-sdk=0.3.198".into()),
            image_digest: None,
            components: BTreeMap::new(),
        });
        let mut artifact = smoke(&case, true, Some(1));
        artifact["request"]["config_sha256"] = Value::String("a".repeat(64));
        artifact["target"]["provenance"] = json!([
            {
                "check": "provenance:claude:adapter",
                "status": "ok",
                "detail": "source=immutable-image-label package=@agentclientprotocol/claude-agent-acp version=0.55.0",
                "remedy": ""
            },
            {
                "check": "provenance:claude:agent-cli",
                "status": "ok",
                "detail": "source=immutable-image-label package=@anthropic-ai/claude-agent-sdk version=0.3.198",
                "remedy": ""
            }
        ]);
        assert!(drift_for(&case, &artifact).is_empty());

        artifact["target"]["provenance"][1]["detail"] =
            Value::String("package=@openai/codex version=0.144.1".into());
        assert!(drift_for(&case, &artifact).contains(&"provenance.agent_cli".into()));

        artifact["target"]["provenance"][1]["detail"] =
            Value::String("package=@anthropic-ai/claude-agent-sdk version=0.3.198".into());
        artifact["target"]["provenance"][1]["status"] = Value::String("warn".into());
        assert!(drift_for(&case, &artifact).contains(&"provenance.agent_cli".into()));
    }

    #[test]
    fn successful_api_case_binds_exact_credential_and_effective_capabilities() {
        let mut case = case("api", EvidenceStatus::Pass);
        case.execution_mode = ExecutionMode::RemoteApi;
        case.auth_path = AuthPath::ApiKeyEnv;
        case.credential_env = Some("EXPECTED_API_KEY".into());
        case.effort = Some("xhigh".into());
        case.mode = Some("read-only".into());
        let mut artifact = smoke(&case, true, Some(1));
        artifact["target"]["execution_mode"] = Value::String("remote_api".into());
        artifact["target"]["authentication"] = json!({
            "path": "api_key_env",
            "name": {"state": "value", "value": "EXPECTED_API_KEY"},
            "present": true
        });
        artifact["request"]["effort"] = Value::String("xhigh".into());
        artifact["request"]["mode"] = Value::String("read-only".into());
        artifact["session"]["effective_request"] = json!({
            "model": case.model,
            "effort": "xhigh",
            "mode": "read-only"
        });
        assert!(drift_for(&case, &artifact).is_empty());

        let mut wrong_credential = artifact.clone();
        wrong_credential["target"]["authentication"]["name"]["value"] =
            Value::String("OTHER_API_KEY".into());
        assert!(
            drift_for(&case, &wrong_credential).contains(&"authentication.credential_env".into())
        );

        let mut absent_credential = artifact.clone();
        absent_credential["target"]["authentication"]["present"] = Value::Bool(false);
        assert!(
            drift_for(&case, &absent_credential).contains(&"authentication.credential_env".into())
        );

        let mut wrong_effective = artifact;
        wrong_effective["session"]["effective_request"]["model"] =
            Value::String("different-model".into());
        wrong_effective["session"]["effective_request"]["effort"] = Value::String("high".into());
        let drift = drift_for(&case, &wrong_effective);
        assert!(drift.contains(&"capability.effective_model".into()));
        assert!(drift.contains(&"capability.effective_effort".into()));
    }

    #[test]
    fn container_drift_requires_the_exact_immutable_image_identity() {
        let mut case = case("reader", EvidenceStatus::Pass);
        case.execution_mode = ExecutionMode::ContainerRo;
        case.expected_image_digest = Some(format!("sha256:{}", "b".repeat(64)));
        let mut artifact = smoke(&case, true, Some(1));
        artifact["target"]["execution_mode"] = Value::String("container_ro".into());
        artifact["target"]["provenance"] = json!([{
            "check": "provenance:reader:image",
            "status": "ok",
            "detail": format!("runtime=podman immutable_id=sha256:{}", "b".repeat(64)),
            "remedy": ""
        }]);
        assert!(drift_for(&case, &artifact).is_empty());

        artifact["target"]["provenance"][0]["detail"] = Value::String(format!(
            "runtime=podman immutable_id=sha256:{}",
            "c".repeat(64)
        ));
        assert!(drift_for(&case, &artifact).contains(&"provenance.image".into()));
    }

    #[test]
    fn pinned_compare_reports_each_drift_dimension_independently() {
        let case = case("pinned", EvidenceStatus::Pass);
        let smoke = smoke(&case, true, Some(1));
        let result = CaseResult {
            case_id: case.id.clone(),
            baseline_case_id: None,
            lane: Lane::Pinned,
            evidence_path: EvidencePath::BridgeSmoke,
            probe: ProbeType::Minimal,
            billable: true,
            execution: ExecutionState::Completed,
            expected_status: EvidenceStatus::Pass,
            actual_status: EvidenceStatus::Pass,
            expectation_met: true,
            classification: Classification::Support,
            candidate_outcome: None,
            artifact_policy: case.artifact,
            duration_ms: 1,
            not_run_reason: None,
            runner_error_code: None,
            drift: Vec::new(),
            budget_violations: Vec::new(),
            smoke: Some(smoke),
        };
        let mut before = baseline_from_result(&result);
        before.status = EvidenceStatus::Fail;
        before.execution_mode = json!({"changed": true});
        before.provenance = json!({"changed": true});
        before.capability = json!({"changed": true});
        before.authentication = json!({"changed": true});
        before.phase = json!({"changed": true});
        before.terminal = json!({"changed": true});
        before.diagnostic = json!({"changed": true});
        let current = AggregateArtifact {
            schema_version: 1,
            candidate: candidate_identity(),
            manifest: ManifestIdentity {
                schema_version: 1,
                canonical_path: "/tmp/manifest.toml".into(),
                sha256: "a".repeat(64),
            },
            selection: selection(),
            environment_owner: "test-runner".into(),
            started_at_ms: 1,
            ended_at_ms: 2,
            cancelled: false,
            success: true,
            budget: BudgetOutcome {
                timeout_secs: 1,
                max_tokens: 10,
                max_cost_usd: None,
                observed_tokens: 1,
                observed_cost_usd: 0.0,
                token_observation_missing_cases: 0,
                cost_observation_missing_cases: 1,
                exhausted: false,
            },
            floating_summary: None,
            results: vec![result],
        };
        let baseline = BaselineArtifact {
            schema_version: 1,
            manifest_schema_version: 1,
            manifest_sha256: "a".repeat(64),
            aggregate: baseline_from_aggregate(&current),
            cases: vec![before],
        };

        let report = compare_artifacts(&current, &baseline).unwrap();
        assert!(!report.equal);
        assert_eq!(report.changes.len(), 1);
        assert_eq!(report.changes[0].case_id, "pinned");
        assert_eq!(
            report.changes[0].dimensions,
            [
                "status",
                "execution_mode",
                "provenance",
                "capability",
                "authentication",
                "phase",
                "terminal",
                "diagnostic",
            ]
        );

        let mut invalid_baseline = baseline;
        invalid_baseline.manifest_sha256 = "not-a-digest".into();
        assert!(compare_artifacts(&current, &invalid_baseline)
            .unwrap_err()
            .to_string()
            .contains("invalid manifest identity"));
    }

    #[test]
    fn comparison_keeps_prompt_count_lifecycle_drops_and_retry_metadata() {
        let mut case = case("pinned", EvidenceStatus::Fail);
        case.lane = Lane::Pinned;
        let mut smoke = smoke(&case, false, Some(1));
        smoke["turn"]["prompt_calls"] = json!(0);
        smoke["diagnostics"]["dropped_events"] = json!(0);
        smoke["diagnostics"]["lifecycle"] = json!([{
            "transition": {"phase": "resolve", "status": "failed", "at_ms": 10},
            "failure": smoke["diagnostics"]["failure"].clone()
        }]);
        let result = CaseResult {
            case_id: case.id.clone(),
            baseline_case_id: None,
            lane: Lane::Pinned,
            evidence_path: EvidencePath::BridgeSmoke,
            probe: ProbeType::Minimal,
            billable: true,
            execution: ExecutionState::Completed,
            expected_status: EvidenceStatus::Fail,
            actual_status: EvidenceStatus::Fail,
            expectation_met: true,
            classification: Classification::Support,
            candidate_outcome: None,
            artifact_policy: case.artifact,
            duration_ms: 1,
            not_run_reason: None,
            runner_error_code: None,
            drift: Vec::new(),
            budget_violations: Vec::new(),
            smoke: Some(smoke),
        };
        let baseline_case = baseline_from_result(&result);
        let mut changed = result;
        let changed_smoke = changed.smoke.as_mut().unwrap();
        changed_smoke["turn"]["prompt_calls"] = json!(2);
        changed_smoke["diagnostics"]["dropped_events"] = json!(1);
        changed_smoke["diagnostics"]["lifecycle"][0]["failure"]["retry_after_ms"] = json!(1234);
        changed_smoke["diagnostics"]["lifecycle"][0]["transition"]["at_ms"] = json!(999);
        let current = AggregateArtifact {
            schema_version: 1,
            candidate: candidate_identity(),
            manifest: ManifestIdentity {
                schema_version: 1,
                canonical_path: "/tmp/manifest.toml".into(),
                sha256: "a".repeat(64),
            },
            selection: selection(),
            environment_owner: "test-runner".into(),
            started_at_ms: 1,
            ended_at_ms: 2,
            cancelled: false,
            success: false,
            budget: BudgetOutcome {
                timeout_secs: 1,
                max_tokens: 10,
                max_cost_usd: None,
                observed_tokens: 1,
                observed_cost_usd: 0.0,
                token_observation_missing_cases: 0,
                cost_observation_missing_cases: 1,
                exhausted: false,
            },
            floating_summary: None,
            results: vec![changed],
        };
        let baseline = BaselineArtifact {
            schema_version: 1,
            manifest_schema_version: 1,
            manifest_sha256: "a".repeat(64),
            aggregate: baseline_from_aggregate(&current),
            cases: vec![baseline_case],
        };

        let report = compare_artifacts(&current, &baseline).unwrap();
        assert!(!report.equal);
        assert_eq!(report.changes[0].dimensions, ["terminal", "diagnostic"]);
    }

    #[test]
    fn comparison_keeps_blocking_case_and_aggregate_outcomes() {
        let mut case = case("pinned", EvidenceStatus::Pass);
        case.lane = Lane::Pinned;
        case.classification = Classification::Support;
        let accepted = CaseResult {
            case_id: case.id.clone(),
            baseline_case_id: None,
            lane: Lane::Pinned,
            evidence_path: EvidencePath::BridgeSmoke,
            probe: ProbeType::Minimal,
            billable: true,
            execution: ExecutionState::Completed,
            expected_status: EvidenceStatus::Pass,
            actual_status: EvidenceStatus::Pass,
            expectation_met: true,
            classification: Classification::Support,
            candidate_outcome: None,
            artifact_policy: case.artifact.clone(),
            duration_ms: 1,
            not_run_reason: None,
            runner_error_code: None,
            drift: Vec::new(),
            budget_violations: Vec::new(),
            smoke: Some(smoke(&case, true, Some(1))),
        };
        let accepted_aggregate = aggregate_artifact(vec![accepted.clone()]);
        let baseline = BaselineArtifact {
            schema_version: 1,
            manifest_schema_version: 1,
            manifest_sha256: "a".repeat(64),
            aggregate: baseline_from_aggregate(&accepted_aggregate),
            cases: vec![baseline_from_result(&accepted)],
        };

        let mut over_budget = accepted.clone();
        over_budget.expectation_met = false;
        over_budget
            .budget_violations
            .push("case_token_cap_exceeded".into());
        let mut current = aggregate_artifact(vec![over_budget]);
        current.success = false;
        current.budget.exhausted = true;
        let report = compare_artifacts(&current, &baseline).unwrap();
        assert!(
            !report.equal,
            "blocking budget drift must not compare equal"
        );

        let before_failure = runner_failure_result(&case, Duration::ZERO, "candidate_changed");
        let mut before_failure_aggregate = aggregate_artifact(vec![before_failure.clone()]);
        before_failure_aggregate.success = false;
        let failure_baseline = BaselineArtifact {
            schema_version: 1,
            manifest_schema_version: 1,
            manifest_sha256: "a".repeat(64),
            aggregate: baseline_from_aggregate(&before_failure_aggregate),
            cases: vec![baseline_from_result(&before_failure)],
        };
        let after_failure = runner_failure_result(&case, Duration::ZERO, "artifact_missing");
        let mut current = aggregate_artifact(vec![after_failure]);
        current.success = false;
        let report = compare_artifacts(&current, &failure_baseline).unwrap();
        assert!(
            !report.equal,
            "different runner failure codes must not compare equal"
        );
    }

    #[test]
    fn comparison_ignores_lifecycle_timestamps_but_keeps_nested_failed_phase() {
        let mut case = case("pinned", EvidenceStatus::Fail);
        case.lane = Lane::Pinned;
        let mut smoke = smoke(&case, false, Some(1));
        smoke["phase"] = json!("resolve");
        smoke["diagnostics"]["lifecycle"] = json!([{
            "transition": {"phase": "resolve", "status": "failed", "at_ms": 10},
            "failure": {"failed_phase": "resolve", "class": "agent_crashed"}
        }]);
        let result = CaseResult {
            case_id: case.id.clone(),
            baseline_case_id: None,
            lane: Lane::Pinned,
            evidence_path: EvidencePath::BridgeSmoke,
            probe: ProbeType::Minimal,
            billable: true,
            execution: ExecutionState::Completed,
            expected_status: EvidenceStatus::Fail,
            actual_status: EvidenceStatus::Fail,
            expectation_met: true,
            classification: Classification::Support,
            candidate_outcome: None,
            artifact_policy: case.artifact,
            duration_ms: 1,
            not_run_reason: None,
            runner_error_code: None,
            drift: Vec::new(),
            budget_violations: Vec::new(),
            smoke: Some(smoke),
        };
        let baseline_case = baseline_from_result(&result);
        let mut current = AggregateArtifact {
            schema_version: 1,
            candidate: candidate_identity(),
            manifest: ManifestIdentity {
                schema_version: 1,
                canonical_path: "/tmp/manifest.toml".into(),
                sha256: "a".repeat(64),
            },
            selection: selection(),
            environment_owner: "test-runner".into(),
            started_at_ms: 1,
            ended_at_ms: 2,
            cancelled: false,
            success: false,
            budget: BudgetOutcome {
                timeout_secs: 1,
                max_tokens: 10,
                max_cost_usd: None,
                observed_tokens: 1,
                observed_cost_usd: 0.0,
                token_observation_missing_cases: 0,
                cost_observation_missing_cases: 1,
                exhausted: false,
            },
            floating_summary: None,
            results: vec![result],
        };
        let baseline = BaselineArtifact {
            schema_version: 1,
            manifest_schema_version: 1,
            manifest_sha256: "a".repeat(64),
            aggregate: baseline_from_aggregate(&current),
            cases: vec![baseline_case],
        };

        current.results[0].smoke.as_mut().unwrap()["diagnostics"]["lifecycle"][0]["transition"]
            ["at_ms"] = json!(999);
        assert!(compare_artifacts(&current, &baseline).unwrap().equal);

        current.results[0].smoke.as_mut().unwrap()["diagnostics"]["lifecycle"][0]["failure"]
            ["failed_phase"] = json!("prompt_stream");
        let report = compare_artifacts(&current, &baseline).unwrap();
        assert!(!report.equal);
        assert_eq!(report.changes[0].dimensions, ["diagnostic"]);
    }

    #[test]
    fn pinned_case_rejects_automatic_model_selection() {
        let dir = tempfile::tempdir().unwrap();
        let mut pinned = case("pinned-auto", EvidenceStatus::Pass);
        pinned.lane = Lane::Pinned;
        pinned.classification = Classification::Support;
        pinned.baseline_case = None;
        pinned.resolved = None;
        pinned.model = "auto".into();
        pinned.pins = Some(PinSet {
            model: "auto".into(),
            adapter: Some("codex-acp=1.1.2".into()),
            agent_cli: Some("codex=0.1.0".into()),
            config_sha256: "a".repeat(64),
            image_digest: None,
            components: BTreeMap::new(),
        });
        let manifest = loaded(dir.path(), vec![pinned]).manifest;

        let error = validate_manifest(&manifest).unwrap_err();
        assert!(error.to_string().contains("floating"));
    }

    #[test]
    fn checked_in_pinned_manifest_covers_each_claimed_path_with_exact_configs() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let loaded = load_manifest(&root.join(DEFAULT_MANIFEST)).unwrap();
        let expected = BTreeSet::from([
            "claude-direct-host-cli-fable",
            "claude-host-acp-044-fable",
            "claude-host-acp-055-fable",
            "claude-managed-no-egress-055-fable",
            "claude-reader-055-fable",
            "codex-host-bridge-gpt56-sol",
            "codex-reader-bridge-gpt56-sol",
            "kiro-host-stale",
            "kiro-reader-stale",
        ]);
        let actual = loaded
            .manifest
            .cases
            .iter()
            .map(|case| case.id.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(actual, expected);

        for case in &loaded.manifest.cases {
            assert_eq!(case.lane, Lane::Pinned, "{}", case.id);
            let pins = case.pins.as_ref().expect("all R3b rows are pinned");
            let config = resolve_case_path(&loaded.canonical_path, &case.config);
            let snapshot = local_file::read_regular_file_bounded(
                &config,
                "checked-in compatibility config",
                MAX_MANIFEST_BYTES,
            )
            .unwrap_or_else(|error| panic!("{} config {config:?}: {error}", case.id));
            assert_eq!(snapshot.sha256, pins.config_sha256, "{}", case.id);
        }
    }

    #[test]
    fn checked_in_baseline_matches_the_checked_in_manifest_identity() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let loaded = load_manifest(&root.join(DEFAULT_MANIFEST)).unwrap();
        let baseline: BaselineArtifact = load_json(
            &root.join(DEFAULT_BASELINE),
            "compatibility pinned baseline",
        )
        .unwrap();
        assert_eq!(
            baseline.manifest_schema_version,
            loaded.manifest.schema_version
        );
        assert_eq!(baseline.manifest_sha256, loaded.sha256);
        assert!(
            baseline.cases.is_empty(),
            "the checked-in baseline must remain unpromoted until authorized live evidence replaces this assertion"
        );
    }

    #[tokio::test]
    async fn staged_candidate_is_owner_executable_nonwritable_and_digest_drift_refuses_before_spawn(
    ) {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"#!/bin/sh\nexit 0\n".to_vec();
        let snapshot = local_file::LocalFileSnapshot {
            canonical_path: dir.path().join("source"),
            sha256: local_file::sha256_hex(&bytes),
            bytes,
        };
        let scratch = scratch_in(dir.path());
        let staged = stage_candidate(&snapshot, &scratch).unwrap();
        assert_eq!(std::fs::read(&staged.staged_path).unwrap(), snapshot.bytes);
        #[cfg(unix)]
        assert_eq!(
            std::fs::metadata(&staged.staged_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o500
        );

        #[cfg(unix)]
        if !running_as_root() {
            assert!(
                std::fs::OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(&staged.staged_path)
                    .is_err(),
                "the staged inode must never be exposed with owner write permission"
            );
        }

        #[cfg(unix)]
        {
            let mut permissions = std::fs::metadata(&staged.staged_path)
                .unwrap()
                .permissions();
            permissions.set_mode(0o700);
            std::fs::set_permissions(&staged.staged_path, permissions).unwrap();
        }
        std::fs::write(&staged.staged_path, b"changed").unwrap();
        let invoker = ProcessSmokeInvoker {
            executable: staged,
            artifact_directory: &scratch.pin,
            expected_sha256: snapshot.sha256,
        };
        let request = SmokeRequest {
            agent: "never-spawn".into(),
            config: dir.path().join("missing.toml"),
            model: "test-model".into(),
            effort: None,
            mode: None,
            session_cwd: None,
            timeout_secs: 1,
            artifact_path: dir.path().join("must-not-exist.json"),
        };
        let cancellation = AtomicBool::new(false);
        let admission = test_spawn_admission(&cancellation);
        let result = invoker.invoke(&request, &admission).await;
        assert_eq!(result.runner_error_code, Some("candidate_binary_changed"));
        assert!(result.artifact.is_none());
        assert!(!request.artifact_path.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn staged_candidate_exec_is_bound_to_the_verified_file_object() {
        let dir = tempfile::tempdir().unwrap();
        let snapshot = local_file::read_regular_file_bounded(
            Path::new("/usr/bin/true"),
            "test true executable",
            MAX_EXECUTABLE_BYTES,
        )
        .unwrap();
        let replacement = local_file::read_regular_file_bounded(
            Path::new("/usr/bin/false"),
            "test false executable",
            MAX_EXECUTABLE_BYTES,
        )
        .unwrap();
        let scratch = scratch_in(dir.path());
        let staged = stage_candidate(&snapshot, &scratch).unwrap();
        let staged_path = staged.staged_path.clone();
        let moved = dir.path().join("verified-object");
        let invoker = ProcessSmokeInvoker {
            executable: staged,
            artifact_directory: &scratch.pin,
            expected_sha256: snapshot.sha256,
        };
        let request = SmokeRequest {
            agent: "test-agent".into(),
            config: dir.path().join("missing.toml"),
            model: "test-model".into(),
            effort: None,
            mode: None,
            session_cwd: None,
            timeout_secs: 1,
            artifact_path: dir.path().join("artifact.json"),
        };

        let cancellation = AtomicBool::new(false);
        let admission = test_spawn_admission(&cancellation);
        let result = invoker
            .invoke_after_candidate_check(&request, &admission, || {
                std::fs::rename(&staged_path, &moved).unwrap();
                std::fs::write(&staged_path, &replacement.bytes).unwrap();
                let mut permissions = std::fs::metadata(&staged_path).unwrap().permissions();
                permissions.set_mode(0o700);
                std::fs::set_permissions(&staged_path, permissions).unwrap();
            })
            .await;

        assert!(result.process_success);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn staged_candidate_cannot_be_overwritten_in_place_after_digest_check() {
        let dir = tempfile::tempdir().unwrap();
        let snapshot = local_file::read_regular_file_bounded(
            Path::new("/usr/bin/true"),
            "test true executable",
            MAX_EXECUTABLE_BYTES,
        )
        .unwrap();
        let scratch = scratch_in(dir.path());
        let staged = stage_candidate(&snapshot, &scratch).unwrap();
        let staged_path = staged.staged_path.clone();
        let invoker = ProcessSmokeInvoker {
            executable: staged,
            artifact_directory: &scratch.pin,
            expected_sha256: snapshot.sha256,
        };
        let request = SmokeRequest {
            agent: "test-agent".into(),
            config: dir.path().join("missing.toml"),
            model: "test-model".into(),
            effort: None,
            mode: None,
            session_cwd: None,
            timeout_secs: 1,
            artifact_path: dir.path().join("artifact.json"),
        };

        let cancellation = AtomicBool::new(false);
        let admission = test_spawn_admission(&cancellation);
        let result = invoker
            .invoke_after_candidate_check(&request, &admission, || {
                if !running_as_root() {
                    let overwrite = std::fs::OpenOptions::new()
                        .write(true)
                        .truncate(true)
                        .open(&staged_path);
                    assert!(
                        overwrite.is_err(),
                        "an ordinary same-owner writer must not reopen the staged inode"
                    );
                }
                assert_eq!(std::fs::read(&staged_path).unwrap(), snapshot.bytes);
            })
            .await;

        assert!(result.process_success);
    }

    #[test]
    fn output_guard_rejects_bare_git_repository_ancestors() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("HEAD"), b"ref: refs/heads/main\n").unwrap();
        std::fs::create_dir(dir.path().join("objects")).unwrap();
        std::fs::create_dir_all(dir.path().join("refs/heads")).unwrap();
        let nested = dir.path().join("evidence");
        std::fs::create_dir(&nested).unwrap();

        assert!(ensure_output_outside_repositories(&nested.join("aggregate.json")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn canonical_output_parent_refuses_after_symlink_or_object_retarget() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let safe = dir.path().join("safe");
        let repository = dir.path().join("repository");
        std::fs::create_dir(&safe).unwrap();
        std::fs::create_dir(&repository).unwrap();
        std::fs::create_dir(repository.join(".git")).unwrap();
        let alias = dir.path().join("output-parent");
        symlink(&safe, &alias).unwrap();
        let requested = alias.join("aggregate.json");

        let resolved = ensure_output_outside_repositories(&requested).unwrap();
        let moved = dir.path().join("moved-safe");
        std::fs::rename(&safe, &moved).unwrap();
        symlink(&repository, &safe).unwrap();
        std::fs::remove_file(&alias).unwrap();
        symlink(&repository, &alias).unwrap();
        let error = resolved.prepare_output().unwrap_err();

        assert!(error.to_string().contains("parent identity changed"));
        assert!(!moved.join("aggregate.json").exists());
        assert!(!repository.join("aggregate.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn output_creation_is_bound_to_the_guarded_parent_object() {
        let dir = tempfile::tempdir().unwrap();
        let safe = dir.path().join("safe");
        let moved = dir.path().join("moved-safe");
        std::fs::create_dir(&safe).unwrap();
        let requested = safe.join("aggregate.json");
        let resolved = ensure_output_outside_repositories(&requested).unwrap();

        let error = resolved
            .prepare_output_after_guard(|| {
                std::fs::rename(&safe, &moved).unwrap();
                std::fs::create_dir(&safe).unwrap();
                std::fs::create_dir(safe.join(".git")).unwrap();
            })
            .unwrap_err();

        assert!(error.to_string().contains("parent identity changed"));
        assert!(!safe.join("aggregate.json").exists());
        assert!(!moved.join("aggregate.json").exists());
    }

    #[test]
    fn setup_failure_after_output_creation_preserves_a_valid_blocking_aggregate() {
        let dir = tempfile::tempdir().unwrap();
        let output_path = dir.path().join("aggregate.json");
        let output_directory = ensure_output_outside_repositories(&output_path).unwrap();
        let loaded = loaded(
            dir.path(),
            vec![
                case("first", EvidenceStatus::Pass),
                case("second", EvidenceStatus::Pass),
            ],
        );
        let setup = setup_incomplete_aggregate(
            &loaded,
            &candidate_identity(),
            &selection(),
            &[0, 1],
            "test-runner",
        );
        let output = output_directory
            .prepare_output_with_setup_evidence(&setup)
            .unwrap();
        drop(output);

        let aggregate: AggregateArtifact = load_json(&output_path, "test setup aggregate").unwrap();
        assert!(!aggregate.success);
        assert_eq!(
            aggregate.results[0].runner_error_code.as_deref(),
            Some("compatibility_setup_incomplete")
        );
        assert_eq!(aggregate.results[1].execution, ExecutionState::NotRun);
        assert_eq!(
            aggregate.results[1].not_run_reason.as_deref(),
            Some("prior_runner_failure")
        );
        #[cfg(unix)]
        assert_eq!(
            std::fs::metadata(output_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn final_aggregate_atomically_replaces_setup_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let output_path = dir.path().join("aggregate.json");
        let output_directory = ensure_output_outside_repositories(&output_path).unwrap();
        let loaded = loaded(dir.path(), vec![case("first", EvidenceStatus::Pass)]);
        let setup = setup_incomplete_aggregate(
            &loaded,
            &candidate_identity(),
            &selection(),
            &[0],
            "test-runner",
        );
        let output = output_directory
            .prepare_output_with_setup_evidence(&setup)
            .unwrap();
        let mut provisional_reader = File::open(&output_path).unwrap();
        let mut final_aggregate = setup.clone();
        final_aggregate.success = true;
        final_aggregate.ended_at_ms += 1;

        output_directory
            .replace_setup_with_final(&output, &setup, &final_aggregate)
            .unwrap();

        let published: AggregateArtifact = load_json(&output_path, "published aggregate").unwrap();
        assert!(published.success);
        provisional_reader.seek(SeekFrom::Start(0)).unwrap();
        let preserved: AggregateArtifact = serde_json::from_reader(provisional_reader).unwrap();
        assert!(
            !preserved.success,
            "a reader of the provisional inode must retain valid blocking setup evidence"
        );
        assert_eq!(
            preserved.results[0].runner_error_code.as_deref(),
            Some("compatibility_setup_incomplete")
        );
        assert!(std::fs::read_dir(dir.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".a2a-compat-")
        }));
    }

    #[test]
    fn final_aggregate_replacement_refuses_target_rebinding() {
        let dir = tempfile::tempdir().unwrap();
        let output_path = dir.path().join("aggregate.json");
        let moved_setup = dir.path().join("moved-setup.json");
        let output_directory = ensure_output_outside_repositories(&output_path).unwrap();
        let loaded = loaded(dir.path(), vec![case("first", EvidenceStatus::Pass)]);
        let setup = setup_incomplete_aggregate(
            &loaded,
            &candidate_identity(),
            &selection(),
            &[0],
            "test-runner",
        );
        let output = output_directory
            .prepare_output_with_setup_evidence(&setup)
            .unwrap();
        std::fs::rename(&output_path, &moved_setup).unwrap();
        std::fs::write(&output_path, b"replacement must remain untouched").unwrap();
        let mut final_aggregate = setup.clone();
        final_aggregate.success = true;

        let error = output_directory
            .replace_setup_with_final(&output, &setup, &final_aggregate)
            .unwrap_err();

        assert!(error.to_string().contains("target identity changed"));
        assert_eq!(
            std::fs::read(&output_path).unwrap(),
            b"replacement must remain untouched"
        );
        let preserved: AggregateArtifact =
            load_json(&moved_setup, "moved setup aggregate").unwrap();
        assert!(!preserved.success);
        assert_eq!(
            preserved.results[0].runner_error_code.as_deref(),
            Some("compatibility_setup_incomplete")
        );
        let staging: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".a2a-compat-")
            })
            .collect();
        assert_eq!(staging.len(), 1);
        assert!(staging[0]
            .file_name()
            .to_string_lossy()
            .starts_with(".a2a-compat-setup-"));
        let recovery: AggregateArtifact =
            load_json(&staging[0].path(), "setup recovery aggregate").unwrap();
        assert!(!recovery.success);
        assert_eq!(
            recovery.results[0].runner_error_code.as_deref(),
            Some("compatibility_setup_incomplete")
        );
    }

    #[tokio::test]
    async fn pinned_support_not_run_unknown_cannot_green_release_aggregate() {
        let dir = tempfile::tempdir().unwrap();
        let candidate = candidate_identity();
        let selection = selection();
        let selected_indices = [0];
        let cancelled = AtomicBool::new(false);
        let invoker = FakeInvoker::new(Vec::new());

        let mut non_goal = case("unknown-non-goal", EvidenceStatus::Unknown);
        non_goal.lane = Lane::Pinned;
        non_goal.classification = Classification::NonGoal;
        non_goal.os = "other-os".into();
        let advisory = build_aggregate(
            AggregateInputs {
                loaded: &loaded(dir.path(), vec![non_goal]),
                candidate: &candidate,
                selection: &selection,
                selected_indices: &selected_indices,
                environment_owner: "test-runner",
                scratch: dir.path(),
                cancellation_requested: &cancelled,
            },
            &invoker,
        )
        .await;
        assert!(advisory.success, "a pinned non-goal may remain advisory");

        for expected in [EvidenceStatus::Unknown, EvidenceStatus::Stale] {
            let mut support = case("unrun-support", expected);
            support.lane = Lane::Pinned;
            support.classification = Classification::Support;
            support.os = "other-os".into();
            let blocking = build_aggregate(
                AggregateInputs {
                    loaded: &loaded(dir.path(), vec![support]),
                    candidate: &candidate,
                    selection: &selection,
                    selected_indices: &selected_indices,
                    environment_owner: "test-runner",
                    scratch: dir.path(),
                    cancellation_requested: &cancelled,
                },
                &invoker,
            )
            .await;
            assert_eq!(blocking.results[0].execution, ExecutionState::NotRun);
            assert!(
                !blocking.success,
                "a release-blocking pinned support case must execute before the aggregate can green"
            );
        }
    }
}
