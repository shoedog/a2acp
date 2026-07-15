//! One explicitly acknowledged, bounded, billable fixed-prompt probe (R2c).
//!
//! This is deliberately not built on the workflow runner: one CLI invocation owns one registry
//! resolution, one session configuration, and one prompt. There is no retry or alternate target.

use std::fs::{File, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bridge_core::catalog::is_blocked_model_id;
use bridge_core::diagnostics::{
    diagnostic_timestamp_ms, DiagnosticFailureClass, DiagnosticPhase, DiagnosticRedactor,
    FailureDiagnostic, FailureDiagnosticInput, FailureDisposition, InMemoryDiagnosticObserver,
    RedactedDiagnosticId,
};
use bridge_core::domain::{
    effective_config, AgentKind, AgentOverride, EffectiveConfig, Effort, Part, PermissionDecision,
    PermissionRequest, SessionContext, SessionSpec,
};
use bridge_core::error::BridgeError;
use bridge_core::ids::{AgentId, SessionId};
use bridge_core::orch::{OrchEventKind, UsageSnapshot};
use bridge_core::ports::{
    AgentBackend, AgentRegistry, BackendObservers, PolicyEngine, Resolved, RichEventSink, Update,
};
use bridge_core::SessionCwd;
use bridge_registry::registry::Registry;
use futures::StreamExt;
use serde::Serialize;
use serde_json::Value;

use crate::{
    doctor, epoch_secs, make_spawn_fn, recover_orphans, run_guard_runtimes, BoxError, RunEndGuard,
    SMOKE_USAGE,
};

pub(crate) const FIXED_PROMPT: &str = "Reply exactly PONG. Do not use tools.";
const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_TIMEOUT_SECS: u64 = 900;
const CLEANUP_TIMEOUT_SECS: u64 = 10;
const MAX_CAPTURED_TEXT_BYTES: usize = 1024;
const DIAGNOSTIC_CAPACITY: usize = 128;
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_EXECUTABLE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
struct FallbackSmokeGuard {
    expected_config_sha256: String,
    expected_executable_sha256: String,
    source_agent: AgentId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SmokeArgs {
    agent: AgentId,
    config: PathBuf,
    model: Option<String>,
    effort: Option<Effort>,
    mode: Option<String>,
    session_cwd: Option<PathBuf>,
    timeout_secs: u64,
    include_redacted_stderr: bool,
    out: Option<PathBuf>,
    fallback_guard: Option<FallbackSmokeGuard>,
}

fn parse_args(args: &[String]) -> Result<SmokeArgs, BoxError> {
    // The acknowledgement barrier wins before any path lookup or config/registry work. Help is handled
    // by the caller and is the sole non-billable exception.
    if !args.iter().any(|arg| arg == "--acknowledge-billable") {
        return Err(format!(
            "smoke: refusing a potentially billable turn without --acknowledge-billable\n{SMOKE_USAGE}"
        )
        .into());
    }

    let mut agent = None;
    let mut config = None;
    let mut model = None;
    let mut effort = None;
    let mut mode = None;
    let mut session_cwd = None;
    let mut timeout_secs = None;
    let mut include_redacted_stderr = false;
    let mut out = None;
    let mut expected_config_sha256 = None;
    let mut expected_executable_sha256 = None;
    let mut fallback_source_agent = None;
    let mut require_host_fallback_eligible = false;
    let mut acknowledged = false;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let value = |it: &mut std::slice::Iter<'_, String>, flag: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("smoke: {flag} requires a value"))
        };
        match arg.as_str() {
            "--acknowledge-billable" if acknowledged => {
                return Err("smoke: duplicate --acknowledge-billable".into());
            }
            "--acknowledge-billable" => acknowledged = true,
            "--include-redacted-stderr" if include_redacted_stderr => {
                return Err("smoke: duplicate --include-redacted-stderr".into());
            }
            "--include-redacted-stderr" => include_redacted_stderr = true,
            "--agent" if agent.is_some() => return Err("smoke: duplicate --agent".into()),
            "--agent" => agent = Some(value(&mut it, "--agent")?),
            "--config" if config.is_some() => return Err("smoke: duplicate --config".into()),
            "--config" => config = Some(PathBuf::from(value(&mut it, "--config")?)),
            "--model" if model.is_some() => return Err("smoke: duplicate --model".into()),
            "--model" => model = Some(value(&mut it, "--model")?),
            "--effort" if effort.is_some() => return Err("smoke: duplicate --effort".into()),
            "--effort" => effort = Some(value(&mut it, "--effort")?),
            "--mode" if mode.is_some() => return Err("smoke: duplicate --mode".into()),
            "--mode" => mode = Some(value(&mut it, "--mode")?),
            "--session-cwd" if session_cwd.is_some() => {
                return Err("smoke: duplicate --session-cwd".into());
            }
            "--session-cwd" => {
                session_cwd = Some(PathBuf::from(value(&mut it, "--session-cwd")?));
            }
            "--timeout-secs" if timeout_secs.is_some() => {
                return Err("smoke: duplicate --timeout-secs".into());
            }
            "--timeout-secs" => timeout_secs = Some(value(&mut it, "--timeout-secs")?),
            "--out" if out.is_some() => return Err("smoke: duplicate --out".into()),
            "--out" => out = Some(PathBuf::from(value(&mut it, "--out")?)),
            "--expected-config-sha256" if expected_config_sha256.is_some() => {
                return Err("smoke: duplicate --expected-config-sha256".into());
            }
            "--expected-config-sha256" => {
                expected_config_sha256 = Some(value(&mut it, "--expected-config-sha256")?);
            }
            "--expected-executable-sha256" if expected_executable_sha256.is_some() => {
                return Err("smoke: duplicate --expected-executable-sha256".into());
            }
            "--expected-executable-sha256" => {
                expected_executable_sha256 = Some(value(&mut it, "--expected-executable-sha256")?);
            }
            "--fallback-source-agent" if fallback_source_agent.is_some() => {
                return Err("smoke: duplicate --fallback-source-agent".into());
            }
            "--fallback-source-agent" => {
                fallback_source_agent = Some(value(&mut it, "--fallback-source-agent")?);
            }
            "--require-host-fallback-eligible" if require_host_fallback_eligible => {
                return Err("smoke: duplicate --require-host-fallback-eligible".into());
            }
            "--require-host-fallback-eligible" => require_host_fallback_eligible = true,
            other => {
                return Err(format!("smoke: unknown argument {other:?}\n{SMOKE_USAGE}").into());
            }
        }
    }

    if !acknowledged {
        return Err(format!(
            "smoke: refusing a potentially billable turn without --acknowledge-billable\n{SMOKE_USAGE}"
        )
        .into());
    }

    let agent = validate_raw_id("--agent", agent.ok_or("smoke: --agent is required")?)?;
    let agent = AgentId::parse(agent).map_err(|_| "smoke: --agent must be non-empty")?;
    let config = config.ok_or("smoke: --config is required")?;
    if config.as_os_str().is_empty() {
        return Err("smoke: --config must be non-empty".into());
    }

    let model = model
        .map(|raw| validate_raw_id("--model", raw))
        .transpose()?;
    if model.as_deref().is_some_and(is_blocked_model_id) {
        return Err("smoke: --model names a bridge-blocked model id".into());
    }
    let effort = effort
        .map(|raw| Effort::from_str(&raw).map_err(|error| format!("smoke: {error}")))
        .transpose()?;
    let mode = mode.map(|raw| validate_raw_id("--mode", raw)).transpose()?;

    let timeout_secs = timeout_secs
        .map(|raw| {
            raw.parse::<u64>()
                .map_err(|_| "smoke: --timeout-secs must be an integer in 1..=900".to_string())
        })
        .transpose()?
        .unwrap_or(DEFAULT_TIMEOUT_SECS);
    if !(1..=MAX_TIMEOUT_SECS).contains(&timeout_secs) {
        return Err("smoke: --timeout-secs must be in 1..=900".into());
    }

    if out.as_deref() == Some(Path::new("-")) {
        return Err("smoke: --out requires an explicit file path, not '-'".into());
    }
    if out.as_ref().is_some_and(|path| path.as_os_str().is_empty()) {
        return Err("smoke: --out requires a non-empty file path".into());
    }
    if session_cwd
        .as_ref()
        .is_some_and(|path| path.as_os_str().is_empty())
    {
        return Err("smoke: --session-cwd requires a non-empty directory path".into());
    }

    let guard_count = usize::from(expected_config_sha256.is_some())
        + usize::from(expected_executable_sha256.is_some())
        + usize::from(fallback_source_agent.is_some())
        + usize::from(require_host_fallback_eligible);
    let fallback_guard = match guard_count {
        0 => None,
        4 if session_cwd.is_some() => {
            let expected_config_sha256 = expected_config_sha256.expect("counted above");
            let expected_executable_sha256 = expected_executable_sha256.expect("counted above");
            if !crate::local_file::valid_sha256(&expected_config_sha256)
                || !crate::local_file::valid_sha256(&expected_executable_sha256)
            {
                return Err(
                    "smoke: fallback guard digests must be 64 hexadecimal characters".into(),
                );
            }
            let source = validate_raw_id(
                "--fallback-source-agent",
                fallback_source_agent.expect("counted above"),
            )?;
            Some(FallbackSmokeGuard {
                expected_config_sha256: expected_config_sha256.to_ascii_lowercase(),
                expected_executable_sha256: expected_executable_sha256.to_ascii_lowercase(),
                source_agent: AgentId::parse(source)
                    .map_err(|_| "smoke: invalid --fallback-source-agent")?,
            })
        }
        4 => return Err("smoke: fallback guard requires --session-cwd".into()),
        _ => {
            return Err(
                "smoke: fallback guard flags must be supplied together as a closed set".into(),
            )
        }
    };

    Ok(SmokeArgs {
        agent,
        config,
        model,
        effort,
        mode,
        session_cwd,
        timeout_secs,
        include_redacted_stderr,
        out,
        fallback_guard,
    })
}

