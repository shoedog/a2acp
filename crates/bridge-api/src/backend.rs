//! ApiBackend — the non-process OpenAI-compatible AgentBackend.
use crate::config::ApiConfig;
use crate::wire::{ChatRequest, Message, SseAccumulator};
use bridge_core::domain::{EffectiveConfig, Part, PermissionDecision, PermissionRequest, SessionContext};
use bridge_core::error::BridgeError;
use bridge_core::ids::SessionId;
use bridge_core::ports::{AgentBackend, BackendStream, PolicyEngine, Update, STOP_REASON_CANCELLED};
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::watch;

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
        Self { model: None, cancel: watch::channel(false).0 }
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
    fn decide(&self, _: &PermissionRequest, _: &SessionContext) -> Result<PermissionDecision, BridgeError> {
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
        if let Ok(mut p) = self.policy.lock() { *p = policy; }
        self
    }

    /// Test/inspection helper: the stashed effective model for a session.
    pub fn session_model(&self, s: &SessionId) -> Option<String> {
        self.sessions.lock().ok()?.get(s).and_then(|st| st.model.clone())
    }

    /// The session's cancel sender (creating the slot if absent).
    fn session_cancel(&self, s: &SessionId) -> watch::Sender<bool> {
        let mut map = self.sessions.lock().expect("sessions lock");
        map.entry(s.clone()).or_default().cancel.clone()
    }

    fn resolve_api_key(&self) -> Option<String> {
        self.cfg.api_key_env.as_ref().and_then(|var| std::env::var(var).ok())
    }
    fn resolve_model(&self, s: &SessionId) -> Option<String> {
        self.session_model(s).or_else(|| self.cfg.model.clone())
    }
}

#[async_trait::async_trait]
impl AgentBackend for ApiBackend {
    async fn prompt(&self, session: &SessionId, parts: Vec<Part>) -> Result<BackendStream, BridgeError> {
        let url = format!("{}/chat/completions", self.cfg.base_url.trim_end_matches('/'));
        let model = self.resolve_model(session);
        let api_key = self.resolve_api_key();
        let do_stream = self.cfg.stream;
        let client = self.client.clone();

        // Cancel: reset for this fresh turn, THEN subscribe so a later send(true)
        // is observed as a change. `select!` on `changed()` fires even while parked
        // awaiting the next SSE chunk.
        let cancel_tx = self.session_cancel(session);
        let _ = cancel_tx.send(false);
        let mut cancel_rx = cancel_tx.subscribe();

        let messages: Vec<Message> = vec![Message::user(
            parts.iter().map(|p| p.text.as_str()).collect::<Vec<_>>().join("\n"),
        )];

        let stream = async_stream::try_stream! {
            if *cancel_rx.borrow() {
                yield Update::Done { stop_reason: STOP_REASON_CANCELLED.into() }; return;
            }
            let req = ChatRequest { model: model.clone(), messages: messages.clone(),
                tools: vec![crate::tool::tool_def()], stream: do_stream };
            let mut builder = client.post(&url).json(&req);
            if let Some(k) = &api_key { builder = builder.bearer_auth(k); }
            let resp = builder.send().await.map_err(|_| BridgeError::AgentCrashed)?;
            if !resp.status().is_success() { Err(BridgeError::AgentCrashed)?; }

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
                let chunk = chunk.map_err(|_| BridgeError::AgentCrashed)?;
                buf.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(nl) = buf.find('\n') {
                    let line: String = buf.drain(..=nl).collect();
                    match acc.push_sse_line(&line) {
                        Ok(Some(text)) => { yield Update::Text(text); }
                        Ok(None) => {}
                        Err(_) => { Err(BridgeError::FrameError)?; } // ParseError → FrameError
                    }
                    if acc.is_done() { break 'read; }
                }
            }
            let _parsed = acc.finish(); // Task 8 inspects tool_calls; text-only milestone ends here.
            yield Update::Done { stop_reason: "stop".into() };
        };
        Ok(Box::pin(stream))
    }

    async fn cancel(&self, session: &SessionId) -> Result<(), BridgeError> {
        let _ = self.session_cancel(session).send(true);
        Ok(())
    }

    async fn configure_session(&self, session: &SessionId, cfg: &EffectiveConfig) -> Result<(), BridgeError> {
        let mut map = self.sessions.lock().expect("sessions lock");
        map.entry(session.clone()).or_default().model = cfg.model.clone();
        Ok(())
    }

    async fn forget_session(&self, session: &SessionId) {
        if let Ok(mut map) = self.sessions.lock() { map.remove(session); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::{EffectiveConfig, PermissionDecision, PermissionRequest, SessionContext};
    use bridge_core::ids::SessionId;
    use bridge_core::ports::{AgentBackend, PolicyEngine};
    use bridge_core::error::BridgeError;
    use std::sync::Arc;

    struct DenyAll;
    impl PolicyEngine for DenyAll {
        fn decide(&self, _: &PermissionRequest, _: &SessionContext) -> Result<PermissionDecision, BridgeError> {
            Err(BridgeError::PermissionDenied)
        }
    }

    #[tokio::test]
    async fn configure_session_stashes_model_and_object_safe() {
        let be = ApiBackend::new(crate::config::ApiConfig::new("http://127.0.0.1:1"));
        let s = SessionId::parse("s1").unwrap();
        be.configure_session(&s, &EffectiveConfig { model: Some("haiku".into()), ..Default::default() })
            .await.unwrap();
        assert_eq!(be.session_model(&s).as_deref(), Some("haiku"));
        be.forget_session(&s).await;
        assert!(be.session_model(&s).is_none());
        let _obj: Arc<dyn AgentBackend> = Arc::new(ApiBackend::new(crate::config::ApiConfig::new("http://127.0.0.1:1")));
    }

    #[tokio::test]
    async fn with_policy_swaps_engine() {
        let be = ApiBackend::new(crate::config::ApiConfig::new("http://127.0.0.1:1")).with_policy(Arc::new(DenyAll));
        fn assert_send_sync<T: Send + Sync>(_: &T) {}
        assert_send_sync(&be);
    }
}
