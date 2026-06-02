//! OpenAI-compatible wire types + a TOLERANT streamed-response parser.
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Request ──────────────────────────────────────────────────────────────────
#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Value>,
    pub stream: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self { role: "user".into(), content: Some(text.into()), tool_calls: None, tool_call_id: None }
    }
    pub fn assistant_tool_calls(calls: Vec<ToolCall>) -> Self {
        Self { role: "assistant".into(), content: None, tool_calls: Some(calls), tool_call_id: None }
    }
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self { role: "tool".into(), content: Some(content.into()), tool_calls: None,
            tool_call_id: Some(tool_call_id.into()) }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "default_function")]
    pub kind: String,
    pub function: FunctionCall,
}
fn default_function() -> String { "function".into() }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

use std::collections::BTreeMap;

/// Parse error for the wire layer. A real type (NOT `()`), so `pub fn` returning
/// `Result<_, ParseError>` does not trip `clippy::result_unit_err` under `-D warnings`.
/// The backend maps it to `BridgeError::FrameError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseError;

// ── Streamed response chunk shapes ──────────────────────────────────────────
#[derive(Debug, Deserialize)]
struct StreamChunk { #[serde(default)] choices: Vec<StreamChoice> }
#[derive(Debug, Deserialize)]
struct StreamChoice {
    #[serde(default)] delta: Delta,
    #[serde(default)] finish_reason: Option<String>,
}
#[derive(Debug, Default, Deserialize)]
struct Delta {
    #[serde(default)] content: Option<String>,
    #[serde(default)] tool_calls: Option<Vec<ToolCallFragment>>,
}
#[derive(Debug, Deserialize)]
struct ToolCallFragment {
    #[serde(default)] index: Option<usize>,
    #[serde(default)] id: Option<String>,
    #[serde(default)] function: Option<FunctionFragment>,
}
#[derive(Debug, Default, Deserialize)]
struct FunctionFragment {
    #[serde(default)] name: Option<String>,
    #[serde(default)] arguments: Option<String>,
}

#[derive(Debug, Default, Clone)]
struct PartialToolCall { id: String, name: String, arguments: String }

/// The result of consuming a (streamed or non-streamed) response.
#[derive(Debug, Default)]
pub struct ParsedTurn {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
}

/// Tolerant streamed-SSE accumulator. Buffers tool_call fragments by `index`
/// when present, else by a running positional counter. Treats *any* accumulated
/// tool calls as a tool round regardless of the finish_reason string.
#[derive(Debug, Default)]
pub struct SseAccumulator {
    text: String,
    calls: BTreeMap<usize, PartialToolCall>,
    next_pos: usize,
    done: bool,
}

impl SseAccumulator {
    /// Feed one raw SSE line (e.g. `data: {...}` or `data: [DONE]`). Returns the
    /// text delta (if any) to surface immediately, or `Err(ParseError)` on malformed JSON.
    #[must_use = "a text delta may need surfacing as Update::Text"]
    pub fn push_sse_line(&mut self, line: &str) -> Result<Option<String>, ParseError> {
        let line = line.trim();
        let Some(payload) = line.strip_prefix("data:") else { return Ok(None) };
        let payload = payload.trim();
        if payload.is_empty() { return Ok(None) }
        if payload == "[DONE]" { self.done = true; return Ok(None) }
        let chunk: StreamChunk = serde_json::from_str(payload).map_err(|_| ParseError)?;
        let mut emitted = None;
        for choice in chunk.choices {
            if let Some(c) = choice.delta.content {
                if !c.is_empty() { self.text.push_str(&c); emitted = Some(c); }
            }
            if let Some(frags) = choice.delta.tool_calls {
                for f in frags { self.absorb_fragment(f); }
            }
            if choice.finish_reason.is_some() { self.done = true; }
        }
        Ok(emitted)
    }

    fn absorb_fragment(&mut self, f: ToolCallFragment) {
        let key = match f.index {
            Some(i) => i,
            // No index: a new id starts a new slot, else append to the latest.
            None if f.id.is_some() => { let k = self.next_pos; self.next_pos += 1; k }
            None => self.next_pos.saturating_sub(1),
        };
        if f.index.is_some() { self.next_pos = self.next_pos.max(key + 1); }
        let slot = self.calls.entry(key).or_default();
        if let Some(id) = f.id { slot.id = id; }
        if let Some(func) = f.function {
            if let Some(n) = func.name { slot.name = n; }
            if let Some(a) = func.arguments { slot.arguments.push_str(&a); }
        }
    }

    pub fn is_done(&self) -> bool { self.done }

    pub fn finish(self) -> ParsedTurn {
        let tool_calls = self.calls.into_values()
            .filter(|p| !p.name.is_empty())
            .map(|p| ToolCall {
                id: if p.id.is_empty() { "call_0".into() } else { p.id },
                kind: "function".into(),
                function: FunctionCall { name: p.name, arguments: p.arguments },
            })
            .collect();
        ParsedTurn { text: self.text, tool_calls }
    }
}

#[cfg(test)]
mod stream_tests {
    use super::*;

    fn feed(acc: &mut SseAccumulator, lines: &[&str]) {
        for l in lines { let _ = acc.push_sse_line(l); } // push_sse_line is #[must_use]
    }

