// Protocol lifecycle only; tool dispatch is injected by mcp/mod.rs.
use crate::mcp::error::*;
use serde_json::{json, Value};

#[derive(Default)]
pub struct Lifecycle {
    initialized: bool,
}

impl Lifecycle {
    /// Handle initialize/initialized/tools/list and lifecycle errors. Returns a reply for
    /// request messages, or None for handled notifications. `tools/call` returns None here; the
    /// caller routes it to the tool dispatcher only when `self.initialized`.
    pub fn handle_meta(&mut self, msg: &Value) -> Option<Value> {
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        match method {
            "initialize" => {
                // Echo the client's requested `protocolVersion` for compatibility: a strict MCP client
                // (codex-acp / claude-agent-acp) rejects the handshake — and never registers our tools —
                // if the server answers with a version it doesn't recognize. lsp-mcp's surface is just the
                // stable tools/{list,call} protocol, so echoing the client's version is safe. Fall back to a
                // known-stable version only when the client omits one.
                let proto = msg
                    .get("params")
                    .and_then(|p| p.get("protocolVersion"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("2025-06-18");
                Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": proto,
                        "capabilities": {
                            "tools": {}
                        },
                        "serverInfo": {
                            "name": "lsp-mcp",
                            "version": env!("CARGO_PKG_VERSION")
                        }
                    }
                }))
            }
            "notifications/initialized" => {
                self.initialized = true;
                None
            }
            "tools/list" => Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": crate::mcp::tool_schemas()
                }
            })),
            "tools/call" if !self.initialized => Some(err(
                &id,
                INVALID_REQUEST,
                "received tools/call before initialized",
            )),
            "ping" => Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {}
            })),
            _ => None,
        }
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized
    }
}

pub fn read_frame_stdin<R: std::io::BufRead>(r: &mut R) -> std::io::Result<Option<Vec<u8>>> {
    crate::lsp::codec::read_frame(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(msgs: &[&str]) -> Vec<serde_json::Value> {
        let mut out = Vec::new();
        let mut lc = Lifecycle::default();
        for m in msgs {
            let v: serde_json::Value = serde_json::from_str(m).unwrap();
            if let Some(reply) = lc.handle_meta(&v) {
                out.push(reply);
            }
        }
        out
    }

    #[test]
    fn tools_call_before_initialized_is_32600() {
        let out = drive(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"definition","arguments":{}}}"#,
        ]);
        assert_eq!(out[0]["error"]["code"], -32600);
    }

    #[test]
    fn initialize_then_tools_list_ok() {
        let out = drive(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        ]);
        assert_eq!(out[0]["id"], 1);
        assert!(out
            .iter()
            .any(|m| m["id"] == 2 && m["result"]["tools"].is_array()));
    }

    #[test]
    fn initialize_echoes_client_protocol_version() {
        // A strict MCP client (codex-acp/claude-agent-acp) rejects the handshake — and never registers
        // our tools — unless the server answers with a protocolVersion it recognizes. Echo the client's.
        let out = drive(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#,
        ]);
        assert_eq!(out[0]["result"]["protocolVersion"], "2024-11-05");
    }

    #[test]
    fn initialize_falls_back_when_client_omits_version() {
        let out = drive(&[r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#]);
        assert_eq!(out[0]["result"]["protocolVersion"], "2025-06-18");
    }
}
