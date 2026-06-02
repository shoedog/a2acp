//! The single stub tool. Its only purpose is to make the model emit a tool_call
//! so the permission control-flow (surface B) runs. Side-effect-free + deterministic.
use serde_json::{json, Value};

pub const TOOL_NAME: &str = "get_current_time";

/// The OpenAI `tools[]` entry advertised on every request.
pub fn tool_def() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": TOOL_NAME,
            "description": "Return the current server time as an ISO-8601 string.",
            "parameters": { "type": "object", "properties": {}, "required": [] }
        }
    })
}

/// Execute the stub. Deterministic constant — the value is irrelevant; the
/// control-flow (decide → execute → feed result) is the point.
pub fn run_tool() -> String {
    "2026-01-01T00:00:00Z".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_def_is_get_current_time_function() {
        let d = tool_def();
        assert_eq!(d["type"], "function");
        assert_eq!(d["function"]["name"], "get_current_time");
        assert!(d["function"]["parameters"]["type"] == "object");
    }

    #[test]
    fn run_tool_returns_deterministic_stub() {
        assert_eq!(run_tool(), "2026-01-01T00:00:00Z");
    }
}
