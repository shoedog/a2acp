//! Serve-side warm-session manager (Slice 0). Sibling to the registry + TaskStore. Owns the
//! contextId->handle table + the registry lease that pins the warm backend. Keyed by A2A contextId.

use bridge_core::domain::{effective_config, AgentOverride, SessionSpec};
use bridge_core::error::BridgeError;
use bridge_core::ids::{
    AgentId, ContextId, OperationId, SessionGeneration, SessionHandleId, SessionId,
};
use bridge_core::ports::{AgentBackend, AgentRegistry, Lease};
use bridge_core::session_cwd::SessionCwd;
use bridge_core::session_fingerprint::SessionSpecFingerprint;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    Idle,
    Running,
}

struct WarmHandle {
    #[allow(dead_code)] // surfaced by handle ops in later slices
    id: SessionHandleId,
    agent: AgentId,
    backend: Arc<dyn AgentBackend>,
    backend_session: SessionId,
    generation: SessionGeneration,
    fingerprint: SessionSpecFingerprint,
    lease: Box<dyn Lease>,
    state: SessionState,
    #[allow(dead_code)]
    op: Option<OperationId>,
    last_used: Instant,
}

/// What a checked-out warm turn needs to dispatch.
pub struct WarmTurn {
    pub backend: Arc<dyn AgentBackend>,
    pub session: SessionId,
}

/// Status snapshot (spec §5).
pub struct SessionStatusInfo {
    pub state: &'static str,
    pub agent: String,
    pub generation: u64,
    pub idle_age_ms: u128,
}

pub struct SessionManager {
    registry: Arc<dyn AgentRegistry>,
    by_context: Mutex<HashMap<ContextId, WarmHandle>>,
    idle_ttl: Duration,
    now: Box<dyn Fn() -> Instant + Send + Sync>,
    seq: std::sync::atomic::AtomicU64,
}

impl SessionManager {
    pub fn new(registry: Arc<dyn AgentRegistry>, idle_ttl: Duration) -> Self {
        Self::new_with_clock(registry, idle_ttl, Box::new(Instant::now))
    }

