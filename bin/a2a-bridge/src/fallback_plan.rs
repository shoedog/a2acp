//! R2d local, non-billable fallback recommendation.
//!
//! This module reads one explicit diagnostic artifact and one explicit config, validates both, and
//! emits a plan. It has no registry resolution, backend, subprocess, network, workflow, or prompt path.

use std::path::{Path, PathBuf};

use bridge_core::diagnostics::{
    DiagnosticEvent, DiagnosticFailureClass, DiagnosticOperation, DiagnosticPhase,
    FailureDiagnostic, FailureDisposition, PhaseStatus,
};
use bridge_core::domain::{AgentKind, RegistrySnapshot};
use bridge_core::ids::AgentId;
use bridge_core::SessionCwd;
use serde::{Deserialize, Serialize};

use crate::BoxError;

pub(crate) const USAGE: &str = "\
usage: a2a-bridge fallback-plan --from <failed-smoke-v2-artifact.json>
                                --host-agent <explicit-agent-id>
                                [--confirm-trusted-own-repo-read-only]
                                --config <path>

Validate one complete local failed R2c smoke-v2 artifact and emit a versioned recommendation for a
distinct host verification smoke. Hand-assembled task envelopes are not accepted because they are not
bound to persisted lifecycle/config evidence.
This command never resolves, spawns, prompts, retries, resumes, or performs network access. An eligible
plan still requires a separately invoked, explicitly billable smoke command.";

const MAX_SOURCE_BYTES: u64 = 1024 * 1024;
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_ID_BYTES: usize = 128;
const MAX_PATH_BYTES: usize = 16 * 1024;
const SMOKE_SCHEMA_V2: u16 = 2;
const MAX_EXECUTABLE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug)]
struct FallbackArgs {
    source: PathBuf,
    host_agent: AgentId,
    host_agent_raw: String,
    confirm_trusted_own_repo_read_only: bool,
    config: PathBuf,
}

fn take_value(args: &[String], index: &mut usize, flag: &str) -> Result<String, BoxError> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| format!("fallback-plan: {flag} requires a value").into())
}

fn set_once<T>(slot: &mut Option<T>, value: T, flag: &str) -> Result<(), BoxError> {
    if slot.replace(value).is_some() {
        return Err(format!("fallback-plan: duplicate {flag}").into());
    }
    Ok(())
}

fn validate_id(label: &str, value: String) -> Result<String, BoxError> {
    if value.is_empty()
        || value.len() > MAX_ID_BYTES
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(format!(
            "fallback-plan: {label} must be a non-empty, unpadded id without control characters"
        )
        .into());
    }
    Ok(value)
}

fn validated_path_text(path: &Path, label: &str) -> Result<String, BoxError> {
    let value = path
        .to_str()
        .ok_or_else(|| format!("fallback-plan: {label} must be valid UTF-8"))?;
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(format!("fallback-plan: {label} contains control characters").into());
    }
    Ok(value.to_owned())
}

