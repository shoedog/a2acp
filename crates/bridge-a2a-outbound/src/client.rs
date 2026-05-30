// client.rs — outbound A2A HTTP client (spec §3.1.2/§7).
//
// `A2aClient` posts a `SendStreamingMessage` JSON-RPC request to a remote
// peer and returns the raw `reqwest::Response` so callers can stream the SSE
// reply. `open_stream` (Task 4) wraps that with an SSE parser that emits
// `bridge_core::translator::Event` items and captures the peer task id.

use bridge_core::domain::{Part, PeerTaskId};
use bridge_core::error::BridgeError;
use bridge_core::ports::DelegationStream;
use bridge_core::translator::Event;
use futures::StreamExt;
use tokio::sync::watch;

/// Outbound A2A peer client.
///
/// Constructed once per configured peer; `send_streaming` is called per
/// delegation request. The returned `Response` carries an SSE body that
/// Task 4's parser will consume.
pub struct A2aClient {
    http: reqwest::Client,
    url: String,
    bearer: String,
}

impl A2aClient {
    /// Create a new client.
    ///
    /// `base_url` — the root URL of the remote A2A peer (e.g. `"http://peer:8080"`).
    /// `auth`     — bearer token in the form `"bearer:TOKEN"` (prefix is stripped).
    /// `timeout`  — per-request timeout (spec §3.1.7).
    pub fn new(base_url: &str, auth: &str, timeout: std::time::Duration) -> Self {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("reqwest client build");
        let bearer = auth.strip_prefix("bearer:").unwrap_or(auth).to_string();
        let url = base_url.trim_end_matches('/').to_string() + "/";
        Self { http, url, bearer }
    }

    /// POST a `SendStreamingMessage` JSON-RPC request to the peer.
    ///
    /// Builds the `a2a::Message` from `parts`, wraps it in
    /// `a2a::SendMessageRequest`, serialises as JSON-RPC, adds the bearer
    /// `Authorization` and `A2A-Version` headers, and returns the raw
    /// response. The caller is responsible for streaming / parsing the SSE
    /// body (Task 4).
    pub async fn send_streaming(&self, parts: &[Part]) -> Result<reqwest::Response, BridgeError> {
        let msg = a2a::Message {
            message_id: a2a::new_message_id(),
            context_id: Some(a2a::new_context_id()),
            task_id: None,
            role: a2a::Role::User,
            parts: parts
                .iter()
                .map(|p| a2a::Part::text(p.text.clone()))
                .collect(),
            metadata: None,
            extensions: None,
            reference_task_ids: None,
        };

        let req = a2a::SendMessageRequest {
            message: msg,
            configuration: None,
            metadata: None,
            tenant: None,
        };

        let rpc = a2a::JsonRpcRequest::new(
            a2a::JsonRpcId::String("req-1".into()),
            a2a::methods::SEND_STREAMING_MESSAGE,
            Some(serde_json::to_value(&req).map_err(|_| BridgeError::UpstreamA2aError)?),
        );

        self.http
            .post(&self.url)
            .bearer_auth(&self.bearer)
            .header(a2a::SVC_PARAM_VERSION, a2a::VERSION)
            .json(&rpc)
            .send()
            .await
            .map_err(|_| BridgeError::UpstreamA2aError)
    }

