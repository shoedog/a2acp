use bridge_api::{ApiBackend, ApiConfig};
use bridge_core::diagnostics::{
    DiagnosticFailureClass, DiagnosticPhase, FailureDiagnostic, FailureDisposition,
    InMemoryDiagnosticObserver, PhaseStatus,
};
use bridge_core::domain::Part;
use bridge_core::error::BridgeError;
use bridge_core::ids::SessionId;
use bridge_core::ports::{AgentBackend, BackendObservers, Update};
use futures::StreamExt;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn observed_turn(
    backend: &ApiBackend,
    session: &str,
) -> (
    Vec<Update>,
    Option<BridgeError>,
    Arc<InMemoryDiagnosticObserver>,
) {
    let observer = Arc::new(InMemoryDiagnosticObserver::new(32).unwrap());
    let mut stream = backend
        .prompt_with_observers(
            &SessionId::parse(session).unwrap(),
            vec![Part { text: "hi".into() }],
            BackendObservers::diagnostic_only(observer.clone()),
        )
        .await
        .unwrap();
    let mut updates = Vec::new();
    let mut error = None;
    while let Some(item) = stream.next().await {
        match item {
            Ok(update) => updates.push(update),
            Err(turn_error) => error = Some(turn_error),
        }
    }
    (updates, error, observer)
}

fn transition_sequence(
    events: &[bridge_core::diagnostics::DiagnosticEvent],
) -> Vec<(DiagnosticPhase, PhaseStatus)> {
    events
        .iter()
        .map(|event| (event.transition().phase(), event.transition().status()))
        .collect()
}

fn agent_failure(error: Option<BridgeError>) -> Box<FailureDiagnostic> {
    let BridgeError::AgentFailure { diagnostic } = error.expect("terminal failure") else {
        panic!("failure path must return a structured AgentFailure");
    };
    diagnostic
}

async fn one_response_server(response: Vec<u8>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![0_u8; 64 * 1024];
        let _ = socket.read(&mut request).await;
        socket.write_all(&response).await.unwrap();
        let _ = socket.shutdown().await;
    });
    format!("http://{address}/v1")
}

fn raw_response(status: &str, content_type: &str, body: &[u8], claimed_len: usize) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {claimed_len}\r\nconnection: close\r\n\r\n"
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

#[tokio::test]
async fn successful_first_send_records_complete_post_barrier_lifecycle() {
    let server = MockServer::start().await;
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let backend = ApiBackend::new(ApiConfig::new(format!("{}/v1", server.uri())));
    let (updates, error, observer) = observed_turn(&backend, "success").await;

    assert!(error.is_none(), "successful turn returned {error:?}");
    assert!(matches!(updates.last(), Some(Update::Done { stop_reason }) if stop_reason == "stop"));
    assert_eq!(
        transition_sequence(&observer.snapshot().await),
        vec![
            (DiagnosticPhase::PromptStart, PhaseStatus::Started),
            (DiagnosticPhase::PromptStart, PhaseStatus::Completed),
            (DiagnosticPhase::PromptStream, PhaseStatus::Started),
            (DiagnosticPhase::PromptStream, PhaseStatus::Completed),
            (DiagnosticPhase::PromptFinish, PhaseStatus::Started),
            (DiagnosticPhase::PromptFinish, PhaseStatus::Completed),
        ]
    );
}

#[tokio::test]
async fn structured_provider_limit_is_fatal_after_send_installation() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(429).set_body_json(serde_json::json!({
            "error": {
                "code": "usage_limit_reached",
                "type": "rate_limit_error",
                "retry_after_ms": 1234
            }
        })))
        .mount(&server)
        .await;

    let backend = ApiBackend::new(ApiConfig::new(format!("{}/v1", server.uri())));
    let (_, error, observer) = observed_turn(&backend, "provider-limit").await;
    let original_error = error.clone().expect("terminal failure");
    let diagnostic = agent_failure(error);
    assert_eq!(diagnostic.class(), DiagnosticFailureClass::ProviderLimit);
    assert_eq!(diagnostic.disposition(), FailureDisposition::Fatal);
    assert_eq!(diagnostic.code().as_str(), "upstream.provider_limit");
    assert!(diagnostic.prompt_may_have_been_accepted());
    assert!(!original_error.is_transient());
    assert_eq!(
        bridge_core::error::warm_session_survivability(&original_error),
        bridge_core::error::WarmSessionSurvivability::Expire
    );
    assert!(!diagnostic.class().is_container_fallback_class());
    let serialized = serde_json::to_value(&diagnostic).unwrap();
    assert_eq!(serialized["retry_after_ms"], 1234);
    assert_eq!(
        transition_sequence(&observer.snapshot().await),
        vec![
            (DiagnosticPhase::PromptStart, PhaseStatus::Started),
            (DiagnosticPhase::PromptStart, PhaseStatus::Completed),
            (DiagnosticPhase::PromptStream, PhaseStatus::Started),
            (DiagnosticPhase::PromptStream, PhaseStatus::Failed),
        ]
    );
}

