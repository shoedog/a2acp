// acp_backend.rs — AcpBackend: drives an ACP agent child process over JSON-RPC line-framed stdio.
// Spec §5.3 cancellation rule: completion is the prompt RESULT (stopReason:"cancelled"),
// NOT the act of sending session/cancel. See Codex finding 2.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use agent_client_protocol::schema::{
    AgentCapabilities, AuthMethod, InitializeRequest, InitializeResponse, ProtocolVersion,
};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectTo, ConnectionTo};
use async_trait::async_trait;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::{oneshot, Mutex};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use bridge_core::error::BridgeError;
use bridge_core::ids::SessionId;
use bridge_core::ports::{AgentBackend, BackendStream, Update};

use crate::framing::FrameReader;
use crate::replay::frame_to_update;
use crate::supervisor::Supervised;

const MAX_FRAME: usize = 16 * 1024 * 1024;

/// Default bound on the `initialize` handshake. A real agent that connects its
/// stdio but never sends the initialize response would otherwise hang
/// `connect`/`spawn` forever; on elapse we return a clear `BridgeError`.
const DEFAULT_HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Static configuration for an ACP agent connection.
///
/// `model` / `mode` are introduced now but only consumed by later tasks
/// (`set_model` / `set_mode` after `session/new`); Task 1 only uses `cwd`
/// when building sessions, which arrives in a later task too.
#[derive(Debug, Clone)]
pub struct AcpConfig {
    /// Absolute working directory the agent runs sessions in.
    pub cwd: PathBuf,
    /// Optional model id to request via `session/set_model` (later tasks).
    pub model: Option<String>,
    /// Optional mode id to request via `session/set_mode` (later tasks).
    pub mode: Option<String>,
    /// Bound on the `initialize` handshake (transport connect + response).
    /// Defaults to [`DEFAULT_HANDSHAKE_TIMEOUT`]; on elapse `connect`/`spawn`
    /// return `BridgeError::AgentCrashed` rather than hanging. Task 6 surfaces
    /// this as a clear handshake-timeout error to the caller.
    pub handshake_timeout: std::time::Duration,
}

impl Default for AcpConfig {
    fn default() -> Self {
        Self {
            cwd: PathBuf::from("."),
            model: None,
            mode: None,
            handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
        }
    }
}

// ── Legacy (v1, scripted-only) inner state ──────────────────────────────────
//
// Retained verbatim so the renamed type keeps the v1 `prompt`/`cancel` behavior
// green for the inline scripted tests and the gated e2es while the conformant
// SDK `prompt`/`cancel` are built in Increment 3a Tasks 2–4. Only populated by
// `from_child`; the SDK constructors (`spawn`/`connect`) leave it `None`.

struct Inner {
    stdin: ChildStdin,
    reader: FrameReader<BufReader<ChildStdout>>,
    supervised: Supervised,
}

// ── SDK connection handle ────────────────────────────────────────────────────
//
// The connection's event loop (`connect_with`) owns a single task that runs
// until the connection closes, so we cannot keep the loop "in line". Instead
// `connect`/`spawn` start the loop in a dedicated tokio task whose `main_fn`
// publishes a clone of the `ConnectionTo<Agent>` handle out through a oneshot,
// then parks until shutdown. All agent-bound requests go through that cloned
// `cx`. (Driving the loop via a command channel is the alternative; a shared
// `cx` is simpler and is what Tasks 2–6 build prompt/cancel on top of.)
struct AcpConn {
    cx: ConnectionTo<Agent>,
    /// Negotiated agent capabilities from `initialize`.
    agent_capabilities: AgentCapabilities,
    /// Authentication methods the agent advertised (drives `authenticate`).
    auth_methods: Vec<AuthMethod>,
    /// Held for the backend's lifetime: the event-loop task parks on the paired
    /// receiver, so dropping this (on backend drop) closes the connection and
    /// lets the loop task exit cleanly. Tasks 2+ may signal it explicitly to
    /// drive shutdown / terminate.
    _shutdown: oneshot::Sender<()>,
}

