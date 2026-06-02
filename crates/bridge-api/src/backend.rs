//! ApiBackend — the non-process OpenAI-compatible AgentBackend.
use crate::config::ApiConfig;
use bridge_core::domain::{EffectiveConfig, Part, PermissionDecision, PermissionRequest, SessionContext};
use bridge_core::error::BridgeError;
use bridge_core::ids::SessionId;
use bridge_core::ports::{AgentBackend, BackendStream, PolicyEngine};
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
    // Used in Task 7 (turn loop); suppressed until then.
    #[allow(dead_code)]
    cfg: ApiConfig,
    #[allow(dead_code)]
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
}

#[async_trait::async_trait]
impl AgentBackend for ApiBackend {
    async fn prompt(&self, _session: &SessionId, _parts: Vec<Part>) -> Result<BackendStream, BridgeError> {
        // Filled in Task 7.
        Err(BridgeError::AgentCrashed)
    }

    async fn cancel(&self, session: &SessionId) -> Result<(), BridgeError> {
        // send(true) errors only if there are no receivers (no in-flight turn) — ignore.
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
