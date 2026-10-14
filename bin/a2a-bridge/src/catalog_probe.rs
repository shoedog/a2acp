//! Host-side model-catalog probe: discover each configured agent's advertised models/effort/modes.
//!
//! The advertised list is account/adapter-driven and SANDBOX-INDEPENDENT (spec §2), so every probe runs
//! HOST-SIDE (the agent's `[sandbox]` is ignored). Strategy is chosen by `(kind, cmd)`:
//! - `kind=api` → OpenAI `GET {base_url}/models`;
//! - `cmd` basename `kiro-cli` → native `kiro-cli chat --list-models` (auth-free; its ACP handshake times
//!   out host-side because host kiro is unauthed — see spec §2);
//! - else (claude/codex) → a clean ACP `session/new` via [`AcpBackend::describe_options`].
//!
//! Every probe is bounded by [`PROBE_TIMEOUT`]. Server-side [`probe_all`] still degrades per-agent to the
//! success-only Agent Card catalog, while [`probe_all_report`] retains bounded, redacted failures for the
//! operator-facing `models` CLI. Serve startup uses [`probe_all_at_startup`], which re-probes the failed
//! agents once after [`STARTUP_RETRY_DELAY`]; every failed probe is logged at WARN.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bridge_acp::acp_backend::{AcpBackend, AcpConfig};
use bridge_core::catalog::{
    parse_kiro_list_models, parse_ollama_models, sanitize_model_caps, AgentCaps, ModelCatalog,
};
use bridge_core::diagnostics::{DiagnosticRedactor, FailureDiagnostic};
use bridge_core::domain::{AgentEntry, AgentKind};
use bridge_core::ports::{AgentBackend, DiagnosticObserver};
use serde::Serialize;

/// Per-agent probe bound. The kiro host-side ACP hang made this load-bearing (spec §2): without it a
/// single unresponsive adapter would block startup/CLI forever.
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
const PROCESS_STDERR_LINE_BYTES: usize = 4 * 1024;
/// Pause before serve startup's one re-probe of the agents whose first probe failed. Short and fixed:
/// it only gives an adapter that lost a cold-start race a moment to settle before the listener binds.
const STARTUP_RETRY_DELAY: Duration = Duration::from_secs(3);
/// `attempt` values on the probe log events: every probe is attempt 1; only the startup retry is 2.
const FIRST_ATTEMPT: u32 = 1;
const STARTUP_RETRY_ATTEMPT: u32 = 2;

/// Which discovery strategy an entry uses. Pure (kind/cmd only) so the dispatch decision is unit-tested
/// without spawning a real adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Strategy {
    Api,
    Kiro,
    Acp,
}

impl Strategy {
    fn as_str(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Kiro => "kiro",
            Self::Acp => "acp",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CatalogProbePhase {
    Spawn,
    Discovery,
    Timeout,
}

/// Stable operator-facing failure category. This is additive to [`CatalogProbePhase`]: phase says
/// where discovery stopped, while category distinguishes failures that share that phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CatalogProbeCategory {
    Spawn,
    Timeout,
    ProcessExit,
    ProviderAcp,
    ResponseParse,
    Transport,
    Configuration,
}

impl CatalogProbeCategory {
    fn as_str(self) -> &'static str {
        match self {
            Self::Spawn => "spawn",
            Self::Timeout => "timeout",
            Self::ProcessExit => "process_exit",
            Self::ProviderAcp => "provider_acp",
            Self::ResponseParse => "response_parse",
            Self::Transport => "transport",
            Self::Configuration => "configuration",
        }
    }
}

