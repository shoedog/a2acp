pub mod error;
pub mod transport;

use crate::lsp::LspClient;
use crate::mcp::error::*;
use crate::mcp::transport::Lifecycle;
use crate::shape::render_hits;
use serde_json::{json, Value};
use std::io::BufReader;

/// Readiness budget for `ensure_ready`, in seconds. `LSP_MCP_READY_SECS` overrides the default; a
/// cold (even warm-cached) rust-analyzer cold index can exceed the old 30s. Pure for testing.
fn parse_ready_secs(var: Option<String>) -> u64 {
    var.and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(90)
}

/// The configured readiness budget as a Duration.
fn ready_timeout() -> std::time::Duration {
    std::time::Duration::from_secs(parse_ready_secs(std::env::var("LSP_MCP_READY_SECS").ok()))
}

fn name_arg(a: &Value) -> Result<&str, Value> {
    a.get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| json!({"error":"missing required string arg `name`"}))
}

pub fn tool_schemas() -> Vec<Value> {
    let name_only = json!({
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "description": "symbol name to resolve"
            }
        },
        "required": ["name"]
    });
    vec![
        json!({
            "name": "workspace_symbol",
            "description": "Find a symbol by name across the repo (entry point).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string"
                    }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "document_symbols",
            "description": "Outline of a file's symbols.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file": {
                        "type": "string"
                    }
                },
                "required": ["file"]
            }
        }),
        json!({
            "name": "definition",
            "description": "Type-resolved go-to-definition of a symbol.",
            "inputSchema": name_only
        }),
        json!({
            "name": "references",
            "description": "All references to a symbol (blast radius); type-resolved across the language's generics/polymorphism.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string"
                    },
                    "include_declaration": {
                        "type": "boolean"
                    }
                },
                "required": ["name"]
            }
        }),
        json!({
            "name": "hover",
            "description": "Resolved type + signature + docs at a symbol.",
            "inputSchema": name_only
        }),
        json!({
            "name": "implementations",
            "description": "Implementations of a symbol (Rust trait impls; Python subclasses / overrides).",
            "inputSchema": name_only
        }),
        json!({
            "name": "call_hierarchy",
            "description": "Type-resolved callers/callees of a symbol.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string"
                    },
                    "direction": {
                        "type": "string",
                        "enum": ["incoming", "outgoing"]
                    }
                },
                "required": ["name"]
            }
        }),
    ]
}

fn ok(id: &Value, body: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{
                "type": "text",
                "text": body.to_string()
            }]
        }
    })
}

/// An `isError` tool result the agent can read (and, for not-ready, retry on). Shared shape so the three
/// error paths (spawn/respawn failure, not-ready, tool-body failure) can't drift.
fn iserror(id: &Value, text: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "isError": true,
            "content": [{ "type": "text", "text": text.into() }]
        }
    })
}

/// The honest "language server not ready" reply: the agent reads it and retries, instead of an empty hit
/// list it misreads as "no lsp tool". Language-neutral — lsp-mcp is polyglot (RA / gopls / basedpyright).
fn not_ready_response(id: &Value) -> Value {
    iserror(
        id,
        "the language server is still indexing (or could not index offline); retry shortly",
    )
}

fn dispatch(id: &Value, params: &Value, s: &mut LspClient) -> Value {
    let tool = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let a = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    log_tool_call(tool, &a);
    s.touch();
    match s.ensure_ready(ready_timeout()) {
        Err(e) => iserror(id, format!("lsp-mcp error: {e}")),
        Ok(false) => not_ready_response(id),
        Ok(true) => match dispatch_body(tool, &a, s) {
            Ok(body) => ok(id, body),
            Err(e) => iserror(id, format!("lsp-mcp error: {e}")),
        },
    }
}

/// Format one tool-call observability line. Pure — testable.
fn format_call_log_line(pid: u32, secs: u64, tool: &str, args: &Value) -> String {
    format!(
        "t={secs} pid={pid} tool={tool} args={}\n",
        serde_json::to_string(args).unwrap_or_default()
    )
}

/// Where to record tool calls: `$LSP_MCP_LOG`, else `~/.local/share/a2a/lsp-mcp-calls.log`.
fn call_log_path() -> Option<std::path::PathBuf> {
    match std::env::var("LSP_MCP_LOG") {
        Ok(p) if !p.is_empty() => Some(p.into()),
        _ => std::env::var("HOME")
            .ok()
            .map(|h| std::path::Path::new(&h).join(".local/share/a2a/lsp-mcp-calls.log")),
    }
}

