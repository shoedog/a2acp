// main.rs — A2A Bridge v3b composition root (spec §7/§8, Task 12).
//
// Wires all port implementations together into a runnable binary:
//   AlwaysGrant (auth) -> SkillRoute (route) -> Registry -> AcpBackend (backend)
//   AutoPolicy (policy) | SqliteStore (store) | PeerDelegation or StubDelegation (delegation)
//
// The agent registry is driven by a `FileConfigSource` over `a2a-bridge.toml`:
// the initial `load()` is the desired state `Registry::new` VALIDATES (boot fails
// loud on bad config, spec §7), and a detached reconcile loop applies every
// subsequent `watch()` snapshot so on-disk edits hot-reload the live registry.
//
// Server listens on cfg.server.addr (default 127.0.0.1:8080).

mod config;
mod route;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bridge_a2a_inbound::server::InboundServer;
use bridge_a2a_outbound::{PeerDelegation, StubDelegation};
use bridge_acp::acp_backend::{AcpBackend, AcpConfig};
use bridge_core::domain::AgentEntry;
use bridge_core::error::BridgeError;
use bridge_core::ports::{AgentBackend, AgentRegistry, ConfigSource, DelegationPort, PolicyEngine};
use bridge_policy::{auth::AlwaysGrant, permission::AutoPolicy};
use bridge_registry::registry::{Registry, SpawnFn};
use bridge_store::sqlite::SqliteStore;
use config::{FileConfigSource, RegistryConfig};
use route::SkillRoute;

/// Path of the on-disk registry config the bridge watches + hot-reloads.
const CONFIG_PATH: &str = "a2a-bridge.toml";

/// Built-in default config (new 3b registry schema) materialised to disk when
/// `a2a-bridge.toml` is absent, so the `FileConfigSource` load/watch pipeline has
/// a concrete path to read and the parent directory to watch. A sensible
/// single-agent default: one `kiro` agent running `kiro-cli acp`.
const DEFAULT_CONFIG: &str = r#"default = "kiro"

[registry]
allowed_cmds = ["kiro-cli"]

[[agents]]
id   = "kiro"
cmd  = "kiro-cli"
args = ["acp"]

