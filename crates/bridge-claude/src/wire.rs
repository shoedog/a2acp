//! stream-json wire codec: serialize a user turn to stdin NDJSON; parse stdout
//! NDJSON into a small internal event enum (tolerant — unmodeled lines drop).
use serde_json::Value;

/// Parsed, modeled stdout event. Everything else (tool calls, usage, unknown
/// types) is dropped by `parse_line` returning `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeEvent {
    Init {
        session_id: String,
    },
    Text(String),
    /// Terminal success result.
    ResultOk {
        stop_reason: Option<String>,
    },
    /// Terminal error result (subtype e.g. error_max_turns / error_during_execution).
    ResultErr {
        subtype: String,
    },
}

/// Serialize one user turn as the stdin NDJSON line (no trailing newline).
pub fn user_envelope(text: &str) -> String {
    serde_json::json!({
        "type": "user",
        "message": { "role": "user", "content": [ { "type": "text", "text": text } ] }
    })
    .to_string()
}

/// Parse one stdout line. Returns None for blank/unmodeled/unparseable lines.
pub fn parse_line(line: &str) -> Option<ClaudeEvent> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let v: Value = serde_json::from_str(line).ok()?;
    match v.get("type")?.as_str()? {
        "system" if v.get("subtype").and_then(Value::as_str) == Some("init") => {
            let sid = v.get("session_id")?.as_str()?.to_string();
            Some(ClaudeEvent::Init { session_id: sid })
        }
        "assistant" => {
            // Concatenate all text blocks in message.content.
            let content = v.get("message")?.get("content")?.as_array()?;
            let mut text = String::new();
            for block in content {
                if block.get("type").and_then(Value::as_str) == Some("text") {
                    if let Some(t) = block.get("text").and_then(Value::as_str) {
                        text.push_str(t);
                    }
                }
            }
            if text.is_empty() {
                None
            } else {
                Some(ClaudeEvent::Text(text))
            }
        }
        "result" => {
            let subtype = v
                .get("subtype")
                .and_then(Value::as_str)
                .unwrap_or("success");
            if subtype == "success" {
                let stop = v
                    .get("stop_reason")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                Some(ClaudeEvent::ResultOk { stop_reason: stop })
            } else {
                Some(ClaudeEvent::ResultErr {
                    subtype: subtype.to_string(),
                })
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_shape() {
        assert_eq!(
            user_envelope("hi"),
            r#"{"message":{"content":[{"text":"hi","type":"text"}],"role":"user"},"type":"user"}"#
        );
    }

    #[test]
    fn parses_init() {
        let l = r#"{"type":"system","subtype":"init","session_id":"abc-123"}"#;
        assert_eq!(
            parse_line(l),
            Some(ClaudeEvent::Init {
                session_id: "abc-123".into()
            })
        );
    }

    #[test]
    fn parses_assistant_text() {
        let l = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"PONG"}]}}"#;
        assert_eq!(parse_line(l), Some(ClaudeEvent::Text("PONG".into())));
    }

    #[test]
    fn parses_result_success_and_error() {
        assert_eq!(
            parse_line(r#"{"type":"result","subtype":"success"}"#),
            Some(ClaudeEvent::ResultOk { stop_reason: None })
        );
        assert_eq!(
            parse_line(r#"{"type":"result","subtype":"error_max_turns"}"#),
            Some(ClaudeEvent::ResultErr {
                subtype: "error_max_turns".into()
            })
        );
    }

    #[test]
    fn drops_unmodeled_and_garbage() {
        assert_eq!(parse_line(r#"{"type":"usage","tokens":5}"#), None);
        assert_eq!(parse_line("not json"), None);
        assert_eq!(parse_line(""), None);
    }
}
