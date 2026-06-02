//! Gated live test against a real local Ollama. Run manually:
//!   brew install ollama && ollama serve && ollama pull qwen3.5:9b
//!   cargo test -p bridge-api --test live_ollama -- --ignored api_live_two_turns
use bridge_api::{ApiBackend, ApiConfig};
use bridge_core::domain::Part;
use bridge_core::ids::SessionId;
use bridge_core::ports::{AgentBackend, Update};
use futures::StreamExt;

fn base_url() -> String {
    std::env::var("OLLAMA_BASE_URL").unwrap_or_else(|_| "http://localhost:11434/v1".into())
}

async fn run(be: &ApiBackend, s: &SessionId, text: &str) -> Vec<Update> {
    let mut st = be
        .prompt(s, vec![Part { text: text.into() }])
        .await
        .unwrap();
    let mut out = Vec::new();
    while let Some(i) = st.next().await {
        out.push(i.unwrap());
    }
    out
}

#[tokio::test]
#[ignore = "requires a local Ollama with qwen3.5:9b"]
async fn api_live_two_turns() {
    let mut cfg = ApiConfig::new(base_url());
    cfg.model = Some("qwen3.5:9b".into());
    let be = ApiBackend::new(cfg);
    let s = SessionId::parse("live").unwrap();

    // Turn 1: plain text.
    let t1 = run(&be, &s, "Reply with a short greeting.").await;
    let text1: String = t1
        .iter()
        .filter_map(|u| {
            if let Update::Text(t) = u {
                Some(t.clone())
            } else {
                None
            }
        })
        .collect();
    assert!(!text1.trim().is_empty(), "turn 1 produced text");
    assert!(matches!(t1.last(), Some(Update::Done { .. })));

    // Turn 2: force a tool call. The stub tool returns "2026-01-01T00:00:00Z"; if it ran
    // AND its result reached the follow-up completion, the model's answer references 2026.
    let t2 = run(&be, &s, "What is the current time? You MUST call the get_current_time tool, then state the time it returned.").await;
    let text2: String = t2
        .iter()
        .filter_map(|u| {
            if let Update::Text(t) = u {
                Some(t.clone())
            } else {
                None
            }
        })
        .collect();
    assert!(matches!(t2.last(), Some(Update::Done { .. })));
    assert!(!t2.iter().any(|u| matches!(u, Update::Permission(_)))); // silent decision
    assert!(
        text2.contains("2026"),
        "the stub tool's result reached the model's answer: {text2:?}"
    );
}