impl CatalogProbePhase {
    fn as_str(self) -> &'static str {
        match self {
            Self::Spawn => "spawn",
            Self::Discovery => "discovery",
            Self::Timeout => "timeout",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProbeError {
    phase: CatalogProbePhase,
    category: CatalogProbeCategory,
    reason: String,
    diagnostic: Option<FailureDiagnostic>,
}

impl ProbeError {
    fn spawn(reason: impl Into<String>) -> Self {
        Self {
            phase: CatalogProbePhase::Spawn,
            category: CatalogProbeCategory::Spawn,
            reason: reason.into(),
            diagnostic: None,
        }
    }

    fn configuration(reason: impl Into<String>) -> Self {
        Self {
            phase: CatalogProbePhase::Discovery,
            category: CatalogProbeCategory::Configuration,
            reason: reason.into(),
            diagnostic: None,
        }
    }

    fn transport(reason: impl Into<String>) -> Self {
        Self {
            phase: CatalogProbePhase::Discovery,
            category: CatalogProbeCategory::Transport,
            reason: reason.into(),
            diagnostic: None,
        }
    }

    fn process_exit(reason: impl Into<String>) -> Self {
        Self {
            phase: CatalogProbePhase::Discovery,
            category: CatalogProbeCategory::ProcessExit,
            reason: reason.into(),
            diagnostic: None,
        }
    }

    fn provider_acp(reason: impl Into<String>) -> Self {
        Self {
            phase: CatalogProbePhase::Discovery,
            category: CatalogProbeCategory::ProviderAcp,
            reason: reason.into(),
            diagnostic: None,
        }
    }

    fn response_parse(reason: impl Into<String>) -> Self {
        Self {
            phase: CatalogProbePhase::Discovery,
            category: CatalogProbeCategory::ResponseParse,
            reason: reason.into(),
            diagnostic: None,
        }
    }

    #[cfg(test)]
    fn timeout() -> Self {
        Self::timeout_after(PROBE_TIMEOUT)
    }

    fn timeout_after(limit: Duration) -> Self {
        Self {
            phase: CatalogProbePhase::Timeout,
            category: CatalogProbeCategory::Timeout,
            reason: format!("probe timed out after {} seconds", limit.as_secs()),
            diagnostic: None,
        }
    }

    fn from_bridge(
        phase: CatalogProbePhase,
        fallback_category: CatalogProbeCategory,
        context: impl AsRef<str>,
        error: bridge_core::error::BridgeError,
    ) -> Self {
        let diagnostic = match &error {
            bridge_core::error::BridgeError::AgentFailure { diagnostic } => {
                Some((**diagnostic).clone())
            }
            _ => None,
        };
        let category = diagnostic.as_ref().map_or(fallback_category, |diagnostic| {
            use bridge_core::diagnostics::DiagnosticFailureClass;

            match diagnostic.class() {
                DiagnosticFailureClass::Timeout => CatalogProbeCategory::Timeout,
                DiagnosticFailureClass::AgentProcess
                    if phase == CatalogProbePhase::Spawn
                        && diagnostic.code().as_str().starts_with("acp.spawn.") =>
                {
                    CatalogProbeCategory::Spawn
                }
                DiagnosticFailureClass::AgentProcess => CatalogProbeCategory::ProcessExit,
                _ => fallback_category,
            }
        });
        let deepest = diagnostic
            .as_ref()
            .and_then(|diagnostic| diagnostic.causes().last().cloned())
            .unwrap_or_else(|| error.to_string());
        Self {
            phase,
            category,
            reason: format!("{}: {deepest}", context.as_ref()),
            diagnostic,
        }
    }
}

/// A failed CLI discovery record. All dynamic text is sanitized and byte-bounded at construction;
/// private fields prevent callers from bypassing that boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct CatalogProbeFailure {
    agent: String,
    strategy: Strategy,
    phase: CatalogProbePhase,
    category: CatalogProbeCategory,
    #[serde(skip_serializing_if = "Option::is_none")]
    executable: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    configured_version: Option<String>,
    error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostic: Option<FailureDiagnostic>,
}

impl CatalogProbeFailure {
    fn build(entry: &AgentEntry, strategy: Strategy, error: ProbeError) -> Self {
        let known_values = entry
            .api_key_env
            .as_deref()
            .and_then(|name| std::env::var(name).ok())
            .into_iter();
        let redactor = DiagnosticRedactor::new(known_values);
        Self {
            agent: redactor.sanitize_stderr_line(entry.id.as_str(), 64),
            strategy,
            phase: error.phase,
            category: error.category,
            executable: entry
                .cmd
                .as_deref()
                .map(|cmd| redactor.sanitize_stderr_line(cmd, 256)),
            configured_version: entry
                .version
                .as_deref()
                .map(|version| redactor.sanitize_stderr_line(version, 128)),
            error: redactor.sanitize_stderr_line(&error.reason, 512),
            diagnostic: error.diagnostic,
        }
    }

    pub(super) fn cli_message(&self) -> String {
        format!(
            "models: probe failed for agent '{}' during {} (category={}): {}",
            self.agent,
            self.phase.as_str(),
            self.category.as_str(),
            self.error
        )
    }
}

/// The CLI-facing probe result. The success catalog retains the public Agent Card shape; failures are
/// carried separately so server callers can continue to degrade without advertising error records.
pub(super) struct CatalogProbeReport {
    pub(super) catalog: ModelCatalog,
    pub(super) failures: BTreeMap<String, CatalogProbeFailure>,
}

/// Select the probe strategy from `(kind, cmd)`. `kind=api` wins; otherwise the `cmd` basename `kiro-cli`
/// routes to the native list, and everything else (claude/codex, incl. container_rw ACP agents) to the
/// host ACP describe.
fn probe_strategy(entry: &AgentEntry) -> Strategy {
    if entry.kind == AgentKind::Api {
        return Strategy::Api;
    }
    let basename = entry.cmd.as_deref().map(|cmd| {
        Path::new(cmd)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(cmd)
            .to_string()
    });
    if basename.as_deref() == Some("kiro-cli") {
        Strategy::Kiro
    } else {
        Strategy::Acp
    }
}

/// Probe ONE agent host-side (sandbox ignored), bounded by `limit`; strategy chosen by `(kind, cmd)`.
async fn probe_agent(
    entry: &AgentEntry,
    cwd: &Path,
    limit: Duration,
) -> Result<AgentCaps, CatalogProbeFailure> {
    let strategy = probe_strategy(entry);
    let fut = async {
        match strategy {
            Strategy::Api => probe_api(entry).await,
            Strategy::Kiro => probe_kiro(entry).await,
            Strategy::Acp => probe_acp_host(entry, cwd).await,
        }
    };
    let result = match tokio::time::timeout(limit, fut).await {
        Ok(result) => result,
        Err(_) => Err(ProbeError::timeout_after(limit)),
    };
    result.map_err(|error| CatalogProbeFailure::build(entry, strategy, error))
}

/// One agent's bounded probe. A seam so the startup retry runs against scripted outcomes in tests
/// (no adapters, no processes) while production always goes through [`HostProber`].
#[async_trait::async_trait]
trait AgentProber: Send + Sync {
    async fn probe(&self, entry: &AgentEntry, cwd: &Path)
        -> Result<AgentCaps, CatalogProbeFailure>;
}

/// The production prober: [`probe_agent`] under a per-agent bound ([`PROBE_TIMEOUT`] outside tests).
struct HostProber {
    limit: Duration,
}

const HOST_PROBER: HostProber = HostProber {
    limit: PROBE_TIMEOUT,
};

#[async_trait::async_trait]
impl AgentProber for HostProber {
    async fn probe(
        &self,
        entry: &AgentEntry,
        cwd: &Path,
    ) -> Result<AgentCaps, CatalogProbeFailure> {
        probe_agent(entry, cwd, self.limit).await
    }
}

/// kiro native list: `kiro-cli chat --list-models` (host-side; requires current CLI login state).
async fn probe_kiro(entry: &AgentEntry) -> Result<AgentCaps, ProbeError> {
    let cmd = entry.cmd.clone().unwrap_or_else(|| "kiro-cli".into());
    // `kill_on_drop`: when the per-agent timeout cancels this future, dropping the child SIGKILLs it
    // instead of leaving a stray `kiro-cli` running after its probe has already been reported failed.
    let out = tokio::process::Command::new(&cmd)
        .args(["chat", "--list-models"])
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| ProbeError::spawn(format!("spawn {cmd}: {e}")))?;
    if !out.status.success() {
        let mut reason = format!("{cmd} exited with {}", out.status);
        if let Some(deepest) = deepest_process_stderr_line(&out.stderr) {
            reason.push_str(": ");
            reason.push_str(&deepest);
        }
        return Err(ProbeError::process_exit(reason));
    }
    Ok(parse_kiro_list_models(&String::from_utf8_lossy(
        &out.stdout,
    )))
}