#[tokio::test]
async fn bare_429_is_unknown_not_legacy_overload() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;

    let backend = ApiBackend::new(ApiConfig::new(format!("{}/v1", server.uri())));
    let (_, error, _) = observed_turn(&backend, "bare-429").await;
    let BridgeError::AgentFailure { diagnostic } = error.expect("terminal failure") else {
        panic!("bare 429 must be a structured AgentFailure");
    };
    assert_eq!(diagnostic.class(), DiagnosticFailureClass::Unknown);
    assert_eq!(diagnostic.disposition(), FailureDisposition::Fatal);
    assert_eq!(diagnostic.code().as_str(), "upstream.unknown");
    assert!(diagnostic.prompt_may_have_been_accepted());
}

#[tokio::test]
async fn blocked_model_remains_pre_send_and_records_no_prompt_phase() {
    let observer = Arc::new(InMemoryDiagnosticObserver::new(8).unwrap());
    let mut config = ApiConfig::new("http://127.0.0.1:1/v1");
    config.model = Some("claude-fable-5.1[1m]".into());
    let backend = ApiBackend::new(config);

    let result = backend
        .prompt_with_observers(
            &SessionId::parse("blocked-model").unwrap(),
            vec![Part { text: "hi".into() }],
            BackendObservers::diagnostic_only(observer.clone()),
        )
        .await;
    assert!(matches!(result, Err(BridgeError::ConfigInvalid { .. })));
    assert!(observer.snapshot().await.is_empty());
}

#[tokio::test]
async fn first_send_timeout_is_post_barrier_fatal() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(250)))
        .mount(&server)
        .await;
    let mut config = ApiConfig::new(format!("{}/v1", server.uri()));
    config.request_timeout = Duration::from_millis(25);
    let backend = ApiBackend::new(config);

    let (_, error, _) = observed_turn(&backend, "timeout").await;
    let diagnostic = agent_failure(error);
    assert_eq!(diagnostic.class(), DiagnosticFailureClass::Timeout);
    assert_eq!(diagnostic.code().as_str(), "api.prompt.timeout");
    assert_eq!(diagnostic.disposition(), FailureDisposition::Fatal);
    assert!(diagnostic.prompt_may_have_been_accepted());
}

#[tokio::test]
async fn truncated_non_success_body_is_transport_not_provider_evidence() {
    let response = raw_response(
        "429 Too Many Requests",
        "application/json",
        br#"{"error":{"code":"usage_limit_reached"}}"#,
        4096,
    );
    let backend = ApiBackend::new(ApiConfig::new(one_response_server(response).await));

    let (_, error, _) = observed_turn(&backend, "error-body-read").await;
    let diagnostic = agent_failure(error);
    assert_eq!(diagnostic.class(), DiagnosticFailureClass::Transport);
    assert_eq!(diagnostic.code().as_str(), "api.prompt.error_body_read");
    assert_eq!(diagnostic.disposition(), FailureDisposition::Fatal);
}

#[tokio::test]
async fn truncated_sse_chunk_is_structured_transport_failure() {
    let body = b"data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n";
    let response = raw_response("200 OK", "text/event-stream", body, 4096);
    let backend = ApiBackend::new(ApiConfig::new(one_response_server(response).await));

    let (updates, error, _) = observed_turn(&backend, "sse-read").await;
    assert!(updates
        .iter()
        .any(|update| matches!(update, Update::Text(text) if text == "partial")));
    let diagnostic = agent_failure(error);
    assert_eq!(diagnostic.class(), DiagnosticFailureClass::Transport);
    assert_eq!(diagnostic.code().as_str(), "api.prompt.sse_read");
    assert_eq!(diagnostic.disposition(), FailureDisposition::Fatal);
}