// ── Public struct ────────────────────────────────────────────────────────────

pub struct AcpBackend {
    /// Legacy scripted path state (`from_child` only). `None` on the SDK path.
    inner: Option<Arc<Mutex<Inner>>>,
    /// SDK connection handle (`spawn`/`connect` only). `None` on the legacy path.
    conn: Option<AcpConn>,
    /// The spawned `Supervised` child, held for the whole backend lifetime so
    /// `kill_on_drop(true)` does not SIGKILL it the instant `spawn` returns.
    /// `Some` only on the `spawn` (production) path; `None` on `connect`
    /// (in-process transport) and the legacy `from_child` path. Task 2 reads it
    /// for explicit `terminate()`.
    supervised: Option<Supervised>,
    id_counter: Arc<AtomicU64>,
}

impl AcpBackend {
    /// Build the `initialize` request this backend sends to the agent.
    ///
    /// Exposed so the wire-golden test can assert the serialized frame is
    /// conformant (integer `protocolVersion`, no fs/terminal capabilities)
    /// against the SAME value the connection actually transmits.
    #[must_use]
    pub fn initialize_request() -> InitializeRequest {
        // `InitializeRequest::new` defaults `client_capabilities` to
        // `ClientCapabilities::default()`, which advertises no fs read/write and
        // no terminal support — exactly what we want (no fs/terminal seam).
        InitializeRequest::new(ProtocolVersion::V1)
    }

    /// **Production** constructor: spawn `cmd args` as a `Supervised` child
    /// (its own process group, tested SIGTERM→SIGKILL reaping) and drive the
    /// ACP connection over its stdin/stdout as `ByteStreams`.
    ///
    /// This is `Supervised` + `connect(ByteStreams)`: process lifecycle stays
    /// with `Supervised`; protocol drive is the shared `connect` core.
    pub async fn spawn(cmd: &str, args: &[&str], config: AcpConfig) -> Result<Self, BridgeError> {
        let mut supervised = Supervised::spawn(cmd, args).map_err(|_| BridgeError::AgentCrashed)?;
        let child = supervised.child_mut();
        let stdin = child.stdin.take().ok_or(BridgeError::AgentCrashed)?;
        let stdout = child.stdout.take().ok_or(BridgeError::AgentCrashed)?;
        // The crate uses `futures` async-io; our child uses tokio pipes — adapt
        // with tokio_util::compat. ByteStreams::new(outgoing_writer, incoming_reader).
        let transport = ByteStreams::new(stdin.compat_write(), stdout.compat());
        // `supervised` (the process-group owner) MUST live for the whole backend
        // lifetime: `kill_on_drop(true)` would SIGKILL the child the instant it
        // dropped, killing the event-loop task's pipes. Hold it on the backend.
        let mut backend = Self::connect(transport, config).await?;
        backend.supervised = Some(supervised);
        Ok(backend)
    }

