pub mod error;
pub mod transport;

use crate::lsp::LspSession;
use crate::mcp::error::*;
use crate::mcp::transport::Lifecycle;
use crate::shape::render_hits;
use serde_json::{json, Value};
use std::io::BufReader;

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
            "description": "All references to a symbol (blast radius); resolves generics/traits.",
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
            "description": "Trait impls / who implements a trait or type.",
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

fn dispatch(id: &Value, params: &Value, s: &mut LspSession) -> Value {
    let tool = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let a = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    match dispatch_body(tool, &a, s) {
        Ok(body) => ok(id, body),
        // Tool failures are reported as content with isError, so the agent sees the reason and degrades.
        Err(e) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "isError": true,
                "content": [{
                    "type": "text",
                    "text": format!("lsp-mcp error: {e}")
                }]
            }
        }),
    }
}

fn dispatch_body(tool: &str, a: &Value, s: &mut LspSession) -> anyhow::Result<Value> {
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

/// Block on stdin, driving the MCP loop against a warm `LspSession`.
pub fn serve(mut session: LspSession) -> anyhow::Result<()> {
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
            crate::lsp::codec::write_frame(&mut out, serde_json::to_vec(&reply)?.as_slice())?;
        }
    }
    session.shutdown();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
