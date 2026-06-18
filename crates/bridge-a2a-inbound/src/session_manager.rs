//! Serve-side warm-session manager (Slice 0). Sibling to the registry + TaskStore. Owns the
//! contextId->handle table + the registry lease that pins the warm backend. Keyed by A2A contextId.

use bridge_core::domain::{effective_config, AgentOverride, SessionSpec};
use bridge_core::error::BridgeError;
use bridge_core::ids::{
    AgentId, ContextId, OperationId, SessionGeneration, SessionHandleId, SessionId,
};
use bridge_core::orch::{AgentSessionCaps, ReconcileOutcome};
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
    /// A warm reconcile (model/effort re-apply) is in flight. The handle is OWNED by that reconcile:
    /// not re-claimable (checkout -> HandleBusy) and not removable (cancel/release set
    /// `expire_after_reconcile` instead) until it resolves. Closes the ABA + release-reuse races.
    Reconciling,
    /// A non-clean reconcile is tearing the handle down (release_session in flight). Held as a tombstone
    /// so a concurrent checkout (HandleBusy) can't re-mint the same backend_session id before release ends.
    Expiring,
}

struct WarmHandle {
    #[allow(dead_code)] // surfaced by handle ops in later slices
    id: SessionHandleId,
    agent: AgentId,
    backend: Arc<dyn AgentBackend>,
    backend_session: SessionId,
    caps: AgentSessionCaps,
    generation: SessionGeneration,
    fingerprint: SessionSpecFingerprint,
    lease: Box<dyn Lease>,
    state: SessionState,
    /// Set by cancel()/release() while `Reconciling` so the in-flight reconcile expires the handle on
    /// resolve (the handle is never mutated/removed out from under an active reconcile). [PF-2/9/10]
    expire_after_reconcile: bool,
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
    pub capabilities: AgentSessionCaps,
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
            if h.state != SessionState::Idle {
                // Running / Reconciling / Expiring all mean the handle is busy.
                return Err(BridgeError::HandleBusy);
            }
            let resolved = self.registry.resolve(&agent).await?;
            let eff = effective_config(&resolved.entry, overrides.as_ref());
            let fp = SessionSpecFingerprint {
                agent: agent.clone(),
                config: eff,
                cwd: cwd.as_ref().map(|c| c.as_str().to_string()),
            };
            let d = h.fingerprint.diff(&fp);
            if d.is_empty() {
                h.state = SessionState::Running;
                h.op = Some(op);
                h.last_used = (self.now)();
                return Ok(WarmTurn {
                    backend: h.backend.clone(),
                    session: h.backend_session.clone(),
                });
            }
            if d.contains(&"agent") {
                return Err(BridgeError::ConfigMismatch { field: "agent" });
            }
            if d.contains(&"cwd") {
                return Err(BridgeError::ConfigMismatch { field: "cwd" });
            }
            if d.contains(&"mode") {
                return Err(BridgeError::ConfigReseedRequired { field: "mode" });
            }
            if d.contains(&"model") && fp.config.model.is_none() {
                return Err(BridgeError::ConfigReseedRequired { field: "model" });
            }
            if d.contains(&"effort") && fp.config.effort.is_none() {
                return Err(BridgeError::ConfigReseedRequired { field: "effort" });
            }
            let reseed_field = if d.contains(&"model") {
                "model"
            } else {
                "effort"
            };
            let claimed_id = h.id.clone();
            let backend = h.backend.clone();
            let backend_session = h.backend_session.clone();
            // Claim the handle as Reconciling: a concurrent checkout is now HandleBusy (no ABA re-claim) and
            // cancel/release defer (set expire_after_reconcile) rather than mutate/remove it.
            h.state = SessionState::Reconciling;
            h.expire_after_reconcile = false;
            h.last_used = (self.now)();
            drop(tab);

