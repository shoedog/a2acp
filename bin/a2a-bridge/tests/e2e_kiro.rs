// e2e_kiro.rs — Gated real-Kiro end-to-end smoke test (spec S1, Task 17).
//
// This test stands up the REAL pipeline against a live `kiro-cli acp` process:
//   Supervised::spawn("kiro-cli", ["acp"])
//     -> KiroBackend::from_child
//     -> InboundServer (AlwaysGrant, AutoPolicy, inline RouteDecision -> "kiro")
//     -> axum server on an ephemeral TCP port
//
// It then sends a real A2A `SendStreamingMessage` JSON-RPC call via reqwest,
// reads the SSE stream body, and asserts that:
//   1. The body contains an `artifact-update` frame.
//   2. The body contains `PONG` (case-insensitive match; we ask kiro to say it).
//
// Run command (requires `kiro-cli whoami` to succeed):
//   cargo test -p a2a-bridge --test e2e_kiro -- --ignored --nocapture
//
// NOT run in default CI (the test is `#[ignore]`).

use std::sync::Arc;

use bridge_a2a_inbound::server::InboundServer;
use bridge_acp::{kiro::KiroBackend, supervisor::Supervised};
use bridge_core::domain::TaskMeta;
use bridge_core::error::BridgeError;
use bridge_core::ids::AgentId;
use bridge_core::ports::RouteDecision;
use bridge_policy::auth::AlwaysGrant;
use bridge_policy::permission::AutoPolicy;
use bridge_store::sqlite::SqliteStore;
use serde_json::json;

// ---- inline route (mirrors main.rs AlwaysKiro) ----

struct E2eKiroRoute;

impl RouteDecision for E2eKiroRoute {
    fn route(&self, _meta: &TaskMeta) -> Result<AgentId, BridgeError> {
        AgentId::parse("kiro")
    }
}

// ---- the smoke test ----

#[ignore = "needs an authenticated kiro-cli on PATH"]
#[tokio::test]
async fn real_kiro_round_trip_returns_pong() {
    // 1. Spawn the real kiro-cli agent child process.
    let supervised = Supervised::spawn("kiro-cli", &["acp"])
        .expect("kiro-cli must be on PATH and executable; run `kiro-cli whoami` first");

    let backend = Arc::new(KiroBackend::from_child(supervised));

    // 2. Wire all ports — mirrors the composition root in main.rs exactly.
    let auth = Arc::new(AlwaysGrant);
    let policy = Arc::new(AutoPolicy);
    let route = Arc::new(E2eKiroRoute);
    let store = Arc::new(SqliteStore::open_in_memory().expect("sqlite in-memory must open"));

    // Bind ephemeral port before building the server so we know the port.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local_addr");
    let base_url = format!("http://{addr}");

    let server = Arc::new(InboundServer::new(
        backend,
        store,
        policy,
        route,
        auth,
        base_url.clone(),
    ));
    let router = server.router();

    // 3. Serve on the ephemeral port in a background task.
    tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("axum serve must not error");
    });

    // 4. Give the server and kiro a moment to be ready.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // 5. Build the A2A SendStreamingMessage JSON-RPC body.
    //    Shape mirrors the server unit tests (server.rs tests::post_request).
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "SendStreamingMessage",
        "params": {
            "message": {
                "text": "Reply with exactly the single word PONG and nothing else."
            }
        }
    });

    // 6. POST to the ephemeral server; read the full SSE response body.
    let client = reqwest::Client::builder()
        .build()
        .expect("reqwest client must build");

    let response = client
        .post(format!("{base_url}/"))
        .header("Content-Type", "application/json")
        .header("A2A-Version", "1.0")
        .json(&body)
        .send()
        .await
        .expect("HTTP request must succeed");

    assert!(
        response.status().is_success(),
        "server must return a 2xx status; got {}",
        response.status()
    );

    // Collect the full SSE body (non-incremental is fine for a smoke test).
    let body_text = response
        .text()
        .await
        .expect("response body must be valid UTF-8");

    eprintln!("=== SSE response body ===\n{body_text}\n=========================");

    // 7. Assert success criteria S1.
    assert!(
        body_text.contains("artifact-update"),
        "SSE body must contain an 'artifact-update' frame; got:\n{body_text}"
    );
    assert!(
        body_text.to_ascii_uppercase().contains("PONG"),
        "SSE body must contain 'PONG' (case-insensitive); got:\n{body_text}"
    );
}
