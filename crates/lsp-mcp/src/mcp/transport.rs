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

/// MCP-over-stdio framing: **newline-delimited JSON-RPC** (one compact JSON message per line). This is the
/// MCP stdio transport standard — DISTINCT from the LSP `Content-Length` framing (`crate::lsp::codec`) used
/// to talk to the language server. The agents' MCP clients (claude-agent-acp, codex) speak newline framing;
/// answering them with `Content-Length` left them waiting for headers that never came, so the lsp tools
/// never registered (every host-reviewer `mcp__lsp__*` call returned "unavailable"). prism-mcp uses newline.
pub fn read_line_frame<R: std::io::BufRead>(r: &mut R) -> std::io::Result<Option<Vec<u8>>> {
    loop {
        let mut buf = Vec::new();
        let n = r.read_until(b'\n', &mut buf)?;
        if n == 0 {
            return Ok(None); // EOF — peer closed stdin
        }
        // Trim the line terminator (\n or \r\n) and skip blank lines between messages.
        while buf.last() == Some(&b'\n') || buf.last() == Some(&b'\r') {
            buf.pop();
        }
        if buf.is_empty() {
            continue;
        }
        return Ok(Some(buf));
    }
}

/// Write one MCP reply to the agent: compact JSON + a single `\n`, then flush.
pub fn write_line_frame<W: std::io::Write>(w: &mut W, body: &[u8]) -> std::io::Result<()> {
    w.write_all(body)?;
    w.write_all(b"\n")?;
    w.flush()
}

/// Read one MCP request from the agent (stdin). MCP stdio = newline-delimited JSON.
pub fn read_frame_stdin<R: std::io::BufRead>(r: &mut R) -> std::io::Result<Option<Vec<u8>>> {
    read_line_frame(r)
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

    #[test]
    fn read_line_frame_reads_newline_delimited_json() {
        // MCP stdio = newline-delimited JSON (NOT Content-Length). The agents send this.
        let wire = b"{\"jsonrpc\":\"2.0\",\"id\":1}\n{\"jsonrpc\":\"2.0\",\"id\":2}\n";
        let mut r = std::io::BufReader::new(&wire[..]);
        let a = read_line_frame(&mut r).unwrap().unwrap();
        assert_eq!(a, br#"{"jsonrpc":"2.0","id":1}"#);
        let b = read_line_frame(&mut r).unwrap().unwrap();
        assert_eq!(b, br#"{"jsonrpc":"2.0","id":2}"#);
        assert!(read_line_frame(&mut r).unwrap().is_none()); // EOF
    }

    #[test]
    fn read_line_frame_skips_blank_lines_and_trims_crlf() {
        let wire = b"\r\n{\"id\":1}\r\n\n";
        let mut r = std::io::BufReader::new(&wire[..]);
        assert_eq!(read_line_frame(&mut r).unwrap().unwrap(), br#"{"id":1}"#);
        assert!(read_line_frame(&mut r).unwrap().is_none());
    }

    #[test]
    fn write_line_frame_appends_single_newline() {
        let mut buf = Vec::new();
        write_line_frame(&mut buf, br#"{"id":1}"#).unwrap();
        assert_eq!(buf, b"{\"id\":1}\n");
    }
}
