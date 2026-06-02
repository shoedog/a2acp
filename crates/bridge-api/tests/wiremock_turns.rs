use bridge_api::{ApiBackend, ApiConfig};
use bridge_core::domain::{Part, PermissionDecision, PermissionRequest, SessionContext};
use bridge_core::error::BridgeError;
use bridge_core::ids::SessionId;
use bridge_core::ports::{AgentBackend, PolicyEngine, Update};
use futures::StreamExt;
use std::sync::Arc;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn sse(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).insert_header("content-type", "text/event-stream").set_body_string(body)
}

async fn drain(be: &ApiBackend, s: &SessionId) -> Vec<Update> {
    let mut st = be.prompt(s, vec![Part { text: "hi".into() }]).await.unwrap();
    let mut out = Vec::new();
    while let Some(item) = st.next().await { out.push(item.unwrap()); }
    out
}

#[tokio::test]
async fn text_round_trip_yields_text_then_done_no_permission() {
    let server = MockServer::start().await;
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n\
                data: {\"choices\":[{\"delta\":{\"content\":\" world\"},\"finish_reason\":\"stop\"}]}\n\n\
                data: [DONE]\n\n";
    Mock::given(method("POST")).and(path("/v1/chat/completions"))
        .respond_with(sse(body)).mount(&server).await;

    let be = ApiBackend::new(ApiConfig::new(format!("{}/v1", server.uri())));
    let updates = drain(&be, &SessionId::parse("s1").unwrap()).await;

    let text: String = updates.iter().filter_map(|u| if let Update::Text(t) = u { Some(t.clone()) } else { None }).collect();
    assert_eq!(text, "Hello world");
    assert!(matches!(updates.last(), Some(Update::Done { stop_reason }) if stop_reason == "stop"));
    assert!(!updates.iter().any(|u| matches!(u, Update::Permission(_))), "API backend NEVER yields Permission");
}

#[tokio::test]
async fn tool_approve_path_executes_and_feeds_result() {
    let server = MockServer::start().await;
    let call1 = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"get_current_time\",\"arguments\":\"{}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n";
    let call2 = "data: {\"choices\":[{\"delta\":{\"content\":\"It is 2026.\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    // The follow-up (and only the follow-up) carries the tool result.
    Mock::given(method("POST")).and(path("/v1/chat/completions"))
        .and(body_string_contains("2026-01-01T00:00:00Z"))
        .respond_with(sse(call2)).up_to_n_times(1).mount(&server).await;
    Mock::given(method("POST")).and(path("/v1/chat/completions"))
        .respond_with(sse(call1)).mount(&server).await;

    let be = ApiBackend::new(ApiConfig::new(format!("{}/v1", server.uri()))); // default = auto-approve
    let updates = drain(&be, &SessionId::parse("s2").unwrap()).await;

    let text: String = updates.iter().filter_map(|u| if let Update::Text(t) = u { Some(t.clone()) } else { None }).collect();
    assert_eq!(text, "It is 2026.");
    assert!(matches!(updates.last(), Some(Update::Done { stop_reason }) if stop_reason == "stop"));
    assert!(!updates.iter().any(|u| matches!(u, Update::Permission(_))));

    // EXACTLY two requests; the follow-up carries the PRECISE assistant + tool messages.
    let reqs = server.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 2, "one tool round → exactly two completions");
    let body: serde_json::Value = serde_json::from_slice(&reqs[1].body).unwrap();
    let msgs = body["messages"].as_array().unwrap();
    assert_eq!(msgs[0]["role"], "user");
    assert_eq!(msgs[1]["role"], "assistant");
    assert_eq!(msgs[1]["tool_calls"][0]["id"], "call_1");
    assert_eq!(msgs[2]["role"], "tool");
    assert_eq!(msgs[2]["tool_call_id"], "call_1");
    assert_eq!(msgs[2]["content"], "2026-01-01T00:00:00Z");
}

struct Deny;
impl PolicyEngine for Deny {
    fn decide(&self, _: &PermissionRequest, _: &SessionContext) -> Result<PermissionDecision, BridgeError> {
        Err(BridgeError::PermissionDenied)
    }
}
struct Abstain;
impl PolicyEngine for Abstain {
    fn decide(&self, _: &PermissionRequest, _: &SessionContext) -> Result<PermissionDecision, BridgeError> {
        Err(BridgeError::FrameError) // any non-PermissionDenied Err = abstain
    }
}

async fn tool_then_text(server: &MockServer) {
    let call1 = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"get_current_time\",\"arguments\":\"{}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n";
    let call2 = "data: {\"choices\":[{\"delta\":{\"content\":\"done\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    Mock::given(method("POST")).and(path("/v1/chat/completions"))
        .and(body_string_contains("\"role\":\"tool\""))
        .respond_with(sse(call2)).up_to_n_times(1).mount(server).await;
    Mock::given(method("POST")).and(path("/v1/chat/completions")).respond_with(sse(call1)).mount(server).await;
}

#[tokio::test]
async fn deny_arm_feeds_denial_and_does_not_run_tool() {
    let server = MockServer::start().await;
    tool_then_text(&server).await;
    let be = ApiBackend::new(ApiConfig::new(format!("{}/v1", server.uri()))).with_policy(Arc::new(Deny));
    let _ = drain(&be, &SessionId::parse("s3").unwrap()).await;
    let reqs = server.received_requests().await.unwrap();
    let second = String::from_utf8_lossy(&reqs[1].body);
    assert!(second.contains("permission denied: tool not executed"));
    assert!(!second.contains("2026-01-01T00:00:00Z"), "stub tool MUST NOT have run");
}

#[tokio::test]
async fn abstain_arm_feeds_refusal() {
    let server = MockServer::start().await;
    tool_then_text(&server).await;
    let be = ApiBackend::new(ApiConfig::new(format!("{}/v1", server.uri()))).with_policy(Arc::new(Abstain));
    let _ = drain(&be, &SessionId::parse("s4").unwrap()).await;
    let reqs = server.received_requests().await.unwrap();
    let second = String::from_utf8_lossy(&reqs[1].body);
    assert!(second.contains("permission unavailable: tool not executed"));
}