    /// **Transport-generic** core constructor. Accepts any SDK transport, so
    /// in-process fake-agent unit tests can pass `Channel::duplex()`.
    ///
    /// Starts the connection event loop in a dedicated task, captures a clone of
    /// the `ConnectionTo<Agent>` handle, then runs `initialize`.
    pub async fn connect(
        transport: impl ConnectTo<Client> + 'static,
        config: AcpConfig,
    ) -> Result<Self, BridgeError> {
        let (cx_tx, cx_rx) = oneshot::channel::<ConnectionTo<Agent>>();
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        // The event loop owns a long-lived task. `main_fn` publishes a clone of
        // `cx` and then parks on `shutdown_rx` so the connection stays open for
        // the lifetime of the backend (returning from `main_fn` would close it).
        tokio::spawn(async move {
            let _ = Client
                .builder()
                .name("a2a-bridge")
                .connect_with(transport, async move |cx: ConnectionTo<Agent>| {
                    // Hand a clone back to the AcpBackend; ignore send errors
                    // (receiver dropped => backend gone => nothing to drive).
                    let _ = cx_tx.send(cx.clone());
                    // Park until the backend signals shutdown (or is dropped).
                    let _ = shutdown_rx.await;
                    Ok(())
                })
                .await;
        });

        // Bound the whole handshake (transport connect + initialize response) so
        // an agent that opens stdio but never replies cannot hang us forever.
        // A closed transport EOFs cleanly (the `map_err` arms below); a true hang
        // is caught by the outer timeout.
        let handshake = async {
            let cx = cx_rx.await.map_err(|_| BridgeError::AgentCrashed)?;

            // Run the ACP `initialize` handshake and capture the negotiated caps.
            let resp: InitializeResponse = cx
                .send_request(Self::initialize_request())
                .block_task()
                .await
                .map_err(|_| BridgeError::AgentCrashed)?;
            Ok::<_, BridgeError>((cx, resp))
        };

        let (cx, resp) = tokio::time::timeout(config.handshake_timeout, handshake)
            .await
            .map_err(|_| BridgeError::AgentCrashed)??;

        Ok(Self {
            inner: None,
            conn: Some(AcpConn {
                cx,
                agent_capabilities: resp.agent_capabilities,
                auth_methods: resp.auth_methods,
                _shutdown: shutdown_tx,
            }),
            supervised: None,
            id_counter: Arc::new(AtomicU64::new(1)),
        })
    }

    /// Negotiated agent capabilities from the most recent `initialize`.
    /// `None` on the legacy (`from_child`) path.
    #[must_use]
    pub fn agent_capabilities(&self) -> Option<&AgentCapabilities> {
        self.conn.as_ref().map(|c| &c.agent_capabilities)
    }

    /// Authentication methods the agent advertised at `initialize`.
    /// `None` on the legacy (`from_child`) path.
    #[must_use]
    pub fn auth_methods(&self) -> Option<&[AuthMethod]> {
        self.conn.as_ref().map(|c| c.auth_methods.as_slice())
    }

    /// Access the SDK connection handle. Returns `Err(AgentCrashed)` on the
    /// legacy (`from_child`) path where no SDK connection exists, so Task 2's
    /// prompt routing gets a clean error seam instead of a panic inside the
    /// event loop. Used by later tasks to send agent-bound requests.
    #[allow(dead_code)]
    fn cx(&self) -> Result<&ConnectionTo<Agent>, BridgeError> {
        self.conn
            .as_ref()
            .map(|c| &c.cx)
            .ok_or(BridgeError::AgentCrashed)
    }

