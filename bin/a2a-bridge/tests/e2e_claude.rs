//! Increment 3c inbound e2e: a real ClaudeCliBackend (driving a python fake claude)
//! behind the InboundServer; two sequential turns on one TaskId must reach the SAME
//! warm proc with retained context, AND turn 2 is sent only AFTER the backend
//! observed forget_session for turn 1 (synchronized — no false-pass).
mod common;

use axum::http::{Request, StatusCode};
use bridge_a2a_inbound::server::InboundServer;
use bridge_claude::{ClaudeCliBackend, ClaudeConfig};
use bridge_core::domain::{EffectiveConfig, Part, RouteTarget, TaskMeta};
use bridge_core::error::BridgeError;
use bridge_core::ids::{AgentId, SessionId};
use bridge_core::ports::{AgentBackend, BackendStream, PolicyEngine, RouteDecision, SessionStore};
use bridge_policy::auth::AlwaysGrant;
use bridge_policy::permission::AutoPolicy;
use bridge_store::sqlite::SqliteStore;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex, OnceLock};
use tower::ServiceExt;

// ---- SSE payload helpers ----

/// Extract all `data:` payloads from an SSE body (one per line starting with "data: ").
fn sse_data_payloads(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|s| s.trim_end_matches('\r').to_owned())
        .collect()
}