    /// Open a streaming connection to the peer, parse the SSE body, and return
    /// a `Stream<Result<Event, BridgeError>>` plus a watch receiver for the
    /// peer task id captured from the first response event.
    ///
    /// Mapping rules (Codex 2/3/6/7):
    /// - Invalid JSON → `Err(UpstreamA2aError)`, end.
    /// - JSON with no known key (`statusUpdate`/`artifactUpdate`/`task`/`message`) → skip.
    /// - `StatusUpdate` terminal-Completed → end cleanly (no item).
    /// - `StatusUpdate` terminal-Failed/Canceled/Rejected → `Err`, end.
    /// - `StatusUpdate` non-terminal → `Event::status(message text or "")`.
    /// - `ArtifactUpdate` lastChunk=true → `Event::artifact(text)`, end.
    /// - `ArtifactUpdate` else → `Event::status(text)`.
    /// - `Task` → capture id if present, no event.
    /// - `Message` → `Event::status(text)`.
    /// - Clean EOF before any terminal signal → `Err(UpstreamA2aError)`.
    pub async fn open_stream(
        &self,
        parts: &[Part],
    ) -> Result<(DelegationStream, watch::Receiver<Option<PeerTaskId>>), BridgeError> {
        let resp = self.send_streaming(parts).await?;

        if !resp.status().is_success() {
            return Err(BridgeError::UpstreamA2aError);
        }

        let (tx, rx) = watch::channel::<Option<PeerTaskId>>(None);
        let byte_stream = resp.bytes_stream();

        let stream = async_stream::stream! {
            // SSE framing state.
            let mut buf = String::new();
            // Accumulated `data:` payload lines for the current SSE event.
            let mut pending_data = String::new();

            let mut byte_stream = std::pin::pin!(byte_stream);

            // `done` tracks whether a terminal signal was received.  It is used only in
            // the EOF arm to decide whether to emit the premature-EOF error.
            let mut done = false;

            'outer: loop {
                let next = byte_stream.next().await;

                // ── EOF ─────────────────────────────────────────────────────────
                let bytes = match next {
                    None => {
                        // Process any partial line still in the buffer.
                        let leftover = buf.trim_end_matches('\r').trim_end_matches('\n').to_string();
                        if !leftover.is_empty() {
                            process_sse_line(&leftover, &mut pending_data);
                        }
                        // Dispatch any pending SSE payload.
                        if !pending_data.is_empty() {
                            let json_str = std::mem::take(&mut pending_data);
                            match parse_and_map(&json_str, &tx) {
                                MapResult::Skip => {}
                                MapResult::Event(ev) => { yield Ok(ev); }
                                MapResult::Terminal => { done = true; }
                                MapResult::TerminalEvent(ev) => { yield Ok(ev); done = true; }
                                MapResult::Error(e) => { yield Err(e); done = true; }
                            }
                        }
                        if !done {
                            yield Err(BridgeError::UpstreamA2aError);
                        }
                        break 'outer;
                    }
                    // ── Network error ────────────────────────────────────────────
                    Some(Err(_)) => {
                        yield Err(BridgeError::UpstreamA2aError);
                        break 'outer;
                    }
                    Some(Ok(b)) => b,
                };

                // ── Data bytes received ──────────────────────────────────────────
                let chunk = match std::str::from_utf8(&bytes) {
                    Ok(s) => s.to_string(),
                    Err(_) => {
                        yield Err(BridgeError::UpstreamA2aError);
                        break 'outer;
                    }
                };
                buf.push_str(&chunk);

                // Extract complete '\n'-terminated lines.
                while let Some(pos) = buf.find('\n') {
                    let line = buf[..pos].trim_end_matches('\r').to_string();
                    buf = buf[pos + 1..].to_string();

                    if line.is_empty() {
                        // Blank line → SSE event boundary.
                        if !pending_data.is_empty() {
                            let json_str = std::mem::take(&mut pending_data);
                            match parse_and_map(&json_str, &tx) {
                                MapResult::Skip => {}
                                MapResult::Event(ev) => {
                                    yield Ok(ev);
                                }
                                MapResult::Terminal => {
                                    break 'outer;
                                }
                                MapResult::TerminalEvent(ev) => {
                                    yield Ok(ev);
                                    break 'outer;
                                }
                                MapResult::Error(e) => {
                                    yield Err(e);
                                    break 'outer;
                                }
                            }
                        }
                    } else {
                        process_sse_line(&line, &mut pending_data);
                    }
                }
            }
        };

        Ok((Box::pin(stream), rx))
    }
}

// ---------------------------------------------------------------------------
// SSE line processor
// ---------------------------------------------------------------------------

/// Accumulate `data:` lines into `pending_data`; ignore other SSE field lines
/// (`event:`, `id:`, `retry:`, comment lines starting with `:`).
fn process_sse_line(line: &str, pending_data: &mut String) {
    if let Some(rest) = line.strip_prefix("data:") {
        // SSE spec: single optional space after field name separator.
        let payload = rest.strip_prefix(' ').unwrap_or(rest);
        if !pending_data.is_empty() {
            pending_data.push('\n');
        }
        pending_data.push_str(payload);
    }
    // All other field names and comment lines are silently ignored.
}

// ---------------------------------------------------------------------------
// JSON → Event mapping
// ---------------------------------------------------------------------------

/// The result of mapping a single SSE `data:` payload.
enum MapResult {
    /// Unknown top-level key — skip this event, continue streaming.
    Skip,
    /// Yield this event and continue streaming.
    Event(Event),
    /// Terminal reached cleanly (Completed / lastChunk=true already emitted):
    /// end the stream without an error item.
    Terminal,
    /// Yield this event, then end cleanly (ArtifactUpdate lastChunk=true).
    TerminalEvent(Event),
    /// Yield this error and end the stream.
    Error(BridgeError),
}

