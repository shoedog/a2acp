// e2e_delegate_bridge.rs — Gated bridge-to-bridge end-to-end test (spec S2 + S2a, Task 11).
//
// Stands up two REAL bridge instances (A and B) in-process:
//   Bridge B: InboundServer wired to a real AcpBackend (kiro-cli acp) on an
//             ephemeral port. Serves the "kiro-code" skill.
//   Bridge A: InboundServer wired to PeerDelegation pointing at Bridge B, with
//             SkillRoute sending "delegate" requests to Bridge B.
//
// The test POSTs a `SendStreamingMessage` with skill="delegate" and text "reply PONG"
// to Bridge A. The request flows:
//   Client → Bridge A (inbound) → PeerDelegation → Bridge B (inbound) → AcpBackend
//
// Bridge B's SSE response streams back through Bridge A's delegation path and out as
// Bridge A's own SSE response to the test client.
//
// Asserts that Bridge A's SSE response contains an artifact-update frame with "PONG".
//
// Run command (requires `kiro-cli whoami` to succeed):
//   cargo test -p a2a-bridge --test e2e_delegate_bridge -- --ignored --nocapture

use std::sync::Arc;

use bridge_a2a_inbound::server::InboundServer;
use bridge_a2a_outbound::{PeerDelegation, StubDelegation};
use bridge_acp::{
    acp_backend::{AcpBackend, AcpConfig},
    supervisor::Supervised,
};
use bridge_core::domain::{RouteTarget, TaskMeta};
use bridge_core::error::BridgeError;
use bridge_core::ids::AgentId;
use bridge_core::ports::{DelegationPort, RouteDecision};
use bridge_policy::auth::AlwaysGrant;
use bridge_policy::permission::AutoPolicy;
use bridge_store::sqlite::SqliteStore;
use serde_json::json;

// ---- inline routes ----

/// Always routes to local Kiro (used by Bridge B).
struct AlwaysKiroRoute;

impl RouteDecision for AlwaysKiroRoute {
    fn route(&self, _meta: &TaskMeta) -> Result<RouteTarget, BridgeError> {
        Ok(RouteTarget::Local(AgentId::parse("kiro")?))
    }
}

/// Routes skill="delegate" to the peer; everything else to local Kiro (used by Bridge A).
struct E2eSkillRoute;

impl RouteDecision for E2eSkillRoute {
    fn route(&self, meta: &TaskMeta) -> Result<RouteTarget, BridgeError> {
        if meta.skill.as_deref() == Some("delegate") {
            Ok(RouteTarget::Delegate)
        } else {
            Ok(RouteTarget::Local(AgentId::parse("kiro")?))
        }
    }
}

// ---- helper: start an InboundServer on an ephemeral TCP port ----

/// Bind an ephemeral TCP port, serve the given router in a background task, and return
/// the base URL (`http://127.0.0.1:<port>`).
async fn serve_on_ephemeral_port(router: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral TCP port");
    let addr = listener.local_addr().expect("local_addr");
    let base_url = format!("http://127.0.0.1:{}", addr.port());
    tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("axum serve must not error");
    });
    base_url
}

// ---- the gated bridge-to-bridge e2e test ----

