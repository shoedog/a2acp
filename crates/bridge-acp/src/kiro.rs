// kiro.rs — KiroBackend: drives a Kiro child process over JSON-RPC line-framed stdio.
// Spec §5.3 cancellation rule: completion is the prompt RESULT (stopReason:"cancelled"),
// NOT the act of sending session/cancel. See Codex finding 2.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::Mutex;

use bridge_core::error::BridgeError;
use bridge_core::ids::SessionId;
use bridge_core::ports::{AgentBackend, BackendStream, Update};

use crate::framing::FrameReader;
use crate::replay::frame_to_update;
use crate::supervisor::Supervised;

const MAX_FRAME: usize = 16 * 1024 * 1024;

// ── Inner state shared across all async ops ─────────────────────────────────

struct Inner {
    stdin: ChildStdin,
    reader: FrameReader<BufReader<ChildStdout>>,
    supervised: Supervised,
}

// ── Public struct ────────────────────────────────────────────────────────────

pub struct KiroBackend {
    inner: Arc<Mutex<Inner>>,
    id_counter: Arc<AtomicU64>,
}

impl KiroBackend {
    /// Construct from an already-spawned scripted child (used in tests and when
    /// the caller has already set up the process).
    pub fn from_child(mut supervised: Supervised) -> Self {
        let child = supervised.child_mut();
        let stdin = child.stdin.take().expect("stdin must be piped");
        let stdout = child.stdout.take().expect("stdout must be piped");
        let reader = FrameReader::new(BufReader::new(stdout), MAX_FRAME);
        Self {
            inner: Arc::new(Mutex::new(Inner {
                stdin,
                reader,
                supervised,
            })),
            id_counter: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Send `session/new`, read back the `{result:{sessionId}}` response.
    pub async fn new_session(&self) -> Result<SessionId, BridgeError> {
        let id = self.next_id();
        let mut g = self.inner.lock().await;
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/new",
            "params": {}
        });
        write_line(&mut g.stdin, &req).await?;
        // Read frames until we get the result for this id.
        loop {
            let frame = g.reader.next().await.ok_or(BridgeError::AgentCrashed)??;
            if frame.get("id").and_then(|v| v.as_u64()) == Some(id) {
                let sid = frame
                    .pointer("/result/sessionId")
                    .and_then(|v| v.as_str())
                    .ok_or(BridgeError::FrameError)?;
                return SessionId::parse(sid);
            }
            // unexpected frame before the session/new reply — skip it
        }
    }

    /// Send `session/cancel`, then wait for the prompt result to arrive with
    /// `stopReason:"cancelled"`. On timeout, SIGTERM the process group and
    /// return `Err(CancelTimeout)`.
    ///
    /// NOTE: This method reads frames directly from the child's stdout reader.
    /// It must only be called when the stream returned by `prompt()` has been
    /// dropped (or will not be polled concurrently), otherwise both this
    /// method and the stream would contend for the same reader.
    pub async fn cancel_with_timeout(
        &self,
        session: &SessionId,
        grace: std::time::Duration,
    ) -> Result<(), BridgeError> {
        self.send_cancel(session).await?;
        // Wait for the child's stdout to produce the cancelled result within grace.
        let result = tokio::time::timeout(grace, self.wait_for_done()).await;
        match result {
            Ok(Ok(_stop_reason)) => Ok(()),
            Ok(Err(e)) => Err(e),
            Err(_elapsed) => {
                // Grace elapsed — kill the process group, reap, return CancelTimeout.
                let dummy = Supervised::spawn("/bin/sh", &["-c", "exit 0"])
                    .map_err(|_| BridgeError::AgentCrashed)?;
                let supervised = {
                    let mut g = self.inner.lock().await;
                    std::mem::replace(&mut g.supervised, dummy)
                };
                supervised
                    .terminate(std::time::Duration::from_millis(100))
                    .await;
                Err(BridgeError::CancelTimeout)
            }
        }
    }

    // ── Private helpers ──────────────────────────────────────────────────────

    fn next_id(&self) -> u64 {
        self.id_counter.fetch_add(1, Ordering::Relaxed)
    }

    async fn send_cancel(&self, session: &SessionId) -> Result<(), BridgeError> {
        let id = self.next_id();
        let mut g = self.inner.lock().await;
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/cancel",
            "params": { "sessionId": session.as_str() }
        });
        write_line(&mut g.stdin, &req).await
    }

    /// Read frames from stdout until a `Done` update arrives; return the stop_reason.
    /// This is used by `cancel_with_timeout` to wait for the prompt result.
    async fn wait_for_done(&self) -> Result<String, BridgeError> {
        loop {
            let frame = {
                let mut g = self.inner.lock().await;
                g.reader.next().await
            };
            match frame {
                None => return Err(BridgeError::AgentCrashed),
                Some(Err(e)) => return Err(e),
                Some(Ok(v)) => {
                    if let Some(Update::Done { stop_reason }) = frame_to_update(v) {
                        return Ok(stop_reason);
                    }
                    // other frames (notifications) are consumed silently
                }
            }
        }
    }
}