fn validate_raw_id(flag: &str, raw: String) -> Result<String, String> {
    if raw.is_empty() || raw.trim() != raw || raw.contains('\0') {
        return Err(format!("smoke: {flag} must be a non-empty, unpadded id"));
    }
    Ok(raw)
}

#[derive(Serialize)]
struct SmokeArtifactV2 {
    schema_version: u16,
    success: bool,
    bridge: BridgeIdentity,
    attempt: AttemptRecord,
    request: RequestRecord,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<TargetRecord>,
    session: SessionRecord,
    turn: TurnRecord,
    diagnostics: DiagnosticsRecord,
    cleanup: CleanupRecord,
}

#[derive(Serialize)]
struct BridgeIdentity {
    package_version: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    git_commit: Option<&'static str>,
}

#[derive(Serialize)]
struct AttemptRecord {
    id: String,
    timeout_secs: u64,
    started_at_ms: i64,
    ended_at_ms: i64,
    timed_out: bool,
    prompt_may_have_been_accepted: bool,
}

#[derive(Serialize)]
struct RequestRecord {
    agent: String,
    requested_config_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    canonical_config_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    config_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fallback_guard: Option<FallbackGuardRecord>,
}

#[derive(Serialize)]
struct FallbackGuardRecord {
    expected_config_sha256: String,
    expected_executable_sha256: String,
    source_agent: String,
    require_host_fallback_eligible: bool,
}

#[derive(Serialize)]
struct TargetRecord {
    execution_mode: &'static str,
    provenance: Vec<doctor::CheckResult>,
    authentication: AuthenticationRecord,
}

#[derive(Serialize)]
#[serde(tag = "path", rename_all = "snake_case")]
enum AuthenticationRecord {
    ApiKeyEnv {
        name: RedactedDiagnosticId,
        present: bool,
    },
    PreAuthenticated,
    ConfiguredMethod {
        method: RedactedDiagnosticId,
    },
    Automatic,
    NotApplicable,
}

#[derive(Serialize)]
struct SessionRecord {
    id: String,
    configure_calls: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    effective_request: Option<EffectiveConfigRecord>,
}

#[derive(Serialize)]
struct EffectiveConfigRecord {
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
}

#[derive(Serialize)]
struct TurnRecord {
    prompt: &'static str,
    prompt_calls: u8,
    terminal_state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_reason: Option<&'static str>,
    exact_pong: bool,
    text_bytes: u64,
    tool_event_count: u64,
    permission_update_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<SmokeUsage>,
}

impl Default for TurnRecord {
    fn default() -> Self {
        Self {
            prompt: FIXED_PROMPT,
            prompt_calls: 0,
            terminal_state: "not_started",
            stop_reason: None,
            exact_pong: false,
            text_bytes: 0,
            tool_event_count: 0,
            permission_update_count: 0,
            usage: None,
        }
    }
}

#[derive(Serialize)]
struct SmokeUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    used: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost: Option<SmokeCost>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal: Option<bridge_core::orch::TerminalUsage>,
    at_ms: i64,
}

#[derive(Serialize)]
struct SmokeCost {
    amount: f64,
    currency: String,
}

#[derive(Serialize)]
struct DiagnosticsRecord {
    lifecycle: Vec<Value>,
    dropped_events: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure: Option<Value>,
    stderr_text: &'static str,
}

#[derive(Serialize)]
struct CleanupRecord {
    grace_timeout_secs: u64,
    cancel: &'static str,
    release: &'static str,
    retire: &'static str,
    run_scoped_backstop: &'static str,
}

struct CleanupOutcome {
    record: CleanupRecord,
    error: Option<BridgeError>,
}

impl Default for CleanupRecord {
    fn default() -> Self {
        Self {
            grace_timeout_secs: CLEANUP_TIMEOUT_SECS,
            cancel: "not_needed",
            release: "not_needed",
            retire: "not_needed",
            run_scoped_backstop: "not_needed",
        }
    }
}

struct ArtifactState {
    artifact: SmokeArtifactV2,
    failure: Option<FailureDiagnostic>,
    observer: Arc<InMemoryDiagnosticObserver>,
}

impl ArtifactState {
    fn new(args: &SmokeArgs) -> Self {
        let now = diagnostic_timestamp_ms();
        let id = format!(
            "smoke-{}-{}",
            std::process::id(),
            crate::implement::nonce(8)
        );
        Self {
            artifact: SmokeArtifactV2 {
                schema_version: 2,
                success: false,
                bridge: BridgeIdentity {
                    package_version: env!("CARGO_PKG_VERSION"),
                    git_commit: build_git_commit(),
                },
                attempt: AttemptRecord {
                    id: id.clone(),
                    timeout_secs: args.timeout_secs,
                    started_at_ms: now,
                    ended_at_ms: now,
                    timed_out: false,
                    prompt_may_have_been_accepted: false,
                },
                request: RequestRecord {
                    agent: args.agent.as_str().to_owned(),
                    requested_config_path: args.config.to_string_lossy().into_owned(),
                    canonical_config_path: None,
                    config_sha256: None,
                    model: args.model.clone(),
                    effort: args.effort.as_ref().map(crate::effort_to_string),
                    mode: args.mode.clone(),
                    session_cwd: None,
                    fallback_guard: args
                        .fallback_guard
                        .as_ref()
                        .map(|guard| FallbackGuardRecord {
                            expected_config_sha256: guard.expected_config_sha256.clone(),
                            expected_executable_sha256: guard.expected_executable_sha256.clone(),
                            source_agent: guard.source_agent.as_str().to_owned(),
                            require_host_fallback_eligible: true,
                        }),
                },
                target: None,
                session: SessionRecord {
                    id,
                    configure_calls: 0,
                    effective_request: None,
                },
                turn: TurnRecord::default(),
                diagnostics: DiagnosticsRecord {
                    lifecycle: Vec::new(),
                    dropped_events: 0,
                    failure: None,
                    stderr_text: if args.include_redacted_stderr {
                        "best_effort"
                    } else {
                        "excluded"
                    },
                },
                cleanup: CleanupRecord::default(),
            },
            failure: None,
            observer: Arc::new(
                InMemoryDiagnosticObserver::new(DIAGNOSTIC_CAPACITY)
                    .expect("positive smoke diagnostic capacity")
                    .with_redacted_stderr(args.include_redacted_stderr),
            ),
        }
    }

    fn fail_static(
        &mut self,
        phase: DiagnosticPhase,
        class: DiagnosticFailureClass,
        code: &'static str,
        summary: &'static str,
        accepted: bool,
    ) {
        self.failure = Some(static_failure(phase, class, code, summary, accepted));
        self.artifact.attempt.prompt_may_have_been_accepted = accepted;
    }

    fn fail_error(&mut self, error: &BridgeError, fallback_phase: DiagnosticPhase, accepted: bool) {
        if let BridgeError::AgentFailure { diagnostic } = error {
            self.artifact.attempt.prompt_may_have_been_accepted |=
                diagnostic.prompt_may_have_been_accepted();
            self.failure = Some((**diagnostic).clone());
            return;
        }
        let (class, code, summary) = safe_error_category(error);
        self.fail_static(fallback_phase, class, code, summary, accepted);
    }

    fn fail_turn_error(
        &mut self,
        error: &BridgeError,
        fallback_phase: DiagnosticPhase,
        accepted: bool,
    ) {
        if !matches!(error, BridgeError::FrameError) {
            self.fail_error(error, fallback_phase, accepted);
            return;
        }
        let (class, code, summary) = if self.artifact.turn.tool_event_count > 0 {
            (
                DiagnosticFailureClass::Protocol,
                "smoke.tool_attempt",
                "Agent attempted to use a tool during smoke",
            )
        } else if self.artifact.turn.permission_update_count > 0 {
            (
                DiagnosticFailureClass::Protocol,
                "smoke.permission_update",
                "Agent requested permission during smoke",
            )
        } else {
            match self.artifact.turn.terminal_state {
                "cancelled" => (
                    DiagnosticFailureClass::Canceled,
                    "smoke.cancelled",
                    "Smoke turn was canceled",
                ),
                "eof_without_terminal" => (
                    DiagnosticFailureClass::Protocol,
                    "smoke.missing_terminal",
                    "Agent stream ended without terminal evidence",
                ),
                "non_success_terminal" => (
                    DiagnosticFailureClass::Protocol,
                    "smoke.non_success_terminal",
                    "Agent returned a non-success terminal reason",
                ),
                "completed" => (
                    DiagnosticFailureClass::Protocol,
                    "smoke.not_exact_pong",
                    "Agent terminal output did not match the fixed smoke response",
                ),
                _ => (
                    DiagnosticFailureClass::Protocol,
                    "smoke.terminal_contract",
                    "Agent response did not satisfy the smoke terminal contract",
                ),
            }
        };
        self.fail_static(fallback_phase, class, code, summary, accepted);
    }

    async fn finalize(mut self, include_stderr: bool) -> SmokeArtifactV2 {
        self.artifact.attempt.ended_at_ms = diagnostic_timestamp_ms();
        let events = self.observer.snapshot().await;
        self.artifact.diagnostics.dropped_events = self.observer.dropped_count().await;
        self.artifact.diagnostics.lifecycle = events
            .iter()
            .filter_map(|event| serde_json::to_value(event).ok())
            .map(|mut value| {
                if !include_stderr {
                    strip_stderr_text(&mut value);
                }
                value
            })
            .collect();
        self.artifact.diagnostics.failure = self.failure.as_ref().and_then(|failure| {
            let mut value = serde_json::to_value(failure).ok()?;
            if !include_stderr {
                strip_stderr_text(&mut value);
            }
            Some(value)
        });
        self.artifact
    }
}

fn build_git_commit() -> Option<&'static str> {
    option_env!("VERGEN_GIT_SHA")
        .or(option_env!("A2A_BRIDGE_GIT_SHA"))
        .filter(|value| !value.is_empty())
}

