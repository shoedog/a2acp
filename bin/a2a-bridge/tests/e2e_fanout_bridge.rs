// e2e_fanout_bridge.rs — Gated bridge-to-bridge fan-out end-to-end test (spec S3, Task 7).
//
// Stands up two REAL bridge instances (A and B) in-process:
//
//   Bridge B: InboundServer wired to a real AcpBackend (kiro-cli acp) on an
//             ephemeral port. Serves the "kiro-code" skill via AlwaysKiroRoute.
//             Bridge B is the "peer" from A's perspective.
//
//   Bridge A: InboundServer wired to:
//             - ReplayBackend (local Kiro side; yields a "KIRO_ART" artifact)
//             - PeerDelegation pointing at Bridge B (peer side; yields whatever Kiro returns)
//             - FanoutSkillRoute: skill="fan-out" -> RouteTarget::Fanout
//
// The test POSTs a `SendStreamingMessage` with skill="fan-out" and text "reply PONG"
// to Bridge A. The request flows fan-out style:
//
//   Client
//     -> Bridge A (InboundServer / Fanout)
//          -> local ReplayBackend          => source=kiro artifact
//          -> PeerDelegation -> Bridge B  => source=peer artifact (via Kiro)
//     <- merged SSE (both artifacts + terminal Completed)
//
// Asserts:
//   1. Bridge A's SSE contains a source=kiro ArtifactUpdate.
//   2. Bridge A's SSE contains a source=peer ArtifactUpdate.
//   3. The LAST SSE frame is a terminal statusUpdate(Completed).
//   4. All data: payloads parse as a2a::StreamResponse (wire-conformance).
//
// Run command (requires `kiro-cli whoami` to succeed):
//   cargo test -p a2a-bridge --test e2e_fanout_bridge -- --ignored --nocapture

use std::sync::Arc;

mod common;

use bridge_a2a_inbound::server::InboundServer;
use bridge_a2a_outbound::{PeerDelegation, StubDelegation};
use bridge_acp::{
    acp_backend::{AcpBackend, AcpConfig},
    replay::ReplayBackend,
};
use bridge_core::domain::{RouteTarget, TaskMeta};
use bridge_core::error::BridgeError;
use bridge_core::ids::AgentId;
use bridge_core::ports::{DelegationPort, RouteDecision};
use bridge_core::process::Supervised;
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

/// Routes skill="fan-out" to Fanout (local + peer); everything else to local Kiro.
/// Used by Bridge A.
struct FanoutSkillRoute;

impl RouteDecision for FanoutSkillRoute {
    fn route(&self, meta: &TaskMeta) -> Result<RouteTarget, BridgeError> {
        if meta.skill.as_deref() == Some("fan-out") {
            Ok(RouteTarget::Fanout)
        } else {
            Ok(RouteTarget::Local(AgentId::parse("kiro")?))
        }
    }
}

// ---- helper: serve a router on an ephemeral TCP port ----

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

// ---- NDJSON for Bridge A's local ReplayBackend ----

/// Yields one `session/update` text "KIRO_ART" then a Done frame.
fn kiro_ndjson() -> Vec<u8> {
    let text_frame = r#"{"method":"session/update","params":{"text":"KIRO_ART"}}"#;
    let done_frame = r#"{"result":{"stopReason":"end_turn"}}"#;
    format!("{text_frame}\n{done_frame}\n").into_bytes()
}

// ---- the gated bridge-to-bridge fan-out e2e test ----