/// Record a tool call to stderr + the call-log file. Best-effort observability — never fails the call.
fn log_tool_call(tool: &str, args: &Value) {
    use std::io::Write;
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format_call_log_line(std::process::id(), secs, tool, args);
    eprint!("[lsp-mcp] {line}");
    if let Some(path) = call_log_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

fn dispatch_body(tool: &str, a: &Value, s: &mut LspClient) -> anyhow::Result<Value> {
    Ok(match tool {
        "workspace_symbol" => {
            let q = a["query"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("missing `query`"))?;
            render_hits(&s.workspace_symbol(q)?)
        }
        "document_symbols" => {
            let f = a["file"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("missing `file`"))?;
            render_hits(&s.document_symbols(std::path::Path::new(f))?)
        }
        "definition" => {
            render_hits(&s.definition(name_arg(a).map_err(|e| anyhow::anyhow!(e.to_string()))?)?)
        }
        "references" => render_hits(&s.references(
            name_arg(a).map_err(|e| anyhow::anyhow!(e.to_string()))?,
            a["include_declaration"].as_bool().unwrap_or(true),
        )?),
        "hover" => {
            json!({"hover": s.hover(
                name_arg(a).map_err(|e| anyhow::anyhow!(e.to_string()))?
            )?})
        }
        "implementations" => render_hits(
            &s.implementations(name_arg(a).map_err(|e| anyhow::anyhow!(e.to_string()))?)?,
        ),
        "call_hierarchy" => render_hits(&s.call_hierarchy(
            name_arg(a).map_err(|e| anyhow::anyhow!(e.to_string()))?,
            a["direction"].as_str().unwrap_or("incoming") == "incoming",
        )?),
        other => return Err(anyhow::anyhow!("unknown tool `{other}`")),
    })
}

/// Block on stdin, driving the MCP loop against a warm `LspClient`.
pub fn serve(mut session: LspClient) -> anyhow::Result<()> {
    let stdin = std::io::stdin();
    let mut r = BufReader::new(stdin.lock());
    let mut out = std::io::stdout();
    let mut lc = Lifecycle::default();
    while let Some(body) = transport::read_frame_stdin(&mut r)? {
        let msg: Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let reply = if msg.get("method").and_then(|m| m.as_str()) == Some("tools/call")
            && lc.is_initialized()
        {
            Some(dispatch(&msg["id"], &msg["params"], &mut session))
        } else if let Some(r) = lc.handle_meta(&msg) {
            Some(r)
        } else if msg.get("id").is_some() && msg.get("method").is_some() {
            Some(err(&msg["id"], METHOD_NOT_FOUND, "unknown method"))
        } else {
            None
        };
        if let Some(reply) = reply {
            transport::write_line_frame(&mut out, serde_json::to_vec(&reply)?.as_slice())?;
        }
    }
    session.shutdown();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_log_line_carries_tool_and_args() {
        let line = format_call_log_line(42, 1_700_000_000, "references", &json!({"name": "add"}));
        assert!(line.contains("pid=42"), "{line}");
        assert!(line.contains("tool=references"), "{line}");
        assert!(line.contains("\"name\":\"add\""), "{line}");
        assert!(line.ends_with('\n'));
    }

    #[test]
    fn parse_ready_secs_defaults_and_overrides() {
        assert_eq!(parse_ready_secs(None), 90);
        assert_eq!(parse_ready_secs(Some(String::new())), 90);
        assert_eq!(parse_ready_secs(Some("notanum".into())), 90);
        assert_eq!(parse_ready_secs(Some("0".into())), 90); // 0 is meaningless → default
        assert_eq!(parse_ready_secs(Some("120".into())), 120);
    }

    #[test]
    fn not_ready_response_is_iserror_with_retry_hint() {
        let r = not_ready_response(&json!(7));
        assert_eq!(r["id"], json!(7));
        assert_eq!(r["result"]["isError"], json!(true));
        let txt = r["result"]["content"][0]["text"].as_str().unwrap();
        assert!(txt.contains("indexing"), "{txt}");
        assert!(txt.contains("retry"), "{txt}");
    }

    #[test]
    fn exposes_the_seven_tools() {
        let names: Vec<String> = tool_schemas()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        for n in [
            "workspace_symbol",
            "document_symbols",
            "definition",
            "references",
            "hover",
            "implementations",
            "call_hierarchy",
        ] {
            assert!(names.iter().any(|name| name == n), "missing tool {n}");
        }
        assert_eq!(names.len(), 7);
        for t in tool_schemas() {
            assert_eq!(t["inputSchema"]["type"], "object");
        }
    }
}