fn static_failure(
    phase: DiagnosticPhase,
    class: DiagnosticFailureClass,
    code: &'static str,
    summary: &'static str,
    accepted: bool,
) -> FailureDiagnostic {
    let last_completed_phase = if accepted {
        match phase {
            DiagnosticPhase::PromptStart => Some(DiagnosticPhase::ConfigApply),
            DiagnosticPhase::PromptStream => Some(DiagnosticPhase::PromptStart),
            DiagnosticPhase::PromptFinish => Some(DiagnosticPhase::PromptStream),
            DiagnosticPhase::Teardown => Some(DiagnosticPhase::PromptFinish),
            _ => None,
        }
    } else {
        None
    };
    FailureDiagnostic::build_static_code(
        FailureDiagnosticInput {
            failed_phase: phase,
            last_completed_phase,
            class,
            disposition: FailureDisposition::Fatal,
            code: String::new(),
            summary: summary.to_owned(),
            causes: Vec::new(),
            stderr_observed: false,
            stderr_line_count: 0,
            stderr_scope: None,
            stderr_tail: None,
            stderr_redaction: None,
            retry_after_ms: None,
            reset_at_ms: None,
            prompt_may_have_been_accepted: accepted,
        },
        code,
        &DiagnosticRedactor::default(),
    )
    .expect("bridge-owned smoke diagnostic is valid")
}

fn safe_error_category(
    error: &BridgeError,
) -> (DiagnosticFailureClass, &'static str, &'static str) {
    use DiagnosticFailureClass as Class;
    match error {
        BridgeError::ModelNotAvailable => (Class::Model, "smoke.model", "Model unavailable"),
        BridgeError::AgentNotAuthenticated | BridgeError::AuthRequired { .. } => (
            Class::Authentication,
            "smoke.authentication",
            "Agent authentication failed",
        ),
        BridgeError::AgentTimedOut | BridgeError::CancelTimeout => {
            (Class::Timeout, "smoke.timeout", "Agent turn timed out")
        }
        BridgeError::AgentCrashed { .. } => (
            Class::AgentProcess,
            "smoke.agent_process",
            "Agent process failed",
        ),
        BridgeError::AgentOverloaded => (
            Class::Overloaded,
            "smoke.overloaded",
            "Agent reported overload",
        ),
        BridgeError::ConfigInvalid { .. }
        | BridgeError::ConfigMismatch { .. }
        | BridgeError::ConfigReseedRequired { .. }
        | BridgeError::InvalidRequest { .. } => {
            (Class::Config, "smoke.config", "Smoke configuration failed")
        }
        BridgeError::PermissionDenied | BridgeError::PermissionRequired { .. } => (
            Class::Protocol,
            "smoke.tool_refused",
            "Agent attempted a disallowed tool operation",
        ),
        BridgeError::FrameError => (
            Class::Protocol,
            "smoke.terminal_contract",
            "Agent response did not satisfy the smoke terminal contract",
        ),
        _ => (Class::Unknown, "smoke.failure", "Smoke attempt failed"),
    }
}

fn is_timeout_failure(error: &BridgeError) -> bool {
    if matches!(
        error,
        BridgeError::AgentTimedOut | BridgeError::CancelTimeout
    ) {
        return true;
    }
    if let BridgeError::AgentFailure { diagnostic } = error {
        return diagnostic.class() == DiagnosticFailureClass::Timeout;
    }
    false
}

fn strip_stderr_text(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("stderr_tail");
            map.remove("stderr_redaction");
            for value in map.values_mut() {
                strip_stderr_text(value);
            }
        }
        Value::Array(values) => values.iter_mut().for_each(strip_stderr_text),
        _ => {}
    }
}

#[derive(Default)]
struct SmokeRichSink {
    tool_events: AtomicU64,
}

impl SmokeRichSink {
    fn tool_event_count(&self) -> u64 {
        self.tool_events.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl RichEventSink for SmokeRichSink {
    fn record(&self, kind: OrchEventKind) {
        if matches!(
            kind,
            OrchEventKind::ToolCall { .. } | OrchEventKind::ToolCallUpdate { .. }
        ) {
            self.tool_events.fetch_add(1, Ordering::SeqCst);
        }
    }

    async fn flush(&self) -> Result<(), BridgeError> {
        Ok(())
    }
}

struct DenyAllPolicy;

impl PolicyEngine for DenyAllPolicy {
    fn decide(
        &self,
        _req: &PermissionRequest,
        _ctx: &SessionContext,
    ) -> Result<PermissionDecision, BridgeError> {
        Err(BridgeError::PermissionDenied)
    }
}

struct DrainResult {
    turn: TurnRecord,
    error: Option<BridgeError>,
}

enum ResolveOnce {
    Resolved(Resolved),
    Failed(BridgeError),
    TimedOut,
}

fn turn_failure_phase(turn: &TurnRecord) -> DiagnosticPhase {
    if turn.prompt_calls == 0 {
        DiagnosticPhase::ConfigApply
    } else if turn.terminal_state == "prompt_failed" {
        DiagnosticPhase::PromptStart
    } else if matches!(
        turn.terminal_state,
        "eof_without_terminal" | "backend_error" | "timeout"
    ) {
        DiagnosticPhase::PromptStream
    } else {
        DiagnosticPhase::PromptFinish
    }
}

async fn resolve_once(
    registry: &dyn AgentRegistry,
    agent: &AgentId,
    observer: Arc<dyn bridge_core::ports::DiagnosticObserver>,
    deadline: tokio::time::Instant,
) -> ResolveOnce {
    match tokio::time::timeout_at(deadline, registry.resolve_observed(agent, observer)).await {
        Err(_) => ResolveOnce::TimedOut,
        Ok(Err(error)) => ResolveOnce::Failed(error),
        Ok(Ok(resolved)) => ResolveOnce::Resolved(resolved),
    }
}

async fn execute_one(
    backend: Arc<dyn AgentBackend>,
    session: &SessionId,
    spec: &SessionSpec,
    observer: Arc<InMemoryDiagnosticObserver>,
    rich: Arc<SmokeRichSink>,
    deadline: tokio::time::Instant,
) -> DrainResult {
    let mut turn = TurnRecord::default();

    match tokio::time::timeout_at(deadline, backend.configure_session(session, spec)).await {
        Err(_) => {
            turn.terminal_state = "timeout";
            return DrainResult {
                turn,
                error: Some(BridgeError::AgentTimedOut),
            };
        }
        Ok(Err(error)) => {
            turn.terminal_state = "config_failed";
            return DrainResult {
                turn,
                error: Some(error),
            };
        }
        Ok(Ok(())) => turn.prompt_calls = 0,
    }

    turn.prompt_calls = 1;
    let rich_dyn: Arc<dyn RichEventSink> = rich.clone();
    let diagnostic_dyn: Arc<dyn bridge_core::ports::DiagnosticObserver> = observer;
    let stream = match tokio::time::timeout_at(
        deadline,
        backend.prompt_with_observers(
            session,
            vec![Part {
                text: FIXED_PROMPT.to_owned(),
            }],
            BackendObservers::new(diagnostic_dyn, Some(rich_dyn)),
        ),
    )
    .await
    {
        Err(_) => {
            turn.terminal_state = "timeout";
            return DrainResult {
                turn,
                error: Some(BridgeError::AgentTimedOut),
            };
        }
        Ok(Err(error)) => {
            turn.terminal_state = "prompt_failed";
            return DrainResult {
                turn,
                error: Some(error),
            };
        }
        Ok(Ok(stream)) => stream,
    };

    match tokio::time::timeout_at(deadline, drain_stream(stream, Arc::clone(&rich))).await {
        Err(_) => {
            turn.terminal_state = "timeout";
            turn.tool_event_count = rich.tool_event_count();
            DrainResult {
                turn,
                error: Some(BridgeError::AgentTimedOut),
            }
        }
        Ok(result) => result,
    }
}

async fn drain_stream(
    mut stream: bridge_core::ports::BackendStream,
    rich: Arc<SmokeRichSink>,
) -> DrainResult {
    let mut turn = TurnRecord {
        prompt_calls: 1,
        ..TurnRecord::default()
    };
    let mut captured = Vec::new();
    let mut capture_overflow = false;
    let mut usage: Option<UsageSnapshot> = None;

    while let Some(update) = stream.next().await {
        match update {
            Err(error) => {
                turn.terminal_state = "backend_error";
                turn.tool_event_count = rich.tool_event_count();
                turn.usage = usage.map(smoke_usage);
                return DrainResult {
                    turn,
                    error: Some(error),
                };
            }
            Ok(Update::Text(text)) => {
                turn.text_bytes = turn.text_bytes.saturating_add(text.len() as u64);
                if captured.len().saturating_add(text.len()) <= MAX_CAPTURED_TEXT_BYTES {
                    captured.extend_from_slice(text.as_bytes());
                } else {
                    capture_overflow = true;
                    captured.clear();
                }
            }
            Ok(Update::Permission(_)) => {
                turn.permission_update_count = turn.permission_update_count.saturating_add(1);
            }
            Ok(Update::Usage(next)) => {
                let mut next = next;
                if let Some(previous) = &usage {
                    next.merge_missing_from(previous);
                }
                usage = Some(next);
            }
            Ok(Update::Done { stop_reason }) => {
                let safe_reason = safe_stop_reason(&stop_reason);
                turn.stop_reason = Some(safe_reason);
                turn.terminal_state = if safe_reason == "cancelled" {
                    "cancelled"
                } else if matches!(safe_reason, "end_turn" | "stop") {
                    "completed"
                } else {
                    "non_success_terminal"
                };
                break;
            }
        }
    }

    let _ = rich.flush().await;
    turn.tool_event_count = rich.tool_event_count();
    turn.usage = usage.map(smoke_usage);
    if turn.stop_reason.is_none() {
        turn.terminal_state = "eof_without_terminal";
    }
    turn.exact_pong =
        !capture_overflow && std::str::from_utf8(&captured).is_ok_and(|text| text.trim() == "PONG");

    let successful = turn.terminal_state == "completed"
        && turn.exact_pong
        && turn.tool_event_count == 0
        && turn.permission_update_count == 0;
    let error = (!successful).then_some(BridgeError::FrameError);
    DrainResult { turn, error }
}

fn safe_stop_reason(reason: &str) -> &'static str {
    match reason {
        "end_turn" => "end_turn",
        "stop" => "stop",
        "cancelled" => "cancelled",
        "max_tokens" => "max_tokens",
        "max_turn_requests" => "max_turn_requests",
        "max_tool_rounds" => "max_tool_rounds",
        "refusal" => "refusal",
        _ => "other",
    }
}

fn smoke_usage(value: UsageSnapshot) -> SmokeUsage {
    SmokeUsage {
        used: value.used,
        size: value.size,
        cost: value.cost.and_then(|cost| {
            (cost.amount.is_finite() && cost.amount >= 0.0).then(|| SmokeCost {
                amount: cost.amount,
                currency: if cost.currency.len() == 3
                    && cost.currency.bytes().all(|byte| byte.is_ascii_uppercase())
                {
                    cost.currency
                } else {
                    "unknown".into()
                },
            })
        }),
        terminal: value.terminal,
        at_ms: value.at_ms,
    }
}

pub(crate) fn execution_mode(entry: &bridge_core::domain::AgentEntry) -> &'static str {
    match entry.kind {
        AgentKind::Api => "remote_api",
        AgentKind::ContainerRw => "container_rw",
        AgentKind::Acp if entry.sandbox.is_some() => "container_ro",
        AgentKind::Acp => "host",
    }
}

