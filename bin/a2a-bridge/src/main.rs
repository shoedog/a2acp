// main.rs — A2A Bridge v1 composition root (spec §8, Task 15).
//
// Wires all port implementations together into a runnable binary:
//   AlwaysGrant (auth) -> AlwaysKiro (route) -> KiroBackend (backend)
//   AutoPolicy (policy) | SqliteStore (store) | StubDelegation (delegation)
//
// Server listens on cfg.server.addr (default 127.0.0.1:8080).

mod config;
mod route;

use std::sync::Arc;

use bridge_a2a_inbound::server::InboundServer;
use bridge_a2a_outbound::StubDelegation;
use bridge_acp::{kiro::KiroBackend, supervisor::Supervised};
use bridge_policy::{auth::AlwaysGrant, permission::AutoPolicy};
use bridge_store::sqlite::SqliteStore;
use config::Config;
use route::AlwaysKiro;

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

    // 3. Spawn the agent child process.
    let args_ref: Vec<&str> = cfg.agent.args.iter().map(String::as_str).collect();
    let supervised = Supervised::spawn(&cfg.agent.cmd, &args_ref)?;
    let backend = Arc::new(KiroBackend::from_child(supervised));

    // 4. Build all port Arc<dyn Trait> wrappers.
    let auth = Arc::new(AlwaysGrant);
    let policy = Arc::new(AutoPolicy);
    let route = Arc::new(AlwaysKiro);
    let store = Arc::new(SqliteStore::open_in_memory()?);
    // StubDelegation is not passed to InboundServer (unused on happy path) but is
    // constructed here to prove the composition root is wired end-to-end.
    let _delegation = Arc::new(StubDelegation);

    // 5. Construct the inbound server and build its axum router.
    //    InboundServer::new(backend, store, policy, route, auth, base_url)
    let base_url = format!("http://{}", cfg.server.addr);
    let server = Arc::new(InboundServer::new(
        backend, store, policy, route, auth, base_url,
    ));
    let router = server.router();

    // 6. Bind and serve.
    let listener = tokio::net::TcpListener::bind(&cfg.server.addr).await?;
    tracing::info!(addr = %cfg.server.addr, agent = %cfg.agent.name, "a2a-bridge listening");
    axum::serve(listener, router).await?;

    Ok(())
}
