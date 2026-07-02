//! Host-side model-catalog probe: discover each configured agent's advertised models/effort/modes.
//!
//! The advertised list is account/adapter-driven and SANDBOX-INDEPENDENT (spec §2), so every probe runs
//! HOST-SIDE (the agent's `[sandbox]` is ignored). Strategy is chosen by `(kind, cmd)`:
//! - `kind=api` → OpenAI `GET {base_url}/models`;
//! - `cmd` basename `kiro-cli` → native `kiro-cli chat --list-models` (auth-free; its ACP handshake times
//!   out host-side because host kiro is unauthed — see spec §2);
//! - else (claude/codex) → a clean ACP `session/new` via [`AcpBackend::describe_options`].
//!
//! Every probe is bounded by [`PROBE_TIMEOUT`] and [`probe_all`] degrades per-agent: a failing/timing-out
//! agent is logged and OMITTED (the catalog holds only successes), so one bad agent never fails the rest.

use std::path::Path;
use std::time::Duration;

use bridge_acp::acp_backend::{AcpBackend, AcpConfig};
use bridge_core::catalog::{
    parse_kiro_list_models, parse_ollama_models, sanitize_model_caps, AgentCaps, ModelCatalog,
};
use bridge_core::domain::{AgentEntry, AgentKind};
use bridge_core::ports::AgentBackend; // for `.retire()`

/// Per-agent probe bound. The kiro host-side ACP hang made this load-bearing (spec §2): without it a
/// single unresponsive adapter would block startup/CLI forever.
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// Which discovery strategy an entry uses. Pure (kind/cmd only) so the dispatch decision is unit-tested
/// without spawning a real adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Strategy {
    Api,
    Kiro,
    Acp,
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
pub async fn probe_agent(entry: &AgentEntry, cwd: &Path) -> Result<AgentCaps, String> {
    let fut = async {
        match probe_strategy(entry) {
            Strategy::Api => probe_api(entry).await,
            Strategy::Kiro => probe_kiro(entry).await,
            Strategy::Acp => probe_acp_host(entry, cwd).await,
        }
    };
    match tokio::time::timeout(PROBE_TIMEOUT, fut).await {
        Ok(result) => result,
        Err(_) => Err("probe timed out".into()),
    }
}

/// kiro native list: `kiro-cli chat --list-models` (auth-free, no container needed).
async fn probe_kiro(entry: &AgentEntry) -> Result<AgentCaps, String> {
    let cmd = entry.cmd.clone().unwrap_or_else(|| "kiro-cli".into());
    let out = tokio::process::Command::new(&cmd)
        .args(["chat", "--list-models"])
        .output()
        .await
        .map_err(|e| format!("spawn {cmd}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{cmd} exited {:?}", out.status.code()));
    }
    Ok(parse_kiro_list_models(&String::from_utf8_lossy(
        &out.stdout,
    )))
}

/// api (ollama) list: OpenAI `GET {base_url}/models`. `base_url` already ends in `/v1` per the example
/// configs, so we append `/models`. The api backend never validates the model, so this is the only source.
async fn probe_api(entry: &AgentEntry) -> Result<AgentCaps, String> {
    let base = entry
        .base_url
        .as_deref()
        .ok_or("api agent missing base_url")?;
    let url = format!("{}/models", base.trim_end_matches('/'));
    let body = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("GET {url}: {e}"))?
        .text()
        .await
        .map_err(|e| format!("read {url}: {e}"))?;
    parse_ollama_models(&body).map_err(|e| format!("parse {url}: {e}"))
}

/// ACP host describe (claude/codex): spawn the adapter HOST-SIDE (sandbox stripped, no container reaper, no
/// MCP, nothing configured), read the advertised options via [`AcpBackend::describe_options`], then reap.
async fn probe_acp_host(entry: &AgentEntry, cwd: &Path) -> Result<AgentCaps, String> {
    let cmd = entry
        .cmd
        .as_deref()
        .ok_or("acp agent missing cmd")?
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
        container: None,
        mcp: Vec::new(),
        ..AcpConfig::default()
    };
    let backend = AcpBackend::spawn(&cmd, &argv, acp)
        .await
        .map_err(|e| format!("spawn {cmd}: {e}"))?;
    let caps = backend
        .describe_options(cwd)
        .await
        .map_err(|e| format!("describe_options: {e}"));
    // Graceful teardown (SIGTERM→SIGKILL of the host child). If the outer timeout cancels this future
    // mid-await, dropping `backend` SIGKILLs the child (`kill_on_drop`) — so neither path leaks.
    let _ = backend.retire().await;
    caps
}

/// Probe every entry concurrently; failures are logged + omitted (the catalog only holds successes).
pub async fn probe_all(entries: &[(String, AgentEntry)], cwd: &Path) -> ModelCatalog {
    let futs = entries
        .iter()
        .map(|(id, entry)| async move { (id.clone(), probe_agent(entry, cwd).await) });
    let results = futures::future::join_all(futs).await;
    collect_catalog(results)
}

/// Fold per-agent probe results into a catalog: keep `Ok`, log + DROP `Err` (graceful degradation). Pure,
/// so the degrade behavior is unit-tested without faking async adapters.
fn collect_catalog(results: Vec<(String, Result<AgentCaps, String>)>) -> ModelCatalog {
    results
        .into_iter()
        .filter_map(|(id, result)| match result {
            Ok(caps) => Some((id, sanitize_model_caps(caps))),
            Err(reason) => {
                tracing::warn!(agent = %id, %reason, "model probe failed; omitting from catalog");
                None
            }
        })
        .collect()
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

    #[test]
    fn collect_catalog_omits_failures() {
        let results = vec![
            (
                "ok".to_string(),
                Ok(AgentCaps {
                    models: vec!["m".into(), "claude-fable-5.1[1m]".into()],
                    ..Default::default()
                }),
            ),
            ("bad".to_string(), Err("boom".to_string())),
        ];
        let catalog = collect_catalog(results);
        assert!(catalog.contains_key("ok"), "ok agent kept");
        assert!(!catalog.contains_key("bad"), "failed agent omitted");
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog["ok"].models, vec!["m"]);
    }
}