fn artifact_redactor(
    entry: &bridge_core::domain::AgentEntry,
    session_cwd: Option<&SessionCwd>,
) -> DiagnosticRedactor {
    let mut known_values: Vec<String> = entry
        .api_key_env
        .as_deref()
        .and_then(|name| std::env::var(name).ok())
        .into_iter()
        .collect();
    let cwd = session_cwd
        .map(SessionCwd::as_str)
        .or(entry.session_cwd.as_deref())
        .or(entry.cwd.as_deref())
        .unwrap_or_default();
    known_values.extend(bridge_core::mcp::env_redaction_values(&entry.mcp, cwd));
    DiagnosticRedactor::new(known_values)
}

fn authentication(
    entry: &bridge_core::domain::AgentEntry,
    redactor: &DiagnosticRedactor,
) -> AuthenticationRecord {
    if entry.kind == AgentKind::Api {
        if let Some(name) = &entry.api_key_env {
            return AuthenticationRecord::ApiKeyEnv {
                name: redactor.sanitize_diagnostic_id(name.clone()),
                present: std::env::var_os(name).is_some(),
            };
        }
        return AuthenticationRecord::NotApplicable;
    }
    if entry.pre_authenticated {
        AuthenticationRecord::PreAuthenticated
    } else if let Some(method) = &entry.auth_method {
        AuthenticationRecord::ConfiguredMethod {
            method: redactor.sanitize_diagnostic_id(method.clone()),
        }
    } else {
        AuthenticationRecord::Automatic
    }
}

fn artifact_provenance(
    snapshot: &bridge_core::domain::RegistrySnapshot,
    agent: &AgentId,
) -> Vec<doctor::CheckResult> {
    let mut rows = doctor::provenance_rows_for_agent(snapshot, agent);
    let auth_check = format!("provenance:{}:auth", agent.as_str());
    for row in &mut rows {
        if row.check == auth_check {
            // The doctor row contains arbitrary configured ids as human-readable text. The smoke
            // artifact carries the same path separately through tagged all-or-nothing ids.
            row.detail = "authentication path recorded in target.authentication".into();
        }
    }
    rows
}

fn effective_record(config: &EffectiveConfig) -> EffectiveConfigRecord {
    EffectiveConfigRecord {
        model: config.model.clone(),
        effort: config.effort.as_ref().map(crate::effort_to_string),
        mode: config.mode.clone(),
    }
}

fn canonical_session_cwd(path: &Path) -> Result<SessionCwd, ()> {
    let canonical = std::fs::canonicalize(path).map_err(|_| ())?;
    if !canonical.is_dir() {
        return Err(());
    }
    SessionCwd::parse(&canonical.to_string_lossy()).map_err(|_| ())
}

async fn cleanup_backend(
    backend: Arc<dyn AgentBackend>,
    session: &SessionId,
    observer: Arc<InMemoryDiagnosticObserver>,
    cancel_first: bool,
) -> CleanupOutcome {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(CLEANUP_TIMEOUT_SECS);
    cleanup_backend_until(backend, session, observer, cancel_first, deadline).await
}

async fn cleanup_backend_until(
    backend: Arc<dyn AgentBackend>,
    session: &SessionId,
    observer: Arc<InMemoryDiagnosticObserver>,
    cancel_first: bool,
    deadline: tokio::time::Instant,
) -> CleanupOutcome {
    let (cancel, cancel_error) = if cancel_first {
        cleanup_step(tokio::time::timeout_at(deadline, backend.cancel(session)).await)
    } else {
        ("not_needed", None)
    };
    let observer: Arc<dyn bridge_core::ports::DiagnosticObserver> = observer;
    let (release, release_error) = cleanup_step(
        tokio::time::timeout_at(
            deadline,
            backend.release_session_observed(session, observer),
        )
        .await,
    );
    let (retire, retire_error) =
        cleanup_step(tokio::time::timeout_at(deadline, backend.retire()).await);
    CleanupOutcome {
        record: CleanupRecord {
            grace_timeout_secs: CLEANUP_TIMEOUT_SECS,
            cancel,
            release,
            retire,
            run_scoped_backstop: "invoked_best_effort",
        },
        error: cancel_error.or(release_error).or(retire_error),
    }
}

fn cleanup_step(
    result: Result<Result<(), BridgeError>, tokio::time::error::Elapsed>,
) -> (&'static str, Option<BridgeError>) {
    match result {
        Ok(Ok(())) => ("completed", None),
        Ok(Err(error)) => ("failed", Some(error)),
        Err(_) => ("timed_out", Some(BridgeError::AgentTimedOut)),
    }
}

fn apply_cleanup_outcome(state: &mut ArtifactState, primary_failed: bool, cleanup: CleanupOutcome) {
    state.artifact.cleanup = cleanup.record;
    if primary_failed {
        if cleanup.error.is_some() {
            state.failure = state
                .failure
                .take()
                .map(FailureDiagnostic::with_secondary_teardown_marker);
        }
    } else if let Some(error) = cleanup.error {
        state.artifact.success = false;
        if matches!(
            error,
            BridgeError::AgentTimedOut | BridgeError::CancelTimeout
        ) {
            state.fail_static(
                DiagnosticPhase::Teardown,
                DiagnosticFailureClass::Timeout,
                "smoke.cleanup_timeout",
                "Smoke cleanup grace timed out",
                true,
            );
        } else {
            state.fail_error(&error, DiagnosticPhase::Teardown, true);
        }
    }
}