/// Retain only the deepest non-empty process-stderr line and cap it before UTF-8 decoding. The final
/// [`CatalogProbeFailure`] constructor applies the shared credential/path redactor and tighter output
/// bound before this text can reach logs or CLI output.
fn deepest_process_stderr_line(stderr: &[u8]) -> Option<String> {
    stderr.rsplit(|byte| *byte == b'\n').find_map(|line| {
        let start = line.iter().position(|byte| !byte.is_ascii_whitespace())?;
        let end = line
            .iter()
            .rposition(|byte| !byte.is_ascii_whitespace())?
            .saturating_add(1);
        let bounded_end = start.saturating_add(PROCESS_STDERR_LINE_BYTES).min(end);
        Some(String::from_utf8_lossy(&line[start..bounded_end]).into_owned())
    })
}

/// api (ollama) list: OpenAI `GET {base_url}/models`. `base_url` already ends in `/v1` per the example
/// configs, so we append `/models`. The api backend never validates the model, so this is the only source.
async fn probe_api(entry: &AgentEntry) -> Result<AgentCaps, ProbeError> {
    let base = entry
        .base_url
        .as_deref()
        .ok_or_else(|| ProbeError::configuration("api agent missing base_url"))?;
    let url = format!("{}/models", base.trim_end_matches('/'));
    let response = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .map_err(|error| {
            ProbeError::transport(format!(
                "GET models endpoint failed: {}",
                error.without_url()
            ))
        })?;
    let status = response.status();
    if !status.is_success() {
        return Err(ProbeError::provider_acp(format!(
            "GET models endpoint returned HTTP {}",
            status.as_u16()
        )));
    }
    let body = response.text().await.map_err(|error| {
        ProbeError::transport(format!(
            "read models response failed: {}",
            error.without_url()
        ))
    })?;
    parse_ollama_models(&body)
        .map_err(|error| ProbeError::response_parse(format!("parse models response: {error}")))
}