#[ignore = "needs authenticated kiro-cli + two bridge instances"]
#[tokio::test]
async fn bridge_a_delegates_through_bridge_b_to_kiro() {
    // ----------------------------------------------------------------
    // Bridge B — real Kiro backend, no delegation (serves kiro-code).
    // ----------------------------------------------------------------

    let supervised_b = Supervised::spawn("kiro-cli", &["acp"])
        .expect("kiro-cli must be on PATH and authenticated; run `kiro-cli whoami` first");
    let backend_b = Arc::new(
        AcpBackend::from_child(
            supervised_b,
            AcpConfig {
                cwd: std::env::current_dir().expect("cwd"),
                ..AcpConfig::default()
            },
        )
        .await
        .expect("ACP connection initializes (B)"),
    );
    let store_b = Arc::new(SqliteStore::open_in_memory().expect("sqlite in-memory (B)"));

    let server_b = Arc::new(InboundServer::new(
        backend_b,
        store_b,
        Arc::new(AutoPolicy),
        Arc::new(AlwaysKiroRoute),
        Arc::new(AlwaysGrant),
        "http://127.0.0.1:0", // placeholder; real URL built after bind
        Arc::new(StubDelegation),
    ));
    let router_b = server_b.router();
    let url_b = serve_on_ephemeral_port(router_b).await;

    // Give Bridge B (and kiro-cli) a moment to be ready.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // ----------------------------------------------------------------
    // Bridge A — PeerDelegation → Bridge B, SkillRoute.
    // ----------------------------------------------------------------

    let delegation_a: Arc<dyn DelegationPort> = Arc::new(PeerDelegation::new(
        &url_b,
        "bearer:test-token",
        std::time::Duration::from_secs(60),
    ));
    let store_a = Arc::new(SqliteStore::open_in_memory().expect("sqlite in-memory (A)"));

    // Bridge A has no local backend (any request routed Local would be an error
    // in a real deployment, but the route ensures delegate skill always goes to B).
    let backend_a: Arc<dyn bridge_core::ports::AgentBackend> = Arc::new(BridgeABackend);

    let server_a = Arc::new(InboundServer::new(
        backend_a,
        store_a,
        Arc::new(AutoPolicy),
        Arc::new(E2eSkillRoute),
        Arc::new(AlwaysGrant),
        "http://127.0.0.1:0", // placeholder
        delegation_a,
    ));
    let router_a = server_a.router();
    let url_a = serve_on_ephemeral_port(router_a).await;

    // Give Bridge A a moment to be ready.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // ----------------------------------------------------------------
    // POST SendStreamingMessage to Bridge A.
    // ----------------------------------------------------------------

    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "SendStreamingMessage",
        "params": {
            "message": {
                "text": "Reply with exactly the single word PONG and nothing else.",
                "metadata": { "a2a-bridge.skill": "delegate" }
            }
        }
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .expect("reqwest client must build");

    let response = client
        .post(format!("{url_a}/"))
        .header("Content-Type", "application/json")
        .header("A2A-Version", "1.0")
        .json(&body)
        .send()
        .await
        .expect("HTTP request to Bridge A must succeed");

    assert!(
        response.status().is_success(),
        "Bridge A must return 2xx; got {}",
        response.status()
    );

    let body_text = response
        .text()
        .await
        .expect("response body must be valid UTF-8");

    eprintln!(
        "=== Bridge A SSE response body ===\n{body_text}\n=================================="
    );

    // ----------------------------------------------------------------
    // Assert S2: the artifact came through A → B → Kiro.
    // ----------------------------------------------------------------
    assert!(
        body_text.contains("artifact-update"),
        "SSE body must contain an 'artifact-update' frame; got:\n{body_text}"
    );
    assert!(
        body_text.to_ascii_uppercase().contains("PONG"),
        "SSE body must contain 'PONG' (case-insensitive) from Kiro via Bridge B; got:\n{body_text}"
    );

    // Wire-conformance: all data: payloads must parse as a2a::StreamResponse.
    let payloads: Vec<String> = body_text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|s| s.trim_end_matches('\r').to_owned())
        .collect();

    assert!(
        !payloads.is_empty(),
        "no data payloads in SSE body: {body_text}"
    );

    for payload in &payloads {
        let _: a2a::StreamResponse = serde_json::from_str(payload).unwrap_or_else(|e| {
            panic!("data payload must parse as StreamResponse: {e}: {payload}")
        });
    }

    // Final frame: terminal statusUpdate(Completed) synthesized after the stream ends.
    let last = payloads.last().unwrap();
    let sr: a2a::StreamResponse = serde_json::from_str(last).unwrap();
    assert!(
        matches!(
            &sr,
            a2a::StreamResponse::StatusUpdate(e)
                if e.status.state == a2a::TaskState::Completed
        ),
        "final SSE frame must be terminal statusUpdate(Completed): {last}"
    );
    // Penultimate frame must be the ArtifactUpdate.
    let penultimate = &payloads[payloads.len() - 2];
    let sr2: a2a::StreamResponse = serde_json::from_str(penultimate).unwrap();
    assert!(
        matches!(sr2, a2a::StreamResponse::ArtifactUpdate(_)),
        "penultimate SSE frame must be ArtifactUpdate: {penultimate}"
    );
}

// ---- stub local backend for Bridge A (never called on the delegate path) ----

struct BridgeABackend;

#[async_trait::async_trait]
impl bridge_core::ports::AgentBackend for BridgeABackend {
    async fn prompt(
        &self,
        _s: &bridge_core::ids::SessionId,
        _p: Vec<bridge_core::domain::Part>,
    ) -> Result<bridge_core::ports::BackendStream, BridgeError> {
        Err(BridgeError::UpstreamA2aError)
    }
    async fn cancel(&self, _s: &bridge_core::ids::SessionId) -> Result<(), BridgeError> {
        Ok(())
    }
}