#[tokio::test]
async fn clean_sse_eof_without_terminal_evidence_is_fatal_protocol_failure() {
    let body =
        b"data: {\"choices\":[{\"delta\":{\"content\":\"partial\"},\"finish_reason\":null}]}\n\n";
    let response = raw_response("200 OK", "text/event-stream", body, body.len());
    let backend = ApiBackend::new(ApiConfig::new(one_response_server(response).await));

    let (updates, error, _) = observed_turn(&backend, "clean-incomplete-sse").await;
    assert!(updates
        .iter()
        .any(|update| matches!(update, Update::Text(text) if text == "partial")));
    let diagnostic = agent_failure(error);
    assert_eq!(diagnostic.class(), DiagnosticFailureClass::Protocol);
    assert_eq!(diagnostic.code().as_str(), "api.prompt.sse_incomplete");
    assert_eq!(diagnostic.disposition(), FailureDisposition::Fatal);
    assert!(diagnostic.prompt_may_have_been_accepted());
}

#[tokio::test]
async fn clean_sse_eof_after_finish_reason_remains_successful_without_done_sentinel() {
    let body = b"data: {\"choices\":[{\"delta\":{\"content\":\"complete\"},\"finish_reason\":\"stop\"}]}\n\n";
    let response = raw_response("200 OK", "text/event-stream", body, body.len());
    let backend = ApiBackend::new(ApiConfig::new(one_response_server(response).await));

    let (updates, error, _) = observed_turn(&backend, "clean-terminal-sse").await;
    assert!(
        error.is_none(),
        "terminal finish_reason must permit clean EOF"
    );
    assert!(updates
        .iter()
        .any(|update| matches!(update, Update::Text(text) if text == "complete")));
    assert!(matches!(
        updates.last(),
        Some(Update::Done { stop_reason }) if stop_reason == "stop"
    ));
}

#[tokio::test]
async fn truncated_nonstream_body_is_structured_transport_failure() {
    let response = raw_response(
        "200 OK",
        "application/json",
        br#"{"choices":[{"message":{"content":"partial"}}]}"#,
        4096,
    );
    let mut config = ApiConfig::new(one_response_server(response).await);
    config.stream = false;
    let backend = ApiBackend::new(config);

    let (_, error, _) = observed_turn(&backend, "body-read").await;
    let diagnostic = agent_failure(error);
    assert_eq!(diagnostic.class(), DiagnosticFailureClass::Transport);
    assert_eq!(diagnostic.code().as_str(), "api.prompt.body_read");
    assert_eq!(diagnostic.disposition(), FailureDisposition::Fatal);
}

#[tokio::test]
async fn malformed_nonstream_body_is_structured_protocol_failure() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string("not-json"),
        )
        .mount(&server)
        .await;
    let mut config = ApiConfig::new(format!("{}/v1", server.uri()));
    config.stream = false;
    let backend = ApiBackend::new(config);

    let (_, error, _) = observed_turn(&backend, "body-parse").await;
    let diagnostic = agent_failure(error);
    assert_eq!(diagnostic.class(), DiagnosticFailureClass::Protocol);
    assert_eq!(diagnostic.code().as_str(), "api.prompt.body_parse");
    assert_eq!(diagnostic.disposition(), FailureDisposition::Fatal);
}

#[tokio::test]
async fn later_tool_round_status_failure_stays_post_barrier_and_is_not_replayed() {
    let server = MockServer::start().await;
    let first = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"get_current_time\",\"arguments\":\"{}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n";
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(wiremock::matchers::body_string_contains(
            "\"role\":\"tool\"",
        ))
        .respond_with(ResponseTemplate::new(503).set_body_json(serde_json::json!({
            "error": {"code": "server_overloaded"}
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(first),
        )
        .mount(&server)
        .await;
    let backend = ApiBackend::new(ApiConfig::new(format!("{}/v1", server.uri())));

    let (_, error, _) = observed_turn(&backend, "later-status").await;
    let diagnostic = agent_failure(error);
    assert_eq!(diagnostic.class(), DiagnosticFailureClass::Overloaded);
    assert_eq!(diagnostic.code().as_str(), "upstream.overloaded");
    assert_eq!(diagnostic.disposition(), FailureDisposition::Fatal);
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn later_tool_round_send_failure_is_fatal_after_exactly_one_accepted_request() {
    let body = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"get_current_time\",\"arguments\":\"{}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n";
    let response = raw_response("200 OK", "text/event-stream", body.as_bytes(), body.len());
    let backend = ApiBackend::new(ApiConfig::new(one_response_server(response).await));

    let (_, error, _) = observed_turn(&backend, "later-send").await;
    let diagnostic = agent_failure(error);
    assert_eq!(diagnostic.class(), DiagnosticFailureClass::Transport);
    assert_eq!(diagnostic.code().as_str(), "api.prompt.send");
    assert_eq!(diagnostic.disposition(), FailureDisposition::Fatal);
    assert!(diagnostic.prompt_may_have_been_accepted());
}
