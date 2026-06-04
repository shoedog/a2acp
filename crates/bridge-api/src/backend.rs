//! ApiBackend — the non-process OpenAI-compatible AgentBackend.
use crate::config::ApiConfig;
use crate::wire::{ChatRequest, Message, SseAccumulator, ToolCall};
use bridge_core::domain::{
    Part, PermissionDecision, PermissionRequest, SessionContext, SessionSpec,
};
use bridge_core::error::BridgeError;
use bridge_core::ids::SessionId;
use bridge_core::ports::{
    AgentBackend, BackendStream, PolicyEngine, Update, STOP_REASON_CANCELLED,
};
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::watch;

/// Map a non-success upstream HTTP status to the RIGHT `BridgeError` variant so
/// the disposition is correct: `429` → `AgentOverloaded`, `401`/`403` →
/// `AgentNotAuthenticated` (→ `SetState(AuthRequired)` — "fix credentials"),
/// everything else → `AgentCrashed{reason}` (→ `Failed`). The status string never
/// embeds the URL/body, so the wire-leak guard is preserved.
fn map_http_error(status: reqwest::StatusCode) -> BridgeError {
    match status {
        reqwest::StatusCode::TOO_MANY_REQUESTS => BridgeError::AgentOverloaded,
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
            BridgeError::AgentNotAuthenticated
        }
        _ => BridgeError::agent_crashed(format!("upstream API returned error status: {status}")),
    }
}

/// Per-session state: the stashed effective model + a `watch` channel used as the
/// cancel signal. A `watch` (level-triggered, version-counted) lets the turn loop
/// `select!` on cancellation even while parked awaiting the next SSE chunk — an
/// `AtomicBool` polled only between chunks cannot cancel during a stall.
struct SessionState {
    model: Option<String>,
    cancel: watch::Sender<bool>,
}
impl Default for SessionState {
    fn default() -> Self {
        Self {
            model: None,
            cancel: watch::channel(false).0,
        }
    }
}

pub struct ApiBackend {
    cfg: ApiConfig,
    client: reqwest::Client,
    policy: Arc<StdMutex<Arc<dyn PolicyEngine>>>,
    sessions: Arc<StdMutex<HashMap<SessionId, SessionState>>>,
}

/// Default policy: approve everything (mirrors AcpBackend's default auto-approver).
struct AutoApprove;
impl PolicyEngine for AutoApprove {
    fn decide(
        &self,
        _: &PermissionRequest,
        _: &SessionContext,
    ) -> Result<PermissionDecision, BridgeError> {
        Ok(PermissionDecision::Approve)
    }
}

impl ApiBackend {
    pub fn new(cfg: ApiConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(cfg.request_timeout)
            .build()
            .expect("reqwest client builds");
        Self {
            cfg,
            client,
            policy: Arc::new(StdMutex::new(Arc::new(AutoApprove) as Arc<dyn PolicyEngine>)),
            sessions: Arc::new(StdMutex::new(HashMap::new())),
        }
    }

    #[must_use]
    pub fn with_policy(self, policy: Arc<dyn PolicyEngine>) -> Self {
        if let Ok(mut p) = self.policy.lock() {
            *p = policy;
        }
        self
    }

    /// Test/inspection helper: the stashed effective model for a session.
    pub fn session_model(&self, s: &SessionId) -> Option<String> {
        self.sessions
            .lock()
            .ok()?
            .get(s)
            .and_then(|st| st.model.clone())
    }

    /// The session's cancel sender (creating the slot if absent).
    fn session_cancel(&self, s: &SessionId) -> watch::Sender<bool> {
        let mut map = self.sessions.lock().expect("sessions lock");
        map.entry(s.clone()).or_default().cancel.clone()
    }

    fn resolve_api_key(&self) -> Option<String> {
        self.cfg
            .api_key_env
            .as_ref()
            .and_then(|var| std::env::var(var).ok())
    }
    fn resolve_model(&self, s: &SessionId) -> Option<String> {
        self.session_model(s).or_else(|| self.cfg.model.clone())
    }
}

