use bridge_api::wire::SseAccumulator;
use serde_json::Value;

fn fixture() -> Value {
    let raw = include_str!("fixtures/ollama-openai-compat.json");
    serde_json::from_str(raw).expect("fixture is valid JSON")
}

#[test]
fn fixture_has_provenance() {
    // Honest: accept REAL-CAPTURE or SHAPE-AUTHORED, but the field MUST be present + non-empty.
    let f = fixture();
    let p = f["_provenance"].as_str().unwrap_or("");
    assert!(p == "REAL-CAPTURE" || p == "SHAPE-AUTHORED", "provenance must be declared honestly, got {p:?}");
    assert_eq!(f["model"], "qwen3.5:9b");
}

#[test]
fn captured_text_turn_replays_through_parser() {
    let sse = fixture()["text_turn_sse"].as_str().unwrap().to_string();
    let mut acc = SseAccumulator::default();
    for line in sse.split('\n') { let _ = acc.push_sse_line(line); }
    let out = acc.finish();
    assert!(!out.text.is_empty(), "text turn parses to non-empty text");
}

#[test]
fn captured_tool_turn_replays_to_a_tool_call() {
    let sse = fixture()["tool_turn_sse"].as_str().unwrap().to_string();
    let mut acc = SseAccumulator::default();
    for line in sse.split('\n') { let _ = acc.push_sse_line(line); }
    let out = acc.finish();
    assert_eq!(out.tool_calls.len(), 1);
    assert_eq!(out.tool_calls[0].function.name, "get_current_time");
}