[server]
addr = "127.0.0.1:8080"
"#;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    // 1. Observability — install tracing subscriber (idempotent).
    bridge_observ::init();

    // 2. Configuration — ensure `a2a-bridge.toml` exists (materialise the built-in
    //    default if absent) so the FileConfigSource has a concrete file to load +
    //    a parent directory to watch.
    let config_path = PathBuf::from(CONFIG_PATH);
    if !config_path.exists() {
        std::fs::write(&config_path, DEFAULT_CONFIG)?;
    }

    // 3. Build the policy engine FIRST so the SAME engine drives both the inbound
    //    server's permission decisions AND each backend's REVERSE
    //    `session/request_permission` decisions (threaded via `with_policy`), so
    //    the system applies one consistent permission policy in both directions.
    let policy = Arc::new(AutoPolicy);

    // 4. SpawnFn — the registry's adapter factory. Lazily spawns a real AcpBackend
    //    per entry: it runs initialize → authenticate, owns the `Supervised` child
    //    for the backend's lifetime, and applies the configured mode/model after
    //    each `session/new`. `model`/`mode` here are the per-MINT FALLBACK; the
    //    per-session `configure_session` overrides them at dispatch (Task 6).
    let policy_for_spawn = Arc::clone(&policy) as Arc<dyn PolicyEngine>;
    let spawn: SpawnFn = Arc::new(move |entry: Arc<AgentEntry>| {
        let policy = Arc::clone(&policy_for_spawn);
        Box::pin(async move {
            // Absolute working directory (ACP §11A). A relative configured value
            // is joined onto current_dir() to become absolute; an absolute one is
            // used as-is; absent falls back to the bridge's current directory.
            let cwd = match entry.cwd.clone() {
                Some(c) => {
                    let p = PathBuf::from(c);
                    if p.is_absolute() {
                        p
                    } else {
                        std::env::current_dir()
                            .map_err(|e| BridgeError::ConfigInvalid {
                                reason: format!("cwd: {e}"),
                            })?
                            .join(p)
                    }
                }
                None => std::env::current_dir().map_err(|e| BridgeError::ConfigInvalid {
                    reason: format!("cwd: {e}"),
                })?,
            };
            let args: Vec<String> = entry.args.clone();
            let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
            use bridge_core::domain::AgentKind;
            match entry.kind {
                AgentKind::Acp => {
                    let acp = AcpConfig {
                        cwd,
                        model: entry.model.clone(),
                        mode: entry.mode.clone(),
                        auth_method: entry.auth_method.clone(),
                        // handshake_timeout / cancel_grace: reuse the codebase defaults.
                        ..AcpConfig::default()
                    };
                    let be = AcpBackend::spawn(&entry.cmd, &args_ref, acp)
                        .await?
                        // Thread the system policy into the backend so its reverse-permission
                        // decisions match the inbound server's policy (Task 5/6).
                        .with_policy(policy);
                    Ok(Arc::new(be) as Arc<dyn AgentBackend>)
                }
            }
        })
    });

    // 5. Config source + registry. `load()` is the initial desired state; the
    //    delegation `[delegation]` env-expansion + `[server] addr` ride along on
    //    the RegistryConfig we re-read below for the non-registry fields.
    let source = FileConfigSource::new(config_path.clone());
    let snapshot = source.load().await?; // initial desired state
                                         // Fan-out source label (wire-observable in fan-out artifacts): the default
                                         // entry's `name` if set, else the default agent id, so a non-Kiro default
                                         // (e.g. codex) isn't mislabeled "kiro".
    let default_label = snapshot
        .entries
        .iter()
        .find(|e| e.id == snapshot.default)
        .and_then(|e| e.name.clone())
        .unwrap_or_else(|| snapshot.default.as_str().to_string());
    // Registry::new VALIDATES the snapshot → boot fails loud on bad config (spec §7).
    let registry = Arc::new(Registry::new(snapshot, spawn)?);

    // 6. Reconcile loop — consume `watch()` and `apply()` each new snapshot so
    //    on-disk edits hot-reload the live registry. The watch stream is held for
    //    the bridge's lifetime by the spawned task.
    {
        let reg = Arc::clone(&registry);
        let mut watch = source.watch();
        tokio::spawn(async move {
            use futures::StreamExt;
            while let Some(snap) = watch.next().await {
                if let Err(e) = reg.apply(snap).await {
                    tracing::error!(error = ?e, "registry reconcile failed");
                }
            }
        });
    }

    // 7. Build the remaining port Arc<dyn Trait> wrappers.
    let auth = Arc::new(AlwaysGrant);
    let route = Arc::new(SkillRoute::new(Arc::clone(&registry) as _));
    let store = Arc::new(SqliteStore::open_in_memory()?);

    // Read the non-registry config sections (server addr, delegation) directly:
    // the FileConfigSource snapshot only carries the registry. Re-parsing the same
    // file is cheap and keeps the [server]/[delegation] parsing (incl. env-expansion)
    // working on the RegistryConfig path.
    let raw = std::fs::read_to_string(&config_path)?;
    let cfg = RegistryConfig::parse(&raw)?;

    // Delegation port: real PeerDelegation when [delegation] is configured; StubDelegation otherwise.
    let delegation: Arc<dyn DelegationPort> = match &cfg.delegation {
        Some(d) => Arc::new(PeerDelegation::new(
            &d.peer_url,
            &d.auth,
            Duration::from_secs(d.timeout_secs),
        )),
        None => Arc::new(StubDelegation),
    };

    // 8. Construct the inbound server and build its axum router.
    //    InboundServer::new(registry, store, policy, route, auth, base_url, delegation, local_source_label)
    // The inbound server holds the agent registry (3b): first-message LOCAL dispatch
    // resolves the routed agent id, applies its effective config, and binds the task.
    let base_url = format!("http://{}", cfg.server.addr);
    let server = Arc::new(InboundServer::new(
        Arc::clone(&registry) as _,
        store,
        policy,
        route,
        auth,
        base_url,
        delegation,
        default_label.clone(),
    ));
    let router = server.router();

    // 9. Bind and serve.
    let listener = tokio::net::TcpListener::bind(&cfg.server.addr).await?;
    tracing::info!(addr = %cfg.server.addr, agent = %default_label, "a2a-bridge listening");
    axum::serve(listener, router).await?;

    Ok(())
}