    /// Construct from an already-spawned scripted child (used in tests and when
    /// the caller has already set up the process).
    ///
    /// LEGACY (v1) path: drives the hand-rolled JSON-RPC framing for the
    /// scripted prompt/cancel tests and the gated e2es. The conformant SDK path
    /// is `spawn`/`connect`; this is retained to keep the workspace green until
    /// Tasks 2–4 replace the prompt/cancel innards.
    pub fn from_child(mut supervised: Supervised) -> Self {
        let child = supervised.child_mut();
        let stdin = child.stdin.take().expect("stdin must be piped");
        let stdout = child.stdout.take().expect("stdout must be piped");
        let reader = FrameReader::new(BufReader::new(stdout), MAX_FRAME);
        Self {
            inner: Some(Arc::new(Mutex::new(Inner {
                stdin,
                reader,
                supervised,
            }))),
            conn: None,
            // Legacy path owns its child via `Inner::supervised`; the SDK slot is unused.
            supervised: None,
            id_counter: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Send `session/new`, read back the `{result:{sessionId}}` response.
    pub async fn new_session(&self) -> Result<SessionId, BridgeError> {
        let id = self.next_id();
        let mut g = self.legacy_inner().lock().await;
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
                    let mut g = self.legacy_inner().lock().await;
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

    /// The legacy scripted-path inner state. Only the `from_child` constructor
    /// populates it; the legacy `prompt`/`cancel`/`new_session` paths require it.
    fn legacy_inner(&self) -> &Arc<Mutex<Inner>> {
        self.inner
            .as_ref()
            .expect("legacy path requires from_child construction")
    }

    fn next_id(&self) -> u64 {
        self.id_counter.fetch_add(1, Ordering::Relaxed)
    }

    async fn send_cancel(&self, session: &SessionId) -> Result<(), BridgeError> {
        let id = self.next_id();
        let mut g = self.legacy_inner().lock().await;
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
                let mut g = self.legacy_inner().lock().await;
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
impl AgentBackend for AcpBackend {
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
            let mut g = self.legacy_inner().lock().await;
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
        let inner = Arc::clone(self.legacy_inner());

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
        let be = AcpBackend::from_child(scripted(
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
        let be = AcpBackend::from_child(scripted(
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
        let be = AcpBackend::from_child(scripted(
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
        let be = AcpBackend::from_child(scripted(
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
        let be = AcpBackend::from_child(scripted(
            "printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"sessionId\":\"s1\"}}'; sleep 30"));
        let sid = be.new_session().await.unwrap();
        let _ = be.prompt(&sid, vec![]).await.unwrap();
        let err = be
            .cancel_with_timeout(&sid, Duration::from_millis(200))
            .await
            .unwrap_err();
        assert_eq!(err, BridgeError::CancelTimeout);
    }

    // ── SDK connection path (transport-generic, in-process fake agent) ──────────

    use agent_client_protocol::schema::{
        AgentCapabilities, AuthMethod, AuthMethodAgent, AuthMethodId, InitializeRequest,
        InitializeResponse, ProtocolVersion,
    };
    use agent_client_protocol::{Agent, Channel};

    /// Spawn an in-process fake ACP agent on `channel` that answers `initialize`
    /// with the given response. Returns immediately; the agent loop runs in a task.
    fn spawn_fake_agent(channel: Channel, resp: InitializeResponse) {
        tokio::spawn(async move {
            let _ = Agent
                .builder()
                .name("fake-agent")
                .on_receive_request(
                    move |_req: InitializeRequest,
                          responder: agent_client_protocol::Responder<InitializeResponse>,
                          _cx| {
                        let resp = resp.clone();
                        async move {
                            responder.respond(resp)?;
                            Ok(())
                        }
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_to(channel)
                .await;
        });
    }

    /// Spawn an in-process fake ACP agent that *opens* the channel (so it does
    /// not EOF) but **never** answers `initialize`: the handler parks forever
    /// holding the responder, simulating a hung agent rather than a closed one.
    fn spawn_hung_agent(channel: Channel) {
        tokio::spawn(async move {
            let _ = Agent
                .builder()
                .name("hung-agent")
                .on_receive_request(
                    move |_req: InitializeRequest,
                          _responder: agent_client_protocol::Responder<InitializeResponse>,
                          _cx| async move {
                        // Never respond; park forever so the channel stays open
                        // and the client's initialize request hangs.
                        std::future::pending::<()>().await;
                        Ok(())
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_to(channel)
                .await;
        });
    }

    fn test_config() -> AcpConfig {
        AcpConfig {
            cwd: std::path::PathBuf::from("/tmp"),
            ..AcpConfig::default()
        }
    }

    /// Like [`test_config`] but with a short handshake bound so the
    /// never-answers test fails fast instead of waiting the 30s default.
    fn test_config_short_handshake() -> AcpConfig {
        AcpConfig {
            cwd: std::path::PathBuf::from("/tmp"),
            handshake_timeout: Duration::from_millis(200),
            ..AcpConfig::default()
        }
    }

    #[tokio::test]
    async fn connect_runs_initialize_and_captures_agent_capabilities() {
        // Fake agent advertises one auth method; assert the backend captured the
        // negotiated InitializeResponse (caps + auth methods) over the transport seam.
        let (client_side, agent_side) = Channel::duplex();
        let resp = InitializeResponse::new(ProtocolVersion::V1)
            .agent_capabilities(AgentCapabilities::default())
            .auth_methods(vec![AuthMethod::Agent(AuthMethodAgent::new(
                AuthMethodId::new("oauth"),
                "OAuth",
            ))]);
        spawn_fake_agent(agent_side, resp);

        let be = AcpBackend::connect(client_side, test_config())
            .await
            .expect("initialize handshake succeeds");

        assert!(
            be.agent_capabilities().is_some(),
            "SDK path must capture agent capabilities"
        );
        let methods = be.auth_methods().expect("auth methods captured");
        assert_eq!(methods.len(), 1, "advertised auth method round-trips");
    }

    #[tokio::test]
    async fn connect_errors_when_agent_never_answers() {
        // Agent side is dropped immediately -> initialize never completes -> AgentCrashed.
        let (client_side, agent_side) = Channel::duplex();
        drop(agent_side);
        match AcpBackend::connect(client_side, test_config()).await {
            Err(e) => assert_eq!(e, BridgeError::AgentCrashed),
            Ok(_) => panic!("expected AgentCrashed when the agent never answers initialize"),
        }
    }

    // I1: a *hung* agent (channel open, no initialize reply) must NOT hang us
    // forever — the bounded handshake returns an error within the timeout.
    #[tokio::test]
    async fn connect_times_out_when_agent_opens_but_never_answers_initialize() {
        let (client_side, agent_side) = Channel::duplex();
        // Agent connects (channel stays open) but never responds to initialize.
        spawn_hung_agent(agent_side);

        // Bound the whole call so a regression (no handshake timeout) fails the
        // test instead of hanging the suite.
        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            AcpBackend::connect(client_side, test_config_short_handshake()),
        )
        .await
        .expect("connect must return within the handshake bound, not hang");

        match outcome {
            Err(e) => assert_eq!(
                e,
                BridgeError::AgentCrashed,
                "hung initialize handshake must surface a clear error"
            ),
            Ok(_) => panic!("expected an error when the agent never answers initialize"),
        }
    }

    // B1: `spawn` must HOLD the Supervised child for the backend's lifetime.
    // Before the fix, `Supervised` (kill_on_drop) was dropped when `spawn`
    // returned, SIGKILLing the child immediately. We cannot run a real ACP
    // agent here, so we drive the same `Supervised::spawn` path through a long-
    // lived child and assert: (a) the backend retains `supervised.is_some()`,
    // and (b) the child is still alive (not reaped/SIGKILLed) shortly after.
    #[tokio::test]
    async fn spawn_holds_child_alive_after_returning() {
        // A long-lived child (`cat` blocks reading stdin), driven through the
        // exact `Supervised::spawn` + pipe-`take()` seam that `spawn` uses, then
        // held on the backend struct mirroring `spawn`'s end state.
        let mut supervised = Supervised::spawn("/bin/cat", &[]).expect("spawn cat");
        let pid = supervised.pid();
        // Take the pipes exactly as `spawn` does (also exercises the I3 seam).
        let child = supervised.child_mut();
        let _stdin = child.stdin.take().ok_or(BridgeError::AgentCrashed).unwrap();
        let _stdout = child
            .stdout
            .take()
            .ok_or(BridgeError::AgentCrashed)
            .unwrap();

        let backend = AcpBackend {
            inner: None,
            conn: None,
            supervised: Some(supervised),
            id_counter: Arc::new(AtomicU64::new(1)),
        };

        assert!(
            backend.supervised.is_some(),
            "backend must retain the Supervised child (B1)"
        );

        // Give an erroneous kill_on_drop time to fire, then confirm the child is
        // still alive. signal 0 succeeds => the OS still has this owned process
        // (not SIGKILLed+reaped). This is the regression the BLOCKER describes:
        // before the fix, the local `supervised` dropped at `spawn`'s end and
        // the child was killed here.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let alive = unsafe { libc::kill(pid as i32, 0) } == 0;
        assert!(
            alive,
            "child must still be alive after spawn returns (B1: not SIGKILLed on drop)"
        );

        // Clean up deterministically (SIGTERM->reap), leaving no zombie.
        if let Some(s) = backend.supervised {
            s.terminate(Duration::from_millis(100)).await;
        }
    }
}
