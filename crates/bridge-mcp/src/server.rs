use std::sync::Arc;

use bridge_coordinator::params::{InjectParams, OpParams, PermitParams};
use bridge_coordinator::session_manager::ResetOutcome;
use bridge_coordinator::Coordinator;
use bridge_core::error::BridgeError;
use bridge_core::ids::{ContextId, TaskId};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

use crate::transport;

pub const MAX_FRAME: usize = 16 * 1024 * 1024;

/// Drive the MCP protocol over a read/write pair.
///
/// The only writes performed by this adapter are framed JSON replies written through `write`.
/// Clean EOF triggers Coordinator shutdown; truncated/invalid frames stop the loop without
/// treating them as EOF.
pub async fn serve<R, W>(read: R, mut write: W, coord: Arc<Coordinator>) -> Result<(), BridgeError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut reader = crate::framing::FrameReader::new(read, MAX_FRAME);
    let mut lc = transport::Lifecycle::default();
    loop {
        match reader.next().await {
            None => {
                coord.shutdown().await;
                break;
            }
            Some(Err(_)) => break,
            Some(Ok(msg)) => {
                let reply = if is_tools_call(&msg) && lc.is_initialized() {
                    Some(dispatch(&msg["id"], &msg["params"], &coord).await)
                } else if let Some(reply) = lc.handle_meta(&msg) {
                    Some(reply)
                } else if msg.get("id").is_some() && msg.get("method").is_some() {
                    Some(transport::unknown_method(&msg["id"]))
                } else {
                    None
                };
                if let Some(reply) = reply {
                    let mut buf =
                        serde_json::to_vec(&reply).map_err(|_| BridgeError::FrameError)?;
                    buf.push(b'\n');
                    write
                        .write_all(&buf)
                        .await
                        .map_err(|_| BridgeError::FrameError)?;
                    let _ = write.flush().await;
                }
            }
        }
    }
    Ok(())
}

fn is_tools_call(msg: &Value) -> bool {
    msg.get("method").and_then(|m| m.as_str()) == Some("tools/call")
}

async fn dispatch(id: &Value, params: &Value, coord: &Coordinator) -> Value {
    let tool = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let result = match tool {
        "run" => match OpParams::from_mcp_args(&args) {
            Ok(p) => coord.prompt(p).await.map(|o| {
                json!({
                    "text": o.text,
                    "stop_reason": o.stop_reason,
                    "context": o.context.as_str()
                })
            }),
            Err(e) => Err(e),
        },
        "continue" => match OpParams::from_mcp_args(&args) {
            Ok(p) => coord.continue_turn(p).await.map(|o| {
                json!({
                    "text": o.text,
                    "stop_reason": o.stop_reason,
                    "context": o.context.as_str()
                })
            }),
            Err(e) => Err(e),
        },
        "inject" => match InjectParams::from_mcp_args(&args) {
            Ok(p) => coord
                .inject(p.into_request())
                .await
                .map(|queued| json!({ "queued": queued })),
            Err(e) => Err(e),
        },
        "permit" => match PermitParams::from_mcp_args(&args) {
            Ok(p) => coord
                .permit(p)
                .await
                .map(|resolved| json!({ "resolved": resolved })),
            Err(e) => Err(e),
        },
        "run_workflow" => match OpParams::from_mcp_args_for_workflow(&args) {
            Ok(p) => coord
                .run_workflow(p)
                .await
                .map(|task| json!({ "task_id": task.as_str() })),
            Err(e) => Err(e),
        },
        "status" => match parse_status_args(&args) {
            Ok((ctx, task)) => coord
                .status(ctx, task)
                .await
                .map(|dto| serde_json::to_value(dto).unwrap_or_default()),
            Err(e) => Err(e),
        },
        "clear" => match parse_ctx(&args) {
            Ok(ctx) => coord.clear(ctx).await.map(|out| match out {
                ResetOutcome::Cleared { generation } => json!({ "generation": generation }),
                ResetOutcome::NotFound => json!({ "not_found": true }),
            }),
            Err(e) => Err(e),
        },
        "cancel_task" => match parse_task(&args) {
            Ok(task) => coord
                .cancel_task(task)
                .await
                .map(|cancelled| json!({ "cancelled": cancelled })),
            Err(e) => Err(e),
        },
        other => return transport::iserror(id, format!("unknown tool {other}")),
    };

    match result {
        Ok(body) => transport::ok(id, body),
        Err(e) => transport::iserror(id, e.client_message()),
    }
}

fn parse_status_args(args: &Value) -> Result<(Option<ContextId>, Option<TaskId>), BridgeError> {
    let ctx = args
        .get("context")
        .and_then(|v| v.as_str())
        .map(ContextId::parse)
        .transpose()
        .map_err(|_| BridgeError::InvalidRequest { field: "context" })?;
    let task = args
        .get("task_id")
        .and_then(|v| v.as_str())
        .map(TaskId::parse)
        .transpose()
        .map_err(|_| BridgeError::InvalidRequest { field: "task_id" })?;
    Ok((ctx, task))
}

fn parse_ctx(args: &Value) -> Result<ContextId, BridgeError> {
    args.get("context")
        .and_then(|v| v.as_str())
        .ok_or(BridgeError::InvalidRequest { field: "context" })
        .and_then(ContextId::parse)
        .map_err(|_| BridgeError::InvalidRequest { field: "context" })
}

fn parse_task(args: &Value) -> Result<TaskId, BridgeError> {
    args.get("task_id")
        .and_then(|v| v.as_str())
        .ok_or(BridgeError::InvalidRequest { field: "task_id" })
        .and_then(TaskId::parse)
        .map_err(|_| BridgeError::InvalidRequest { field: "task_id" })
}