    #[test]
    fn accumulates_text_deltas() {
        let mut acc = SseAccumulator::default();
        feed(&mut acc, &[
            r#"data: {"choices":[{"delta":{"content":"Hel"},"finish_reason":null}]}"#,
            r#"data: {"choices":[{"delta":{"content":"lo"},"finish_reason":"stop"}]}"#,
            "data: [DONE]",
        ]);
        assert!(acc.is_done());
        let out = acc.finish();
        assert_eq!(out.text, "Hello");
        assert!(out.tool_calls.is_empty());
    }

    #[test]
    fn assembles_indexed_tool_call_fragments() {
        let mut acc = SseAccumulator::default();
        feed(&mut acc, &[
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"get_current_time","arguments":""}}]},"finish_reason":null}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#,
        ]);
        let out = acc.finish();
        assert_eq!(out.tool_calls.len(), 1);
        assert_eq!(out.tool_calls[0].id, "call_1");
        assert_eq!(out.tool_calls[0].function.name, "get_current_time");
        assert_eq!(out.tool_calls[0].function.arguments, "{}");
    }

    #[test]
    fn tolerates_missing_index_and_stop_finish() {
        // ollama/ollama#7881: tool call with NO index, finishing "stop".
        let mut acc = SseAccumulator::default();
        feed(&mut acc, &[
            r#"data: {"choices":[{"delta":{"tool_calls":[{"id":"c9","function":{"name":"get_current_time","arguments":"{}"}}]},"finish_reason":"stop"}]}"#,
        ]);
        let out = acc.finish();
        assert_eq!(out.tool_calls.len(), 1, "tool call assembled despite no index + stop finish");
        assert_eq!(out.tool_calls[0].id, "c9");
    }

    #[test]
    fn ignores_blank_and_non_data_lines() {
        let mut acc = SseAccumulator::default();
        feed(&mut acc, &["", ": keep-alive", r#"data: {"choices":[{"delta":{"content":"x"}}]}"#]);
        assert_eq!(acc.finish().text, "x");
    }

    #[test]
    fn malformed_json_line_is_reported() {
        let mut acc = SseAccumulator::default();
        let err = acc.push_sse_line("data: {not json");
        assert!(err.is_err());
    }
}

#[derive(Debug, Deserialize)]
struct NonStreamResponse { #[serde(default)] choices: Vec<NonStreamChoice> }
#[derive(Debug, Deserialize)]
struct NonStreamChoice { message: RespMessage }
#[derive(Debug, Deserialize)]
struct RespMessage {
    #[serde(default)] content: Option<String>,
    #[serde(default)] tool_calls: Option<Vec<ToolCall>>,
}

/// Parse a non-streamed (`stream:false`) chat completion body. Returns
/// `Err(ParseError)` on malformed JSON (mapped to `FrameError` by the backend).
pub fn parse_nonstream(body: &str) -> Result<ParsedTurn, ParseError> {
    let resp: NonStreamResponse = serde_json::from_str(body).map_err(|_| ParseError)?;
    let mut out = ParsedTurn::default();
    if let Some(choice) = resp.choices.into_iter().next() {
        out.text = choice.message.content.unwrap_or_default();
        out.tool_calls = choice.message.tool_calls.unwrap_or_default();
    }
    Ok(out)
}

#[cfg(test)]
mod nonstream_tests {
    use super::*;
    #[test]
    fn parses_message_tool_calls_shape() {
        let body = r#"{"choices":[{"message":{"content":null,"tool_calls":[
            {"id":"call_1","type":"function","function":{"name":"get_current_time","arguments":"{}"}}]},
            "finish_reason":"tool_calls"}]}"#;
        let out = parse_nonstream(body).unwrap();
        assert_eq!(out.tool_calls.len(), 1);
        assert_eq!(out.tool_calls[0].id, "call_1");
        assert!(out.text.is_empty());
    }
    #[test]
    fn parses_plain_text_message() {
        let body = r#"{"choices":[{"message":{"content":"hello"},"finish_reason":"stop"}]}"#;
        let out = parse_nonstream(body).unwrap();
        assert_eq!(out.text, "hello");
        assert!(out.tool_calls.is_empty());
    }
}

#[cfg(test)]
mod request_tests {
    use super::*;
    #[test]
    fn chat_request_serializes_expected_shape() {
        let req = ChatRequest {
            model: Some("qwen3.5:9b".into()),
            messages: vec![Message::user("hi")],
            tools: vec![crate::tool::tool_def()],
            stream: true,
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["model"], "qwen3.5:9b");
        assert_eq!(v["stream"], true);
        assert_eq!(v["messages"][0]["role"], "user");
        assert_eq!(v["messages"][0]["content"], "hi");
        assert_eq!(v["tools"][0]["function"]["name"], "get_current_time");
    }
    #[test]
    fn assistant_tool_call_and_tool_result_messages_serialize() {
        let tc = ToolCall { id: "call_1".into(), kind: "function".into(),
            function: FunctionCall { name: "get_current_time".into(), arguments: "{}".into() } };
        let asst = Message::assistant_tool_calls(vec![tc.clone()]);
        let result = Message::tool_result("call_1", "2026-01-01T00:00:00Z");
        let va = serde_json::to_value(&asst).unwrap();
        let vr = serde_json::to_value(&result).unwrap();
        assert_eq!(va["role"], "assistant");
        assert_eq!(va["tool_calls"][0]["id"], "call_1");
        assert!(va.get("content").is_none() || va["content"].is_null());
        assert_eq!(vr["role"], "tool");
        assert_eq!(vr["tool_call_id"], "call_1");
        assert_eq!(vr["content"], "2026-01-01T00:00:00Z");
    }
}
