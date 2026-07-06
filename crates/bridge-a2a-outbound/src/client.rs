// client.rs — outbound A2A HTTP client (spec §3.1.2/§7).
//
// `A2aClient` owns the outbound A2A TRANSPORT: URL, JSON-RPC envelope + id,
// the `A2A-Version` header, auth policy, timeout policy, and (via `sse.rs`)
// SSE event framing. Two flavours share one implementation:
//   - `new(url, auth, timeout)` — the peer path (`PeerDelegation`): bearer
//     `Authorization`, a client-wide total timeout.
//   - `loopback(url)`           — the CLI's local `serve` client: NO
//     `Authorization` header and NO total timeout (a total timeout would abort
//     a long-running workflow SSE stream — see G-C3).
//
// The peer entry points `send_streaming` / `open_stream` / `cancel` keep their
// exact signatures + observable behaviour; the CLI uses `send_streaming_with`
// / `rpc` / `subscribe_sse`, which leave frame interpretation to the caller.

use crate::sse::{sse_events, SseStream};
use bridge_core::domain::{Part, PeerTaskId};
use bridge_core::error::BridgeError;
use bridge_core::ports::DelegationStream;
use bridge_core::translator::Event;
use futures::StreamExt;
use serde_json::{Map, Value};
use tokio::sync::watch;

/// Whether `send_streaming_with` mints a fresh `taskId` for the request.
///
/// The CLI's `run-workflow --serve` path MUST mint one: the server's fresh-send
/// fallback otherwise synthesises the constant stub `TaskId::parse("task-1")`,
/// which collides concurrent `--serve` runs in the durable store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TaskIdMode {
    /// Mint a fresh `taskId` (`a2a::new_task_id`).
    Mint,
    /// Leave `taskId` unset.
    #[default]
    None,
}

/// Per-call options for [`A2aClient::send_streaming_with`].
#[derive(Debug, Clone, Default)]
pub struct SendOpts {
    /// `message.contextId`, if the caller supplies one.
    pub context_id: Option<String>,
    /// Whether to mint a fresh `message.taskId`.
    pub task_id: TaskIdMode,
    /// `message.metadata` (e.g. `a2a-bridge.skill` / `a2a-bridge.cwd`).
    pub metadata: Option<Map<String, Value>>,
}

/// The reply from a streaming send/subscribe.
///
/// The server answers a streaming request either with an SSE stream (`Events`)
/// or — when it declines to stream — with a unary JSON-RPC body (`Json`, which
/// may carry a JSON-RPC `error` member).
pub enum StreamingReply {
    Events(SseStream),
    Json(Value),
}

/// Transport-level failure talking to an A2A endpoint.
///
/// Carries the underlying `reqwest`/`serde_json` error so callers can render
/// their exact user-facing strings (e.g. the CLI's "cannot reach serve at
/// {url}…"). It is NOT used for JSON-RPC errors that ride inside a decoded body
/// (those are returned as `Ok(Value)` with an `error` member).
#[derive(Debug)]
pub enum ClientError {
    /// The request could not be sent / the body could not be read.
    Transport(reqwest::Error),
    /// The response body was not valid JSON.
    Decode(serde_json::Error),
    /// A non-success HTTP response that was neither JSON (a JSON-RPC error rides as
    /// `Ok(Json)` regardless of status) nor an SSE stream — i.e. a real transport
    /// failure like a 404/500 text/HTML body. Surfaced instead of being silently
    /// treated as an empty event stream.
    Status {
        status: reqwest::StatusCode,
        body: String,
    },
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Transport(e) => write!(f, "{e}"),
            ClientError::Decode(e) => write!(f, "{e}"),
            ClientError::Status { status, body } => {
                let body = body.trim();
                if body.is_empty() {
                    write!(f, "server returned {status}")
                } else {
                    write!(f, "server returned {status}: {body}")
                }
            }
        }
    }
}

impl std::error::Error for ClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ClientError::Transport(e) => Some(e),
            ClientError::Decode(e) => Some(e),
            ClientError::Status { .. } => None,
        }
    }
}

/// Outbound A2A client. See the module header for the two flavours.
pub struct A2aClient {
    http: reqwest::Client,
    url: String,
    /// Bearer token to send as `Authorization`, or `None` for the loopback
    /// path (which sends no `Authorization` header at all).
    auth: Option<String>,
}