#[async_trait::async_trait]
trait CatalogAcpBackend: Send + Sync {
    async fn describe_options_observed(
        &self,
        cwd: &Path,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<AgentCaps, bridge_core::error::BridgeError>;
    async fn retire(&self);
}

#[async_trait::async_trait]
impl CatalogAcpBackend for AcpBackend {
    async fn describe_options_observed(
        &self,
        cwd: &Path,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<AgentCaps, bridge_core::error::BridgeError> {
        AcpBackend::describe_options_observed(self, cwd, observer).await
    }

    async fn retire(&self) {
        let _ = AgentBackend::retire(self).await;
    }
}

#[async_trait::async_trait]
trait CatalogAcpSpawner: Send + Sync {
    async fn spawn_observed(
        &self,
        cmd: &str,
        args: &[&str],
        config: AcpConfig,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<Arc<dyn CatalogAcpBackend>, bridge_core::error::BridgeError>;
}

struct ProductionCatalogAcpSpawner;

#[async_trait::async_trait]
impl CatalogAcpSpawner for ProductionCatalogAcpSpawner {
    async fn spawn_observed(
        &self,
        cmd: &str,
        args: &[&str],
        config: AcpConfig,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<Arc<dyn CatalogAcpBackend>, bridge_core::error::BridgeError> {
        AcpBackend::spawn_observed(cmd, args, config, observer)
            .await
            .map(|backend| Arc::new(backend) as Arc<dyn CatalogAcpBackend>)
    }
}

/// ACP host describe (claude/codex): spawn the adapter HOST-SIDE (sandbox stripped, no container reaper, no
/// MCP, nothing configured), read the advertised options via [`AcpBackend::describe_options`], then reap.
async fn probe_acp_host(entry: &AgentEntry, cwd: &Path) -> Result<AgentCaps, ProbeError> {
    probe_acp_host_with(entry, cwd, &ProductionCatalogAcpSpawner).await
}

async fn probe_acp_host_with(
    entry: &AgentEntry,
    cwd: &Path,
    spawner: &dyn CatalogAcpSpawner,
) -> Result<AgentCaps, ProbeError> {
    let cmd = entry
        .cmd
        .as_deref()
        .ok_or_else(|| ProbeError::spawn("acp agent missing cmd"))?
        .to_string();
    let argv: Vec<&str> = entry.args.iter().map(String::as_str).collect();
    // Host config: discovery configures NOTHING (model/mode are read off the session/new response, not
    // applied), and there is no `:ro` container to reap — the host probe ignores `[sandbox]`.
    let acp = AcpConfig {
        agent_id: entry.id.as_str().to_string(),
        cwd: cwd.to_path_buf(),
        model: None,
        mode: None,
        auth_method: entry.auth_method.clone(),
        pre_authenticated: entry.pre_authenticated,
        // Never the entry's value: a probe session runs no turn, so there is no title thread to
        // head off, and a rename would only leave a named, empty entry in codex's session index.
        rename_new_sessions: false,
        container: None,
        mcp: Vec::new(),
        ..AcpConfig::default()
    };
    let observer: Arc<dyn DiagnosticObserver> = Arc::new(
        bridge_core::diagnostics::InMemoryDiagnosticObserver::new(64)
            .expect("catalog diagnostic capacity is nonzero"),
    );
    let backend = spawner
        .spawn_observed(&cmd, &argv, acp, observer.clone())
        .await
        .map_err(|e| {
            ProbeError::from_bridge(
                CatalogProbePhase::Spawn,
                CatalogProbeCategory::Spawn,
                format!("spawn {cmd}"),
                e,
            )
        })?;
    let caps = backend
        .describe_options_observed(cwd, observer)
        .await
        .map_err(|e| {
            ProbeError::from_bridge(
                CatalogProbePhase::Discovery,
                CatalogProbeCategory::ProviderAcp,
                "describe_options",
                e,
            )
        });
    // Graceful teardown (SIGTERM→SIGKILL of the host child). If the outer timeout cancels this future
    // mid-await, dropping `backend` SIGKILLs the child (`kill_on_drop`) — so neither path leaks.
    let _ = backend.retire().await;
    caps
}

/// Probe the given entries concurrently, one bounded probe each.
async fn probe_each<'a>(
    entries: impl IntoIterator<Item = &'a (String, AgentEntry)>,
    cwd: &Path,
    prober: &dyn AgentProber,
) -> Vec<(String, Result<AgentCaps, CatalogProbeFailure>)> {
    let futs = entries
        .into_iter()
        .map(|(id, entry)| async move { (id.clone(), prober.probe(entry, cwd).await) });
    futures::future::join_all(futs).await
}

/// Probe every entry concurrently and retain both successes and safe failure records for the CLI.
pub(super) async fn probe_all_report(
    entries: &[(String, AgentEntry)],
    cwd: &Path,
) -> CatalogProbeReport {
    collect_probe_report(probe_each(entries, cwd, &HOST_PROBER).await)
}

/// Server/Card callers intentionally keep the legacy success-only catalog and per-agent degradation.
pub async fn probe_all(entries: &[(String, AgentEntry)], cwd: &Path) -> ModelCatalog {
    probe_all_report(entries, cwd).await.catalog
}

/// Serve startup's catalog: [`probe_all`], then ONE re-probe of only the agents that failed, after
/// [`STARTUP_RETRY_DELAY`]. Recovered agents join the catalog; agents that fail again stay omitted until
/// a SIGHUP re-probe. Nothing runs in the background (owner decision: no catalog timer).
pub async fn probe_all_at_startup(entries: &[(String, AgentEntry)], cwd: &Path) -> ModelCatalog {
    probe_all_with_startup_retry(entries, cwd, &HOST_PROBER, STARTUP_RETRY_DELAY)
        .await
        .catalog
}

async fn probe_all_with_startup_retry(
    entries: &[(String, AgentEntry)],
    cwd: &Path,
    prober: &dyn AgentProber,
    retry_delay: Duration,
) -> CatalogProbeReport {
    let mut report = collect_probe_report(probe_each(entries, cwd, prober).await);
    if report.failures.is_empty() {
        return report;
    }
    tokio::time::sleep(retry_delay).await;
    let failed = entries
        .iter()
        .filter(|(id, _)| report.failures.contains_key(id));
    let retried = probe_each(failed, cwd, prober).await;
    for (id, result) in retried {
        match result {
            Ok(caps) => {
                tracing::info!(
                    agent = %id,
                    attempt = STARTUP_RETRY_ATTEMPT,
                    "model probe succeeded on startup retry; agent added to the model catalog"
                );
                report.failures.remove(&id);
                report.catalog.insert(id, sanitize_model_caps(caps));
            }
            Err(failure) => {
                warn_probe_failure(&failure, STARTUP_RETRY_ATTEMPT);
                report.failures.insert(id, failure);
            }
        }
    }
    report
}

/// The one operator-visible record of a failed probe: which agent, how it was probed, why it failed
/// (category plus the bounded, redacted deepest cause), and which attempt this was.
fn warn_probe_failure(failure: &CatalogProbeFailure, attempt: u32) {
    tracing::warn!(
        agent = %failure.agent,
        strategy = failure.strategy.as_str(),
        phase = failure.phase.as_str(),
        category = failure.category.as_str(),
        attempt,
        error = %failure.error,
        "model probe failed; agent omitted from the model catalog"
    );
}

/// Fold per-agent probe results without erasing failures. Pure so both server degradation and CLI
/// representation are covered without spawning real adapters.
fn collect_probe_report(
    results: Vec<(String, Result<AgentCaps, CatalogProbeFailure>)>,
) -> CatalogProbeReport {
    let mut catalog = ModelCatalog::new();
    let mut failures = BTreeMap::new();
    for (id, result) in results {
        match result {
            Ok(caps) => {
                catalog.insert(id, sanitize_model_caps(caps));
            }
            Err(failure) => {
                warn_probe_failure(&failure, FIRST_ATTEMPT);
                failures.insert(id, failure);
            }
        }
    }
    CatalogProbeReport { catalog, failures }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::ids::AgentId;

    fn entry(id: &str, cmd: Option<&str>, kind: AgentKind) -> AgentEntry {
        AgentEntry {
            id: AgentId::parse(id).unwrap(),
            cmd: cmd.map(String::from),
            base_url: None,
            api_key_env: None,
            args: vec![],
            kind,
            model_provider: None,
            model: None,
            effort: None,
            mode: None,
            preflight: false,
            fallback_models: vec![],
            cwd: None,
            session_cwd: None,
            sandbox: None,
            watchdog: None,
            auth_method: None,
            pre_authenticated: false,
            host_fallback_eligible: false,
            rename_new_sessions: false,
            name: None,
            description: None,
            tags: vec![],
            version: None,
            mcp: vec![],
            mcp_delivery: Default::default(),
            extensions: Default::default(),
        }
    }

    #[test]
    fn strategy_dispatch_by_kind_and_cmd() {
        // api kind wins regardless of cmd.
        assert_eq!(
            probe_strategy(&entry("ollama", None, AgentKind::Api)),
            Strategy::Api
        );
        // kiro routes to the native list by cmd basename (even with a full path).
        assert_eq!(
            probe_strategy(&entry(
                "kiro",
                Some("/usr/local/bin/kiro-cli"),
                AgentKind::Acp
            )),
            Strategy::Kiro
        );
        // claude/codex (and any other acp cmd) → host ACP describe.
        assert_eq!(
            probe_strategy(&entry("claude", Some("claude-agent-acp"), AgentKind::Acp)),
            Strategy::Acp
        );
        assert_eq!(
            probe_strategy(&entry("codex", Some("codex-acp"), AgentKind::Acp)),
            Strategy::Acp
        );
        // container_rw ACP agents also describe host-side.
        assert_eq!(
            probe_strategy(&entry("impl", Some("codex-acp"), AgentKind::ContainerRw)),
            Strategy::Acp
        );
    }

    struct RecordingProbeBackend {
        described: std::sync::Mutex<Vec<Arc<dyn DiagnosticObserver>>>,
        retired: std::sync::atomic::AtomicBool,
    }

    #[async_trait::async_trait]
    impl CatalogAcpBackend for RecordingProbeBackend {
        async fn describe_options_observed(
            &self,
            _cwd: &Path,
            observer: Arc<dyn DiagnosticObserver>,
        ) -> Result<AgentCaps, bridge_core::error::BridgeError> {
            self.described.lock().unwrap().push(observer);
            Ok(AgentCaps {
                models: vec!["observed-model".into()],
                ..AgentCaps::default()
            })
        }

        async fn retire(&self) {
            self.retired
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    struct RecordingProbeSpawner {
        spawned: std::sync::Mutex<Vec<Arc<dyn DiagnosticObserver>>>,
        backend: Arc<RecordingProbeBackend>,
    }

    #[async_trait::async_trait]
    impl CatalogAcpSpawner for RecordingProbeSpawner {
        async fn spawn_observed(
            &self,
            _cmd: &str,
            _args: &[&str],
            _config: AcpConfig,
            observer: Arc<dyn DiagnosticObserver>,
        ) -> Result<Arc<dyn CatalogAcpBackend>, bridge_core::error::BridgeError> {
            self.spawned.lock().unwrap().push(observer);
            Ok(self.backend.clone())
        }
    }

    #[tokio::test]
    async fn production_acp_probe_owner_reuses_exact_observer_for_spawn_and_discovery() {
        let backend = Arc::new(RecordingProbeBackend {
            described: std::sync::Mutex::new(Vec::new()),
            retired: std::sync::atomic::AtomicBool::new(false),
        });
        let spawner = RecordingProbeSpawner {
            spawned: std::sync::Mutex::new(Vec::new()),
            backend: backend.clone(),
        };
        let caps = probe_acp_host_with(
            &entry("codex", Some("codex-acp"), AgentKind::Acp),
            Path::new("/tmp"),
            &spawner,
        )
        .await
        .unwrap();

        let spawned = spawner.spawned.lock().unwrap();
        let described = backend.described.lock().unwrap();
        assert_eq!(spawned.len(), 1);
        assert_eq!(described.len(), 1);
        assert!(Arc::ptr_eq(&spawned[0], &described[0]));
        assert_eq!(caps.models, vec!["observed-model"]);
        assert!(
            backend.retired.load(std::sync::atomic::Ordering::SeqCst),
            "production owner must retire the one-shot backend"
        );
    }

    #[tokio::test]
    async fn acp_probe_never_renames_the_sessions_it_mints() {
        struct ConfigCapturingSpawner {
            configs: std::sync::Mutex<Vec<AcpConfig>>,
            backend: Arc<RecordingProbeBackend>,
        }

        #[async_trait::async_trait]
        impl CatalogAcpSpawner for ConfigCapturingSpawner {
            async fn spawn_observed(
                &self,
                _cmd: &str,
                _args: &[&str],
                config: AcpConfig,
                _observer: Arc<dyn DiagnosticObserver>,
            ) -> Result<Arc<dyn CatalogAcpBackend>, bridge_core::error::BridgeError> {
                self.configs.lock().unwrap().push(config);
                Ok(self.backend.clone())
            }
        }

        let spawner = ConfigCapturingSpawner {
            configs: std::sync::Mutex::new(Vec::new()),
            backend: Arc::new(RecordingProbeBackend {
                described: std::sync::Mutex::new(Vec::new()),
                retired: std::sync::atomic::AtomicBool::new(false),
            }),
        };
        // Host and container kinds alike are probed host-side, from an entry that opted in.
        for kind in [AgentKind::Acp, AgentKind::ContainerRw] {
            let mut renaming = entry("codex", Some("codex-acp"), kind);
            renaming.rename_new_sessions = true;
            renaming.pre_authenticated = true;
            probe_acp_host_with(&renaming, Path::new("/tmp"), &spawner)
                .await
                .unwrap();
        }

        let configs = spawner.configs.lock().unwrap();
        assert_eq!(configs.len(), 2);
        for config in configs.iter() {
            assert!(
                config.pre_authenticated,
                "control: the probe config does carry the entry's other settings"
            );
            assert!(
                !config.rename_new_sessions,
                "a probe session never runs a turn, so it must not be renamed"
            );
        }
    }

    #[test]
    fn collect_probe_report_keeps_failures_outside_server_catalog() {
        let bad_entry = entry("bad", Some("bad-acp"), AgentKind::Acp);
        let failure =
            CatalogProbeFailure::build(&bad_entry, Strategy::Acp, ProbeError::provider_acp("boom"));
        let results = vec![
            (
                "ok".to_string(),
                Ok(AgentCaps {
                    models: vec!["m".into(), "claude-fable-5.1[1m]".into()],
                    ..Default::default()
                }),
            ),
            ("bad".to_string(), Err(failure.clone())),
        ];
        let report = collect_probe_report(results);
        assert!(report.catalog.contains_key("ok"), "ok agent kept");
        assert!(
            !report.catalog.contains_key("bad"),
            "failed agent omitted from server catalog"
        );
        assert_eq!(report.catalog.len(), 1);
        assert_eq!(report.catalog["ok"].models, vec!["m"]);
        assert_eq!(report.failures.get("bad"), Some(&failure));
    }

    #[test]
    fn probe_failure_redacts_secret_markers_and_bounds_dynamic_error() {
        let bad_entry = entry("bad", Some("bad-acp"), AgentKind::Acp);
        let failure = CatalogProbeFailure::build(
            &bad_entry,
            Strategy::Acp,
            ProbeError::provider_acp(format!("authorization: top-secret {}", "x".repeat(800))),
        );
        let value = serde_json::to_value(&failure).unwrap();
        assert_eq!(value["phase"], "discovery");
        assert_eq!(value["category"], "provider_acp");
        assert_eq!(value["strategy"], "acp");
        assert!(value["error"].as_str().unwrap().len() <= 512);
        assert!(!value.to_string().contains("top-secret"));
        assert!(value["error"].as_str().unwrap().contains("[REDACTED]"));
    }

    #[test]
    fn timeout_failure_has_distinct_phase_and_bounded_reason() {
        let bad_entry = entry("slow", Some("slow-acp"), AgentKind::Acp);
        let failure = CatalogProbeFailure::build(&bad_entry, Strategy::Acp, ProbeError::timeout());
        let value = serde_json::to_value(&failure).unwrap();
        assert_eq!(value["phase"], "timeout");
        assert_eq!(value["category"], "timeout");
        assert_eq!(
            value["error"],
            format!("probe timed out after {} seconds", PROBE_TIMEOUT.as_secs())
        );
    }

    #[test]
    fn failure_categories_have_stable_text_and_json_tokens() {
        let cases = [
            (CatalogProbeCategory::Spawn, "spawn"),
            (CatalogProbeCategory::Timeout, "timeout"),
            (CatalogProbeCategory::ProcessExit, "process_exit"),
            (CatalogProbeCategory::ProviderAcp, "provider_acp"),
            (CatalogProbeCategory::ResponseParse, "response_parse"),
            (CatalogProbeCategory::Transport, "transport"),
            (CatalogProbeCategory::Configuration, "configuration"),
        ];
        for (category, expected) in cases {
            assert_eq!(category.as_str(), expected);
            assert_eq!(serde_json::to_value(category).unwrap(), expected);
        }
    }

    #[test]
    fn typed_acp_failure_keeps_provider_category_and_deepest_safe_cause() {
        use bridge_core::diagnostics::{
            DiagnosticFailureClass, DiagnosticPhase, FailureDiagnosticInput, FailureDisposition,
        };

        let diagnostic = FailureDiagnostic::build_static_code(
            FailureDiagnosticInput {
                failed_phase: DiagnosticPhase::SessionCreate,
                last_completed_phase: Some(DiagnosticPhase::Authenticate),
                class: DiagnosticFailureClass::Transport,
                disposition: FailureDisposition::RetrySameTarget,
                code: String::new(),
                summary: "ACP discovery failed".into(),
                causes: vec!["deepest-acp-probe-sentinel-30".into()],
                stderr_observed: false,
                stderr_line_count: 0,
                stderr_scope: None,
                stderr_tail: None,
                stderr_redaction: None,
                retry_after_ms: None,
                reset_at_ms: None,
                prompt_may_have_been_accepted: false,
            },
            "acp.discovery.test_failure",
            &DiagnosticRedactor::default(),
        )
        .unwrap();
        let error = ProbeError::from_bridge(
            CatalogProbePhase::Discovery,
            CatalogProbeCategory::ProviderAcp,
            "describe_options",
            bridge_core::error::BridgeError::agent_failure(diagnostic.clone()),
        );
        assert_eq!(error.category, CatalogProbeCategory::ProviderAcp);
        assert!(error.reason.contains("deepest-acp-probe-sentinel-30"));
        assert_eq!(error.diagnostic, Some(diagnostic));
    }

    #[test]
    fn deepest_process_stderr_is_one_prefix_bounded_nonempty_line() {
        let oversized = format!("first\n  deepest-{}  \n\n", "x".repeat(8 * 1024));
        let deepest = deepest_process_stderr_line(oversized.as_bytes()).unwrap();
        assert!(deepest.starts_with("deepest-"));
        assert_eq!(deepest.len(), PROCESS_STDERR_LINE_BYTES);
        assert!(!deepest.contains("first"));
    }

    #[derive(Clone, Copy)]
    enum Scripted {
        Caps,
        TimedOut,
        Exited,
    }

    /// An [`AgentProber`] whose Nth probe of an agent returns that agent's Nth scripted outcome, and
    /// which counts every call. No processes, no timers; probing past the script panics.
    struct ScriptedProber {
        script: BTreeMap<String, Vec<Scripted>>,
        calls: std::sync::Mutex<BTreeMap<String, usize>>,
    }

    impl ScriptedProber {
        fn new(script: &[(&str, &[Scripted])]) -> Self {
            Self {
                script: script
                    .iter()
                    .map(|(id, outcomes)| (id.to_string(), outcomes.to_vec()))
                    .collect(),
                calls: std::sync::Mutex::new(BTreeMap::new()),
            }
        }

        fn calls(&self, id: &str) -> usize {
            self.calls.lock().unwrap().get(id).copied().unwrap_or(0)
        }

        /// One entry per scripted agent; `kiro` gets the native-list strategy, the rest ACP.
        fn entries(&self) -> Vec<(String, AgentEntry)> {
            self.script
                .keys()
                .map(|id| {
                    let cmd = if id == "kiro" {
                        "kiro-cli"
                    } else {
                        "codex-acp"
                    };
                    (id.clone(), entry(id, Some(cmd), AgentKind::Acp))
                })
                .collect()
        }
    }

    #[async_trait::async_trait]
    impl AgentProber for ScriptedProber {
        async fn probe(
            &self,
            entry: &AgentEntry,
            _cwd: &Path,
        ) -> Result<AgentCaps, CatalogProbeFailure> {
            let id = entry.id.as_str();
            let attempt = {
                let mut calls = self.calls.lock().unwrap();
                let count = calls.entry(id.to_string()).or_default();
                *count += 1;
                *count
            };
            let outcome = self.script[id]
                .get(attempt - 1)
                .copied()
                .unwrap_or_else(|| panic!("{id} probed {attempt} times, beyond its script"));
            let error = match outcome {
                Scripted::Caps => {
                    return Ok(AgentCaps {
                        models: vec![format!("{id}-model-from-attempt-{attempt}")],
                        ..AgentCaps::default()
                    })
                }
                Scripted::TimedOut => ProbeError::timeout(),
                Scripted::Exited => ProbeError::process_exit(format!(
                    "fake exited with exit status: 1: deepest-cause-{id}-{attempt} authorization: top-secret"
                )),
            };
            Err(CatalogProbeFailure::build(
                entry,
                probe_strategy(entry),
                error,
            ))
        }
    }

    #[test]
    fn startup_retry_delay_is_three_seconds() {
        assert_eq!(STARTUP_RETRY_DELAY, Duration::from_secs(3));
    }

    #[tokio::test]
    async fn startup_retry_adds_a_recovered_agent_and_probes_the_others_once() {
        use Scripted::*;
        let prober = ScriptedProber::new(&[
            ("claude", &[Caps]),
            ("codex", &[Caps]),
            ("kiro", &[TimedOut, Caps]),
        ]);
        let entries = prober.entries();

        let report =
            probe_all_with_startup_retry(&entries, Path::new("/tmp"), &prober, Duration::ZERO)
                .await;

        assert_eq!(
            report.catalog["kiro"].models,
            vec!["kiro-model-from-attempt-2"]
        );
        assert!(
            report.failures.is_empty(),
            "the recovered failure is cleared"
        );
        assert_eq!(report.catalog.len(), 3);
        assert_eq!(prober.calls("kiro"), 2);
        assert_eq!(
            prober.calls("claude"),
            1,
            "a first-time success is not re-probed"
        );
        assert_eq!(
            prober.calls("codex"),
            1,
            "a first-time success is not re-probed"
        );
    }

    #[tokio::test]
    async fn startup_retry_omits_a_twice_failed_agent_and_leaves_the_others_untouched() {
        use Scripted::*;
        let prober = ScriptedProber::new(&[
            ("claude", &[Caps]),
            ("codex", &[Exited, Caps]),
            ("kiro", &[TimedOut, Exited]),
        ]);
        let entries = prober.entries();

        let report =
            probe_all_with_startup_retry(&entries, Path::new("/tmp"), &prober, Duration::ZERO)
                .await;

        assert!(!report.catalog.contains_key("kiro"), "still omitted");
        assert_eq!(prober.calls("kiro"), 2, "exactly one retry");
        let failure = &report.failures["kiro"];
        assert_eq!(failure.category, CatalogProbeCategory::ProcessExit);
        assert!(
            failure.error.contains("deepest-cause-kiro-2"),
            "the retry's failure is the one retained: {}",
            failure.error
        );
        assert_eq!(report.failures.len(), 1);
        assert_eq!(
            report.catalog["claude"].models,
            vec!["claude-model-from-attempt-1"],
            "a first-time success keeps its own entry"
        );
        assert_eq!(
            report.catalog["codex"].models,
            vec!["codex-model-from-attempt-2"],
            "a sibling's recovery is unaffected"
        );
        assert_eq!(report.catalog.len(), 2);
        assert_eq!(prober.calls("claude"), 1);
    }

    #[tokio::test]
    async fn startup_without_failures_neither_retries_nor_waits() {
        use Scripted::*;
        // An hour-long delay: completing well inside the guard proves no delay was spent.
        let hour = Duration::from_secs(3600);
        let guard = Duration::from_secs(10);
        let healthy = ScriptedProber::new(&[("claude", &[Caps]), ("kiro", &[Caps])]);
        let entries = healthy.entries();
        let report = tokio::time::timeout(
            guard,
            probe_all_with_startup_retry(&entries, Path::new("/tmp"), &healthy, hour),
        )
        .await
        .expect("no failure, so no retry delay");
        assert_eq!(report.catalog.len(), 2);
        assert_eq!(healthy.calls("claude"), 1);
        assert_eq!(healthy.calls("kiro"), 1);

        // Control: the same delay IS spent once an agent fails, so the guard above is meaningful.
        let failing = ScriptedProber::new(&[("kiro", &[Exited, Caps])]);
        let entries = failing.entries();
        let waited = tokio::time::timeout(
            Duration::from_millis(50),
            probe_all_with_startup_retry(&entries, Path::new("/tmp"), &failing, hour),
        )
        .await;
        assert!(waited.is_err(), "a failure waits out the retry delay");
        assert_eq!(failing.calls("kiro"), 1, "the retry follows the delay");
    }

    /// JSON lines written by a capturing subscriber.
    #[derive(Clone)]
    struct CapturedLog(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for CapturedLog {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl CapturedLog {
        fn events(&self) -> Vec<serde_json::Value> {
            let bytes = self.0.lock().unwrap().clone();
            String::from_utf8(bytes)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        }
    }

    #[tokio::test]
    async fn each_failed_probe_warns_with_its_cause_and_a_recovery_logs_info() {
        use Scripted::*;
        let log = CapturedLog(Arc::new(std::sync::Mutex::new(Vec::new())));
        let writer = log.clone();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(move || writer.clone())
            .finish();
        // tracing-core caches each callsite's interest process-wide. While exactly one dispatcher is
        // registered, a callsite's interest comes from whichever thread hits it first, so a parallel
        // test without a subscriber could cache `never` for the shared probe-log callsites. A second
        // live dispatcher makes every registration consult all live dispatchers, this one included.
        let _second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
        let _default = tracing::subscriber::set_default(subscriber);

        let prober = ScriptedProber::new(&[
            ("claude", &[Caps]),
            ("codex", &[Exited, Caps]),
            ("kiro", &[TimedOut, Exited]),
        ]);
        let entries = prober.entries();
        probe_all_with_startup_retry(&entries, Path::new("/tmp"), &prober, Duration::ZERO).await;

        let events = log.events();
        let at = |level: &str| -> Vec<&serde_json::Value> {
            events
                .iter()
                .filter(|event| event["level"] == level)
                .map(|event| &event["fields"])
                .collect()
        };
        let warns = at("WARN");
        let find = |agent: &str, attempt: u32| {
            *warns
                .iter()
                .find(|fields| fields["agent"] == agent && fields["attempt"] == attempt)
                .unwrap_or_else(|| panic!("no WARN for {agent} attempt {attempt}: {events:?}"))
        };
        assert_eq!(warns.len(), 3, "one WARN per failed probe: {events:?}");

        let kiro_first = find("kiro", 1);
        assert_eq!(kiro_first["strategy"], "kiro");
        assert_eq!(kiro_first["category"], "timeout");
        assert_eq!(
            kiro_first["error"],
            format!("probe timed out after {} seconds", PROBE_TIMEOUT.as_secs())
        );

        let kiro_retry = find("kiro", 2);
        assert_eq!(kiro_retry["strategy"], "kiro");
        assert_eq!(kiro_retry["category"], "process_exit");
        let cause = kiro_retry["error"].as_str().unwrap();
        assert!(cause.contains("deepest-cause-kiro-2"), "{cause}");
        assert!(
            !cause.contains("top-secret"),
            "redacted before logging: {cause}"
        );

        let codex_first = find("codex", 1);
        assert_eq!(codex_first["strategy"], "acp");
        assert_eq!(codex_first["category"], "process_exit");
        assert!(codex_first["error"]
            .as_str()
            .unwrap()
            .contains("deepest-cause-codex-1"));

        let infos = at("INFO");
        assert_eq!(infos.len(), 1, "one INFO per recovered agent: {events:?}");
        assert_eq!(infos[0]["agent"], "codex");
        assert_eq!(infos[0]["attempt"], 2);
    }

    /// Whether `pid` is a live, non-zombie process. A SIGKILLed child stays a zombie until tokio's
    /// orphan reaper collects it, but a zombie runs nothing.
    #[cfg(unix)]
    fn process_is_running(pid: libc::pid_t) -> bool {
        // SAFETY: signal 0 only checks that the process exists.
        if unsafe { libc::kill(pid, 0) } != 0 {
            return false;
        }
        #[cfg(target_os = "linux")]
        let state = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| {
                let (_, after_comm) = stat.rsplit_once(')')?;
                after_comm.trim_start().chars().next()
            });
        #[cfg(not(target_os = "linux"))]
        let state = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .ok()
            .and_then(|out| String::from_utf8_lossy(&out.stdout).trim().chars().next());
        state.is_some_and(|state| state != 'Z')
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timed_out_kiro_probe_leaves_no_child_running() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("kiro.pid");
        let kiro = dir.path().join("kiro-cli");
        // Only the probe's `chat --list-models` hangs; the warm-up exec (no args) exits at once.
        std::fs::write(
            &kiro,
            format!(
                "#!/bin/sh\n[ \"$1\" = chat ] || exit 0\necho $$ > '{}'\nexec sleep 30\n",
                pid_file.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&kiro, std::fs::Permissions::from_mode(0o755)).unwrap();
        crate::cli_tests::warm_up_script(&kiro);

        let short = HostProber {
            limit: Duration::from_secs(1),
        };
        let kiro_entry = entry("kiro", Some(kiro.to_str().unwrap()), AgentKind::Acp);
        let failure = short.probe(&kiro_entry, dir.path()).await.unwrap_err();
        assert_eq!(failure.strategy, Strategy::Kiro);
        assert_eq!(failure.category, CatalogProbeCategory::Timeout);

        let pid: libc::pid_t = std::fs::read_to_string(&pid_file)
            .expect("the fake kiro-cli started and recorded its pid")
            .trim()
            .parse()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while process_is_running(pid) {
            if std::time::Instant::now() >= deadline {
                // SAFETY: plain SIGKILL of the test's own leaked child.
                unsafe { libc::kill(pid, libc::SIGKILL) };
                panic!("timed-out kiro probe left its child {pid} running");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}