#[ignore = "needs authenticated kiro-cli + two bridge instances"]
#[tokio::test]
async fn bridge_a_fanout_through_bridge_b_to_kiro() {
    // ----------------------------------------------------------------
    // Bridge B — real Kiro backend, no delegation (serves kiro-code).
    // ----------------------------------------------------------------

    let supervised_b = Supervised::spawn("kiro-cli", &["acp"], None)
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
        common::single_agent_registry("kiro", backend_b),
        store_b,
        Arc::new(AutoPolicy),
        Arc::new(AlwaysKiroRoute),
        Arc::new(AlwaysGrant),
        "http://127.0.0.1:0", // placeholder; real URL built after bind
        Arc::new(StubDelegation),
        "kiro",
    ));
    let router_b = server_b.router();
    let url_b = serve_on_ephemeral_port(router_b).await;

    // Give Bridge B (and kiro-cli) a moment to be ready.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // ----------------------------------------------------------------
    // Bridge A — fan-out:
    //   local side  = ReplayBackend (KIRO_ART)
    //   peer side   = PeerDelegation -> Bridge B -> real Kiro
    // ----------------------------------------------------------------

    let backend_a = Arc::new(ReplayBackend::from_ndjson(kiro_ndjson()));
    let store_a = Arc::new(SqliteStore::open_in_memory().expect("sqlite in-memory (A)"));
    let delegation_a: Arc<dyn DelegationPort> = Arc::new(PeerDelegation::new(
        &url_b,
        "bearer:test-token",
        std::time::Duration::from_secs(120),
    ));

    let server_a = Arc::new(InboundServer::new(
        common::single_agent_registry("kiro", backend_a),
        store_a,
        Arc::new(AutoPolicy),
        Arc::new(FanoutSkillRoute),
        Arc::new(AlwaysGrant),
        "http://127.0.0.1:0", // placeholder
        delegation_a,
        "kiro",
    ));
    let router_a = server_a.router();
    let url_a = serve_on_ephemeral_port(router_a).await;

    // Give Bridge A a moment to be ready.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // ----------------------------------------------------------------
    // POST SendStreamingMessage to Bridge A with skill="fan-out".
    // ----------------------------------------------------------------

    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "SendStreamingMessage",
        "params": {
            "message": {
                "text": "reply PONG",
                "metadata": { "a2a-bridge.skill": "fan-out" }
            }
        }
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
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
        "=== Bridge A fan-out SSE response body ===\n{body_text}\n========================================="
    );

    // ----------------------------------------------------------------
    // Parse every data: payload as a2a::StreamResponse (wire-conformance).
    // ----------------------------------------------------------------

    let payloads: Vec<String> = body_text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|s| s.trim_end_matches('\r').to_owned())
        .collect();

    assert!(
        !payloads.is_empty(),
        "no data: payloads in SSE body: {body_text}"
    );

    let stream_responses: Vec<a2a::StreamResponse> = payloads
        .iter()
        .map(|p| {
            serde_json::from_str(p)
                .unwrap_or_else(|e| panic!("data payload must parse as StreamResponse: {e}: {p}"))
        })
        .collect();

    // ----------------------------------------------------------------
    // S3.1: both source=kiro and source=peer ArtifactUpdate frames present.
    // ----------------------------------------------------------------

    let has_kiro_artifact = stream_responses.iter().any(|sr| {
        if let a2a::StreamResponse::ArtifactUpdate(e) = sr {
            e.metadata
                .as_ref()
                .and_then(|m| m.get("a2a-bridge.source"))
                .and_then(|v| v.as_str())
                == Some("kiro")
        } else {
            false
        }
    });
    assert!(
        has_kiro_artifact,
        "SSE stream must contain an ArtifactUpdate with metadata[a2a-bridge.source]==\"kiro\": {body_text}"
    );

    let has_peer_artifact = stream_responses.iter().any(|sr| {
        if let a2a::StreamResponse::ArtifactUpdate(e) = sr {
            e.metadata
                .as_ref()
                .and_then(|m| m.get("a2a-bridge.source"))
                .and_then(|v| v.as_str())
                == Some("peer")
        } else {
            false
        }
    });
    assert!(
        has_peer_artifact,
        "SSE stream must contain an ArtifactUpdate with metadata[a2a-bridge.source]==\"peer\" (via Bridge B -> Kiro): {body_text}"
    );

    // ----------------------------------------------------------------
    // S3.2: the LAST frame is a terminal statusUpdate(Completed).
    // ----------------------------------------------------------------

    let last = payloads.last().unwrap();
    let last_sr: a2a::StreamResponse = serde_json::from_str(last).unwrap();
    assert!(
        matches!(
            &last_sr,
            a2a::StreamResponse::StatusUpdate(e)
                if e.status.state == a2a::TaskState::Completed
        ),
        "the LAST SSE frame must be a terminal statusUpdate(Completed): {last}"
    );
}
