use bridge_api::{ApiBackend, ApiConfig};
use bridge_core::domain::Part;
use bridge_core::ids::SessionId;
use bridge_core::ports::{AgentBackend, Update};
use futures::StreamExt;
use wiremock::matchers::{method, path};
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