    pub fn new_with_clock(
        registry: Arc<dyn AgentRegistry>,
        idle_ttl: Duration,
        now: Box<dyn Fn() -> Instant + Send + Sync>,
    ) -> Self {
        Self {
            registry,
            by_context: Mutex::new(HashMap::new()),
            idle_ttl,
            now,
            seq: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Start a warm turn: mint (fresh ctx) or resume (known ctx). Resume requires a matching
    /// fingerprint (else ConfigMismatch), a non-retired lease (else SessionExpired), and an Idle
    /// handle (else HandleBusy). Transitions to Running. Resolves the agent exactly once.
    pub async fn checkout_turn(
        &self,
        ctx: &ContextId,
        agent: AgentId,
        overrides: Option<AgentOverride>,
        cwd: Option<SessionCwd>,
        op: OperationId,
    ) -> Result<WarmTurn, BridgeError> {
        let mut tab = self.by_context.lock().await;
        if let Some(h) = tab.get_mut(ctx) {
            if h.lease.is_retired() {
                return Err(BridgeError::SessionExpired);
            }
            if h.state == SessionState::Running {
                return Err(BridgeError::HandleBusy);
            }
            let resolved = self.registry.resolve(&agent).await?;
            let eff = effective_config(&resolved.entry, overrides.as_ref());
            let fp = SessionSpecFingerprint {
                agent: agent.clone(),
                config: eff,
                cwd: cwd.as_ref().map(|c| c.as_str().to_string()),
            };
            if let Some(field) = h.fingerprint.first_mismatch(&fp) {
                return Err(BridgeError::ConfigMismatch { field });
            }
            h.state = SessionState::Running;
            h.op = Some(op);
            h.last_used = (self.now)();
            return Ok(WarmTurn {
                backend: h.backend.clone(),
                session: h.backend_session.clone(),
            });
        }

        let resolved = self.registry.resolve(&agent).await?;
        let eff = effective_config(&resolved.entry, overrides.as_ref());
        let fp = SessionSpecFingerprint {
            agent: agent.clone(),
            config: eff.clone(),
            cwd: cwd.as_ref().map(|c| c.as_str().to_string()),
        };
        let n = self.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let backend_session = SessionId::parse(format!("ctx-{}-g0", ctx.as_str()))
            .map_err(|_| BridgeError::InvalidRequest { field: "contextId" })?;
        resolved
            .backend
            .configure_session(&backend_session, &SessionSpec { config: eff, cwd })
            .await?;
        let turn = WarmTurn {
            backend: resolved.backend.clone(),
            session: backend_session.clone(),
        };
        tab.insert(
            ctx.clone(),
            WarmHandle {
                id: SessionHandleId::parse(format!("h-{n}")).unwrap(),
                agent,
                backend: resolved.backend,
                backend_session,
                generation: SessionGeneration::new(0),
                fingerprint: fp,
                lease: resolved.lease,
                state: SessionState::Running,
                op: Some(op),
                last_used: (self.now)(),
            },
        );
        Ok(turn)
    }

    /// Mark the current turn finished -> Idle (keep warm). Called on producer exit.
    pub async fn finish_turn(&self, ctx: &ContextId) {
        if let Some(h) = self.by_context.lock().await.get_mut(ctx) {
            h.state = SessionState::Idle;
            h.op = None;
            h.last_used = (self.now)();
        }
    }

    pub async fn status(&self, ctx: &ContextId) -> Option<SessionStatusInfo> {
        let tab = self.by_context.lock().await;
        tab.get(ctx).map(|h| SessionStatusInfo {
            state: match h.state {
                SessionState::Idle => "idle",
                SessionState::Running => "running",
            },
            agent: h.agent.as_str().to_string(),
            generation: h.generation.get(),
            idle_age_ms: (self.now)().duration_since(h.last_used).as_millis(),
        })
    }

    pub async fn release(&self, ctx: &ContextId) {
        let h = self.by_context.lock().await.remove(ctx);
        if let Some(h) = h {
            h.backend.release_session(&h.backend_session).await;
            drop(h.lease);
        }
    }

    /// Cancel an in-flight turn but keep the session warm (-> Idle).
    pub async fn cancel(&self, ctx: &ContextId) -> Result<(), BridgeError> {
        let (backend, session) = {
            let mut tab = self.by_context.lock().await;
            let Some(h) = tab.get_mut(ctx) else {
                return Err(BridgeError::SessionNotFound);
            };
            h.state = SessionState::Idle;
            h.op = None;
            (h.backend.clone(), h.backend_session.clone())
        };
        backend.cancel(&session).await
    }

    /// Reap only idle warm sessions past the TTL (never an active turn).
    pub async fn reap_idle(&self) {
        let now = (self.now)();
        let expired: Vec<ContextId> = {
            let tab = self.by_context.lock().await;
            tab.iter()
                .filter(|(_, h)| {
                    h.state == SessionState::Idle
                        && now.duration_since(h.last_used) >= self.idle_ttl
                })
                .map(|(c, _)| c.clone())
                .collect()
        };
        for c in expired {
            self.release(&c).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use bridge_core::domain::{AgentEntry, AgentKind, Part, RegistrySnapshot};
    use bridge_core::ports::{BackendStream, Resolved, Update};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex as StdMutex;

    struct NoopLease;
    impl Lease for NoopLease {}

    #[derive(Clone)]
    struct RetiringLease {
        retired: Arc<AtomicBool>,
    }

    impl Lease for RetiringLease {
        fn is_retired(&self) -> bool {
            self.retired.load(Ordering::SeqCst)
        }
    }

    /// Backend that replies with a fixed text then Done, and records warm-session lifecycle calls.
    struct FakeBackend {
        reply: String,
        releases: StdMutex<Vec<String>>,
        cancels: StdMutex<Vec<String>>,
        configured: StdMutex<Vec<String>>,
    }

    impl FakeBackend {
        fn new(reply: impl Into<String>) -> Self {
            Self {
                reply: reply.into(),
                releases: StdMutex::new(Vec::new()),
                cancels: StdMutex::new(Vec::new()),
                configured: StdMutex::new(Vec::new()),
            }
        }

        fn releases(&self) -> Vec<String> {
            self.releases.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl AgentBackend for FakeBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _p: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            let updates = vec![
                Ok(Update::Text(self.reply.clone())),
                Ok(Update::Done {
                    stop_reason: "end_turn".into(),
                }),
            ];
            Ok(Box::pin(tokio_stream::iter(updates)))
        }

        async fn cancel(&self, s: &SessionId) -> Result<(), BridgeError> {
            self.cancels.lock().unwrap().push(s.as_str().to_string());
            Ok(())
        }

        async fn configure_session(
            &self,
            session: &SessionId,
            _spec: &SessionSpec,
        ) -> Result<(), BridgeError> {
            self.configured
                .lock()
                .unwrap()
                .push(session.as_str().to_string());
            Ok(())
        }

        async fn release_session(&self, session: &SessionId) {
            self.releases
                .lock()
                .unwrap()
                .push(session.as_str().to_string());
        }
    }

    /// Registry resolving a fixed agent entry and a shared recording backend.
    struct FakeRegistry {
        entry: AgentEntry,
        backend: Arc<FakeBackend>,
        retired: Arc<AtomicBool>,
    }

    impl FakeRegistry {
        fn new(entry: AgentEntry, backend: Arc<FakeBackend>) -> Self {
            Self {
                entry,
                backend,
                retired: Arc::new(AtomicBool::new(false)),
            }
        }

        fn retire(&self) {
            self.retired.store(true, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl AgentRegistry for FakeRegistry {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
            if self.entry.id != *id {
                return Err(BridgeError::UnknownAgent {
                    id: id.as_str().into(),
                });
            }
            Ok(Resolved {
                entry: Arc::new(self.entry.clone()),
                backend: self.backend.clone(),
                lease: Box::new(RetiringLease {
                    retired: self.retired.clone(),
                }),
            })
        }

        fn default_id(&self) -> AgentId {
            self.entry.id.clone()
        }

        async fn apply(&self, _snap: RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }

        fn list(&self) -> Vec<AgentId> {
            vec![self.entry.id.clone()]
        }
    }

    fn fake_entry(id: &str) -> AgentEntry {
        AgentEntry {
            id: AgentId::parse(id).unwrap(),
            cmd: Some("fake".into()),
            base_url: None,
            api_key_env: None,
            args: vec![],
            kind: AgentKind::Acp,
            model_provider: None,
            model: None,
            effort: None,
            mode: None,
            cwd: None,
            session_cwd: None,
            sandbox: None,
            mcp: vec![],
            mcp_delivery: Default::default(),
            auth_method: None,
            name: None,
            description: None,
            tags: vec![],
            version: None,
            extensions: Default::default(),
        }
    }

    fn manager() -> (SessionManager, Arc<FakeBackend>, Arc<FakeRegistry>) {
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend.clone()));
        (
            SessionManager::new(registry.clone(), Duration::from_secs(30)),
            backend,
            registry,
        )
    }

    fn agent() -> AgentId {
        AgentId::parse("codex").unwrap()
    }

    fn ctx(id: &str) -> ContextId {
        ContextId::parse(id).unwrap()
    }

    fn op(id: &str) -> OperationId {
        OperationId::parse(id).unwrap()
    }

    #[derive(Clone)]
    struct ManualClock {
        now: Arc<StdMutex<Instant>>,
    }

    impl ManualClock {
        fn new() -> Self {
            Self {
                now: Arc::new(StdMutex::new(Instant::now())),
            }
        }

        fn reader(&self) -> Box<dyn Fn() -> Instant + Send + Sync> {
            let now = self.now.clone();
            Box::new(move || *now.lock().unwrap())
        }

        fn advance(&self, by: Duration) {
            let mut now = self.now.lock().unwrap();
            *now += by;
        }
    }

    #[tokio::test]
    async fn resumes_same_backend_session_after_finish() {
        let (manager, _backend, _registry) = manager();
        let ctx = ctx("abc");

        let first = manager
            .checkout_turn(&ctx, agent(), None, None, op("op-1"))
            .await
            .unwrap();
        manager.finish_turn(&ctx).await;
        let second = manager
            .checkout_turn(&ctx, agent(), None, None, op("op-2"))
            .await
            .unwrap();

        assert_eq!(first.session.as_str(), "ctx-abc-g0");
        assert_eq!(first.session, second.session);
    }

    #[tokio::test]
    async fn concurrent_checkout_returns_handle_busy() {
        let (manager, _backend, _registry) = manager();
        let ctx = ctx("busy");

        manager
            .checkout_turn(&ctx, agent(), None, None, op("op-1"))
            .await
            .unwrap();
        let err = manager
            .checkout_turn(&ctx, agent(), None, None, op("op-2"))
            .await
            .err();

        assert_eq!(err, Some(BridgeError::HandleBusy));
    }

    #[tokio::test]
    async fn model_override_mismatch_returns_config_mismatch() {
        let (manager, _backend, _registry) = manager();
        let ctx = ctx("model");

        manager
            .checkout_turn(
                &ctx,
                agent(),
                Some(AgentOverride {
                    model: Some("gpt-5.5".into()),
                    ..Default::default()
                }),
                None,
                op("op-1"),
            )
            .await
            .unwrap();
        manager.finish_turn(&ctx).await;
        let err = manager
            .checkout_turn(
                &ctx,
                agent(),
                Some(AgentOverride {
                    model: Some("gpt-5.4".into()),
                    ..Default::default()
                }),
                None,
                op("op-2"),
            )
            .await
            .err();

        assert_eq!(err, Some(BridgeError::ConfigMismatch { field: "model" }));
    }

    #[tokio::test]
    async fn release_removes_status_and_releases_backend_session() {
        let (manager, backend, _registry) = manager();
        let ctx = ctx("release");

        manager
            .checkout_turn(&ctx, agent(), None, None, op("op-1"))
            .await
            .unwrap();
        manager.finish_turn(&ctx).await;
        manager.release(&ctx).await;

        assert!(manager.status(&ctx).await.is_none());
        assert_eq!(backend.releases(), vec!["ctx-release-g0"]);
    }

    #[tokio::test]
    async fn reap_idle_removes_only_idle_sessions_past_ttl() {
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend));
        let clock = ManualClock::new();
        let manager =
            SessionManager::new_with_clock(registry, Duration::from_secs(5), clock.reader());
        let idle = ctx("idle");
        let running = ctx("running");

        manager
            .checkout_turn(&idle, agent(), None, None, op("op-1"))
            .await
            .unwrap();
        manager.finish_turn(&idle).await;
        manager
            .checkout_turn(&running, agent(), None, None, op("op-2"))
            .await
            .unwrap();
        clock.advance(Duration::from_secs(6));

        manager.reap_idle().await;

        assert!(manager.status(&idle).await.is_none());
        assert_eq!(manager.status(&running).await.unwrap().state, "running");
    }

    #[tokio::test]
    async fn retired_lease_expires_next_checkout() {
        let (manager, _backend, registry) = manager();
        let ctx = ctx("retired");

        manager
            .checkout_turn(&ctx, agent(), None, None, op("op-1"))
            .await
            .unwrap();
        manager.finish_turn(&ctx).await;
        registry.retire();
        let err = manager
            .checkout_turn(&ctx, agent(), None, None, op("op-2"))
            .await
            .err();

        assert_eq!(err, Some(BridgeError::SessionExpired));
    }

    #[test]
    fn noop_lease_defaults_to_not_retired() {
        assert!(!NoopLease.is_retired());
    }
}