impl A2aClient {
    /// Create a peer client.
    ///
    /// `base_url` — the root URL of the remote A2A peer (e.g. `"http://peer:8080"`).
    /// `auth`     — bearer token in the form `"bearer:TOKEN"` (prefix is stripped).
    /// `timeout`  — client-wide total timeout (spec §3.1.7).
    pub fn new(base_url: &str, auth: &str, timeout: std::time::Duration) -> Self {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("reqwest client build");
        let bearer = auth.strip_prefix("bearer:").unwrap_or(auth).to_string();
        Self {
            http,
            url: normalize_url(base_url),
            auth: Some(bearer),
        }
    }

    /// Create a loopback client for the local `serve` process.
    ///
    /// Sends NO `Authorization` header and sets NO total timeout — a total
    /// timeout spans the response body and would abort a long-running workflow
    /// SSE stream (G-C3).
    pub fn loopback(base_url: &str) -> Self {
        let http = reqwest::Client::builder()
            .build()
            .expect("reqwest client build");
        Self {
            http,
            url: normalize_url(base_url),
            auth: None,
        }
    }

    /// Start a `POST` to the endpoint with the `A2A-Version` header and, on the
    /// peer path, the bearer `Authorization` header. Shared by every request.
    fn base_request(&self) -> reqwest::RequestBuilder {
        let mut req = self
            .http
            .post(&self.url)
            .header(a2a::SVC_PARAM_VERSION, a2a::VERSION);
        if let Some(bearer) = &self.auth {
            req = req.bearer_auth(bearer);
        }
        req
    }

    /// Build the `SendStreamingMessage` JSON-RPC request from `parts` + `opts`
    /// and POST it, returning the raw response.
    async fn post_send_streaming(
        &self,
        parts: &[Part],
        opts: &SendOpts,
    ) -> Result<reqwest::Response, ClientError> {
        let task_id = match opts.task_id {
            TaskIdMode::Mint => Some(a2a::new_task_id()),
            TaskIdMode::None => None,
        };
        let metadata = opts
            .metadata
            .as_ref()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect());

        let msg = a2a::Message {
            message_id: a2a::new_message_id(),
            context_id: opts.context_id.clone(),
            task_id,
            role: a2a::Role::User,
            parts: parts
                .iter()
                .map(|p| a2a::Part::text(p.text.clone()))
                .collect(),
            metadata,
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
            Some(serde_json::to_value(&req).map_err(ClientError::Decode)?),
        );