fn parse_args(args: &[String]) -> Result<Option<FallbackArgs>, BoxError> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{USAGE}");
        return Ok(None);
    }
    let mut source = None;
    let mut host_agent = None;
    let mut config = None;
    let mut confirm = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--from" => {
                let value = take_value(args, &mut index, "--from")?;
                if value == "-" {
                    return Err("fallback-plan: --from must be an explicit local file".into());
                }
                set_once(&mut source, PathBuf::from(value), "--from")?;
            }
            "--host-agent" => {
                let value = validate_id(
                    "--host-agent",
                    take_value(args, &mut index, "--host-agent")?,
                )?;
                set_once(&mut host_agent, value, "--host-agent")?;
            }
            "--config" => {
                let value = take_value(args, &mut index, "--config")?;
                if value == "-" {
                    return Err("fallback-plan: --config must be an explicit local file".into());
                }
                set_once(&mut config, PathBuf::from(value), "--config")?;
            }
            "--confirm-trusted-own-repo-read-only" => {
                if confirm {
                    return Err(
                        "fallback-plan: duplicate --confirm-trusted-own-repo-read-only".into(),
                    );
                }
                confirm = true;
            }
            flag => return Err(format!("fallback-plan: unknown option {flag:?}\n{USAGE}").into()),
        }
        index += 1;
    }
    let source = source.ok_or_else(|| format!("fallback-plan: missing --from\n{USAGE}"))?;
    let host_agent_raw =
        host_agent.ok_or_else(|| format!("fallback-plan: missing --host-agent\n{USAGE}"))?;
    let host_agent = AgentId::parse(host_agent_raw.clone())
        .map_err(|_| "fallback-plan: invalid --host-agent")?;
    let config = config.ok_or_else(|| format!("fallback-plan: missing --config\n{USAGE}"))?;
    Ok(Some(FallbackArgs {
        source,
        host_agent,
        host_agent_raw,
        confirm_trusted_own_repo_read_only: confirm,
        config,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SmokeSourceV2 {
    schema_version: u16,
    success: bool,
    bridge: SmokeBridge,
    attempt: SmokeAttempt,
    request: SmokeRequest,
    target: Option<SmokeTarget>,
    session: SmokeSession,
    turn: SmokeTurn,
    diagnostics: SmokeDiagnostics,
    cleanup: SmokeCleanup,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SmokeBridge {
    package_version: String,
    #[serde(default)]
    git_commit: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SmokeAttempt {
    id: String,
    timeout_secs: u64,
    started_at_ms: i64,
    ended_at_ms: i64,
    timed_out: bool,
    prompt_may_have_been_accepted: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SmokeRequest {
    agent: String,
    requested_config_path: String,
    #[serde(default)]
    canonical_config_path: Option<String>,
    #[serde(default)]
    config_sha256: Option<String>,
    #[serde(default, rename = "model")]
    _model: Option<String>,
    #[serde(default, rename = "effort")]
    _effort: Option<String>,
    #[serde(default, rename = "mode")]
    _mode: Option<String>,
    #[serde(default)]
    session_cwd: Option<String>,
    #[serde(default, rename = "fallback_guard")]
    _fallback_guard: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SmokeTarget {
    execution_mode: String,
    provenance: Vec<SmokeProvenanceRow>,
    authentication: SmokeAuthentication,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum SmokeCheckStatus {
    Ok,
    Warn,
    Fail,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SmokeProvenanceRow {
    check: String,
    #[serde(rename = "status")]
    _status: SmokeCheckStatus,
    detail: String,
    remedy: String,
}

#[derive(Deserialize)]
#[serde(tag = "path", rename_all = "snake_case", deny_unknown_fields)]
enum SmokeAuthentication {
    ApiKeyEnv { name: String, present: bool },
    PreAuthenticated,
    ConfiguredMethod { method: String },
    Automatic,
    NotApplicable,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SmokeSession {
    id: String,
    configure_calls: u8,
    #[serde(default, rename = "effective_request")]
    _effective_request: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SmokeTurn {
    prompt: String,
    prompt_calls: u8,
    terminal_state: String,
    #[serde(default, rename = "stop_reason")]
    _stop_reason: Option<String>,
    exact_pong: bool,
    text_bytes: u64,
    tool_event_count: u64,
    permission_update_count: u64,
    #[serde(default, rename = "usage")]
    _usage: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SmokeDiagnostics {
    lifecycle: Vec<DiagnosticEvent>,
    dropped_events: u64,
    #[serde(default)]
    failure: Option<FailureDiagnostic>,
    #[serde(rename = "stderr_text")]
    _stderr_text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SmokeCleanup {
    grace_timeout_secs: u64,
    cancel: String,
    release: String,
    retire: String,
    run_scoped_backstop: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum IneligibilityReason {
    TrustConfirmationMissing,
    SourceNotFailed,
    SourceDiagnosticMissing,
    SourceDiagnosticsIncomplete,
    SourceConfigProvenanceMissing,
    SourceConfigProvenanceMismatch,
    SourceNotContainerExecution,
    SourceNotReadOnly,
    SourceAgentUnknown,
    SourceAgentConfigurationMismatch,
    SourceFailureNotContainer,
    SourceFailurePhaseInvalid,
    SourceDispositionNotFallbackCandidate,
    SourcePromptMayHaveBeenAccepted,
    SourceTimedOut,
    TargetAgentUnknown,
    TargetAgentNotEligible,
}

struct NormalizedSource {
    schema: &'static str,
    attempt_id: String,
    original_agent: String,
    original_agent_id: AgentId,
    execution_mode: String,
    reported_session_cwd: Option<String>,
    config_canonical_path: Option<String>,
    config_sha256: Option<String>,
    prompt_may_have_been_accepted: bool,
    failure: Option<FailureDiagnostic>,
    reasons: Vec<IneligibilityReason>,
}

fn bounded_nonempty(label: &str, value: &str, max_bytes: usize) -> Result<(), BoxError> {
    if value.is_empty() || value.len() > max_bytes || value.chars().any(char::is_control) {
        return Err(format!("fallback-plan: invalid {label}").into());
    }
    Ok(())
}

fn normalized_evidence_path(label: &str, raw: Option<String>) -> Result<Option<String>, BoxError> {
    raw.map(|value| {
        bounded_nonempty(label, &value, MAX_PATH_BYTES)?;
        if !Path::new(&value).is_absolute() {
            return Err(format!("fallback-plan: {label} must be absolute").into());
        }
        Ok(value)
    })
    .transpose()
}

fn normalized_session_cwd(raw: Option<String>) -> Result<Option<String>, BoxError> {
    raw.map(|value| {
        if value.chars().any(char::is_control) {
            return Err("fallback-plan: source session_cwd contains control characters".into());
        }
        SessionCwd::parse(&value)
            .map(|cwd| cwd.as_str().to_owned())
            .map_err(|_| "fallback-plan: source session_cwd is invalid".into())
    })
    .transpose()
}

fn push_reason(reasons: &mut Vec<IneligibilityReason>, reason: IneligibilityReason) {
    if !reasons.contains(&reason) {
        reasons.push(reason);
    }
}

fn validate_failure(
    failure: Option<&FailureDiagnostic>,
    accepted: bool,
    reasons: &mut Vec<IneligibilityReason>,
) -> Result<(), BoxError> {
    if accepted {
        push_reason(
            reasons,
            IneligibilityReason::SourcePromptMayHaveBeenAccepted,
        );
    }
    let Some(failure) = failure else {
        push_reason(reasons, IneligibilityReason::SourceDiagnosticMissing);
        return Ok(());
    };
    if failure.prompt_may_have_been_accepted() != accepted {
        return Err("fallback-plan: source replay barrier contradicts its diagnostic".into());
    }
    if !failure.class().is_container_fallback_class() {
        push_reason(reasons, IneligibilityReason::SourceFailureNotContainer);
    } else if failure.failed_phase() != DiagnosticPhase::Spawn {
        push_reason(reasons, IneligibilityReason::SourceFailurePhaseInvalid);
    }
    if failure.disposition() != FailureDisposition::ContainerFallbackCandidate {
        push_reason(
            reasons,
            IneligibilityReason::SourceDispositionNotFallbackCandidate,
        );
    }
    Ok(())
}

fn same_failure_identity(left: &FailureDiagnostic, right: &FailureDiagnostic) -> bool {
    left.failed_phase() == right.failed_phase()
        && left.class() == right.class()
        && left.disposition() == right.disposition()
        && left.code() == right.code()
        && left.prompt_may_have_been_accepted() == right.prompt_may_have_been_accepted()
}

fn validate_smoke_lifecycle(
    events: &[DiagnosticEvent],
    outer_failure: Option<&FailureDiagnostic>,
    accepted: bool,
    started_at_ms: i64,
    ended_at_ms: i64,
    reasons: &mut Vec<IneligibilityReason>,
) -> Result<(), BoxError> {
    if events.is_empty() {
        push_reason(reasons, IneligibilityReason::SourceDiagnosticsIncomplete);
        return Ok(());
    }
    let mut open: Vec<(DiagnosticPhase, Option<DiagnosticOperation>)> = Vec::new();
    let mut matched_outer_failure = false;
    let mut previous_at_ms = started_at_ms;
    for event in events {
        let transition = event.transition();
        if transition.at_ms() < previous_at_ms || transition.at_ms() > ended_at_ms {
            return Err("fallback-plan: lifecycle timestamps are outside attempt order".into());
        }
        previous_at_ms = transition.at_ms();
        let key = (transition.phase(), transition.operation());
        if !accepted
            && matches!(
                transition.phase(),
                DiagnosticPhase::PromptStart
                    | DiagnosticPhase::PromptStream
                    | DiagnosticPhase::PromptFinish
            )
        {
            return Err(
                "fallback-plan: lifecycle contradicts its pre-prompt replay barrier".into(),
            );
        }
        match transition.status() {
            PhaseStatus::Started => {
                if open.contains(&key) {
                    return Err("fallback-plan: lifecycle starts one phase twice".into());
                }
                open.push(key);
            }
            PhaseStatus::Completed | PhaseStatus::Skipped | PhaseStatus::Failed => {
                let Some(index) = open.iter().rposition(|candidate| *candidate == key) else {
                    return Err(
                        "fallback-plan: lifecycle closes a phase that was not started".into(),
                    );
                };
                open.remove(index);
            }
        }
        if let Some(event_failure) = event.failure() {
            let Some(outer_failure) = outer_failure else {
                return Err(
                    "fallback-plan: lifecycle failure is missing from the artifact summary".into(),
                );
            };
            matched_outer_failure |= same_failure_identity(event_failure, outer_failure);
        }
    }
    if !open.is_empty() || outer_failure.is_some() && !matched_outer_failure {
        push_reason(reasons, IneligibilityReason::SourceDiagnosticsIncomplete);
    }
    Ok(())
}

fn validate_cleanup(cleanup: &SmokeCleanup) -> Result<(), BoxError> {
    let valid_step =
        |value: &str| matches!(value, "not_needed" | "completed" | "failed" | "timed_out");
    if cleanup.grace_timeout_secs == 0
        || cleanup.grace_timeout_secs > 60
        || !valid_step(&cleanup.cancel)
        || !valid_step(&cleanup.release)
        || !valid_step(&cleanup.retire)
        || !matches!(
            cleanup.run_scoped_backstop.as_str(),
            "not_needed" | "invoked_best_effort"
        )
    {
        return Err("fallback-plan: invalid or incomplete smoke cleanup record".into());
    }
    Ok(())
}

fn validate_target_evidence(
    target: &SmokeTarget,
    source_agent: &str,
    reasons: &mut Vec<IneligibilityReason>,
) -> Result<(), BoxError> {
    let expected_auth = format!("provenance:{source_agent}:auth");
    let expected_model = format!("provenance:{source_agent}:model");
    let mut checks = std::collections::HashSet::new();
    for row in &target.provenance {
        bounded_nonempty("source provenance check", &row.check, MAX_ID_BYTES)?;
        bounded_nonempty("source provenance detail", &row.detail, MAX_PATH_BYTES)?;
        if row.remedy.len() > MAX_PATH_BYTES || row.remedy.chars().any(char::is_control) {
            return Err("fallback-plan: invalid source provenance remedy".into());
        }
        if !checks.insert(row.check.as_str()) {
            return Err("fallback-plan: duplicate source provenance check".into());
        }
    }
    if !checks.contains(expected_auth.as_str()) || !checks.contains(expected_model.as_str()) {
        push_reason(reasons, IneligibilityReason::SourceDiagnosticsIncomplete);
    }
    match &target.authentication {
        SmokeAuthentication::PreAuthenticated | SmokeAuthentication::Automatic => {}
        SmokeAuthentication::ConfiguredMethod { method } => {
            bounded_nonempty("source authentication method", method, MAX_ID_BYTES)?;
        }
        SmokeAuthentication::ApiKeyEnv { name, present } => {
            let _ = present;
            bounded_nonempty("source authentication environment", name, MAX_ID_BYTES)?;
            push_reason(reasons, IneligibilityReason::SourceDiagnosticsIncomplete);
        }
        SmokeAuthentication::NotApplicable => {
            push_reason(reasons, IneligibilityReason::SourceDiagnosticsIncomplete);
        }
    }
    Ok(())
}

fn parse_smoke_source(bytes: &[u8]) -> Result<NormalizedSource, BoxError> {
    let source: SmokeSourceV2 = serde_json::from_slice(bytes)
        .map_err(|error| format!("fallback-plan: invalid smoke artifact: {error}"))?;
    if source.schema_version != SMOKE_SCHEMA_V2 {
        return Err(format!(
            "fallback-plan: unsupported smoke schema {}",
            source.schema_version
        )
        .into());
    }
    validate_id("source attempt id", source.attempt.id.clone())?;
    let source_agent_raw = validate_id("source agent", source.request.agent.clone())?;
    let source_agent_id = AgentId::parse(source_agent_raw.clone())
        .map_err(|_| "fallback-plan: invalid source agent")?;
    bounded_nonempty(
        "source bridge package version",
        &source.bridge.package_version,
        MAX_ID_BYTES,
    )?;
    if let Some(commit) = &source.bridge.git_commit {
        bounded_nonempty("source bridge commit", commit, MAX_ID_BYTES)?;
    }
    bounded_nonempty(
        "source requested config path",
        &source.request.requested_config_path,
        MAX_PATH_BYTES,
    )?;
    let config_canonical_path = normalized_evidence_path(
        "source canonical config path",
        source.request.canonical_config_path,
    )?;
    if let Some(digest) = &source.request.config_sha256 {
        if !crate::local_file::valid_sha256(digest) {
            return Err("fallback-plan: invalid source config SHA-256".into());
        }
    }
    validate_cleanup(&source.cleanup)?;
    if source.attempt.timeout_secs == 0
        || source.attempt.timeout_secs > 900
        || source.attempt.started_at_ms < 0
        || source.attempt.ended_at_ms < source.attempt.started_at_ms
        || source.session.id != source.attempt.id
        || source.session.configure_calls > 1
        || source.turn.prompt != crate::smoke::FIXED_PROMPT
        || source.turn.prompt_calls > 1
    {
        return Err("fallback-plan: inconsistent smoke artifact lifecycle".into());
    }
    let mut reasons = Vec::new();
    if source.success {
        push_reason(&mut reasons, IneligibilityReason::SourceNotFailed);
    }
    if source.attempt.timed_out {
        push_reason(&mut reasons, IneligibilityReason::SourceTimedOut);
    }
    if source.diagnostics.dropped_events > 0 {
        push_reason(
            &mut reasons,
            IneligibilityReason::SourceDiagnosticsIncomplete,
        );
    }
    if config_canonical_path.is_none() || source.request.config_sha256.is_none() {
        push_reason(
            &mut reasons,
            IneligibilityReason::SourceConfigProvenanceMissing,
        );
    }
    let execution_mode = source
        .target
        .as_ref()
        .map(|target| target.execution_mode.clone())
        .unwrap_or_default();
    if !matches!(execution_mode.as_str(), "container_ro" | "container_rw") {
        push_reason(
            &mut reasons,
            IneligibilityReason::SourceNotContainerExecution,
        );
    }
    if execution_mode == "container_rw" {
        push_reason(&mut reasons, IneligibilityReason::SourceNotReadOnly);
    }
    if let Some(target) = source.target.as_ref() {
        validate_target_evidence(target, &source_agent_raw, &mut reasons)?;
    }
    validate_failure(
        source.diagnostics.failure.as_ref(),
        source.attempt.prompt_may_have_been_accepted,
        &mut reasons,
    )?;
    validate_smoke_lifecycle(
        &source.diagnostics.lifecycle,
        source.diagnostics.failure.as_ref(),
        source.attempt.prompt_may_have_been_accepted,
        source.attempt.started_at_ms,
        source.attempt.ended_at_ms,
        &mut reasons,
    )?;
    if !source.attempt.prompt_may_have_been_accepted
        && (source.turn.prompt_calls != 0
            || source.turn.terminal_state != "not_started"
            || source.turn.exact_pong
            || source.turn.text_bytes != 0
            || source.turn.tool_event_count != 0
            || source.turn.permission_update_count != 0)
    {
        return Err("fallback-plan: pre-prompt smoke artifact contains turn activity".into());
    }
    Ok(NormalizedSource {
        schema: "smoke_v2",
        attempt_id: source.attempt.id,
        original_agent: source_agent_raw,
        original_agent_id: source_agent_id,
        execution_mode,
        reported_session_cwd: normalized_session_cwd(source.request.session_cwd)?,
        config_canonical_path,
        config_sha256: source
            .request
            .config_sha256
            .map(|digest| digest.to_ascii_lowercase()),
        prompt_may_have_been_accepted: source.attempt.prompt_may_have_been_accepted,
        failure: source.diagnostics.failure,
        reasons,
    })
}

fn parse_source(bytes: &[u8]) -> Result<NormalizedSource, BoxError> {
    let discriminator: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("fallback-plan: malformed source JSON: {error}"))?;
    if discriminator
        .get("artifact_type")
        .and_then(serde_json::Value::as_str)
        == Some("task_diagnostic")
    {
        return Err(
            "fallback-plan: hand-assembled task diagnostic artifacts are not trusted evidence"
                .into(),
        );
    }
    parse_smoke_source(bytes)
}

fn load_snapshot(
    path: &Path,
) -> Result<(crate::local_file::LocalFileSnapshot, RegistrySnapshot), BoxError> {
    let file = crate::local_file::read_regular_file_bounded(
        path,
        "fallback-plan config",
        MAX_CONFIG_BYTES,
    )?;
    let raw =
        std::str::from_utf8(&file.bytes).map_err(|_| "fallback-plan: config is not valid UTF-8")?;
    let snapshot = crate::validate_registry_config_contents(raw)?;
    Ok((file, snapshot))
}

fn agent_kind_name(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Acp => "acp",
        AgentKind::Api => "api",
        AgentKind::ContainerRw => "container_rw",
    }
}

fn failure_class_name(class: DiagnosticFailureClass) -> &'static str {
    match class {
        DiagnosticFailureClass::Config => "config",
        DiagnosticFailureClass::Authentication => "authentication",
        DiagnosticFailureClass::Model => "model",
        DiagnosticFailureClass::Protocol => "protocol",
        DiagnosticFailureClass::Transport => "transport",
        DiagnosticFailureClass::AgentProcess => "agent_process",
        DiagnosticFailureClass::ContainerRuntime => "container_runtime",
        DiagnosticFailureClass::ContainerImage => "container_image",
        DiagnosticFailureClass::ContainerNetwork => "container_network",
        DiagnosticFailureClass::ContainerMount => "container_mount",
        DiagnosticFailureClass::ContainerCredentials => "container_credentials",
        DiagnosticFailureClass::Timeout => "timeout",
        DiagnosticFailureClass::Overloaded => "overloaded",
        DiagnosticFailureClass::ProviderLimit => "provider_limit",
        DiagnosticFailureClass::Persistence => "persistence",
        DiagnosticFailureClass::Canceled => "canceled",
        DiagnosticFailureClass::Unknown => "unknown",
    }
}

fn disposition_name(disposition: FailureDisposition) -> &'static str {
    match disposition {
        FailureDisposition::Fatal => "fatal",
        FailureDisposition::RetrySameTarget => "retry_same_target",
        FailureDisposition::ContainerFallbackCandidate => "container_fallback_candidate",
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn shell_render(argv: &[String]) -> String {
    argv.iter()
        .map(|value| shell_quote(value))
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Serialize)]
struct FallbackPlanV2 {
    schema_version: u16,
    eligible: bool,
    reasons: Vec<IneligibilityReason>,
    source: SourceRecord,
    target: TargetRecord,
    trust: TrustRecord,
    #[serde(skip_serializing_if = "Option::is_none")]
    rerun: Option<RerunRecord>,
}

#[derive(Serialize)]
struct SourceRecord {
    artifact_schema: &'static str,
    artifact_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reported_session_cwd: Option<String>,
    attempt_id: String,
    original_agent: String,
    execution_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    config_canonical_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    config_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_class: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_disposition: Option<&'static str>,
    prompt_may_have_been_accepted: bool,
}

#[derive(Serialize)]
struct TargetRecord {
    host_agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<&'static str>,
    host_fallback_eligible: bool,
    config_requested_path: String,
    config_canonical_path: String,
    config_sha256: String,
}

#[derive(Serialize)]
struct TrustRecord {
    confirmed_trusted_own_repo_read_only: bool,
    authority: &'static str,
}

#[derive(Serialize)]
struct RerunRecord {
    attempt_semantics: &'static str,
    argv: Vec<String>,
    shell_command: String,
}

fn build_plan(args: FallbackArgs) -> Result<FallbackPlanV2, BoxError> {
    let config_requested_path = validated_path_text(&args.config, "requested config path")?;
    let source_file = crate::local_file::read_regular_file_bounded(
        &args.source,
        "fallback-plan source artifact",
        MAX_SOURCE_BYTES,
    )?;
    let source_path_text =
        validated_path_text(&source_file.canonical_path, "source artifact path")?;
    let source = parse_source(&source_file.bytes)?;
    let (config_file, snapshot) = load_snapshot(&args.config)?;
    let config_path_text =
        validated_path_text(&config_file.canonical_path, "canonical config path")?;

    let mut reasons = source.reasons.clone();
    if source.config_canonical_path.as_deref() != Some(config_path_text.as_str())
        || source.config_sha256.as_deref() != Some(config_file.sha256.as_str())
    {
        push_reason(
            &mut reasons,
            IneligibilityReason::SourceConfigProvenanceMismatch,
        );
    }
    if !args.confirm_trusted_own_repo_read_only {
        push_reason(&mut reasons, IneligibilityReason::TrustConfirmationMissing);
    }
    let mut source_execution_cwd = None;
    match snapshot
        .entries
        .iter()
        .find(|entry| entry.id == source.original_agent_id)
    {
        None => push_reason(&mut reasons, IneligibilityReason::SourceAgentUnknown),
        Some(entry) if crate::smoke::execution_mode(entry) != source.execution_mode => push_reason(
            &mut reasons,
            IneligibilityReason::SourceAgentConfigurationMismatch,
        ),
        Some(entry) => {
            source_execution_cwd = entry
                .sandbox
                .as_ref()
                .and_then(|sandbox| std::fs::canonicalize(&sandbox.mount).ok())
                .and_then(|path| SessionCwd::parse(&path.to_string_lossy()).ok())
                .map(|cwd| cwd.as_str().to_owned());
            if source_execution_cwd.is_none() {
                push_reason(
                    &mut reasons,
                    IneligibilityReason::SourceAgentConfigurationMismatch,
                );
            }
        }
    }
    let target = snapshot
        .entries
        .iter()
        .find(|entry| entry.id == args.host_agent);
    let (kind, marked, target_eligible) = match target {
        Some(entry) => {
            let eligible = entry.host_fallback_eligible
                && matches!(entry.kind, AgentKind::Acp)
                && entry.sandbox.is_none();
            (
                Some(agent_kind_name(entry.kind)),
                entry.host_fallback_eligible,
                eligible,
            )
        }
        None => {
            push_reason(&mut reasons, IneligibilityReason::TargetAgentUnknown);
            (None, false, false)
        }
    };
    if target.is_some() && !target_eligible {
        push_reason(&mut reasons, IneligibilityReason::TargetAgentNotEligible);
    }

    let source_record = SourceRecord {
        artifact_schema: source.schema,
        artifact_path: source_path_text,
        reported_session_cwd: source.reported_session_cwd,
        attempt_id: source.attempt_id,
        original_agent: source.original_agent,
        execution_mode: source.execution_mode,
        config_canonical_path: source.config_canonical_path,
        config_sha256: source.config_sha256,
        failure_class: source
            .failure
            .as_ref()
            .map(|failure| failure_class_name(failure.class())),
        failure_code: source
            .failure
            .as_ref()
            .map(|failure| failure.code().as_str().to_owned()),
        failure_disposition: source
            .failure
            .as_ref()
            .map(|failure| disposition_name(failure.disposition())),
        prompt_may_have_been_accepted: source.prompt_may_have_been_accepted,
    };
    let rerun = if reasons.is_empty() {
        let executable_path = std::env::current_exe().map_err(|error| {
            format!("fallback-plan: cannot resolve current executable: {error}")
        })?;
        let executable = crate::local_file::read_regular_file_bounded(
            &executable_path,
            "fallback-plan executable",
            MAX_EXECUTABLE_BYTES,
        )?;
        let executable_path_text =
            validated_path_text(&executable.canonical_path, "current executable path")?;
        let source_cwd = source_execution_cwd
            .ok_or("fallback-plan: eligible source has no current config-owned mount")?;
        let argv = vec![
            executable_path_text,
            "smoke".to_owned(),
            "--agent".to_owned(),
            args.host_agent_raw.clone(),
            "--config".to_owned(),
            config_path_text.clone(),
            "--acknowledge-billable".to_owned(),
            "--session-cwd".to_owned(),
            source_cwd,
            "--expected-config-sha256".to_owned(),
            config_file.sha256.clone(),
            "--expected-executable-sha256".to_owned(),
            executable.sha256,
            "--fallback-source-agent".to_owned(),
            source_record.original_agent.clone(),
            "--require-host-fallback-eligible".to_owned(),
        ];
        Some(RerunRecord {
            attempt_semantics: "new_distinct_verification_smoke",
            shell_command: shell_render(&argv),
            argv,
        })
    } else {
        None
    };
    Ok(FallbackPlanV2 {
        schema_version: 2,
        eligible: reasons.is_empty(),
        reasons,
        source: source_record,
        target: TargetRecord {
            host_agent: args.host_agent_raw,
            kind,
            host_fallback_eligible: marked,
            config_requested_path,
            config_canonical_path: config_path_text,
            config_sha256: config_file.sha256,
        },
        trust: TrustRecord {
            confirmed_trusted_own_repo_read_only: args.confirm_trusted_own_repo_read_only,
            authority: "local_cli_flag",
        },
        rerun,
    })
}

pub(crate) fn fallback_plan_cmd(args: &[String]) -> Result<(), BoxError> {
    let Some(args) = parse_args(args)? else {
        return Ok(());
    };
    let plan = build_plan(args)?;
    println!("{}", serde_json::to_string_pretty(&plan)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_render_quotes_whitespace_quotes_and_newlines_as_single_arguments() {
        let argv = vec![
            "a2a-bridge".to_owned(),
            "two words".to_owned(),
            "quote'argument".to_owned(),
            "line\nbreak".to_owned(),
        ];
        assert_eq!(
            shell_render(&argv),
            "'a2a-bridge' 'two words' 'quote'\"'\"'argument' 'line\nbreak'"
        );
    }
}