async fn run_attempt(args: &SmokeArgs) -> SmokeArtifactV2 {
    let mut state = ArtifactState::new(args);

    let config_file = match crate::local_file::read_regular_file_bounded(
        &args.config,
        "smoke config",
        MAX_CONFIG_BYTES,
    ) {
        Ok(snapshot) => snapshot,
        Err(_) => {
            state.fail_static(
                DiagnosticPhase::Resolve,
                DiagnosticFailureClass::Config,
                "smoke.config_path",
                "Smoke config must be one bounded regular file",
                false,
            );
            return state.finalize(args.include_redacted_stderr).await;
        }
    };
    let config_path = config_file.canonical_path.clone();
    state.artifact.request.canonical_config_path = Some(config_path.to_string_lossy().into_owned());
    state.artifact.request.config_sha256 = Some(config_file.sha256.clone());

    if let Some(guard) = &args.fallback_guard {
        if guard.expected_config_sha256 != config_file.sha256 {
            state.fail_static(
                DiagnosticPhase::Resolve,
                DiagnosticFailureClass::Config,
                "smoke.fallback_config_drift",
                "Fallback smoke config changed after planning",
                false,
            );
            return state.finalize(args.include_redacted_stderr).await;
        }
        let executable = std::env::current_exe().ok().and_then(|path| {
            crate::local_file::read_regular_file_bounded(
                &path,
                "smoke executable",
                MAX_EXECUTABLE_BYTES,
            )
            .ok()
        });
        if executable.as_ref().map(|file| file.sha256.as_str())
            != Some(guard.expected_executable_sha256.as_str())
        {
            state.fail_static(
                DiagnosticPhase::Resolve,
                DiagnosticFailureClass::Config,
                "smoke.fallback_executable_drift",
                "Fallback smoke executable changed after planning",
                false,
            );
            return state.finalize(args.include_redacted_stderr).await;
        }
    }

    let raw = match std::str::from_utf8(&config_file.bytes) {
        Ok(raw) => raw,
        Err(_) => {
            state.fail_static(
                DiagnosticPhase::Resolve,
                DiagnosticFailureClass::Config,
                "smoke.config_utf8",
                "Smoke config is not valid UTF-8",
                false,
            );
            return state.finalize(args.include_redacted_stderr).await;
        }
    };
    let snapshot = match crate::validate_registry_config_contents(raw) {
        Ok(snapshot) => snapshot,
        Err(_) => {
            state.fail_static(
                DiagnosticPhase::Resolve,
                DiagnosticFailureClass::Config,
                "smoke.config_load",
                "Smoke config could not be loaded",
                false,
            );
            return state.finalize(args.include_redacted_stderr).await;
        }
    };

    let entry = match snapshot.entries.iter().find(|entry| entry.id == args.agent) {
        Some(entry) => entry.clone(),
        None => {
            state.fail_static(
                DiagnosticPhase::Resolve,
                DiagnosticFailureClass::Config,
                "smoke.unknown_agent",
                "Selected smoke agent is not configured",
                false,
            );
            return state.finalize(args.include_redacted_stderr).await;
        }
    };
    if args.fallback_guard.is_some()
        && (!entry.host_fallback_eligible
            || !matches!(entry.kind, AgentKind::Acp)
            || entry.sandbox.is_some())
    {
        state.fail_static(
            DiagnosticPhase::Resolve,
            DiagnosticFailureClass::Config,
            "smoke.fallback_target_drift",
            "Fallback smoke target is no longer an eligible host ACP entry",
            false,
        );
        return state.finalize(args.include_redacted_stderr).await;
    }
    let session_cwd = match args.session_cwd.as_deref() {
        Some(path) => match canonical_session_cwd(path) {
            Ok(cwd) => {
                state.artifact.request.session_cwd = Some(cwd.as_str().to_owned());
                Some(cwd)
            }
            Err(()) => {
                state.fail_static(
                    DiagnosticPhase::ConfigApply,
                    DiagnosticFailureClass::Config,
                    "smoke.session_cwd",
                    "Smoke session cwd is not an existing directory",
                    false,
                );
                return state.finalize(args.include_redacted_stderr).await;
            }
        },
        None => None,
    };
    if let Some(guard) = &args.fallback_guard {
        let source = snapshot
            .entries
            .iter()
            .find(|entry| entry.id == guard.source_agent);
        let source_cwd = source
            .filter(|entry| execution_mode(entry) == "container_ro")
            .and_then(|entry| entry.sandbox.as_ref())
            .and_then(|sandbox| canonical_session_cwd(Path::new(&sandbox.mount)).ok());
        if !session_cwd
            .as_ref()
            .zip(source_cwd.as_ref())
            .is_some_and(|(cwd, root)| cwd.is_under(root))
        {
            state.fail_static(
                DiagnosticPhase::ConfigApply,
                DiagnosticFailureClass::Config,
                "smoke.fallback_source_drift",
                "Fallback smoke cwd is not within the current source container mount",
                false,
            );
            return state.finalize(args.include_redacted_stderr).await;
        }
    }

    let artifact_redactor = artifact_redactor(&entry, session_cwd.as_ref());
    state.artifact.target = Some(TargetRecord {
        execution_mode: execution_mode(&entry),
        provenance: artifact_provenance(&snapshot, &args.agent),
        authentication: authentication(&entry, &artifact_redactor),
    });

    let overrides = AgentOverride {
        model: args.model.clone(),
        effort: args.effort,
        mode: args.mode.clone(),
    };
    let effective = effective_config(&entry, Some(&overrides));
    let spec = SessionSpec {
        config: effective.clone(),
        cwd: session_cwd,
    };
    state.artifact.session.effective_request = Some(effective_record(&effective));

    let host = bridge_core::liveness::host_id();
    let instance_id = format!("{}-{}", std::process::id(), crate::implement::nonce(8));
    let lease = match bridge_core::liveness::acquire_lease(&instance_id) {
        Ok(lease) => lease,
        Err(_) => {
            state.fail_static(
                DiagnosticPhase::Resolve,
                DiagnosticFailureClass::Persistence,
                "smoke.lease",
                "Smoke run lease could not be acquired",
                false,
            );
            return state.finalize(args.include_redacted_stderr).await;
        }
    };
    let run = bridge_core::run_identity::RunHandle {
        instance_id: instance_id.clone(),
        host: host.clone(),
        lease: lease.path().to_string_lossy().into_owned(),
        start: epoch_secs(),
    };
    // A guarded fallback target is already proven to be unsandboxed ACP. Do not consult the degraded
    // container runtime while attempting to verify the independent host lane; this run cannot create a
    // container, so it needs neither orphan recovery nor a container run-end sweep.
    let run_guard = if args.fallback_guard.is_none() {
        recover_orphans(&snapshot, &config_path, &host);
        Some(RunEndGuard {
            runtimes: run_guard_runtimes(&snapshot, &config_path),
            instance_id,
        })
    } else {
        None
    };
    let policy: Arc<dyn PolicyEngine> = Arc::new(DenyAllPolicy);
    let spawn = make_spawn_fn(policy, config_path, run, None, 1, None);
    let registry = match Registry::new_observed(snapshot, spawn) {
        Ok(registry) => Arc::new(registry),
        Err(error) => {
            state.fail_error(&error, DiagnosticPhase::Resolve, false);
            if run_guard.is_some() {
                state.artifact.cleanup.run_scoped_backstop = "invoked_best_effort";
            }
            drop(run_guard);
            drop(lease);
            return state.finalize(args.include_redacted_stderr).await;
        }
    };

    // One absolute turn deadline covers resolve/spawn, session config/mint, prompt installation, and
    // terminal drain. Cleanup has its own short bounded grace so a timeout can still reap ownership.
    state.artifact.attempt.started_at_ms = diagnostic_timestamp_ms();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(args.timeout_secs);
    let observer_dyn: Arc<dyn bridge_core::ports::DiagnosticObserver> = state.observer.clone();
    let resolved = match resolve_once(registry.as_ref(), &args.agent, observer_dyn, deadline).await
    {
        ResolveOnce::TimedOut => {
            state.artifact.attempt.timed_out = true;
            state.fail_static(
                DiagnosticPhase::Resolve,
                DiagnosticFailureClass::Timeout,
                "smoke.resolve_timeout",
                "Smoke agent resolution timed out",
                false,
            );
            registry.invalidate(&args.agent).await;
            if run_guard.is_some() {
                state.artifact.cleanup.run_scoped_backstop = "invoked_best_effort";
            }
            drop(registry);
            drop(run_guard);
            drop(lease);
            return state.finalize(args.include_redacted_stderr).await;
        }
        ResolveOnce::Failed(error) => {
            state.artifact.attempt.timed_out = is_timeout_failure(&error);
            state.fail_error(&error, DiagnosticPhase::Resolve, false);
            if run_guard.is_some() {
                state.artifact.cleanup.run_scoped_backstop = "invoked_best_effort";
            }
            drop(registry);
            drop(run_guard);
            drop(lease);
            return state.finalize(args.include_redacted_stderr).await;
        }
        ResolveOnce::Resolved(resolved) => resolved,
    };

    let backend = Arc::clone(&resolved.backend);
    let session = SessionId::parse(state.artifact.session.id.clone())
        .expect("generated non-empty smoke session id");
    state.artifact.session.configure_calls = 1;
    let rich = Arc::new(SmokeRichSink::default());
    let result = execute_one(
        Arc::clone(&backend),
        &session,
        &spec,
        Arc::clone(&state.observer),
        rich,
        deadline,
    )
    .await;
    state.artifact.turn = result.turn;

    if let Some(error) = &result.error {
        state.artifact.attempt.timed_out = is_timeout_failure(error);
        let accepted = state.artifact.turn.prompt_calls == 1;
        let phase = turn_failure_phase(&state.artifact.turn);
        state.fail_turn_error(error, phase, accepted);
    } else {
        state.artifact.success = true;
        state.artifact.attempt.prompt_may_have_been_accepted = true;
    }

    let cleanup = cleanup_backend(
        backend,
        &session,
        Arc::clone(&state.observer),
        result.error.is_some(),
    )
    .await;
    apply_cleanup_outcome(&mut state, result.error.is_some(), cleanup);
    drop(resolved);
    drop(registry);
    drop(run_guard);
    drop(lease);
    state.finalize(args.include_redacted_stderr).await
}

fn prepare_artifact_file(out: Option<&Path>, config: &Path) -> Result<Option<File>, BoxError> {
    out.map(|path| {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options.open(path).map_err(|error| -> BoxError {
            format!(
                "smoke: cannot open artifact {} before attempt: {error}",
                path.display()
            )
            .into()
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let (Ok(config), Ok(artifact)) = (std::fs::metadata(config), file.metadata()) {
                if config.dev() == artifact.dev() && config.ino() == artifact.ino() {
                    return Err("smoke: --out must not alias --config".into());
                }
            }
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|error| -> BoxError {
                    format!(
                        "smoke: cannot restrict artifact {} before attempt: {error}",
                        path.display()
                    )
                    .into()
                })?;
        }
        #[cfg(not(unix))]
        {
            if artifact_aliases_config(config, path) {
                return Err("smoke: --out must not alias --config".into());
            }
        }
        Ok(file)
    })
    .transpose()
}

fn artifact_aliases_config(config: &Path, artifact: &Path) -> bool {
    if lexical_absolute(config)
        .zip(lexical_absolute(artifact))
        .is_some_and(|(config, artifact)| config == artifact)
    {
        return true;
    }
    if let (Ok(config), Ok(artifact)) = (
        std::fs::canonicalize(config),
        std::fs::canonicalize(artifact),
    ) {
        if config == artifact {
            return true;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(config), Ok(artifact)) = (std::fs::metadata(config), std::fs::metadata(artifact))
        {
            return config.dev() == artifact.dev() && config.ino() == artifact.ino();
        }
    }
    false
}

fn lexical_absolute(path: &Path) -> Option<PathBuf> {
    use std::path::Component;
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return None;
                }
            }
        }
    }
    Some(normalized)
}

fn write_artifact(artifact: &SmokeArtifactV2, out: Option<&mut File>) -> Result<(), BoxError> {
    match out {
        Some(file) => {
            serde_json::to_writer_pretty(&mut *file, artifact)?;
            file.write_all(b"\n")?;
            file.flush()?;
        }
        None => {
            let stdout = std::io::stdout();
            let mut lock = stdout.lock();
            serde_json::to_writer_pretty(&mut lock, artifact)?;
            lock.write_all(b"\n")?;
            lock.flush()?;
        }
    }
    Ok(())
}