/// Extract and concatenate the agent's reply text from artifact parts only —
/// deliberately excludes envelope fields (taskId, contextId, messageId, etc.)
/// so that a UUID containing '7' cannot cause a false pass.
///
/// Parses every `data:` payload as an `a2a::StreamResponse`; for each
/// `ArtifactUpdate` frame, collects every `Part` whose content is `Text`.
fn sse_agent_text(body: &str) -> String {
    sse_data_payloads(body)
        .iter()
        .filter_map(|payload| serde_json::from_str::<a2a::StreamResponse>(payload).ok())
        .filter_map(|sr| {
            if let a2a::StreamResponse::ArtifactUpdate(ev) = sr {
                Some(ev.artifact.parts)
            } else {
                None
            }
        })
        .flatten()
        .filter_map(|part| {
            if let a2a::PartContent::Text(t) = part.content {
                Some(t)
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("")
}

// ---- python fake (self-contained; default behavior = remember the number) ----
const FAKE_PY: &str = r#"#!/usr/bin/env python3
import sys, json
out = sys.stdout
out.write(json.dumps({"type":"system","subtype":"init","session_id":"fake-sid"})+"\n"); out.flush()
memory = None
for line in sys.stdin:
    line=line.strip()
    if not line: continue
    try: v=json.loads(line)
    except Exception: continue
    try: text=v["message"]["content"][0]["text"]
    except Exception: text=""
    for w in text.split():
        if w.lstrip("-").isdigit(): memory=w
    reply = memory if memory is not None else "OK"
    out.write(json.dumps({"type":"assistant","message":{"content":[{"type":"text","text":reply}]}})+"\n")
    out.write(json.dumps({"type":"result","subtype":"success","stop_reason":"end_turn"})+"\n")
    out.flush()
"#;

fn write_fake() -> String {
    let dir = std::env::temp_dir().join(format!("v3c-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("fake_claude.py");
    std::fs::write(&script, FAKE_PY).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script.to_string_lossy().into_owned()
}

// ---- forget-observing wrapper (process-global set the test polls) ----
fn forgotten() -> &'static Mutex<std::collections::HashSet<String>> {
    static F: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    F.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}
struct ForgetTracking(Arc<dyn AgentBackend>);
#[async_trait::async_trait]
impl AgentBackend for ForgetTracking {
    async fn prompt(&self, s: &SessionId, p: Vec<Part>) -> Result<BackendStream, BridgeError> {
        self.0.prompt(s, p).await
    }
    async fn cancel(&self, s: &SessionId) -> Result<(), BridgeError> {
        self.0.cancel(s).await
    }
    async fn configure_session(
        &self,
        s: &SessionId,
        c: &EffectiveConfig,
    ) -> Result<(), BridgeError> {
        self.0.configure_session(s, c).await
    }
    async fn forget_session(&self, s: &SessionId) {
        // Record ONLY AFTER the real forget_session completes, so the turn-2 gate
        // can never race ahead of a (hypothetically broken) inner forget that kills
        // the proc — the test must observe the REAL eviction, not just the call (Cx#3).
        self.0.forget_session(s).await;
        forgotten().lock().unwrap().insert(s.as_str().to_string());
    }
    async fn retire(&self) -> Result<(), BridgeError> {
        self.0.retire().await
    }
}

// ---- inline route → always Local("claude") ----
struct ClaudeRoute;
impl RouteDecision for ClaudeRoute {
    fn route(&self, _m: &TaskMeta) -> Result<RouteTarget, BridgeError> {
        Ok(RouteTarget::Local(AgentId::parse("claude")?))
    }
}

async fn body_string(resp: axum::response::Response) -> String {
    let b = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(b.into()).unwrap()
}
fn streaming_req(task: &str, text: &str) -> Request<axum::body::Body> {
    let body = serde_json::to_vec(&json!({
        "jsonrpc": "2.0", "id": 1, "method": "SendStreamingMessage",
        "params": { "taskId": task, "message": { "text": text } }
    }))
    .unwrap();
    Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/json")
        .header("A2A-Version", "1.0")
        .body(axum::body::Body::from(body))
        .unwrap()
}
async fn wait_until(mut f: impl FnMut() -> bool) {
    for _ in 0..500 {
        if f() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("condition not met within timeout");
}

fn build_router(be: Arc<dyn AgentBackend>) -> axum::Router {
    let store: Arc<dyn SessionStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let policy: Arc<dyn PolicyEngine> = Arc::new(AutoPolicy);
    let route: Arc<dyn RouteDecision> = Arc::new(ClaudeRoute);
    let srv = Arc::new(InboundServer::new(
        common::single_agent_registry("claude", be),
        store,
        policy,
        route,
        Arc::new(AlwaysGrant),
        "http://localhost:8080",
        Arc::new(bridge_a2a_outbound::StubDelegation),
        "claude",
    ));
    srv.router()
}

#[tokio::test]
async fn inbound_two_turns_same_task_reuse_warm_proc() {
    let cmd = write_fake();
    let real = Arc::new(
        ClaudeCliBackend::spawn(
            &cmd,
            ClaudeConfig {
                cwd: std::path::PathBuf::from("."),
                ..ClaudeConfig::default()
            },
        )
        .await
        .unwrap(),
    );
    let be: Arc<dyn AgentBackend> = Arc::new(ForgetTracking(real));
    let router = build_router(be);

    // Turn 1: remember 7.
    let r1 = router
        .clone()
        .oneshot(streaming_req("t-warm", "Remember the number 7"))
        .await
        .unwrap();
    assert_eq!(r1.status(), StatusCode::OK);
    let b1 = body_string(r1).await;
    let reply1 = sse_agent_text(&b1);
    assert!(
        reply1.contains('7'),
        "turn1 agent reply must contain '7' (got {:?}); full SSE: {b1}",
        reply1
    );

    // SYNCHRONIZE on the async BindingGuard eviction (session = "session-t-warm").
    wait_until(|| forgotten().lock().unwrap().contains("session-t-warm")).await;

    // Turn 2: same task → re-binds the SAME backend → SAME warm proc (still remembers 7).
    let r2 = router
        .oneshot(streaming_req(
            "t-warm",
            "What number did I ask you to remember? Reply with just the number.",
        ))
        .await
        .unwrap();
    assert_eq!(r2.status(), StatusCode::OK);
    let b2 = body_string(r2).await;
    let reply2 = sse_agent_text(&b2);
    // Warm-continuity proof: a cold-respawned proc (no memory) replies "OK";
    // a warm proc that retained context replies "7".  Both conditions are required
    // so that a UUID-digit false-pass is structurally impossible.
    assert!(
        reply2.contains('7') && !reply2.contains("OK"),
        "turn2 agent reply must contain '7' and not contain 'OK' (warm proc, retained memory); \
         got {:?}; full SSE: {b2}",
        reply2
    );
}