        self.base_request()
            .json(&rpc)
            .send()
            .await
            .map_err(ClientError::Transport)
    }

    /// POST a `SendStreamingMessage` JSON-RPC request (peer path).
    ///
    /// Mints a fresh `contextId`, no `taskId`, no metadata — identical
    /// observable behaviour to before. Returns the raw response for the SSE
    /// parser in `open_stream`.
    pub async fn send_streaming(&self, parts: &[Part]) -> Result<reqwest::Response, BridgeError> {
        let opts = SendOpts {
            context_id: Some(a2a::new_context_id()),
            task_id: TaskIdMode::None,
            metadata: None,
        };
        self.post_send_streaming(parts, &opts)
            .await
            .map_err(|_| BridgeError::UpstreamA2aError)
    }

    /// POST a `SendStreamingMessage` request with caller-supplied options and
    /// return the decoded reply.
    ///
    /// The client owns message construction, ids, headers, and content-type
    /// discrimination; the caller owns frame interpretation. A JSON body
    /// (server declined to stream) is returned as `Json` regardless of HTTP
    /// status so a JSON-RPC `error` reaches the caller.
    pub async fn send_streaming_with(
        &self,
        parts: &[Part],
        opts: SendOpts,
    ) -> Result<StreamingReply, ClientError> {
        let resp = self.post_send_streaming(parts, &opts).await?;
        self.decode_streaming_reply(resp).await
    }

    /// Generic unary JSON-RPC call (loopback path).
    ///
    /// Returns the parsed JSON body **regardless of HTTP status** — the A2A
    /// server rides JSON-RPC errors for invalid request/params on HTTP 400 with
    /// an `error` member, and callers inspect `v["error"]`. `ClientError` is
    /// only for transport / undecodable-body failures.
    pub async fn rpc(&self, method: &str, params: Value) -> Result<Value, ClientError> {
        let rpc = a2a::JsonRpcRequest::new(a2a::JsonRpcId::Number(1), method, Some(params));
        let resp = self
            .base_request()
            .json(&rpc)
            .send()
            .await
            .map_err(ClientError::Transport)?;
        // Deliberately NOT `error_for_status()` — parse the body on any status.
        let text = resp.text().await.map_err(ClientError::Transport)?;
        serde_json::from_str(&text).map_err(ClientError::Decode)
    }

    /// Subscribe to an SSE stream for a JSON-RPC `method` (e.g.
    /// `SubscribeToTask`), optionally resuming from `last_event_id` via the
    /// `Last-Event-ID` header.
    pub async fn subscribe_sse(
        &self,
        method: &str,
        params: Value,
        last_event_id: Option<String>,
    ) -> Result<StreamingReply, ClientError> {
        let rpc = a2a::JsonRpcRequest::new(a2a::JsonRpcId::Number(1), method, Some(params));
        let mut req = self.base_request().json(&rpc);
        if let Some(cursor) = last_event_id {
            req = req.header("Last-Event-ID", cursor);
        }
        let resp = req.send().await.map_err(ClientError::Transport)?;
        self.decode_streaming_reply(resp).await
    }

    /// Discriminate a streaming response by content type: an `application/json`
    /// body is a unary reply (`Json`); anything else is an SSE stream
    /// (`Events`).
    async fn decode_streaming_reply(
        &self,
        resp: reqwest::Response,
    ) -> Result<StreamingReply, ClientError> {
        if is_application_json(&resp) {
            // JSON body — a unary reply or a JSON-RPC error. Parse regardless of HTTP
            // status (the server rides JSON-RPC errors on HTTP 400).
            let text = resp.text().await.map_err(ClientError::Transport)?;
            let v = serde_json::from_str(&text).map_err(ClientError::Decode)?;
            Ok(StreamingReply::Json(v))
        } else if resp.status().is_success() {
            // 2xx non-JSON → the SSE event stream.
            Ok(StreamingReply::Events(Box::pin(sse_events(resp))))
        } else {
            // Non-success, non-JSON, non-SSE (e.g. a 404/500 text/HTML body): a real
            // HTTP failure. Surface it rather than yielding an empty event stream that
            // callers would mistake for a clean end.
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            Err(ClientError::Status { status, body })
        }
    }

    /// POST a `CancelTask` JSON-RPC request to the peer.
    ///
    /// Builds a `CancelTaskRequest` with the given `peer_task_id`, wraps it in
    /// a JSON-RPC request, and POSTs it to the peer. Non-2xx or transport
    /// errors are mapped to `UpstreamA2aError`.
    pub async fn cancel(&self, peer_task_id: &str) -> Result<(), BridgeError> {
        let params = a2a::CancelTaskRequest {
            id: peer_task_id.to_string(),
            metadata: None,
            tenant: None,
        };

        let rpc = a2a::JsonRpcRequest::new(
            a2a::JsonRpcId::String("cancel-1".into()),
            a2a::methods::CANCEL_TASK,
            Some(serde_json::to_value(&params).map_err(|_| BridgeError::UpstreamA2aError)?),
        );

        let resp = self
            .base_request()
            .json(&rpc)
            .send()
            .await
            .map_err(|_| BridgeError::UpstreamA2aError)?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(BridgeError::UpstreamA2aError)
        }
    }

    /// Open a streaming connection to the peer, parse the SSE body, and return
    /// a `Stream<Result<Event, BridgeError>>` plus a watch receiver for the
    /// peer task id captured from the first response event.
    ///
    /// Mapping rules (spec §3.1.4, Increment 2.6 Task 3):
    /// - Invalid JSON → `Err(UpstreamA2aError)`, end.
    /// - JSON with no known key (`statusUpdate`/`artifactUpdate`/`task`/`message`) → skip.
    /// - `StatusUpdate` terminal-Completed → end cleanly (no item).
    /// - `StatusUpdate` terminal-Failed/Canceled/Rejected → `Err`, end.
    /// - `StatusUpdate` non-terminal → `Event::status(message text or "")`.
    /// - `ArtifactUpdate` (any chunk) → `Event::artifact(text)`, continue (do NOT end on lastChunk).
    /// - `Task` → capture id if present, no event.
    /// - `Message` → `Event::status(text)`.
    /// - Clean EOF before any terminal `StatusUpdate` → `Err(UpstreamA2aError)`.
    pub async fn open_stream(
        &self,
        parts: &[Part],
    ) -> Result<(DelegationStream, watch::Receiver<Option<PeerTaskId>>), BridgeError> {
        let resp = self.send_streaming(parts).await?;

        if !resp.status().is_success() {
            return Err(BridgeError::UpstreamA2aError);
        }

        let (tx, rx) = watch::channel::<Option<PeerTaskId>>(None);
        let events = sse_events(resp);

        let stream = async_stream::stream! {
            let mut events = std::pin::pin!(events);

            // `done` tracks whether a terminal signal was received. It is used
            // only after the loop to decide whether to emit the premature-EOF
            // error.
            let mut done = false;

            while let Some(item) = events.next().await {
                let ev = match item {
                    Ok(ev) => ev,
                    // Network error / invalid UTF-8 mid-stream.
                    Err(_) => {
                        yield Err(BridgeError::UpstreamA2aError);
                        done = true;
                        break;
                    }
                };

                // An empty-data event is never produced by `sse_events`; every
                // `SseEvent` carries a JSON payload to decode.
                match parse_and_map(&ev.data, &tx) {
                    MapResult::Skip => {}
                    MapResult::Event(e) => {
                        yield Ok(e);
                    }
                    MapResult::Terminal => {
                        done = true;
                        break;
                    }
                    MapResult::Error(e) => {
                        yield Err(e);
                        done = true;
                        break;
                    }
                }
            }

            if !done {
                yield Err(BridgeError::UpstreamA2aError);
            }
        };

        Ok((Box::pin(stream), rx))
    }
}