// ── AgentBackend impl ────────────────────────────────────────────────────────

#[async_trait]
impl AgentBackend for KiroBackend {
    /// Write `session/prompt` to the child's stdin and return a stream that
    /// yields `Update`s from the child's stdout until a Done frame arrives.
    ///
    /// The stream drives the child's stdout reader directly; `cancel()` writes
    /// `session/cancel` to stdin. The COMPLETION of a cancel is the prompt
    /// RESULT carrying `stopReason:"cancelled"` — which arrives on this stream.
    async fn prompt(
        &self,
        session: &SessionId,
        parts: Vec<bridge_core::domain::Part>,
    ) -> Result<BackendStream, BridgeError> {
        let id = self.next_id();
        let session_id = session.as_str().to_string();

        {
            let mut g = self.inner.lock().await;
            let serialized_parts: Vec<serde_json::Value> = parts
                .iter()
                .map(|p| serde_json::json!({ "text": p.text }))
                .collect();
            let req = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "session/prompt",
                "params": {
                    "sessionId": &session_id,
                    "parts": serialized_parts
                }
            });
            write_line(&mut g.stdin, &req).await?;
        }

        // Build a stream that pulls frames from the shared reader.
        // We hold the Arc<Mutex<Inner>> and lock it per frame.
        let inner = Arc::clone(&self.inner);

        let stream = futures::stream::unfold(
            (inner, id, false), // (inner, prompt_id, done)
            |(inner, prompt_id, done)| async move {
                if done {
                    return None;
                }
                loop {
                    let frame = {
                        let mut g = inner.lock().await;
                        g.reader.next().await
                    };
                    match frame {
                        None => return None, // child closed stdout
                        Some(Err(e)) => {
                            return Some((Err(e), (inner, prompt_id, true)));
                        }
                        Some(Ok(v)) => {
                            // Check if this is the result for our prompt request.
                            let is_our_result =
                                v.get("id").and_then(|x| x.as_u64()) == Some(prompt_id);
                            if is_our_result {
                                // Must be a result frame — map it.
                                match frame_to_update(v) {
                                    Some(u @ Update::Done { .. }) => {
                                        return Some((Ok(u), (inner, prompt_id, true)));
                                    }
                                    Some(u) => {
                                        return Some((Ok(u), (inner, prompt_id, false)));
                                    }
                                    None => {
                                        // Result for our id with no recognized shape —
                                        // must still surface as a terminal Done so the
                                        // caller never sees a silent stream close
                                        // (Issue 3, §5.3 "naive bridge" failure).
                                        return Some((
                                            Ok(Update::Done {
                                                stop_reason: "unknown".into(),
                                            }),
                                            (inner, prompt_id, true),
                                        ));
                                    }
                                }
                            }
                            // Notification or other frame — map and yield if recognized.
                            match frame_to_update(v) {
                                Some(Update::Done { stop_reason }) => {
                                    // Done arrived as a notification (shouldn't happen in
                                    // well-behaved protocol, but handle defensively).
                                    return Some((
                                        Ok(Update::Done { stop_reason }),
                                        (inner, prompt_id, true),
                                    ));
                                }
                                Some(u) => {
                                    return Some((Ok(u), (inner, prompt_id, false)));
                                }
                                None => continue, // skip unknown frames
                            }
                        }
                    }
                }
            },
        );

        Ok(Box::pin(stream))
    }

    /// Write `session/cancel` to the child's stdin and return immediately.
    ///
    /// Spec §5.3 / Codex finding 2: cancellation completion is signalled by
    /// the prompt RESULT arriving on the BackendStream with
    /// `stopReason:"cancelled"`, NOT by the act of sending this notification.
    /// The caller must poll the stream to observe the completion.
    async fn cancel(&self, session: &SessionId) -> Result<(), BridgeError> {
        self.send_cancel(session).await
    }
}

// ── Utility ──────────────────────────────────────────────────────────────────