#[async_trait::async_trait]
impl AgentBackend for ApiBackend {
    async fn prompt(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
    ) -> Result<BackendStream, BridgeError> {
        let url = format!(
            "{}/chat/completions",
            self.cfg.base_url.trim_end_matches('/')
        );
        let model = self.resolve_model(session);
        let api_key = self.resolve_api_key();
        let do_stream = self.cfg.stream;
        let client = self.client.clone();
        let policy = self.policy.clone();
        let max_rounds = self.cfg.max_tool_rounds;

        // Cancel: reset for this fresh turn, THEN subscribe so a later send(true)
        // is observed as a change. `select!` on `changed()` fires even while parked
        // awaiting the next SSE chunk.
        let cancel_tx = self.session_cancel(session);
        let _ = cancel_tx.send(false);
        let mut cancel_rx = cancel_tx.subscribe();

        let mut messages: Vec<Message> = vec![Message::user(
            parts
                .iter()
                .map(|p| p.text.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        )];

        let stream = async_stream::try_stream! {
            for _round in 0..max_rounds {
                if *cancel_rx.borrow() {
                    yield Update::Done { stop_reason: STOP_REASON_CANCELLED.into() }; return;
                }
                let req = ChatRequest { model: model.clone(), messages: messages.clone(),
                    tools: vec![crate::tool::tool_def()], stream: do_stream };
                let mut builder = client.post(&url).json(&req);
                if let Some(k) = &api_key { builder = builder.bearer_auth(k); }
                let resp = builder.send().await.map_err(|e| BridgeError::agent_crashed(format!("HTTP request to upstream API failed: {e}")))?;
                if !resp.status().is_success() { Err(map_http_error(resp.status()))?; }

                let parsed = if do_stream {
                    let mut acc = SseAccumulator::default();
                    let mut bytes = resp.bytes_stream();
                    let mut buf = String::new();
                    'read: loop {
                        let chunk = tokio::select! {
                            biased;
                            changed = cancel_rx.changed() => {
                                if changed.is_ok() && *cancel_rx.borrow() {
                                    yield Update::Done { stop_reason: STOP_REASON_CANCELLED.into() }; return;
                                }
                                continue 'read;
                            }
                            maybe = bytes.next() => match maybe { Some(c) => c, None => break 'read },
                        };
                        let chunk = chunk.map_err(|e| BridgeError::agent_crashed(format!("error reading SSE chunk from upstream API: {e}")))?;
                        buf.push_str(&String::from_utf8_lossy(&chunk));
                        while let Some(nl) = buf.find('\n') {
                            let line: String = buf.drain(..=nl).collect();
                            match acc.push_sse_line(&line) {
                                Ok(Some(text)) => { yield Update::Text(text); }
                                Ok(None) => {}
                                Err(_) => { Err(BridgeError::FrameError)?; }
                            }
                            if acc.is_done() { break 'read; }
                        }
                    }
                    // Flush a trailing line that arrived without a newline at EOF — but
                    // ONLY if no terminal was seen (otherwise `buf` is post-[DONE] noise,
                    // and a chunk-split partial "[DON" would falsely FrameError).
                    if !acc.is_done() && !buf.trim().is_empty() {
                        match acc.push_sse_line(&buf) {
                            Ok(Some(text)) => { yield Update::Text(text); }
                            Ok(None) => {}
                            Err(_) => { Err(BridgeError::FrameError)?; }
                        }
                    }
                    acc.finish()
                } else {
                    let body = resp.text().await.map_err(|e| BridgeError::agent_crashed(format!("failed to read non-streaming response body from upstream API: {e}")))?;
                    let p = crate::wire::parse_nonstream(&body).map_err(|_| BridgeError::FrameError)?;
                    if !p.text.is_empty() { yield Update::Text(p.text.clone()); }
                    p
                };
                if parsed.tool_calls.is_empty() {
                    yield Update::Done { stop_reason: "stop".into() }; return;
                }
                // Tool round: decide each call SILENTLY via the injected policy.
                // NO Update::Permission is yielded — the backend is the sole authority.
                messages.push(Message::assistant_tool_calls(parsed.tool_calls.clone()));
                for tc in &parsed.tool_calls {
                    let result = decide_tool(&policy, tc);
                    messages.push(Message::tool_result(tc.id.clone(), result));
                }
                // continue → re-POST with the appended tool results.
            }
            yield Update::Done { stop_reason: "max_tool_rounds".into() };
        };
        Ok(Box::pin(stream))
    }

    async fn cancel(&self, session: &SessionId) -> Result<(), BridgeError> {
        // Only signal an EXISTING slot — never mint a new one (a forgotten session
        // has no in-flight turn to cancel; minting a fresh channel here would lose
        // the signal vs the running turn's receiver).
        if let Ok(map) = self.sessions.lock() {
            if let Some(st) = map.get(session) {
                let _ = st.cancel.send(true);
            }
        }
        Ok(())
    }

    async fn configure_session(
        &self,
        session: &SessionId,
        spec: &SessionSpec,
    ) -> Result<(), BridgeError> {
        let mut map = self.sessions.lock().expect("sessions lock");
        map.entry(session.clone()).or_default().model = spec.config.model.clone();
        Ok(())
    }

    async fn forget_session(&self, session: &SessionId) {
        if let Ok(mut map) = self.sessions.lock() {
            map.remove(session);
        }
    }
}

/// Silent permission decision for one tool call → the `content` of its tool-result
/// message. Approve runs the stub tool; Deny/abstain feed a refusal string.
fn decide_tool(policy: &Arc<StdMutex<Arc<dyn PolicyEngine>>>, tc: &ToolCall) -> String {
    let req = PermissionRequest::with_id(tc.id.clone(), /*interactive=*/ false);
    let decision = policy.lock().ok().map(|p| p.decide(&req, &SessionContext));
    match decision {
        Some(Ok(PermissionDecision::Approve)) => {
            if tc.function.name == crate::tool::TOOL_NAME { crate::tool::run_tool() }
            else { format!("unknown tool: {}", tc.function.name) }
        }
        Some(Err(BridgeError::PermissionDenied)) => "permission denied: tool not executed".into(),
        _ /* abstain / poisoned */ => "permission unavailable: tool not executed".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::{
        EffectiveConfig, PermissionDecision, PermissionRequest, SessionContext, SessionSpec,
    };
    use bridge_core::error::BridgeError;
    use bridge_core::ids::SessionId;
    use bridge_core::ports::{AgentBackend, PolicyEngine};
    use std::sync::Arc;

    struct DenyAll;
    impl PolicyEngine for DenyAll {
        fn decide(
            &self,
            _: &PermissionRequest,
            _: &SessionContext,
        ) -> Result<PermissionDecision, BridgeError> {
            Err(BridgeError::PermissionDenied)
        }
    }

    #[tokio::test]
    async fn configure_session_stashes_model_and_object_safe() {
        let be = ApiBackend::new(crate::config::ApiConfig::new("http://127.0.0.1:1"));
        let s = SessionId::parse("s1").unwrap();
        be.configure_session(
            &s,
            &SessionSpec::from_config(EffectiveConfig {
                model: Some("haiku".into()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
        assert_eq!(be.session_model(&s).as_deref(), Some("haiku"));
        be.forget_session(&s).await;
        assert!(be.session_model(&s).is_none());
        let _obj: Arc<dyn AgentBackend> = Arc::new(ApiBackend::new(crate::config::ApiConfig::new(
            "http://127.0.0.1:1",
        )));
    }

    #[tokio::test]
    async fn with_policy_swaps_engine() {
        let be = ApiBackend::new(crate::config::ApiConfig::new("http://127.0.0.1:1"))
            .with_policy(Arc::new(DenyAll));
        fn assert_send_sync<T: Send + Sync>(_: &T) {}
        assert_send_sync(&be);
    }
}
