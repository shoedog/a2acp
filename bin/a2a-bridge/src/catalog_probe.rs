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
//! operator-facing `models` CLI.

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

/// Which discovery strategy an entry uses. Pure (kind/cmd only) so the dispatch decision is unit-tested
/// without spawning a real adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Strategy {
    Api,
    Kiro,
    Acp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CatalogProbePhase {
    Spawn,
    Discovery,
    Timeout,
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
    reason: String,
    diagnostic: Option<FailureDiagnostic>,
}

impl ProbeError {
    fn spawn(reason: impl Into<String>) -> Self {
        Self {
            phase: CatalogProbePhase::Spawn,
            reason: reason.into(),
            diagnostic: None,
        }
    }

    fn discovery(reason: impl Into<String>) -> Self {
        Self {
            phase: CatalogProbePhase::Discovery,
            reason: reason.into(),
            diagnostic: None,
        }
    }

    fn timeout() -> Self {
        Self {
            phase: CatalogProbePhase::Timeout,
            reason: format!("probe timed out after {} seconds", PROBE_TIMEOUT.as_secs()),
            diagnostic: None,
        }
    }

    fn from_bridge(
        phase: CatalogProbePhase,
        context: impl AsRef<str>,
        error: bridge_core::error::BridgeError,
    ) -> Self {
        let diagnostic = match &error {
            bridge_core::error::BridgeError::AgentFailure { diagnostic } => {
                Some((**diagnostic).clone())
            }
            _ => None,
        };
        let deepest = diagnostic
            .as_ref()
            .and_then(|diagnostic| diagnostic.causes().last().cloned())
            .unwrap_or_else(|| error.to_string());
        Self {
            phase,
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
            "models: probe failed for agent '{}' during {}: {}",
            self.agent,
            self.phase.as_str(),
            self.error
        )
    }

    pub(super) fn error(&self) -> &str {
        &self.error
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

/// Probe ONE agent host-side (sandbox ignored). Timeout-bounded; the strategy is chosen by `(kind, cmd)`.
async fn probe_agent(entry: &AgentEntry, cwd: &Path) -> Result<AgentCaps, CatalogProbeFailure> {
    let strategy = probe_strategy(entry);
    let fut = async {
        match strategy {
            Strategy::Api => probe_api(entry).await,
            Strategy::Kiro => probe_kiro(entry).await,
            Strategy::Acp => probe_acp_host(entry, cwd).await,
        }
    };
    let result = match tokio::time::timeout(PROBE_TIMEOUT, fut).await {
        Ok(result) => result,
        Err(_) => Err(ProbeError::timeout()),
    };
    result.map_err(|error| CatalogProbeFailure::build(entry, strategy, error))
}

/// kiro native list: `kiro-cli chat --list-models` (auth-free, no container needed).
async fn probe_kiro(entry: &AgentEntry) -> Result<AgentCaps, ProbeError> {
    let cmd = entry.cmd.clone().unwrap_or_else(|| "kiro-cli".into());
    let out = tokio::process::Command::new(&cmd)
        .args(["chat", "--list-models"])
        .output()
        .await
        .map_err(|e| ProbeError::spawn(format!("spawn {cmd}: {e}")))?;
    if !out.status.success() {
        return Err(ProbeError::discovery(format!(
            "{cmd} exited {:?}",
            out.status.code()
        )));
    }
    Ok(parse_kiro_list_models(&String::from_utf8_lossy(
        &out.stdout,
    )))
}

/// api (ollama) list: OpenAI `GET {base_url}/models`. `base_url` already ends in `/v1` per the example
/// configs, so we append `/models`. The api backend never validates the model, so this is the only source.
async fn probe_api(entry: &AgentEntry) -> Result<AgentCaps, ProbeError> {
    let base = entry
        .base_url
        .as_deref()
        .ok_or_else(|| ProbeError::discovery("api agent missing base_url"))?;
    let url = format!("{}/models", base.trim_end_matches('/'));
    let body = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .map_err(|e| ProbeError::discovery(format!("GET {url}: {e}")))?
        .text()
        .await
        .map_err(|e| ProbeError::discovery(format!("read {url}: {e}")))?;
    parse_ollama_models(&body).map_err(|e| ProbeError::discovery(format!("parse {url}: {e}")))
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
            ProbeError::from_bridge(CatalogProbePhase::Spawn, format!("spawn {cmd}"), e)
        })?;
    let caps = backend
        .describe_options_observed(cwd, observer)
        .await
        .map_err(|e| ProbeError::from_bridge(CatalogProbePhase::Discovery, "describe_options", e));
    // Graceful teardown (SIGTERM→SIGKILL of the host child). If the outer timeout cancels this future
    // mid-await, dropping `backend` SIGKILLs the child (`kill_on_drop`) — so neither path leaks.
    let _ = backend.retire().await;
    caps
}

/// Probe every entry concurrently and retain both successes and safe failure records for the CLI.
pub(super) async fn probe_all_report(
    entries: &[(String, AgentEntry)],
    cwd: &Path,
) -> CatalogProbeReport {
    let futs = entries
        .iter()
        .map(|(id, entry)| async move { (id.clone(), probe_agent(entry, cwd).await) });
    let results = futures::future::join_all(futs).await;
    collect_probe_report(results)
}

/// Server/Card callers intentionally keep the legacy success-only catalog and per-agent degradation.
pub async fn probe_all(entries: &[(String, AgentEntry)], cwd: &Path) -> ModelCatalog {
    probe_all_report(entries, cwd).await.catalog
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
                tracing::warn!(agent = %id, reason = %failure.error(), "model probe failed; omitting from server catalog");
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
            cwd: None,
            session_cwd: None,
            sandbox: None,
            watchdog: None,
            auth_method: None,
            pre_authenticated: false,
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

    #[test]
    fn collect_probe_report_keeps_failures_outside_server_catalog() {
        let bad_entry = entry("bad", Some("bad-acp"), AgentKind::Acp);
        let failure =
            CatalogProbeFailure::build(&bad_entry, Strategy::Acp, ProbeError::discovery("boom"));
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
            ProbeError::discovery(format!("authorization: top-secret {}", "x".repeat(800))),
        );
        let value = serde_json::to_value(&failure).unwrap();
        assert_eq!(value["phase"], "discovery");
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
        assert_eq!(
            value["error"],
            format!("probe timed out after {} seconds", PROBE_TIMEOUT.as_secs())
        );
    }
}
