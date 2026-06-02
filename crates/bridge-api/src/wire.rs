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
