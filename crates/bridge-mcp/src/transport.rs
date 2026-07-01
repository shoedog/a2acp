use serde_json::{json, Value};

const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;

#[derive(Default)]
pub struct Lifecycle {
    initialized: bool,
}

impl Lifecycle {
    /// Handle MCP lifecycle/meta methods. Tool dispatch is owned by `server`.
    pub fn handle_meta(&mut self, msg: &Value) -> Option<Value> {
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        match method {
            "initialize" => {
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
                            "name": "a2a-bridge-mcp",
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
                    "tools": tool_schemas()
                }
            })),
            "tools/call" if !self.initialized => Some(jsonrpc_err(
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

pub fn tool_schemas() -> Vec<Value> {
    vec![
        json!({
            "name": "run",
            "description": "Prompt a warm agent and return the collected turn output.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "input": { "type": "string" },
                    "context": { "type": "string" },
                    "agent": { "type": "string" },
                    "model": { "type": "string" },
                    "effort": { "type": "string" },
                    "mode": { "type": "string" },
                    "cwd": { "type": "string" }
                },
                "required": ["input"]
            }
        }),
        json!({
            "name": "continue",
            "description": "Continue an existing warm context.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "input": { "type": "string" },
                    "context": { "type": "string" }
                },
                "required": ["input", "context"]
            }
        }),
        json!({
            "name": "inject",
            "description": "Queue text for the next turn of an existing warm context.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "context": { "type": "string" },
                    "text": { "type": "string" },
                    "append": { "type": "boolean" },
                    "mode": {
                        "type": "string",
                        "enum": ["prepend_next_turn", "append_next_turn"]
                    },
                    "dedupeKey": { "type": "string" }
                },
                "required": ["context", "text"]
            }
        }),
        json!({
            "name": "permit",
            "description": "Resolve a pending interactive permission request.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "context": { "type": "string" },
                    "generation": { "type": "integer", "minimum": 0 },
                    "op": { "type": "string" },
                    "requestId": { "type": "string" },
                    "decision": {
                        "type": "object",
                        "properties": {
                            "decision": {
                                "type": "string",
                                "enum": ["approve", "deny", "modify", "escalate"]
                            },
                            "optionId": { "type": "string" },
                            "reason": { "type": "string" },
                            "note": { "type": "string" }
                        },
                        "required": ["decision"]
                    }
                },
                "required": ["context", "generation", "op", "requestId", "decision"]
            }
        }),
        json!({
            "name": "run_workflow",
            "description": "Start a detached workflow run from this MCP process's config. Keep project-specific configs/prompts/workflows in the owning repo or /private/tmp; preflight with `a2a-bridge validate --config` and marker-driven `--examples-policy deny` gates when needed.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workflow": { "type": "string" },
                    "input": { "type": "string" },
                    "cwd": { "type": "string" }
                },
                "required": ["workflow", "input"]
            }
        }),
        json!({
            "name": "status",
            "description": "Return status for exactly one warm context or detached task.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "context": { "type": "string" },
                    "task_id": { "type": "string" }
                }
            }
        }),
        json!({
            "name": "clear",
            "description": "Clear an idle warm context.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "context": { "type": "string" }
                },
                "required": ["context"]
            }
        }),
        json!({
            "name": "cancel_task",
            "description": "Cancel a detached task.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task_id": { "type": "string" }
                },
                "required": ["task_id"]
            }
        }),
    ]
}

pub fn ok(id: &Value, body: Value) -> Value {
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

pub fn iserror(id: &Value, text: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "isError": true,
            "content": [{ "type": "text", "text": text.into() }]
        }
    })
}

pub fn jsonrpc_err(id: &Value, code: i64, msg: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": msg.into()
        }
    })
}

pub fn unknown_method(id: &Value) -> Value {
    jsonrpc_err(id, METHOD_NOT_FOUND, "unknown method")
}