            let outcome = backend
                .reconcile_config(
                    &backend_session,
                    &SessionSpec {
                        config: fp.config.clone(),
                        cwd: cwd.clone(),
                    },
                )
                .await;

            let mut tab = self.by_context.lock().await;
            // Re-validate the EXACT claim: the handle must still be the one we set Reconciling. Given the
            // invariants (Reconciling blocks re-claim/remove), anything else is a logic error -> bail.
            let still_ours = matches!(
                tab.get(ctx),
                Some(h) if h.id == claimed_id && h.state == SessionState::Reconciling
            );
            if !still_ours {
                return Err(BridgeError::SessionExpired);
            }
            let cancelled_or_released = tab
                .get(ctx)
                .map(|h| h.expire_after_reconcile)
                .unwrap_or(true);
            let clean = matches!(outcome, Ok(ReconcileOutcome::Applied)) && !cancelled_or_released;
            if clean {
                let h = tab.get_mut(ctx).expect("still_ours");
                h.fingerprint = fp;
                h.state = SessionState::Running;
                h.op = Some(op);
                h.last_used = (self.now)();
                return Ok(WarmTurn {
                    backend: h.backend.clone(),
                    session: h.backend_session.clone(),
                });
            }
            // Non-clean (failed reconcile OR cancel/release arrived mid-window): EXPIRE via an `Expiring`
            // tombstone held across release_session().await so a concurrent checkout (HandleBusy on Expiring)
            // can't re-mint the same backend_session id before release completes.
            tab.get_mut(ctx).expect("still_ours").state = SessionState::Expiring;
            drop(tab);
            backend.release_session(&backend_session).await;
            let mut tab = self.by_context.lock().await;
            if let Some(h) = tab.remove(ctx) {
                drop(h.lease);
            }
            return if cancelled_or_released {
                Err(BridgeError::SessionExpired)
            } else {
                Err(BridgeError::ConfigReseedRequired {
                    field: reseed_field,
                })
            };
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
        let caps = resolved.backend.capabilities();
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
                caps,
                generation: SessionGeneration::new(0),
                fingerprint: fp,
                lease: resolved.lease,
                state: SessionState::Running,
                expire_after_reconcile: false,
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
                SessionState::Reconciling => "reconciling",
                SessionState::Expiring => "expiring",
            },
            agent: h.agent.as_str().to_string(),
            generation: h.generation.get(),
            idle_age_ms: (self.now)().duration_since(h.last_used).as_millis(),
            capabilities: h.caps.clone(),
        })
    }

    pub async fn release(&self, ctx: &ContextId) {
        let h = {
            let mut tab = self.by_context.lock().await;
            if let Some(h) = tab.get_mut(ctx) {
                // A reconcile owns the handle: defer teardown to its resolve (don't remove it out from
                // under the in-flight release / let the backend_session id be reused mid-release).
                if h.state == SessionState::Reconciling || h.state == SessionState::Expiring {
                    h.expire_after_reconcile = true;
                    return;
                }
            }
            tab.remove(ctx)
        };
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
            // A reconcile owns the handle: flag it to expire on resolve rather than resetting to Idle
            // (which would let a third checkout re-claim it under the in-flight reconcile — the ABA bug).
            if h.state == SessionState::Reconciling || h.state == SessionState::Expiring {
                h.expire_after_reconcile = true;
                return Ok(());
            }
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
    use bridge_core::domain::{AgentEntry, AgentKind, Effort, Part, RegistrySnapshot};
    use bridge_core::ports::{BackendStream, Resolved, Update};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;
    use tokio::sync::{oneshot, Notify};

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
        reconciled: StdMutex<Vec<(String, SessionSpec)>>,
        reconcile_result: StdMutex<Result<ReconcileOutcome, BridgeError>>,
        reconcile_gate: StdMutex<Option<oneshot::Receiver<()>>>,
        reconcile_started: Notify,
        reconcile_started_count: AtomicUsize,
        capabilities: AgentSessionCaps,
    }

    impl FakeBackend {
        fn new(reply: impl Into<String>) -> Self {
            Self {
                reply: reply.into(),
                releases: StdMutex::new(Vec::new()),
                cancels: StdMutex::new(Vec::new()),
                configured: StdMutex::new(Vec::new()),
                reconciled: StdMutex::new(Vec::new()),
                reconcile_result: StdMutex::new(Ok(ReconcileOutcome::Applied)),
                reconcile_gate: StdMutex::new(None),
                reconcile_started: Notify::new(),
                reconcile_started_count: AtomicUsize::new(0),
                capabilities: AgentSessionCaps::default(),
            }
        }

        fn with_capabilities(reply: impl Into<String>, capabilities: AgentSessionCaps) -> Self {
            Self {
                capabilities,
                ..Self::new(reply)
            }
        }

        fn releases(&self) -> Vec<String> {
            self.releases.lock().unwrap().clone()
        }

        fn configured(&self) -> Vec<String> {
            self.configured.lock().unwrap().clone()
        }

        fn reconciled(&self) -> Vec<(String, SessionSpec)> {
            self.reconciled.lock().unwrap().clone()
        }

        fn set_reconcile_result(&self, result: Result<ReconcileOutcome, BridgeError>) {
            *self.reconcile_result.lock().unwrap() = result;
        }

        fn block_next_reconcile(&self) -> oneshot::Sender<()> {
            let (tx, rx) = oneshot::channel();
            *self.reconcile_gate.lock().unwrap() = Some(rx);
            tx
        }

        async fn wait_for_reconcile(&self) {
            while self.reconcile_started_count.load(Ordering::SeqCst) == 0 {
                self.reconcile_started.notified().await;
            }
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

        async fn reconcile_config(
            &self,
            session: &SessionId,
            spec: &SessionSpec,
        ) -> Result<ReconcileOutcome, BridgeError> {
            self.reconciled
                .lock()
                .unwrap()
                .push((session.as_str().to_string(), spec.clone()));
            let gate = self.reconcile_gate.lock().unwrap().take();
            self.reconcile_started_count.fetch_add(1, Ordering::SeqCst);
            self.reconcile_started.notify_waiters();
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            self.reconcile_result.lock().unwrap().clone()
        }

        async fn release_session(&self, session: &SessionId) {
            self.releases
                .lock()
                .unwrap()
                .push(session.as_str().to_string());
        }

        fn capabilities(&self) -> AgentSessionCaps {
            self.capabilities.clone()
        }
    }

    /// Registry resolving a fixed agent entry and a shared recording backend.
    struct FakeRegistry {
        entries: Vec<AgentEntry>,
        backend: Arc<FakeBackend>,
        retired: Arc<AtomicBool>,
    }

    impl FakeRegistry {
        fn new(entry: AgentEntry, backend: Arc<FakeBackend>) -> Self {
            Self {
                entries: vec![entry],
                backend,
                retired: Arc::new(AtomicBool::new(false)),
            }
        }

        fn with_entries(entries: Vec<AgentEntry>, backend: Arc<FakeBackend>) -> Self {
            Self {
                entries,
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
            let Some(entry) = self.entries.iter().find(|entry| entry.id == *id) else {
                return Err(BridgeError::UnknownAgent {
                    id: id.as_str().into(),
                });
            };
            Ok(Resolved {
                entry: Arc::new(entry.clone()),
                backend: self.backend.clone(),
                lease: Box::new(RetiringLease {
                    retired: self.retired.clone(),
                }),
            })
        }

        fn default_id(&self) -> AgentId {
            self.entries[0].id.clone()
        }

        async fn apply(&self, _snap: RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }

        fn list(&self) -> Vec<AgentId> {
            self.entries.iter().map(|entry| entry.id.clone()).collect()
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

    fn model_override(model: &str) -> AgentOverride {
        AgentOverride {
            model: Some(model.into()),
            ..Default::default()
        }
    }

    fn effort_override(effort: Effort) -> AgentOverride {
        AgentOverride {
            effort: Some(effort),
            ..Default::default()
        }
    }

    fn mode_override(mode: &str) -> AgentOverride {
        AgentOverride {
            mode: Some(mode.into()),
            ..Default::default()
        }
    }

    fn cwd(path: &str) -> Option<SessionCwd> {
        Some(SessionCwd::parse(path).unwrap())
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
    async fn model_override_change_reconciles_and_advances_fingerprint() {
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend.clone()));
        let manager = SessionManager::new(registry, Duration::from_secs(30));
        let ctx = ctx("model");

        manager
            .checkout_turn(
                &ctx,
                agent(),
                Some(model_override("gpt-5.5")),
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
                Some(model_override("gpt-5.4")),
                None,
                op("op-2"),
            )
            .await;

        assert!(err.is_ok());
        assert_eq!(backend.reconciled().len(), 1);
        assert_eq!(
            backend.reconciled()[0].1.config.model.as_deref(),
            Some("gpt-5.4")
        );

        manager.finish_turn(&ctx).await;
        backend.set_reconcile_result(Ok(ReconcileOutcome::Rejected));
        manager
            .checkout_turn(
                &ctx,
                agent(),
                Some(model_override("gpt-5.4")),
                None,
                op("op-3"),
            )
            .await
            .unwrap();
        assert_eq!(backend.reconciled().len(), 1);
    }

    #[tokio::test]
    async fn effort_override_change_reconciles() {
        let (manager, backend, _registry) = manager();
        let ctx = ctx("effort");

        manager
            .checkout_turn(
                &ctx,
                agent(),
                Some(effort_override(Effort::Low)),
                None,
                op("op-1"),
            )
            .await
            .unwrap();
        manager.finish_turn(&ctx).await;

        manager
            .checkout_turn(
                &ctx,
                agent(),
                Some(effort_override(Effort::High)),
                None,
                op("op-2"),
            )
            .await
            .unwrap();

        assert_eq!(backend.reconciled().len(), 1);
        assert_eq!(backend.reconciled()[0].1.config.effort, Some(Effort::High));
    }

    #[tokio::test]
    async fn reconcile_not_advertised_expires_handle_and_next_checkout_mints_cold() {
        let (manager, backend, _registry) = manager();
        let ctx = ctx("not-advertised");

        manager
            .checkout_turn(
                &ctx,
                agent(),
                Some(model_override("gpt-5.5")),
                None,
                op("op-1"),
            )
            .await
            .unwrap();
        manager.finish_turn(&ctx).await;
        backend.set_reconcile_result(Ok(ReconcileOutcome::NotAdvertised));

        let err = manager
            .checkout_turn(
                &ctx,
                agent(),
                Some(model_override("gpt-5.4")),
                None,
                op("op-2"),
            )
            .await
            .err()
            .unwrap();

        assert_eq!(err, BridgeError::ConfigReseedRequired { field: "model" });
        assert!(manager.status(&ctx).await.is_none());
        assert_eq!(backend.releases(), vec!["ctx-not-advertised-g0"]);

        manager
            .checkout_turn(
                &ctx,
                agent(),
                Some(model_override("gpt-5.4")),
                None,
                op("op-3"),
            )
            .await
            .unwrap();
        assert_eq!(
            backend.configured(),
            vec!["ctx-not-advertised-g0", "ctx-not-advertised-g0"]
        );
    }

    #[tokio::test]
    async fn reconcile_rejected_expires_handle() {
        let (manager, backend, _registry) = manager();
        let ctx = ctx("rejected");

        manager
            .checkout_turn(
                &ctx,
                agent(),
                Some(model_override("gpt-5.5")),
                None,
                op("op-1"),
            )
            .await
            .unwrap();
        manager.finish_turn(&ctx).await;
        backend.set_reconcile_result(Ok(ReconcileOutcome::Rejected));

        let err = manager
            .checkout_turn(
                &ctx,
                agent(),
                Some(model_override("gpt-5.4")),
                None,
                op("op-2"),
            )
            .await
            .err()
            .unwrap();

        assert_eq!(err, BridgeError::ConfigReseedRequired { field: "model" });
        assert!(manager.status(&ctx).await.is_none());
        assert_eq!(backend.releases(), vec!["ctx-rejected-g0"]);
    }

    #[tokio::test]
    async fn mode_change_requires_reseed_without_reconcile() {
        let (manager, backend, _registry) = manager();
        let ctx = ctx("mode");

        manager
            .checkout_turn(&ctx, agent(), Some(mode_override("fast")), None, op("op-1"))
            .await
            .unwrap();
        manager.finish_turn(&ctx).await;

        let err = manager
            .checkout_turn(&ctx, agent(), Some(mode_override("slow")), None, op("op-2"))
            .await
            .err()
            .unwrap();

        assert_eq!(err, BridgeError::ConfigReseedRequired { field: "mode" });
        assert_eq!(backend.reconciled().len(), 0);
        assert_eq!(manager.status(&ctx).await.unwrap().state, "idle");
    }

    #[tokio::test]
    async fn cwd_change_beats_model_change_as_config_mismatch() {
        let (manager, backend, _registry) = manager();
        let ctx = ctx("cwd");

        manager
            .checkout_turn(
                &ctx,
                agent(),
                Some(model_override("gpt-5.5")),
                cwd("/work/a"),
                op("op-1"),
            )
            .await
            .unwrap();
        manager.finish_turn(&ctx).await;

        let err = manager
            .checkout_turn(
                &ctx,
                agent(),
                Some(model_override("gpt-5.4")),
                cwd("/work/b"),
                op("op-2"),
            )
            .await
            .err()
            .unwrap();

        assert_eq!(err, BridgeError::ConfigMismatch { field: "cwd" });
        assert_eq!(backend.reconciled().len(), 0);
    }

    #[tokio::test]
    async fn agent_change_is_config_mismatch() {
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::with_entries(
            vec![fake_entry("codex"), fake_entry("claude")],
            backend.clone(),
        ));
        let manager = SessionManager::new(registry, Duration::from_secs(30));
        let ctx = ctx("agent");

        manager
            .checkout_turn(&ctx, agent(), None, None, op("op-1"))
            .await
            .unwrap();
        manager.finish_turn(&ctx).await;

        let err = manager
            .checkout_turn(
                &ctx,
                AgentId::parse("claude").unwrap(),
                None,
                None,
                op("op-2"),
            )
            .await
            .err()
            .unwrap();

        assert_eq!(err, BridgeError::ConfigMismatch { field: "agent" });
        assert_eq!(backend.reconciled().len(), 0);
    }

    #[tokio::test]
    async fn clearing_model_or_effort_requires_reseed_without_reconcile() {
        let (manager, backend, _registry) = manager();
        let model_ctx = ctx("clear-model");

        manager
            .checkout_turn(
                &model_ctx,
                agent(),
                Some(model_override("gpt-5.5")),
                None,
                op("op-1"),
            )
            .await
            .unwrap();
        manager.finish_turn(&model_ctx).await;
        let err = manager
            .checkout_turn(&model_ctx, agent(), None, None, op("op-2"))
            .await
            .err()
            .unwrap();
        assert_eq!(err, BridgeError::ConfigReseedRequired { field: "model" });

        let effort_ctx = ctx("clear-effort");
        manager
            .checkout_turn(
                &effort_ctx,
                agent(),
                Some(effort_override(Effort::High)),
                None,
                op("op-3"),
            )
            .await
            .unwrap();
        manager.finish_turn(&effort_ctx).await;
        let err = manager
            .checkout_turn(&effort_ctx, agent(), None, None, op("op-4"))
            .await
            .err()
            .unwrap();
        assert_eq!(err, BridgeError::ConfigReseedRequired { field: "effort" });
        assert_eq!(backend.reconciled().len(), 0);
    }

    #[tokio::test]
    async fn release_during_reconcile_returns_session_expired_and_preserves_fresh_handle() {
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend.clone()));
        let manager = Arc::new(SessionManager::new(registry, Duration::from_secs(30)));
        let ctx = ctx("release-race");

        manager
            .checkout_turn(
                &ctx,
                agent(),
                Some(model_override("gpt-5.5")),
                None,
                op("op-1"),
            )
            .await
            .unwrap();
        manager.finish_turn(&ctx).await;
        let unblock = backend.block_next_reconcile();

        let in_flight = {
            let manager = manager.clone();
            let ctx = ctx.clone();
            tokio::spawn(async move {
                manager
                    .checkout_turn(
                        &ctx,
                        agent(),
                        Some(model_override("gpt-5.4")),
                        None,
                        op("op-2"),
                    )
                    .await
            })
        };
        backend.wait_for_reconcile().await;

        manager.release(&ctx).await;
        // During the reconcile/release window the handle is OWNED (Reconciling): a concurrent checkout must
        // be HandleBusy — no fresh re-mint of the same backend_session id mid-reconcile (closes the reuse race).
        let busy = manager
            .checkout_turn(
                &ctx,
                agent(),
                Some(model_override("gpt-5.3")),
                None,
                op("op-3"),
            )
            .await
            .err()
            .unwrap();
        assert_eq!(busy, BridgeError::HandleBusy);
        unblock.send(()).unwrap();

        // The in-flight reconcile observes the deferred release and EXPIRES the handle.
        assert_eq!(
            in_flight.await.unwrap().err().unwrap(),
            BridgeError::SessionExpired
        );
        // Handle is gone -> a subsequent checkout mints fresh (cold).
        assert!(manager.status(&ctx).await.is_none());
        manager
            .checkout_turn(
                &ctx,
                agent(),
                Some(model_override("gpt-5.3")),
                None,
                op("op-4"),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn cancel_during_reconcile_expires_claimed_handle() {
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend.clone()));
        let manager = Arc::new(SessionManager::new(registry, Duration::from_secs(30)));
        let ctx = ctx("cancel-race");

        manager
            .checkout_turn(
                &ctx,
                agent(),
                Some(model_override("gpt-5.5")),
                None,
                op("op-1"),
            )
            .await
            .unwrap();
        manager.finish_turn(&ctx).await;
        let unblock = backend.block_next_reconcile();

        let in_flight = {
            let manager = manager.clone();
            let ctx = ctx.clone();
            tokio::spawn(async move {
                manager
                    .checkout_turn(
                        &ctx,
                        agent(),
                        Some(model_override("gpt-5.4")),
                        None,
                        op("op-2"),
                    )
                    .await
            })
        };
        backend.wait_for_reconcile().await;

        manager.cancel(&ctx).await.unwrap();
        unblock.send(()).unwrap();

        assert_eq!(
            in_flight.await.unwrap().err().unwrap(),
            BridgeError::SessionExpired
        );
        assert!(manager.status(&ctx).await.is_none());
        assert_eq!(backend.releases(), vec!["ctx-cancel-race-g0"]);
    }

    #[tokio::test]
    async fn capabilities_are_recorded_on_handle_and_status() {
        let caps = AgentSessionCaps {
            load_session: true,
            resume: true,
            close: true,
            list: true,
            delete: false,
        };
        let backend = Arc::new(FakeBackend::with_capabilities("ok", caps.clone()));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend));
        let manager = SessionManager::new(registry, Duration::from_secs(30));
        let ctx = ctx("caps");

        manager
            .checkout_turn(&ctx, agent(), None, None, op("op-1"))
            .await
            .unwrap();

        assert_eq!(manager.status(&ctx).await.unwrap().capabilities, caps);
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