/// Parse a JSON string and apply the A2A → Event mapping rules (Codex 2/3/6/7).
fn parse_and_map(json_str: &str, tx: &watch::Sender<Option<PeerTaskId>>) -> MapResult {
    // Step 1: parse as serde_json::Value.
    let v: serde_json::Value = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(_) => return MapResult::Error(BridgeError::UpstreamA2aError),
    };

    // Step 2: if none of the four known keys are present → skip (tolerant reader).
    let has_known_key = v.get("statusUpdate").is_some()
        || v.get("artifactUpdate").is_some()
        || v.get("task").is_some()
        || v.get("message").is_some();
    if !has_known_key {
        return MapResult::Skip;
    }

    // Step 3: deserialize as StreamResponse and apply the mapping rules.
    let sr: a2a::StreamResponse = match serde_json::from_value(v) {
        Ok(sr) => sr,
        Err(_) => return MapResult::Error(BridgeError::UpstreamA2aError),
    };

    match sr {
        a2a::StreamResponse::StatusUpdate(e) => {
            // Always capture task id into the watch channel.
            let _ = tx.send(Some(PeerTaskId(e.task_id.clone())));

            if e.status.state.is_terminal() {
                if e.status.state == a2a::TaskState::Completed {
                    MapResult::Terminal
                } else {
                    // Failed / Canceled / Rejected → terminal error.
                    MapResult::Error(BridgeError::UpstreamA2aError)
                }
            } else {
                // Non-terminal: emit Status event with message text (or empty string).
                let text = e
                    .status
                    .message
                    .as_ref()
                    .and_then(|m| m.text())
                    .unwrap_or("")
                    .to_string();
                MapResult::Event(Event::status(text))
            }
        }

        a2a::StreamResponse::ArtifactUpdate(e) => {
            let _ = tx.send(Some(PeerTaskId(e.task_id.clone())));

            // Concatenate text from all parts.
            let text: String = e
                .artifact
                .parts
                .iter()
                .filter_map(|p| p.as_text())
                .collect();

            if e.last_chunk == Some(true) {
                // Final chunk → Artifact event, end stream cleanly.
                MapResult::TerminalEvent(Event::artifact(text))
            } else {
                // Intermediate chunk → Status event, continue.
                MapResult::Event(Event::status(text))
            }
        }

        a2a::StreamResponse::Task(t) => {
            // Capture task id; emit no event, continue.
            let _ = tx.send(Some(PeerTaskId(t.id.clone())));
            MapResult::Skip
        }

        a2a::StreamResponse::Message(m) => {
            let text: String = m.parts.iter().filter_map(|p| p.as_text()).collect();
            MapResult::Event(Event::status(text))
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testpeer;
    use bridge_core::error::BridgeError;
    use bridge_core::translator::EventKind;

    // ---------------------------------------------------------------------------
    // Existing Task 3 tests
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn send_streaming_includes_parts_text_and_auth() {
        let (url, peer) = testpeer::MockPeer::start(testpeer::script_status_then_final("R")).await;
        let client = A2aClient::new(&url, "bearer:TESTTOK", std::time::Duration::from_secs(30));
        let _resp = client
            .send_streaming(&[Part {
                text: "PLEASE PONG".into(),
            }])
            .await
            .unwrap();
        let body = peer.last_request_body();
        assert!(
            serde_json::to_string(&body)
                .unwrap()
                .contains("PLEASE PONG"),
            "body: {body}"
        );
        assert_eq!(body["method"], "SendStreamingMessage");
        assert!(peer.last_authorization().unwrap().contains("TESTTOK"));
    }

    #[tokio::test]
    async fn send_streaming_sets_a2a_version_header() {
        let (url, _peer) = testpeer::MockPeer::start(testpeer::script_status_then_final("x")).await;
        let client = A2aClient::new(&url, "bearer:TOK", std::time::Duration::from_secs(30));
        // If the request succeeds, the version header was sent (mock accepts it).
        let resp = client
            .send_streaming(&[Part {
                text: "ping".into(),
            }])
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn bearer_prefix_is_stripped_from_header() {
        let (url, peer) = testpeer::MockPeer::start(testpeer::script_status_then_final("x")).await;
        let client = A2aClient::new(
            &url,
            "bearer:MYTOKEN123",
            std::time::Duration::from_secs(30),
        );
        let _ = client
            .send_streaming(&[Part {
                text: "ping".into(),
            }])
            .await
            .unwrap();
        // reqwest adds "Bearer " prefix when using bearer_auth; the raw header value
        // should be "Bearer MYTOKEN123" (not "Bearer bearer:MYTOKEN123").
        let auth = peer.last_authorization().unwrap();
        assert!(auth.contains("MYTOKEN123"), "auth: {auth}");
        assert!(!auth.contains("bearer:MYTOKEN123"), "auth: {auth}");
    }

    #[tokio::test]
    async fn multiple_parts_all_in_body() {
        let (url, peer) = testpeer::MockPeer::start(testpeer::script_status_then_final("x")).await;
        let client = A2aClient::new(&url, "bearer:TOK", std::time::Duration::from_secs(30));
        let _ = client
            .send_streaming(&[
                Part {
                    text: "part-one".into(),
                },
                Part {
                    text: "part-two".into(),
                },
            ])
            .await
            .unwrap();
        let body_str = serde_json::to_string(&peer.last_request_body()).unwrap();
        assert!(body_str.contains("part-one"), "body: {body_str}");
        assert!(body_str.contains("part-two"), "body: {body_str}");
    }

    // ---------------------------------------------------------------------------
    // Task 4 tests
    // ---------------------------------------------------------------------------

    fn dur() -> std::time::Duration {
        std::time::Duration::from_secs(30)
    }

    fn part(s: &str) -> Part {
        Part { text: s.into() }
    }

    #[tokio::test]
    async fn final_artifact_only_on_lastchunk() {
        let (url, _p) = testpeer::MockPeer::start(
            testpeer::script_status_then_intermediate_then_final("RESULT"),
        )
        .await;
        let (mut ev, peer_rx) = A2aClient::new(&url, "bearer:T", dur())
            .open_stream(&[part("x")])
            .await
            .unwrap();
        let mut kinds = vec![];
        let mut last_text = String::new();
        while let Some(it) = ev.next().await {
            let e = it.unwrap();
            kinds.push(e.kind().clone());
            last_text = e.text().to_string();
        }
        assert!(
            kinds.iter().filter(|k| **k == EventKind::Status).count() >= 1,
            "expected at least one Status event, got: {kinds:?}"
        );
        assert_eq!(
            kinds.last(),
            Some(&EventKind::Artifact),
            "last event must be Artifact, got: {kinds:?}"
        );
        assert!(
            last_text.contains("RESULT"),
            "artifact text must contain RESULT, got: {last_text:?}"
        );
        assert!(peer_rx.borrow().is_some(), "peer task id must be captured");
    }

    #[tokio::test]
    async fn completed_state_without_artifact_ends_clean() {
        let (url, _p) = testpeer::MockPeer::start(testpeer::script_terminal_state_no_artifact(
            "TASK_STATE_COMPLETED",
        ))
        .await;
        let (mut ev, _rx) = A2aClient::new(&url, "bearer:T", dur())
            .open_stream(&[part("x")])
            .await
            .unwrap();
        while let Some(it) = ev.next().await {
            assert!(
                it.is_ok(),
                "no error expected on clean completion, got: {it:?}"
            );
        }
    }

    #[tokio::test]
    async fn terminal_failure_is_error_not_artifact() {
        for st in [
            "TASK_STATE_FAILED",
            "TASK_STATE_CANCELED",
            "TASK_STATE_REJECTED",
        ] {
            let (url, _p) = testpeer::MockPeer::start(testpeer::script_terminal_failure(st)).await;
            let (mut ev, _rx) = A2aClient::new(&url, "bearer:T", dur())
                .open_stream(&[part("x")])
                .await
                .unwrap();
            let mut items = vec![];
            while let Some(it) = ev.next().await {
                items.push(it);
            }
            assert!(
                matches!(items.last(), Some(Err(BridgeError::UpstreamA2aError))),
                "state {st} must be terminal error, got: {items:?}"
            );
        }
    }

    #[tokio::test]
    async fn clean_eof_before_terminal_is_error() {
        let (url, _p) =
            testpeer::MockPeer::start(testpeer::script_status_then_close_no_terminal()).await;
        let (mut ev, _rx) = A2aClient::new(&url, "bearer:T", dur())
            .open_stream(&[part("x")])
            .await
            .unwrap();
        let mut items = vec![];
        while let Some(it) = ev.next().await {
            items.push(it);
        }
        assert!(
            matches!(items.last(), Some(Err(BridgeError::UpstreamA2aError))),
            "clean EOF before terminal must yield error, got: {items:?}"
        );
    }

    #[tokio::test]
    async fn non_2xx_is_error() {
        let (url, _p) = testpeer::MockPeer::start(testpeer::script_500()).await;
        assert!(
            A2aClient::new(&url, "bearer:T", dur())
                .open_stream(&[part("x")])
                .await
                .is_err(),
            "HTTP 500 must return Err from open_stream"
        );
    }
}