pub(crate) async fn smoke_cmd(args: &[String]) -> Result<(), BoxError> {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
    {
        println!("{SMOKE_USAGE}");
        return Ok(());
    }
    let args = parse_args(args)?;
    if args
        .out
        .as_deref()
        .is_some_and(|out| artifact_aliases_config(&args.config, out))
    {
        return Err("smoke: --out must not alias --config".into());
    }
    // Prove the selected evidence destination is writable before a provider process/request can exist.
    let mut artifact_file = prepare_artifact_file(args.out.as_deref(), &args.config)?;
    bridge_observ::init_stderr();
    let artifact = run_attempt(&args).await;
    let success = artifact.success;
    write_artifact(&artifact, artifact_file.as_mut())?;
    if success {
        Ok(())
    } else {
        Err("smoke: attempt failed; inspect the emitted artifact".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::diagnostics::{StderrRedaction, StderrScope};
    use bridge_core::ports::BackendStream;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;
    use std::time::Instant;

    #[derive(Clone, Copy)]
    enum Behavior {
        Exact,
        Whitespace,
        NoTerminal,
        Wrong,
        Tool,
        Permission,
        Cancelled,
        Refusal,
        Empty,
        Silent,
        ConfigFailure,
        PromptFailure,
    }

    struct FakeBackend {
        behavior: Behavior,
        configure_calls: AtomicUsize,
        prompt_calls: AtomicUsize,
        cancel_calls: AtomicUsize,
        release_calls: AtomicUsize,
        retire_calls: AtomicUsize,
        hanging_release: bool,
        failing_release: bool,
        release_failure_accepted: bool,
        prompts: Mutex<Vec<String>>,
    }

    impl FakeBackend {
        fn new(behavior: Behavior) -> Self {
            Self {
                behavior,
                configure_calls: AtomicUsize::new(0),
                prompt_calls: AtomicUsize::new(0),
                cancel_calls: AtomicUsize::new(0),
                release_calls: AtomicUsize::new(0),
                retire_calls: AtomicUsize::new(0),
                hanging_release: false,
                failing_release: false,
                release_failure_accepted: true,
                prompts: Mutex::new(Vec::new()),
            }
        }

        fn with_hanging_release(mut self) -> Self {
            self.hanging_release = true;
            self
        }

        fn with_unaccepted_failing_release(mut self) -> Self {
            self.failing_release = true;
            self.release_failure_accepted = false;
            self
        }
    }

    #[async_trait::async_trait]
    impl AgentBackend for FakeBackend {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            unreachable!("smoke always supplies both observer channels")
        }

        async fn prompt_with_observers(
            &self,
            _session: &SessionId,
            parts: Vec<Part>,
            observers: BackendObservers,
        ) -> Result<BackendStream, BridgeError> {
            self.prompt_calls.fetch_add(1, Ordering::SeqCst);
            self.prompts
                .lock()
                .unwrap()
                .push(parts.into_iter().map(|part| part.text).collect());
            if matches!(self.behavior, Behavior::PromptFailure) {
                return Err(BridgeError::FrameError);
            }
            let stream: BackendStream = match self.behavior {
                Behavior::Exact => Box::pin(futures::stream::iter(vec![
                    Ok(Update::Text("PONG".into())),
                    Ok(Update::Done {
                        stop_reason: "end_turn".into(),
                    }),
                ])),
                Behavior::Whitespace => Box::pin(futures::stream::iter(vec![
                    Ok(Update::Text("  PONG\n".into())),
                    Ok(Update::Done {
                        stop_reason: "stop".into(),
                    }),
                ])),
                Behavior::NoTerminal => {
                    Box::pin(futures::stream::iter(vec![Ok(Update::Text("PONG".into()))]))
                }
                Behavior::Wrong => Box::pin(futures::stream::iter(vec![
                    Ok(Update::Text("not pong".into())),
                    Ok(Update::Done {
                        stop_reason: "end_turn".into(),
                    }),
                ])),
                Behavior::Tool => {
                    observers
                        .rich
                        .expect("smoke rich observer")
                        .record(OrchEventKind::ToolCall {
                            tool_call_id: "opaque".into(),
                            title: "tool".into(),
                            kind: "other".into(),
                            status: "pending".into(),
                            locations: Vec::new(),
                            content: None,
                        });
                    Box::pin(futures::stream::iter(vec![
                        Ok(Update::Text("PONG".into())),
                        Ok(Update::Done {
                            stop_reason: "end_turn".into(),
                        }),
                    ]))
                }
                Behavior::Permission => Box::pin(futures::stream::iter(vec![
                    Ok(Update::Permission(PermissionRequest::read())),
                    Ok(Update::Text("PONG".into())),
                    Ok(Update::Done {
                        stop_reason: "end_turn".into(),
                    }),
                ])),
                Behavior::Cancelled => Box::pin(futures::stream::iter(vec![
                    Ok(Update::Text("PONG".into())),
                    Ok(Update::Done {
                        stop_reason: "cancelled".into(),
                    }),
                ])),
                Behavior::Refusal => Box::pin(futures::stream::iter(vec![
                    Ok(Update::Text("PONG".into())),
                    Ok(Update::Done {
                        stop_reason: "refusal".into(),
                    }),
                ])),
                Behavior::Empty => Box::pin(futures::stream::empty()),
                Behavior::Silent => Box::pin(futures::stream::pending()),
                Behavior::ConfigFailure => unreachable!("configure rejects first"),
                Behavior::PromptFailure => unreachable!("prompt rejects before stream"),
            };
            Ok(stream)
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            self.cancel_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn release_session_observed(
            &self,
            _session: &SessionId,
            _observer: Arc<dyn bridge_core::ports::DiagnosticObserver>,
        ) -> Result<(), BridgeError> {
            self.release_calls.fetch_add(1, Ordering::SeqCst);
            if self.hanging_release {
                futures::future::pending::<()>().await;
            }
            if self.failing_release {
                Err(BridgeError::agent_failure(static_failure(
                    DiagnosticPhase::Teardown,
                    DiagnosticFailureClass::Transport,
                    "smoke.test_teardown",
                    "Teardown failed",
                    self.release_failure_accepted,
                )))
            } else {
                Ok(())
            }
        }

        async fn retire(&self) -> Result<(), BridgeError> {
            self.retire_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn configure_session(
            &self,
            _session: &SessionId,
            _spec: &SessionSpec,
        ) -> Result<(), BridgeError> {
            self.configure_calls.fetch_add(1, Ordering::SeqCst);
            if matches!(self.behavior, Behavior::ConfigFailure) {
                Err(BridgeError::ModelNotAvailable)
            } else {
                Ok(())
            }
        }
    }

    fn args() -> SmokeArgs {
        SmokeArgs {
            agent: AgentId::parse("test").unwrap(),
            config: PathBuf::from("/tmp/config.toml"),
            model: None,
            effort: None,
            mode: None,
            session_cwd: None,
            timeout_secs: 1,
            include_redacted_stderr: false,
            out: None,
            fallback_guard: None,
        }
    }

    fn cli_args(extra: &[&str]) -> Vec<String> {
        let mut args = vec![
            "--agent".to_string(),
            "test".to_string(),
            "--config".to_string(),
            "/tmp/config.toml".to_string(),
            "--acknowledge-billable".to_string(),
        ];
        args.extend(extra.iter().map(|value| (*value).to_owned()));
        args
    }

    fn observer() -> Arc<InMemoryDiagnosticObserver> {
        Arc::new(InMemoryDiagnosticObserver::new(16).unwrap())
    }

    async fn execute(behavior: Behavior, timeout: Duration) -> (Arc<FakeBackend>, DrainResult) {
        let backend = Arc::new(FakeBackend::new(behavior));
        let backend_dyn: Arc<dyn AgentBackend> = backend.clone();
        let result = execute_one(
            backend_dyn,
            &SessionId::parse("smoke-test").unwrap(),
            &SessionSpec::from_config(EffectiveConfig::default()),
            observer(),
            Arc::new(SmokeRichSink::default()),
            tokio::time::Instant::now() + timeout,
        )
        .await;
        (backend, result)
    }

    #[test]
    fn acknowledgement_and_argument_validation_are_pure_and_strict() {
        let no_ack = vec![
            "--agent".into(),
            "test".into(),
            "--config".into(),
            "/definitely/not/read".into(),
        ];
        assert!(parse_args(&no_ack)
            .unwrap_err()
            .to_string()
            .contains("--acknowledge-billable"));

        for bad in [
            vec!["--timeout-secs", "0"],
            vec!["--timeout-secs", "901"],
            vec!["--timeout-secs", "NaN"],
            vec!["--effort", "turbo"],
            vec!["--model", ""],
            vec!["--mode", " padded "],
            vec!["--out", "-"],
            vec!["--unknown", "x"],
        ] {
            assert!(parse_args(&cli_args(&bad)).is_err(), "accepted {bad:?}");
        }
        assert_eq!(
            parse_args(&cli_args(&[])).unwrap().timeout_secs,
            DEFAULT_TIMEOUT_SECS
        );

        let digest = "0000000000000000000000000000000000000000000000000000000000000000";
        assert!(parse_args(&cli_args(&["--expected-config-sha256", digest])).is_err());
        assert!(parse_args(&cli_args(&[
            "--expected-config-sha256",
            digest,
            "--expected-executable-sha256",
            digest,
            "--fallback-source-agent",
            "source",
            "--require-host-fallback-eligible",
        ]))
        .is_err());
        let guarded = parse_args(&cli_args(&[
            "--session-cwd",
            "/tmp",
            "--expected-config-sha256",
            digest,
            "--expected-executable-sha256",
            digest,
            "--fallback-source-agent",
            "source",
            "--require-host-fallback-eligible",
        ]))
        .unwrap();
        assert!(guarded.fallback_guard.is_some());

        let consumed_ack = vec![
            "--agent".into(),
            "test".into(),
            "--config".into(),
            "/tmp/config.toml".into(),
            "--model".into(),
            "--acknowledge-billable".into(),
        ];
        assert!(
            parse_args(&consumed_ack).is_err(),
            "an acknowledgement consumed as a value is not an acknowledgement flag"
        );
    }

    #[tokio::test]
    async fn exact_pong_terminal_is_one_configure_and_one_fixed_prompt() {
        let (backend, result) = execute(Behavior::Exact, Duration::from_secs(1)).await;
        assert!(result.error.is_none());
        assert!(result.turn.exact_pong);
        assert_eq!(result.turn.terminal_state, "completed");
        assert_eq!(backend.configure_calls.load(Ordering::SeqCst), 1);
        assert_eq!(backend.prompt_calls.load(Ordering::SeqCst), 1);
        assert_eq!(backend.prompts.lock().unwrap().as_slice(), [FIXED_PROMPT]);
    }

    #[tokio::test]
    async fn protocol_whitespace_is_normalized_but_no_other_success_condition_is_relaxed() {
        let (_, whitespace) = execute(Behavior::Whitespace, Duration::from_secs(1)).await;
        assert!(whitespace.error.is_none());
        assert!(whitespace.turn.exact_pong);

        for behavior in [
            Behavior::NoTerminal,
            Behavior::Wrong,
            Behavior::Tool,
            Behavior::Permission,
            Behavior::Cancelled,
            Behavior::Refusal,
            Behavior::Empty,
        ] {
            let (_, result) = execute(behavior, Duration::from_secs(1)).await;
            assert!(result.error.is_some());
        }
    }

    #[tokio::test]
    async fn terminal_contract_failures_keep_specific_static_classification() {
        for (behavior, code, class) in [
            (
                Behavior::Tool,
                "smoke.tool_attempt",
                DiagnosticFailureClass::Protocol,
            ),
            (
                Behavior::Permission,
                "smoke.permission_update",
                DiagnosticFailureClass::Protocol,
            ),
            (
                Behavior::Cancelled,
                "smoke.cancelled",
                DiagnosticFailureClass::Canceled,
            ),
            (
                Behavior::NoTerminal,
                "smoke.missing_terminal",
                DiagnosticFailureClass::Protocol,
            ),
            (
                Behavior::Wrong,
                "smoke.not_exact_pong",
                DiagnosticFailureClass::Protocol,
            ),
            (
                Behavior::Refusal,
                "smoke.non_success_terminal",
                DiagnosticFailureClass::Protocol,
            ),
        ] {
            let (_, result) = execute(behavior, Duration::from_secs(1)).await;
            let error = result.error.as_ref().expect("terminal case must fail");
            let mut state = ArtifactState::new(&args());
            state.artifact.turn = result.turn;
            state.fail_turn_error(error, DiagnosticPhase::PromptFinish, true);
            let diagnostic = state.failure.as_ref().unwrap();
            assert_eq!(diagnostic.code().as_str(), code);
            assert_eq!(diagnostic.class(), class);
        }
    }

    #[tokio::test]
    async fn config_rejection_never_calls_prompt() {
        let (backend, result) = execute(Behavior::ConfigFailure, Duration::from_secs(1)).await;
        assert!(matches!(result.error, Some(BridgeError::ModelNotAvailable)));
        assert_eq!(backend.configure_calls.load(Ordering::SeqCst), 1);
        assert_eq!(backend.prompt_calls.load(Ordering::SeqCst), 0);
        assert_eq!(result.turn.prompt_calls, 0);
    }

    #[tokio::test]
    async fn legacy_prompt_construction_failure_stays_at_prompt_start() {
        let (backend, result) = execute(Behavior::PromptFailure, Duration::from_secs(1)).await;
        assert!(matches!(result.error, Some(BridgeError::FrameError)));
        assert_eq!(result.turn.terminal_state, "prompt_failed");
        assert_eq!(
            turn_failure_phase(&result.turn),
            DiagnosticPhase::PromptStart
        );
        assert_eq!(backend.configure_calls.load(Ordering::SeqCst), 1);
        assert_eq!(backend.prompt_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn silent_backend_is_bounded_by_the_single_deadline() {
        let started = Instant::now();
        let (backend, result) = execute(Behavior::Silent, Duration::from_millis(25)).await;
        assert!(matches!(result.error, Some(BridgeError::AgentTimedOut)));
        assert_eq!(result.turn.terminal_state, "timeout");
        assert_eq!(backend.configure_calls.load(Ordering::SeqCst), 1);
        assert_eq!(backend.prompt_calls.load(Ordering::SeqCst), 1);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn cleanup_is_single_pass_and_uses_a_separate_bound() {
        let backend = Arc::new(FakeBackend::new(Behavior::Exact));
        let backend_dyn: Arc<dyn AgentBackend> = backend.clone();
        let outcome = cleanup_backend_until(
            backend_dyn,
            &SessionId::parse("cleanup").unwrap(),
            observer(),
            true,
            tokio::time::Instant::now() + Duration::from_secs(1),
        )
        .await;
        assert!(outcome.error.is_none());
        assert_eq!(outcome.record.cancel, "completed");
        assert_eq!(outcome.record.release, "completed");
        assert_eq!(outcome.record.retire, "completed");
        assert_eq!(backend.cancel_calls.load(Ordering::SeqCst), 1);
        assert_eq!(backend.release_calls.load(Ordering::SeqCst), 1);
        assert_eq!(backend.retire_calls.load(Ordering::SeqCst), 1);

        let hanging = Arc::new(FakeBackend::new(Behavior::Exact).with_hanging_release());
        let hanging_dyn: Arc<dyn AgentBackend> = hanging.clone();
        let started = Instant::now();
        let outcome = cleanup_backend_until(
            hanging_dyn,
            &SessionId::parse("cleanup-hang").unwrap(),
            observer(),
            true,
            tokio::time::Instant::now() + Duration::from_millis(25),
        )
        .await;
        assert!(matches!(outcome.error, Some(BridgeError::AgentTimedOut)));
        assert_eq!(outcome.record.cancel, "completed");
        assert_eq!(outcome.record.release, "timed_out");
        assert_eq!(outcome.record.retire, "completed");
        assert_eq!(hanging.cancel_calls.load(Ordering::SeqCst), 1);
        assert_eq!(hanging.release_calls.load(Ordering::SeqCst), 1);
        assert_eq!(hanging.retire_calls.load(Ordering::SeqCst), 1);
        assert!(started.elapsed() < Duration::from_secs(1));

        let mut state = ArtifactState::new(&args());
        state.artifact.success = true;
        state.artifact.attempt.prompt_may_have_been_accepted = true;
        apply_cleanup_outcome(&mut state, false, outcome);
        assert_eq!(
            state.failure.as_ref().unwrap().code().as_str(),
            "smoke.cleanup_timeout"
        );
    }

    #[tokio::test]
    async fn cleanup_failure_turns_a_successful_turn_into_terminal_failure() {
        let backend = Arc::new(FakeBackend::new(Behavior::Exact).with_unaccepted_failing_release());
        let backend_dyn: Arc<dyn AgentBackend> = backend;
        let cleanup = cleanup_backend_until(
            backend_dyn,
            &SessionId::parse("cleanup-failure").unwrap(),
            observer(),
            false,
            tokio::time::Instant::now() + Duration::from_secs(1),
        )
        .await;
        assert_eq!(cleanup.record.release, "failed");

        let mut state = ArtifactState::new(&args());
        state.artifact.success = true;
        state.artifact.attempt.prompt_may_have_been_accepted = true;
        apply_cleanup_outcome(&mut state, false, cleanup);
        assert!(!state.artifact.success);
        assert!(
            state.artifact.attempt.prompt_may_have_been_accepted,
            "a teardown-scoped false flag must never lower completed attempt acceptance"
        );
        assert_eq!(
            state.failure.as_ref().unwrap().code().as_str(),
            "smoke.test_teardown"
        );
    }

    #[tokio::test]
    async fn cleanup_failure_is_secondary_to_an_existing_primary_failure() {
        let backend = Arc::new(FakeBackend::new(Behavior::Exact).with_unaccepted_failing_release());
        let backend_dyn: Arc<dyn AgentBackend> = backend;
        let cleanup = cleanup_backend_until(
            backend_dyn,
            &SessionId::parse("cleanup-secondary").unwrap(),
            observer(),
            false,
            tokio::time::Instant::now() + Duration::from_secs(1),
        )
        .await;

        let mut state = ArtifactState::new(&args());
        state.failure = Some(static_failure(
            DiagnosticPhase::PromptStream,
            DiagnosticFailureClass::Transport,
            "smoke.test_primary",
            "Primary failed",
            true,
        ));
        apply_cleanup_outcome(&mut state, true, cleanup);
        let failure = state.failure.as_ref().unwrap();
        assert_eq!(failure.code().as_str(), "smoke.test_primary");
        assert_eq!(
            failure.causes().first().map(String::as_str),
            Some("teardown.secondary")
        );
    }

    struct FakeLease;
    impl bridge_core::ports::Lease for FakeLease {}

    struct FakeRegistry {
        resolves: AtomicUsize,
        backend: Arc<dyn AgentBackend>,
        hang: bool,
    }

    #[async_trait::async_trait]
    impl AgentRegistry for FakeRegistry {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
            self.resolves.fetch_add(1, Ordering::SeqCst);
            if self.hang {
                futures::future::pending::<()>().await;
            }
            Ok(Resolved {
                entry: Arc::new(test_entry(id.as_str())),
                backend: Arc::clone(&self.backend),
                lease: Box::new(FakeLease),
            })
        }

        fn default_id(&self) -> AgentId {
            AgentId::parse("test").unwrap()
        }

        async fn apply(
            &self,
            _snapshot: bridge_core::domain::RegistrySnapshot,
        ) -> Result<(), BridgeError> {
            Ok(())
        }

        fn list(&self) -> Vec<AgentId> {
            vec![self.default_id()]
        }
    }

    fn test_entry(id: &str) -> bridge_core::domain::AgentEntry {
        bridge_core::domain::AgentEntry {
            id: AgentId::parse(id).unwrap(),
            cmd: Some("fake".into()),
            base_url: None,
            api_key_env: None,
            args: Vec::new(),
            kind: AgentKind::Acp,
            model_provider: None,
            model: None,
            effort: None,
            mode: None,
            cwd: None,
            session_cwd: None,
            sandbox: None,
            watchdog: None,
            mcp: Vec::new(),
            mcp_delivery: Default::default(),
            auth_method: None,
            pre_authenticated: false,
            host_fallback_eligible: false,
            name: None,
            description: None,
            tags: Vec::new(),
            version: None,
            extensions: Default::default(),
        }
    }

    #[tokio::test]
    async fn resolve_is_called_once_and_a_silent_resolve_is_bounded() {
        let backend: Arc<dyn AgentBackend> = Arc::new(FakeBackend::new(Behavior::Exact));
        let registry = FakeRegistry {
            resolves: AtomicUsize::new(0),
            backend: Arc::clone(&backend),
            hang: false,
        };
        let diagnostic: Arc<dyn bridge_core::ports::DiagnosticObserver> = observer();
        assert!(matches!(
            resolve_once(
                &registry,
                &AgentId::parse("test").unwrap(),
                diagnostic,
                tokio::time::Instant::now() + Duration::from_secs(1)
            )
            .await,
            ResolveOnce::Resolved(_)
        ));
        assert_eq!(registry.resolves.load(Ordering::SeqCst), 1);

        let hanging = FakeRegistry {
            resolves: AtomicUsize::new(0),
            backend,
            hang: true,
        };
        let diagnostic: Arc<dyn bridge_core::ports::DiagnosticObserver> = observer();
        assert!(matches!(
            resolve_once(
                &hanging,
                &AgentId::parse("test").unwrap(),
                diagnostic,
                tokio::time::Instant::now() + Duration::from_millis(25)
            )
            .await,
            ResolveOnce::TimedOut
        ));
        assert_eq!(hanging.resolves.load(Ordering::SeqCst), 1);
    }

    fn stderr_failure() -> FailureDiagnostic {
        FailureDiagnostic::build_static_code(
            FailureDiagnosticInput {
                failed_phase: DiagnosticPhase::PromptStream,
                last_completed_phase: Some(DiagnosticPhase::PromptStart),
                class: DiagnosticFailureClass::Transport,
                disposition: FailureDisposition::Fatal,
                code: String::new(),
                summary: "Transport failed".into(),
                causes: Vec::new(),
                stderr_observed: true,
                stderr_line_count: 2,
                stderr_scope: Some(StderrScope::Process),
                stderr_tail: Some(vec!["redacted one".into(), "redacted two".into()]),
                stderr_redaction: Some(StderrRedaction::BestEffort),
                retry_after_ms: None,
                reset_at_ms: None,
                prompt_may_have_been_accepted: true,
            },
            "smoke.test_transport",
            &DiagnosticRedactor::default(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn default_artifact_keeps_stderr_metadata_but_removes_all_text() {
        let mut state = ArtifactState::new(&args());
        state.failure = Some(stderr_failure());
        let value = serde_json::to_value(state.finalize(false).await).unwrap();
        let rendered = serde_json::to_string(&value).unwrap();
        assert!(rendered.contains("stderr_observed"));
        assert!(rendered.contains("stderr_line_count"));
        assert!(!rendered.contains("stderr_tail"));
        assert!(!rendered.contains("redacted one"));
        assert_eq!(value["diagnostics"]["stderr_text"], "excluded");

        let mut nested = serde_json::json!({"event": {"failure": {
            "stderr_tail": ["must disappear"], "stderr_redaction": "best_effort",
            "stderr_observed": true
        }}});
        strip_stderr_text(&mut nested);
        assert_eq!(nested["event"]["failure"]["stderr_observed"], true);
        assert!(nested["event"]["failure"].get("stderr_tail").is_none());
    }

    #[test]
    fn artifact_auth_ids_are_all_or_nothing_and_doctor_text_is_not_copied() {
        let secret = "configured-auth-secret";
        let mut entry = test_entry("test");
        entry.auth_method = Some(secret.into());
        let redactor = DiagnosticRedactor::new([secret]);

        let auth = serde_json::to_value(authentication(&entry, &redactor)).unwrap();
        let rendered = serde_json::to_string(&auth).unwrap();
        assert_eq!(auth["path"], "configured_method");
        assert_eq!(auth["method"]["state"], "redacted");
        assert!(!rendered.contains(secret));

        let mut no_key_api = test_entry("api");
        no_key_api.kind = AgentKind::Api;
        no_key_api.cmd = None;
        no_key_api.base_url = Some("http://127.0.0.1:1/v1".into());
        assert_eq!(
            serde_json::to_value(authentication(&no_key_api, &DiagnosticRedactor::default()))
                .unwrap()["path"],
            "not_applicable"
        );

        let snapshot = bridge_core::domain::RegistrySnapshot {
            default: AgentId::parse("test").unwrap(),
            entries: vec![entry],
            allowed_cmds: vec!["fake".into()],
        };
        let provenance = artifact_provenance(&snapshot, &AgentId::parse("test").unwrap());
        let rendered = serde_json::to_string(&provenance).unwrap();
        assert!(rendered.contains("authentication path recorded in target.authentication"));
        assert!(!rendered.contains(secret));
    }

    #[tokio::test]
    async fn stderr_opt_in_is_bounded_and_labeled_best_effort() {
        let mut args = args();
        args.include_redacted_stderr = true;
        let mut state = ArtifactState::new(&args);
        state.failure = Some(stderr_failure());
        let value = serde_json::to_value(state.finalize(true).await).unwrap();
        assert_eq!(
            value["diagnostics"]["failure"]["stderr_tail"][0],
            "redacted one"
        );
        assert_eq!(value["diagnostics"]["stderr_text"], "best_effort");
    }

    #[tokio::test]
    async fn provider_limit_retry_metadata_survives_without_retrying() {
        let now = diagnostic_timestamp_ms();
        let diagnostic = FailureDiagnostic::build_at(
            FailureDiagnosticInput {
                failed_phase: DiagnosticPhase::PromptStream,
                last_completed_phase: Some(DiagnosticPhase::PromptStart),
                class: DiagnosticFailureClass::ProviderLimit,
                disposition: FailureDisposition::Fatal,
                code: "provider.limit".into(),
                summary: "Provider limit".into(),
                causes: Vec::new(),
                stderr_observed: false,
                stderr_line_count: 0,
                stderr_scope: None,
                stderr_tail: None,
                stderr_redaction: None,
                retry_after_ms: Some(1234),
                reset_at_ms: Some(now + 5000),
                prompt_may_have_been_accepted: true,
            },
            &DiagnosticRedactor::default(),
            now,
        )
        .unwrap();
        let mut state = ArtifactState::new(&args());
        state.fail_error(
            &BridgeError::agent_failure(diagnostic),
            DiagnosticPhase::PromptStream,
            true,
        );
        let value = serde_json::to_value(state.finalize(false).await).unwrap();
        assert_eq!(value["diagnostics"]["failure"]["retry_after_ms"], 1234);
        assert_eq!(value["diagnostics"]["failure"]["reset_at_ms"], now + 5000);
        assert_eq!(value["success"], false);
    }

    #[test]
    fn structured_timeout_is_reflected_in_attempt_timeout_state() {
        let timeout = BridgeError::agent_failure(static_failure(
            DiagnosticPhase::PromptStream,
            DiagnosticFailureClass::Timeout,
            "smoke.test_timeout",
            "Timed out",
            true,
        ));
        let provider_limit = BridgeError::agent_failure(static_failure(
            DiagnosticPhase::PromptStream,
            DiagnosticFailureClass::ProviderLimit,
            "smoke.test_limit",
            "Provider limit",
            true,
        ));
        assert!(is_timeout_failure(&timeout));
        assert!(!is_timeout_failure(&provider_limit));
        let BridgeError::AgentFailure { diagnostic } = timeout else {
            unreachable!();
        };
        assert_eq!(
            diagnostic.last_completed_phase(),
            Some(DiagnosticPhase::PromptStart)
        );
    }

    #[test]
    fn usage_cost_preserves_safe_non_usd_currency_and_collapses_unsafe_labels() {
        let cad = smoke_usage(UsageSnapshot {
            cost: Some(bridge_core::orch::UsageCost {
                amount: 1.25,
                currency: "CAD".into(),
            }),
            ..UsageSnapshot::default()
        });
        assert_eq!(cad.cost.unwrap().currency, "CAD");

        let unsafe_label = smoke_usage(UsageSnapshot {
            cost: Some(bridge_core::orch::UsageCost {
                amount: 1.25,
                currency: "USD\nsecret".into(),
            }),
            ..UsageSnapshot::default()
        });
        assert_eq!(unsafe_label.cost.unwrap().currency, "unknown");
    }

    #[tokio::test]
    async fn output_file_contains_one_machine_readable_artifact() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("smoke.json");
        let artifact = ArtifactState::new(&args()).finalize(false).await;
        let mut file = prepare_artifact_file(Some(&path), Path::new("/not-the-config"))
            .unwrap()
            .unwrap();
        #[cfg(unix)]
        assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        write_artifact(&artifact, Some(&mut file)).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["schema_version"], 2);
        assert_eq!(value["cleanup"]["grace_timeout_secs"], CLEANUP_TIMEOUT_SECS);
        assert!(bytes.ends_with(b"\n"));
    }

    #[test]
    fn preexisting_output_is_refused_without_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("smoke.json");
        std::fs::write(&path, b"stale artifact").unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let error = prepare_artifact_file(Some(&path), Path::new("/not-the-config"))
            .expect_err("an existing output path must be refused");

        assert!(error.to_string().contains("cannot open artifact"));
        assert_eq!(std::fs::read(&path).unwrap(), b"stale artifact");
        #[cfg(unix)]
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }
}
