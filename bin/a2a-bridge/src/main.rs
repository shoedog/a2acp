// main.rs — A2A Bridge v2.5 composition root (spec §8, Task 15 / Task 10).
//
// Wires all port implementations together into a runnable binary:
//   AlwaysGrant (auth) -> SkillRoute (route) -> AcpBackend (backend)
//   AutoPolicy (policy) | SqliteStore (store) | PeerDelegation or StubDelegation (delegation)
//
// Server listens on cfg.server.addr (default 127.0.0.1:8080).

mod config;
mod route;

use std::sync::Arc;
use std::time::Duration;

use bridge_a2a_inbound::server::InboundServer;
use bridge_a2a_outbound::{PeerDelegation, StubDelegation};
use bridge_acp::acp_backend::{AcpBackend, AcpConfig};
use bridge_core::domain::{AgentEntry, RegistrySnapshot};
use bridge_core::ids::AgentId;
use bridge_core::ports::{DelegationPort, PolicyEngine};
use bridge_policy::{auth::AlwaysGrant, permission::AutoPolicy};
use bridge_registry::registry::Registry;
use bridge_store::sqlite::SqliteStore;
use config::Config;
use route::SkillRoute;

/// Built-in default config used when `a2a-bridge.toml` is absent.
const DEFAULT_CONFIG: &str = r#"
[agent]
name = "kiro"
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

    // 2. Configuration — read a2a-bridge.toml if present, else use built-in defaults.
    let toml_src = match std::fs::read_to_string("a2a-bridge.toml") {
        Ok(s) => s,
        Err(_) => DEFAULT_CONFIG.to_string(),
    };
    let cfg = Config::parse(&toml_src)?;

    // 3. Build the policy engine FIRST so the SAME engine drives both the inbound
    //    server's permission decisions AND the backend's REVERSE
    //    `session/request_permission` decisions (threaded via `with_policy`), so
    //    the system applies one consistent permission policy in both directions.
    let policy = Arc::new(AutoPolicy);

    // 4. Spawn the agent child process and drive the conformant ACP connection
    //    over its stdio via the SDK. `AcpBackend::spawn` runs initialize →
    //    authenticate, owns the `Supervised` child for the backend's lifetime, and
    //    applies the configured mode/model after each `session/new`.
    let args_ref: Vec<&str> = cfg.agent.args.iter().map(String::as_str).collect();
    let acp_config = AcpConfig {
        // Absolute working directory the agent runs sessions in (ACP §11A).
        cwd: cfg.agent.resolve_cwd()?,
        model: cfg.agent.model.clone(),
        mode: cfg.agent.mode.clone(),
        auth_method: cfg.agent.auth_method.clone(),
        ..AcpConfig::default()
    };
    let backend = Arc::new(
        AcpBackend::spawn(&cfg.agent.cmd, &args_ref, acp_config)
            .await?
            // Thread the system policy into the backend so its reverse-permission
            // decisions match the inbound server's policy (Task 5/6).
            .with_policy(Arc::clone(&policy) as Arc<dyn PolicyEngine>),
    );

    // 5. Build the remaining port Arc<dyn Trait> wrappers.
    let auth = Arc::new(AlwaysGrant);

    // Build a single-entry registry for the configured [agent] so SkillRoute can
    // use default_id() to fall back to it. Task 12 will replace this with the full
    // multi-agent RegistryConfig wiring; for now a minimal snapshot keeps main green.
    let agent_id = AgentId::parse(cfg.agent.name.clone())?;
    let agent_entry = AgentEntry {
        id: agent_id.clone(),
        cmd: cfg.agent.cmd.clone(),
        args: cfg.agent.args.clone(),
        model_provider: None,
        model: cfg.agent.model.clone(),
        effort: None,
        mode: cfg.agent.mode.clone(),
        cwd: cfg.agent.cwd.clone(),
        auth_method: cfg.agent.auth_method.clone(),
        name: Some(cfg.agent.name.clone()),
        description: None,
        tags: vec![],
        version: None,
        extensions: Default::default(),
    };
    let single_snap = RegistrySnapshot {
        default: agent_id,
        entries: vec![agent_entry],
        allowed_cmds: vec![cfg.agent.cmd.clone()],
    };
    // SpawnFn for the single-agent registry: reuse the already-spawned backend.
    let backend_for_spawn = Arc::clone(&backend);
    let spawn_fn: bridge_registry::registry::SpawnFn = Arc::new(move |_entry| {
        let b = Arc::clone(&backend_for_spawn);
        Box::pin(async move { Ok(b as Arc<dyn bridge_core::ports::AgentBackend>) })
    });
    let registry = Arc::new(Registry::new(single_snap, spawn_fn)?);
    let route = Arc::new(SkillRoute::new(registry.clone()));
    let store = Arc::new(SqliteStore::open_in_memory()?);
    // Delegation port: real PeerDelegation when [delegation] is configured; StubDelegation otherwise.
    let delegation: Arc<dyn DelegationPort> = match &cfg.delegation {
        Some(d) => Arc::new(PeerDelegation::new(
            &d.peer_url,
            &d.auth,
            Duration::from_secs(d.timeout_secs),
        )),
        None => Arc::new(StubDelegation),
    };

    // 6. Construct the inbound server and build its axum router.
    //    InboundServer::new(registry, store, policy, route, auth, base_url, delegation, local_source_label)
    // The inbound server now holds the agent registry (3b): first-message LOCAL
    // dispatch resolves the routed agent id, applies its effective config, and binds
    // the task. The local-source label (wire-observable in fan-out artifacts) comes
    // from `[agent] name` so a non-Kiro agent (e.g. codex) isn't mislabeled "kiro".
    let base_url = format!("http://{}", cfg.server.addr);
    let server = Arc::new(InboundServer::new(
        registry,
        store,
        policy,
        route,
        auth,
        base_url,
        delegation,
        cfg.agent.name.clone(),
    ));
    let router = server.router();

    // 7. Bind and serve.
    let listener = tokio::net::TcpListener::bind(&cfg.server.addr).await?;
    tracing::info!(addr = %cfg.server.addr, agent = %cfg.agent.name, "a2a-bridge listening");
    axum::serve(listener, router).await?;

    Ok(())
}