/// Normalise a base URL to a single trailing slash (the A2A server serves the
/// JSON-RPC endpoint at `/`).
fn normalize_url(base_url: &str) -> String {
    base_url.trim_end_matches('/').to_string() + "/"
}

/// Whether a response's `Content-Type` is `application/json`.
fn is_application_json(resp: &reqwest::Response) -> bool {
    resp.headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| {
            ct.split(';')
                .any(|p| p.trim().eq_ignore_ascii_case("application/json"))
        })
        .unwrap_or(false)
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
    /// Terminal `StatusUpdate(Completed)` reached: end the stream without an error item.
    Terminal,
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

            // Every artifact chunk → Artifact event; continue streaming.
            // The stream ends only on a terminal StatusUpdate, not on lastChunk.
            MapResult::Event(Event::artifact(text))
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
    async fn artifacts_emitted_and_stream_ends_on_terminal_status() {
        // New model: every artifactUpdate → Event::artifact (regardless of lastChunk).
        // Stream ends cleanly on the terminal statusUpdate(Completed) with no error item.
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
        // The Working statusUpdate produces at least one Status event.
        assert!(
            kinds.iter().filter(|k| **k == EventKind::Status).count() >= 1,
            "expected at least one Status event, got: {kinds:?}"
        );
        // Both artifact chunks are emitted as Artifact events; the last one contains RESULT.
        assert_eq!(
            kinds.last(),
            Some(&EventKind::Artifact),
            "last event must be Artifact, got: {kinds:?}"
        );
        assert!(
            last_text.contains("RESULT"),
            "last artifact text must contain RESULT, got: {last_text:?}"
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

    // ---------------------------------------------------------------------------
    // Task 3 (new-model) tests: multi-artifact accumulation, terminal StatusUpdate
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn accumulates_multiple_artifacts_and_ends_on_terminal_status() {
        let (url, _p) =
            testpeer::MockPeer::start(testpeer::script_two_artifacts_then_completed("A", "B"))
                .await;
        let (mut ev, _rx) = A2aClient::new(&url, "bearer:T", dur())
            .open_stream(&[part("x")])
            .await
            .unwrap();
        let mut arts = vec![];
        while let Some(it) = ev.next().await {
            let e = it.unwrap();
            if e.kind() == &EventKind::Artifact {
                arts.push(e.text().to_string());
            }
        }
        assert_eq!(arts, vec!["A", "B"]); // BOTH artifacts; stream ended on the terminal Completed status
    }

    #[tokio::test]
    async fn terminal_failed_status_is_error() {
        let (url, _p) =
            testpeer::MockPeer::start(testpeer::script_artifact_then_failed_status()).await;
        let (mut ev, _rx) = A2aClient::new(&url, "bearer:T", dur())
            .open_stream(&[part("x")])
            .await
            .unwrap();
        let mut items = vec![];
        while let Some(it) = ev.next().await {
            items.push(it);
        }
        assert!(matches!(
            items.last(),
            Some(Err(BridgeError::UpstreamA2aError))
        ));
    }

    #[tokio::test]
    async fn artifact_then_clean_eof_without_terminal_is_error() {
        let (url, _p) =
            testpeer::MockPeer::start(testpeer::script_artifact_then_close_no_terminal("A")).await;
        let (mut ev, _rx) = A2aClient::new(&url, "bearer:T", dur())
            .open_stream(&[part("x")])
            .await
            .unwrap();
        let mut items = vec![];
        while let Some(it) = ev.next().await {
            items.push(it);
        }
        assert!(matches!(
            items.last(),
            Some(Err(BridgeError::UpstreamA2aError))
        ));
    }

    // ---------------------------------------------------------------------------
    // New loopback API: rpc() / loopback() / timeout (T-C4, T-C6, T-C8)
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn rpc_sends_version_header_and_loopback_omits_authorization() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "result": { "ok": true }
            })))
            .mount(&server)
            .await;

        let v = A2aClient::loopback(&server.uri())
            .rpc("GetTask", serde_json::json!({ "taskId": "t" }))
            .await
            .unwrap();
        assert_eq!(v["result"]["ok"], true);

        let reqs = server.received_requests().await.unwrap();
        let req = &reqs[0];
        // A2A-Version present.
        assert_eq!(
            req.headers.get("a2a-version").and_then(|h| h.to_str().ok()),
            Some("1.0"),
        );
        // Loopback sends NO Authorization header.
        assert!(
            req.headers.get("authorization").is_none(),
            "loopback must send no Authorization header",
        );
        // Envelope shape preserved.
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["method"], "GetTask");
        assert_eq!(body["params"]["taskId"], "t");
    }

    #[tokio::test]
    async fn rpc_returns_parsed_jsonrpc_error_body_on_http_400() {
        // T-C8: the server rides JSON-RPC errors on HTTP 400. `rpc()` MUST parse
        // and return the body (with its `error` member), NOT a `ClientError`.
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "jsonrpc": "2.0", "id": 1,
                "error": { "code": -32602, "message": "InvalidParams" }
            })))
            .mount(&server)
            .await;

        let v = A2aClient::loopback(&server.uri())
            .rpc("GetTask", serde_json::json!({}))
            .await
            .expect("rpc() must parse the HTTP-400 JSON-RPC error body, not fail with ClientError");
        assert_eq!(v["error"]["message"], "InvalidParams");
        assert_eq!(v["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn streaming_non_2xx_non_json_surfaces_status_error() {
        // F2 (codex review): a non-success response that is neither JSON nor SSE
        // (e.g. a 404 text/plain body) must surface as `ClientError::Status`, NOT be
        // silently decoded as an empty event stream that a caller reads as a clean end.
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(404)
                    .insert_header("content-type", "text/plain")
                    .set_body_string("Not Found"),
            )
            .mount(&server)
            .await;

        let res = A2aClient::loopback(&server.uri())
            .subscribe_sse("SubscribeToTask", serde_json::json!({ "id": "t" }), None)
            .await;
        match res {
            Err(ClientError::Status { status, body }) => {
                assert_eq!(status, reqwest::StatusCode::NOT_FOUND);
                assert!(body.contains("Not Found"), "body carried through: {body:?}");
            }
            Err(e) => panic!("expected ClientError::Status(404), got a different error: {e:?}"),
            Ok(_) => panic!("expected a Status error, got a streaming reply"),
        }
    }

    #[tokio::test]
    async fn loopback_has_no_total_timeout() {
        // T-C6 / G-C3: `loopback` sets NO total timeout. Prove it tolerates a
        // response delay that a short would-be-default timeout would abort.
        use std::time::Duration;
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "jsonrpc": "2.0", "id": 1, "result": {} }))
                    .set_delay(Duration::from_millis(500)),
            )
            .mount(&server)
            .await;

        // Control: a client WITH a short total timeout aborts on the same server.
        let control = A2aClient::new(&server.uri(), "", Duration::from_millis(80));
        assert!(
            control.rpc("GetTask", serde_json::json!({})).await.is_err(),
            "a short total-timeout client must abort on the delayed server",
        );

        // loopback: no total timeout → the delayed response is read successfully.
        let v = A2aClient::loopback(&server.uri())
            .rpc("GetTask", serde_json::json!({}))
            .await
            .expect("loopback must NOT time out on a slow response");
        assert!(v.get("result").is_some());
    }
}