async fn write_line(stdin: &mut ChildStdin, v: &serde_json::Value) -> Result<(), BridgeError> {
    let mut line = serde_json::to_vec(v).expect("serialization is infallible");
    line.push(b'\n');
    stdin
        .write_all(&line)
        .await
        .map_err(|_| BridgeError::AgentCrashed)?;
    stdin.flush().await.map_err(|_| BridgeError::AgentCrashed)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::supervisor::Supervised;
    use bridge_core::error::BridgeError;
    use bridge_core::ports::{AgentBackend, Update};
    use futures::StreamExt;
    use std::time::Duration;

    fn scripted(script: &str) -> Supervised {
        Supervised::spawn("/bin/sh", &["-c", script]).unwrap()
    }

    #[tokio::test]
    async fn new_session_then_prompt_streams_text_then_done() {
        // child: replies sessionId to the first request, then on the prompt emits one update + result.
        let be = KiroBackend::from_child(scripted(
            "printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"sessionId\":\"s1\"}}'; \
             read line; \
             printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{\"text\":\"PONG\"}}'; \
             printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"stopReason\":\"end_turn\"}}'; sleep 1"));
        let sid = be.new_session().await.unwrap();
        assert_eq!(sid.as_str(), "s1");
        let mut s = be.prompt(&sid, vec![]).await.unwrap();
        assert!(matches!(s.next().await, Some(Ok(Update::Text(t))) if t == "PONG"));
        assert!(
            matches!(s.next().await, Some(Ok(Update::Done{stop_reason})) if stop_reason == "end_turn")
        );
    }

    #[tokio::test]
    async fn cancel_completion_is_the_prompt_result_not_the_notification() {
        // child emits sessionId, then an update, then (only after reading the cancel line) the cancelled result.
        let be = KiroBackend::from_child(scripted(
            "printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"sessionId\":\"s1\"}}'; \
             read p; \
             printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{\"text\":\"work\"}}'; \
             read c; \
             printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"stopReason\":\"cancelled\"}}'; sleep 1"));
        let sid = be.new_session().await.unwrap();
        let mut s = be.prompt(&sid, vec![]).await.unwrap();
        assert!(matches!(s.next().await, Some(Ok(Update::Text(_))))); // got the update
        be.cancel(&sid).await.unwrap(); // writes session/cancel
                                        // completion arrives as the prompt RESULT, not from the notification send:
        assert!(
            matches!(s.next().await, Some(Ok(Update::Done{stop_reason})) if stop_reason == "cancelled")
        );
    }

    #[tokio::test]
    async fn unrecognized_result_frame_still_yields_terminal_done() {
        let be = KiroBackend::from_child(scripted(
            "printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"sessionId\":\"s1\"}}'; \
             read p; \
             printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{}}'; sleep 1",
        ));
        let sid = be.new_session().await.unwrap();
        let mut s = be.prompt(&sid, vec![]).await.unwrap();
        // must be a terminal Done, NOT a silent None
        match s.next().await {
            Some(Ok(Update::Done { .. })) => {}
            other => {
                panic!("expected terminal Done for an unrecognized result frame, got {other:?}")
            }
        }
    }

    #[tokio::test]
    async fn prompt_serializes_part_text_into_session_prompt() {
        // child: emits sessionId; reads the prompt line from stdin; echoes that line's content back
        // (stripped of quotes) inside a session/update text; then a result.
        let be = KiroBackend::from_child(scripted(
            "printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"sessionId\":\"s1\"}}'; \
             IFS= read -r _new_req; \
             IFS= read -r line; \
             printf '{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{\"text\":\"GOT:%s\"}}\\n' \"$(printf '%s' \"$line\" | tr -d '\\\"')\"; \
             printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"stopReason\":\"end_turn\"}}'; sleep 1"));
        let sid = be.new_session().await.unwrap();
        let mut s = be
            .prompt(
                &sid,
                vec![bridge_core::domain::Part {
                    text: "HELLO_PART".into(),
                }],
            )
            .await
            .unwrap();
        // the echoed prompt line must contain our part text -> proves it was serialized into session/prompt
        assert!(matches!(s.next().await, Some(Ok(Update::Text(t))) if t.contains("HELLO_PART")));
    }

    #[tokio::test]
    async fn cancel_timeout_sigterms_and_errors() {
        // child gives a session, never returns a prompt result -> cancel_with_timeout times out, reaps, errors.
        let be = KiroBackend::from_child(scripted(
            "printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"sessionId\":\"s1\"}}'; sleep 30"));
        let sid = be.new_session().await.unwrap();
        let _ = be.prompt(&sid, vec![]).await.unwrap();
        let err = be
            .cancel_with_timeout(&sid, Duration::from_millis(200))
            .await
            .unwrap_err();
        assert_eq!(err, BridgeError::CancelTimeout);
    }
}
