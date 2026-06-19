//! Serve-side warm-session manager (Slice 0). Sibling to the registry + TaskStore. Owns the
//! contextId->handle table + the registry lease that pins the warm backend. Keyed by A2A contextId.

use bridge_core::domain::{effective_config, AgentOverride, SessionSpec};
use bridge_core::error::BridgeError;
use bridge_core::ids::{
    AgentId, ContextId, OperationId, SessionGeneration, SessionHandleId, SessionId,
};
use bridge_core::orch::{AgentSessionCaps, ReconcileOutcome, UsageSnapshot};
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
    Resetting,
    Compacting,
}

fn is_claimed(s: SessionState) -> bool {
    matches!(
        s,
        SessionState::Reconciling
            | SessionState::Expiring
            | SessionState::Resetting
            | SessionState::Compacting
    )
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
    usage: UsageSnapshot,
    /// Set by cancel()/release() while `Reconciling` so the in-flight reconcile expires the handle on
    /// resolve (the handle is never mutated/removed out from under an active reconcile). [PF-2/9/10]
    expire_after_reconcile: bool,
    #[allow(dead_code)]
    op: Option<OperationId>,
    pending_seed: Option<String>,
    last_used: Instant,
}

/// What a checked-out warm turn needs to dispatch.
pub struct WarmTurn {
    pub backend: Arc<dyn AgentBackend>,
    pub session: SessionId,
    pub usage_warning: Option<UsageWarning>,
    pub generation: SessionGeneration,
    pub op: OperationId,
    pub seed: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UsageWarning {
    pub used: u64,
    pub size: u64,
    pub fraction: f64,
    pub threshold: f64,
}

pub struct ResetOpts {
    pub force: bool,
}

#[derive(Debug, PartialEq)]
pub enum ResetOutcome {
    Cleared { generation: u64 },
    NotFound,
}

/// Status snapshot (spec §5).
pub struct SessionStatusInfo {
    pub state: &'static str,
    pub agent: String,
    pub generation: u64,
    pub idle_age_ms: u128,
    pub capabilities: AgentSessionCaps,
    pub usage: UsageSnapshot,
    pub over_threshold: Option<bool>,
}

impl SessionStatusInfo {
    /// `used/size` when both are known and `size > 0`, else `None` (degrade-safe:
    /// codex/claude always carry used+size; a non-ACP backend may not). [Slice 2]
    pub fn window_fraction(&self) -> Option<f64> {
        match (self.usage.used, self.usage.size) {
            (Some(u), Some(s)) if s > 0 => Some(u as f64 / s as f64),
            _ => None,
        }
    }
}

pub struct SessionManager {
    registry: Arc<dyn AgentRegistry>,
    by_context: Mutex<HashMap<ContextId, WarmHandle>>,
    idle_ttl: Duration,
    warn_fraction: Option<f64>,
    compact_summarize_timeout: Duration,
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
            warn_fraction: None,
            compact_summarize_timeout: Duration::from_secs(120),
            now,
            seq: std::sync::atomic::AtomicU64::new(0),
        }
    }

    pub fn with_warn_fraction(mut self, f: Option<f64>) -> Self {
        self.warn_fraction = f.filter(|v| *v > 0.0 && *v <= 1.0);
        self
    }

    pub fn with_compact_summarize_timeout(mut self, d: Duration) -> Self {
        self.compact_summarize_timeout = d;
        self
    }

    /// Test-only: observe the stashed next-turn seed (delivery is wired in checkout_turn at Slice-4 T5).
    #[cfg(test)]
    async fn pending_seed(&self, ctx: &ContextId) -> Option<String> {
        self.by_context
            .lock()
            .await
            .get(ctx)
            .and_then(|h| h.pending_seed.clone())
    }

    fn eval_warn(&self, u: &UsageSnapshot) -> Option<UsageWarning> {
        let thr = self.warn_fraction?;
        match (u.used, u.size) {
            (Some(used), Some(size)) if size > 0 && (used as f64 / size as f64) >= thr => {
                Some(UsageWarning {
                    used,
                    size,
                    fraction: used as f64 / size as f64,
                    threshold: thr,
                })
            }
            _ => None,
        }
    }

    fn over_threshold(&self, u: &UsageSnapshot) -> Option<bool> {
        let thr = self.warn_fraction?;
        match (u.used, u.size) {
            (Some(used), Some(size)) if size > 0 => Some((used as f64 / size as f64) >= thr),
            _ => None,
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
                let usage_warning = self.eval_warn(&h.usage);
                h.state = SessionState::Running;
                h.op = Some(op.clone());
                h.last_used = (self.now)();
                let seed = h.pending_seed.take();
                return Ok(WarmTurn {
                    backend: h.backend.clone(),
                    session: h.backend_session.clone(),
                    usage_warning,
                    generation: h.generation,
                    op,
                    seed,
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
                let usage_warning = self.eval_warn(&h.usage);
                h.fingerprint = fp;
                h.state = SessionState::Running;
                h.op = Some(op.clone());
                h.last_used = (self.now)();
                let seed = h.pending_seed.take();
                return Ok(WarmTurn {
                    backend: h.backend.clone(),
                    session: h.backend_session.clone(),
                    usage_warning,
                    generation: h.generation,
                    op,
                    seed,
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
            usage_warning: None,
            generation: SessionGeneration::new(0),
            op: op.clone(),
            seed: None,
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
                usage: UsageSnapshot::default(),
                expire_after_reconcile: false,
                op: Some(op),
                pending_seed: None,
                last_used: (self.now)(),
            },
        );
        Ok(turn)
    }

    /// Mark the current turn finished -> Idle (keep warm). FIX-3: no-op unless this is the SAME generation
    /// and operation AND the handle is Running (a turn only legitimately idles a Running handle); a stale
    /// (reset-away, cancelled, or claim-state) completion touches NOTHING.
    pub async fn finish_turn(&self, ctx: &ContextId, gen: SessionGeneration, op: &OperationId) {
        if let Some(h) = self.by_context.lock().await.get_mut(ctx) {
            if h.generation == gen && h.op.as_ref() == Some(op) && h.state == SessionState::Running
            {
                h.state = SessionState::Idle;
                h.op = None;
                h.last_used = (self.now)();
            }
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
                SessionState::Resetting => "resetting",
                SessionState::Compacting => "compacting",
            },
            agent: h.agent.as_str().to_string(),
            generation: h.generation.get(),
            idle_age_ms: (self.now)().duration_since(h.last_used).as_millis(),
            capabilities: h.caps.clone(),
            usage: h.usage.clone(),
            over_threshold: self.over_threshold(&h.usage),
        })
    }

    /// Record the latest usage snapshot for a warm handle (latest-wins). Stamps `at_ms` here
    /// (the inbound layer has a wall clock; SessionManager.now is monotonic). FIX-7: does NOT
    /// touch `last_used` (usage during a turn is already covered by Running + finish_turn's
    /// refresh; bumping it here only races reap_idle). No-ops a missing/removed handle. [Slice 2]
    pub async fn record_usage(
        &self,
        ctx: &ContextId,
        gen: SessionGeneration,
        op: &OperationId,
        mut snap: UsageSnapshot,
    ) {
        snap.at_ms = crate::workflow_sink::now_ms();
        if let Some(h) = self.by_context.lock().await.get_mut(ctx) {
            if h.generation == gen && h.op.as_ref() == Some(op) && h.state == SessionState::Running
            {
                h.usage = snap;
            }
        }
    }

    pub async fn release(&self, ctx: &ContextId) {
        let h = {
            let mut tab = self.by_context.lock().await;
            if let Some(h) = tab.get_mut(ctx) {
                // A reconcile owns the handle: defer teardown to its resolve (don't remove it out from
                // under the in-flight release / let the backend_session id be reused mid-release).
                if is_claimed(h.state) {
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
            if is_claimed(h.state) {
                h.expire_after_reconcile = true;
                return Ok(());
            }
            let was_running = h.state == SessionState::Running;
            h.state = SessionState::Idle;
            h.op = None;
            if was_running {
                h.last_used = (self.now)();
            }
            (h.backend.clone(), h.backend_session.clone())
        };
        backend.cancel(&session).await
    }

    pub async fn reset_session(
        &self,
        ctx: &ContextId,
        opts: ResetOpts,
    ) -> Result<ResetOutcome, BridgeError> {
        // (1)+(2)+(3) claim under ONE lock hold (FIX-2: never bounce through Idle, never call self.cancel).
        let (backend, old_id, claimed_id, new_gen, new_id, spec) = {
            let mut tab = self.by_context.lock().await;
            let Some(h) = tab.get_mut(ctx) else {
                return Ok(ResetOutcome::NotFound);
            };
            match h.state {
                SessionState::Idle => {}
                SessionState::Running if opts.force => {}
                _ => return Err(BridgeError::HandleBusy),
            }
            let backend = h.backend.clone();
            let old_id = h.backend_session.clone();
            let claimed_id = h.id.clone();
            let new_gen = SessionGeneration::new(h.generation.get() + 1);
            let new_id = SessionId::parse(format!("ctx-{}-g{}", ctx.as_str(), new_gen.get()))
                .map_err(|_| BridgeError::InvalidRequest { field: "contextId" })?;
            let cwd = match h.fingerprint.cwd.as_deref() {
                Some(s) => Some(
                    SessionCwd::parse(s).map_err(|_| BridgeError::ConfigInvalid {
                        reason: "session cwd".into(),
                    })?,
                ),
                None => None,
            };
            let spec = SessionSpec {
                config: h.fingerprint.config.clone(),
                cwd,
            };
            h.state = SessionState::Resetting;
            h.expire_after_reconcile = false;
            (backend, old_id, claimed_id, new_gen, new_id, spec)
        };

        // (4)+(5) PF-13: force pre-cancel (trait-default release_session does NOT cancel, e.g. ApiBackend);
        // release(old) is the drain; FIX-4: CAPTURE configure, no `?`.
        if opts.force {
            let _ = backend.cancel(&old_id).await;
        }
        backend.release_session(&old_id).await;
        let cfg = backend.configure_session(&new_id, &spec).await;

        // (6) re-acquire + revalidate exact claim; commit or EXPIRE (PF-7/PF-15 release the stashed new_id).
        let mut tab = self.by_context.lock().await;
        let still_ours = matches!(tab.get(ctx), Some(h) if h.id == claimed_id && h.state == SessionState::Resetting);
        let new_stashed = cfg.is_ok();
        if !still_ours {
            drop(tab);
            if new_stashed {
                backend.release_session(&new_id).await;
            }
            return Err(BridgeError::SessionExpired);
        }
        let deferred = tab
            .get(ctx)
            .map(|h| h.expire_after_reconcile)
            .unwrap_or(true);
        if cfg.is_err() || deferred {
            drop(tab);
            if new_stashed {
                backend.release_session(&new_id).await;
            }
            let mut tab = self.by_context.lock().await;
            if let Some(h) = tab.remove(ctx) {
                drop(h.lease);
            }
            return match cfg {
                Err(e) => Err(e),
                Ok(()) => Err(BridgeError::SessionExpired),
            };
        }
        let h = tab.get_mut(ctx).expect("still_ours");
        h.backend_session = new_id;
        h.generation = new_gen;
        h.usage = UsageSnapshot::default();
        h.op = None;
        h.pending_seed = None;
        h.state = SessionState::Idle;
        h.last_used = (self.now)();
        Ok(ResetOutcome::Cleared {
            generation: new_gen.get(),
        })
    }

    /// Compact: summarize the gen-N context, reset to N+1, and seed the summary for the next turn.
    /// require-Idle (no force). On ANY summarize failure the handle is EXPIRED (the old context is already
    /// mutated by the failed summarize exchange — no rollback). [Slice 4, FIX-1..14]
    pub async fn compact_session<F, Fut>(
        &self,
        ctx: &ContextId,
        summarize: F,
    ) -> Result<ResetOutcome, BridgeError>
    where
        F: FnOnce(Arc<dyn AgentBackend>, SessionId) -> Fut,
        Fut: std::future::Future<Output = Result<String, BridgeError>>,
    {
        // (1) Claim Idle -> Compacting under one lock; capture incl. the fallible cwd parse BEFORE the flip (FIX-9).
        let (backend, old_id, claimed_id, new_gen, new_id, spec) = {
            let mut tab = self.by_context.lock().await;
            let Some(h) = tab.get_mut(ctx) else {
                return Ok(ResetOutcome::NotFound);
            };
            if h.state != SessionState::Idle {
                return Err(BridgeError::HandleBusy);
            }
            // A pending (undelivered) seed means the last compact's summary is the session's ONLY retained
            // context (the gen N+1 ACP session is empty until the next turn injects it). Re-compacting now
            // would summarize that empty session and OVERWRITE the good summary -> data loss. Reject until the
            // seed is consumed by a real turn. (Whole-branch review; the spawn-detached handler makes a
            // lost-response retry reachable.)
            if h.pending_seed.is_some() {
                return Err(BridgeError::HandleBusy);
            }
            let backend = h.backend.clone();
            let old_id = h.backend_session.clone();
            let claimed_id = h.id.clone();
            let new_gen = SessionGeneration::new(h.generation.get() + 1);
            let new_id = SessionId::parse(format!("ctx-{}-g{}", ctx.as_str(), new_gen.get()))
                .map_err(|_| BridgeError::InvalidRequest { field: "contextId" })?;
            let cwd = match h.fingerprint.cwd.as_deref() {
                Some(s) => Some(
                    SessionCwd::parse(s).map_err(|_| BridgeError::ConfigInvalid {
                        reason: "session cwd".into(),
                    })?,
                ),
                None => None,
            };
            let spec = SessionSpec {
                config: h.fingerprint.config.clone(),
                cwd,
            };
            h.state = SessionState::Compacting;
            h.expire_after_reconcile = false;
            (backend, old_id, claimed_id, new_gen, new_id, spec)
        };

        // (2) Summarize on the gen-N session, TIME-BOUNDED, claim held (FIX-5).
        let summarized = tokio::time::timeout(
            self.compact_summarize_timeout,
            summarize(backend.clone(), old_id.clone()),
        )
        .await;

        // (3) Bad summary (Err / empty / timeout) -> EXPIRE (FIX-1/2). Never restore Idle.
        let summary = match summarized {
            Ok(Ok(s)) if !s.trim().is_empty() => s,
            bad => {
                let err = match bad {
                    Ok(Ok(_)) => BridgeError::AgentCrashed {
                        reason: "compact summary was empty".into(),
                    },
                    Ok(Err(e)) => e,
                    Err(_) => BridgeError::AgentCrashed {
                        reason: "compact summarize timed out".into(),
                    },
                };
                self.expire_after_summarize(ctx, &claimed_id, backend.as_ref(), &old_id)
                    .await;
                return Err(err);
            }
        };

        // (4) Good summary -> reset tail under Compacting (mirrors reset_session:475-519), stash seed on commit.
        backend.release_session(&old_id).await;
        let cfg = backend.configure_session(&new_id, &spec).await;
        let mut tab = self.by_context.lock().await;
        let still_ours = matches!(tab.get(ctx), Some(h) if h.id == claimed_id && h.state == SessionState::Compacting);
        let new_stashed = cfg.is_ok();
        if !still_ours {
            drop(tab);
            if new_stashed {
                backend.release_session(&new_id).await;
            }
            return Err(BridgeError::SessionExpired);
        }
        let deferred = tab
            .get(ctx)
            .map(|h| h.expire_after_reconcile)
            .unwrap_or(true);
        if cfg.is_err() || deferred {
            drop(tab);
            if new_stashed {
                backend.release_session(&new_id).await;
            }
            let mut tab = self.by_context.lock().await;
            if let Some(h) = tab.remove(ctx) {
                drop(h.lease);
            }
            return match cfg {
                Err(e) => Err(e), // FIX-3: configure error, NOT SessionExpired
                Ok(()) => Err(BridgeError::SessionExpired),
            };
        }
        let h = tab.get_mut(ctx).expect("still_ours");
        h.backend_session = new_id;
        h.generation = new_gen;
        h.usage = UsageSnapshot::default();
        h.op = None;
        h.pending_seed = Some(summary);
        h.state = SessionState::Idle;
        h.last_used = (self.now)();
        Ok(ResetOutcome::Cleared {
            generation: new_gen.get(),
        })
    }

    /// EXPIRE a Compacting handle after a failed summarize: tombstone -> release old -> remove + drop lease.
    /// Mirrors the non-clean tail of `checkout_turn` (:276-292).
    async fn expire_after_summarize(
        &self,
        ctx: &ContextId,
        claimed_id: &SessionHandleId,
        backend: &dyn AgentBackend,
        old_id: &SessionId,
    ) {
        {
            let mut tab = self.by_context.lock().await;
            let still_ours = matches!(
                tab.get(ctx),
                Some(h) if h.id == *claimed_id && h.state == SessionState::Compacting
            );
            if !still_ours {
                return;
            }
            tab.get_mut(ctx).expect("still_ours").state = SessionState::Expiring;
        }
        backend.release_session(old_id).await;
        let mut tab = self.by_context.lock().await;
        if let Some(h) = tab.remove(ctx) {
            drop(h.lease);
        }
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
            // Re-validate under the lock and REMOVE atomically: only reap a STILL-Idle, STILL-expired handle.
            // A claim that landed after the snapshot (compact/reset/reconcile flips the state off Idle) OWNS
            // the lifecycle — the reaper must SKIP it, never route through `release` (which would set the
            // deferred-expire flag and make the claim's commit tail kill the handle). [whole-branch review]
            let h = {
                let mut tab = self.by_context.lock().await;
                match tab.get(&c) {
                    Some(h)
                        if h.state == SessionState::Idle
                            && now.duration_since(h.last_used) >= self.idle_ttl =>
                    {
                        tab.remove(&c)
                    }
                    _ => None,
                }
            };
            if let Some(h) = h {
                h.backend.release_session(&h.backend_session).await;
                drop(h.lease);
            }
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
        configure_result: StdMutex<Result<(), BridgeError>>,
        configure_gate: StdMutex<Option<oneshot::Receiver<()>>>,
        configure_started: Notify,
        configure_started_count: AtomicUsize,
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
                configure_result: StdMutex::new(Ok(())),
                configure_gate: StdMutex::new(None),
                configure_started: Notify::new(),
                configure_started_count: AtomicUsize::new(0),
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

        fn cancels(&self) -> Vec<String> {
            self.cancels.lock().unwrap().clone()
        }

        fn configured(&self) -> Vec<String> {
            self.configured.lock().unwrap().clone()
        }

        fn reconciled(&self) -> Vec<(String, SessionSpec)> {
            self.reconciled.lock().unwrap().clone()
        }

        fn set_configure_result(&self, result: Result<(), BridgeError>) {
            *self.configure_result.lock().unwrap() = result;
        }

        fn block_next_configure(&self) -> oneshot::Sender<()> {
            let (tx, rx) = oneshot::channel();
            *self.configure_gate.lock().unwrap() = Some(rx);
            tx
        }

        #[allow(dead_code)]
        async fn wait_for_configure(&self) {
            while self.configure_started_count.load(Ordering::SeqCst) == 0 {
                self.configure_started.notified().await;
            }
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
            let gate = self.configure_gate.lock().unwrap().take();
            self.configure_started_count.fetch_add(1, Ordering::SeqCst);
            self.configure_started.notify_waiters();
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            self.configure_result.lock().unwrap().clone()
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

    fn manager_with_timeout(d: Duration) -> (SessionManager, Arc<FakeBackend>, Arc<FakeRegistry>) {
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend.clone()));
        (
            SessionManager::new(registry.clone(), Duration::from_secs(30))
                .with_compact_summarize_timeout(d),
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

    #[test]
    fn is_claimed_includes_compacting() {
        assert!(super::is_claimed(super::SessionState::Compacting));
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
    async fn reset_on_idle_bumps_generation_releases_old_configures_new_zeroes_usage() {
        let (manager, backend, _r) = manager();
        let c = ctx("reset");
        let turn = manager
            .checkout_turn(&c, agent(), None, None, op("op-1"))
            .await
            .unwrap();
        manager
            .record_usage(
                &c,
                turn.generation,
                &op("op-1"),
                UsageSnapshot {
                    used: Some(7),
                    size: Some(9),
                    cost: None,
                    at_ms: 0,
                },
            )
            .await;
        manager.finish_turn(&c, turn.generation, &op("op-1")).await;
        let out = manager
            .reset_session(&c, ResetOpts { force: false })
            .await
            .unwrap();
        assert!(matches!(out, ResetOutcome::Cleared { generation: 1 }));
        let s = manager.status(&c).await.unwrap();
        assert_eq!(s.generation, 1);
        assert_eq!(s.usage.used, None);
        assert_eq!(s.state, "idle");
        assert_eq!(backend.releases(), vec!["ctx-reset-g0"]);
        assert!(backend.configured().contains(&"ctx-reset-g1".to_string()));
    }

    #[tokio::test]
    async fn reset_on_running_without_force_is_handle_busy() {
        let (manager, _b, _r) = manager();
        let c = ctx("reset-busy");
        manager
            .checkout_turn(&c, agent(), None, None, op("op-1"))
            .await
            .unwrap();
        let err = manager
            .reset_session(&c, ResetOpts { force: false })
            .await
            .err()
            .unwrap();
        assert_eq!(err, BridgeError::HandleBusy);
    }

    #[tokio::test]
    async fn reset_unknown_ctx_is_not_found() {
        let (manager, _b, _r) = manager();
        let out = manager
            .reset_session(&ctx("nope"), ResetOpts { force: false })
            .await
            .unwrap();
        assert!(matches!(out, ResetOutcome::NotFound));
    }

    #[tokio::test]
    async fn compact_advances_generation_and_seeds() {
        let (m, fake, _r) = manager();
        let c = ctx("c1");
        // Warm + idle a session at gen 0.
        let turn = m
            .checkout_turn(&c, agent(), None, None, op("t1"))
            .await
            .unwrap();
        m.finish_turn(&c, turn.generation, &turn.op).await;
        let out = m
            .compact_session(&c, |_b, _s| async { Ok("THE SUMMARY".to_string()) })
            .await
            .unwrap();
        assert_eq!(out, ResetOutcome::Cleared { generation: 1 });
        let st = m.status(&c).await.unwrap();
        assert_eq!(st.generation, 1);
        assert_eq!(st.state, "idle");
        // Exactly the old session is released, once (no double-release).
        assert_eq!(fake.releases(), vec!["ctx-c1-g0".to_string()]);
        assert!(fake.configured().iter().any(|s| s == "ctx-c1-g1")); // new configured
                                                                     // The summary is stashed as the pending seed (delivered to the next checkout in T5).
        assert_eq!(m.pending_seed(&c).await.as_deref(), Some("THE SUMMARY"));
    }

    #[tokio::test]
    async fn compact_on_running_is_handle_busy() {
        let (m, _f, _r) = manager();
        let c = ctx("c2");
        let _turn = m
            .checkout_turn(&c, agent(), None, None, op("t1"))
            .await
            .unwrap(); // Running
        let err = m
            .compact_session(&c, |_b, _s| async { Ok("x".to_string()) })
            .await
            .unwrap_err();
        assert_eq!(err, BridgeError::HandleBusy);
    }

    #[tokio::test]
    async fn compact_unknown_ctx_is_not_found() {
        let (m, _f, _r) = manager();
        let out = m
            .compact_session(&ctx("nope"), |_b, _s| async { Ok("x".to_string()) })
            .await
            .unwrap();
        assert_eq!(out, ResetOutcome::NotFound);
    }

    #[tokio::test]
    async fn compact_bad_summary_expires_handle() {
        for bad in ["__ERR__", "   "] {
            let (m, fake, _r) = manager();
            let c = ctx("c");
            let turn = m
                .checkout_turn(&c, agent(), None, None, op("t1"))
                .await
                .unwrap();
            m.finish_turn(&c, turn.generation, &turn.op).await;
            let b = bad.to_string();
            let err = m
                .compact_session(&c, move |_b, _s| {
                    let b = b.clone();
                    async move {
                        if b == "__ERR__" {
                            Err(BridgeError::AgentCrashed {
                                reason: "boom".into(),
                            })
                        } else {
                            Ok(b)
                        } // whitespace-only -> empty
                    }
                })
                .await
                .unwrap_err();
            assert!(matches!(err, BridgeError::AgentCrashed { .. }));
            assert!(
                m.status(&c).await.is_none(),
                "handle EXPIRED (removed), not restored Idle"
            );
            assert!(
                fake.releases().iter().any(|s| s == "ctx-c-g0"),
                "old session released"
            );
        }
    }

    #[tokio::test]
    async fn compact_summary_timeout_expires() {
        let (m, _f, _r) = manager_with_timeout(std::time::Duration::from_millis(10));
        let c = ctx("c");
        let turn = m
            .checkout_turn(&c, agent(), None, None, op("t1"))
            .await
            .unwrap();
        m.finish_turn(&c, turn.generation, &turn.op).await;
        let err = m
            .compact_session(&c, |_b, _s| async {
                futures::future::pending::<()>().await; // never resolves
                Ok(String::new())
            })
            .await
            .unwrap_err();
        assert!(matches!(err, BridgeError::AgentCrashed { .. }));
        assert!(m.status(&c).await.is_none());
    }

    #[tokio::test]
    async fn compact_oversize_summary_expires() {
        // PFIX-10: a MessageTooLarge from the closure EXPIRES the handle (FIX-1/7) — explicit manager-level test.
        let (m, fake, _r) = manager();
        let c = ctx("c");
        let turn = m
            .checkout_turn(&c, agent(), None, None, op("t1"))
            .await
            .unwrap();
        m.finish_turn(&c, turn.generation, &turn.op).await;
        let err = m
            .compact_session(&c, |_b, _s| async { Err(BridgeError::MessageTooLarge) })
            .await
            .unwrap_err();
        assert_eq!(err, BridgeError::MessageTooLarge);
        assert!(m.status(&c).await.is_none());
        assert!(fake.releases().iter().any(|s| s == "ctx-c-g0"));
    }

    #[tokio::test]
    async fn compact_configure_failure_returns_configure_error() {
        // PFIX-3: set the configure failure AFTER the warm-up (the fake fails EVERY configure incl. g0).
        let (m, fake, _r) = manager();
        let c = ctx("c");
        let turn = m
            .checkout_turn(&c, agent(), None, None, op("t1"))
            .await
            .unwrap(); // configures g0 OK
        m.finish_turn(&c, turn.generation, &turn.op).await;
        fake.set_configure_result(Err(BridgeError::ConfigInvalid {
            reason: "test".into(),
        })); // g1 will fail
        let err = m
            .compact_session(&c, |_b, _s| async { Ok("good summary".to_string()) })
            .await
            .unwrap_err();
        assert!(matches!(err, BridgeError::ConfigInvalid { .. })); // FIX-3: configure error, NOT SessionExpired
        assert!(m.status(&c).await.is_none()); // handle EXPIRED (removed)
    }

    #[tokio::test]
    async fn checkout_consumes_seed_once() {
        let (m, _f, _r) = manager();
        let c = ctx("c");
        let t = m
            .checkout_turn(&c, agent(), None, None, op("t0"))
            .await
            .unwrap();
        m.finish_turn(&c, t.generation, &t.op).await;
        m.compact_session(&c, |_b, _s| async { Ok("SUMMARY".into()) })
            .await
            .unwrap();
        // First checkout after compact carries the seed; clear it; second sees None.
        let t1 = m
            .checkout_turn(&c, agent(), None, None, op("t1"))
            .await
            .unwrap();
        assert_eq!(t1.seed.as_deref(), Some("SUMMARY"));
        m.finish_turn(&c, t1.generation, &t1.op).await;
        let t2 = m
            .checkout_turn(&c, agent(), None, None, op("t2"))
            .await
            .unwrap();
        assert_eq!(t2.seed, None);
    }

    #[tokio::test]
    async fn seed_delivered_on_reconcile_checkout() {
        // FIX-10: the seed is ALSO taken at the post-reconcile clean resume return (:261-275), not only clean-diff.
        // Mirror the clean-reconcile setup in `model_override_change_reconciles_and_advances_fingerprint` (:1277).
        let (m, fake, _r) = manager();
        let c = ctx("c");
        let t = m
            .checkout_turn(&c, agent(), None, None, op("t0"))
            .await
            .unwrap();
        m.finish_turn(&c, t.generation, &t.op).await;
        m.compact_session(&c, |_b, _s| async { Ok("SUMMARY".into()) })
            .await
            .unwrap();
        // A model-override checkout takes the reconcile path; the seed must still be delivered.
        let t1 = m
            .checkout_turn(&c, agent(), Some(model_override("m1")), None, op("t1"))
            .await
            .unwrap();
        assert_eq!(t1.seed.as_deref(), Some("SUMMARY"));
        assert!(
            !fake.reconciled().is_empty(),
            "exercised the reconcile resume path"
        );
    }

    #[tokio::test]
    async fn clear_drops_pending_seed() {
        let (m, _f, _r) = manager();
        let c = ctx("c");
        let t = m
            .checkout_turn(&c, agent(), None, None, op("t0"))
            .await
            .unwrap();
        m.finish_turn(&c, t.generation, &t.op).await;
        m.compact_session(&c, |_b, _s| async { Ok("SUMMARY".into()) })
            .await
            .unwrap();
        m.reset_session(&c, ResetOpts { force: false })
            .await
            .unwrap(); // plain clear after compact
        let t1 = m
            .checkout_turn(&c, agent(), None, None, op("t1"))
            .await
            .unwrap();
        assert_eq!(t1.seed, None, "clear drops the pending seed");
    }

    #[tokio::test]
    async fn stale_completion_during_resetting_window_is_dropped() {
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend.clone()));
        let manager = Arc::new(SessionManager::new(registry, Duration::from_secs(30)));
        let c = ctx("reset-window");
        manager
            .checkout_turn(&c, agent(), None, None, op("op-1"))
            .await
            .unwrap();
        manager
            .record_usage(
                &c,
                SessionGeneration::new(0),
                &op("op-1"),
                UsageSnapshot {
                    used: Some(7),
                    size: Some(9),
                    cost: None,
                    at_ms: 0,
                },
            )
            .await;
        let unblock = backend.block_next_configure();
        let in_flight = {
            let (m, c2) = (manager.clone(), c.clone());
            tokio::spawn(async move { m.reset_session(&c2, ResetOpts { force: true }).await })
        };
        loop {
            if manager.status(&c).await.map(|s| s.state) == Some("resetting") {
                break;
            }
            tokio::task::yield_now().await;
        }
        manager
            .finish_turn(&c, SessionGeneration::new(0), &op("op-1"))
            .await;
        manager
            .record_usage(
                &c,
                SessionGeneration::new(0),
                &op("op-1"),
                UsageSnapshot {
                    used: Some(99),
                    size: Some(100),
                    cost: None,
                    at_ms: 0,
                },
            )
            .await;
        let mid = manager.status(&c).await.unwrap();
        assert_eq!(mid.state, "resetting");
        assert_eq!(mid.usage.used, Some(7));
        unblock.send(()).unwrap();
        assert!(matches!(
            in_flight.await.unwrap().unwrap(),
            ResetOutcome::Cleared { generation: 1 }
        ));
        let s = manager.status(&c).await.unwrap();
        assert_eq!(s.generation, 1);
        assert_eq!(s.state, "idle");
        assert_eq!(s.usage.used, None);
    }

    #[tokio::test]
    async fn reset_configure_failure_expires_handle_and_returns_error() {
        let (manager, backend, _r) = manager();
        let c = ctx("reset-cfg-fail");
        manager
            .checkout_turn(&c, agent(), None, None, op("op-1"))
            .await
            .unwrap();
        manager
            .finish_turn(&c, SessionGeneration::new(0), &op("op-1"))
            .await;
        backend.set_configure_result(Err(BridgeError::ConfigInvalid {
            reason: "boom".into(),
        }));
        let err = manager
            .reset_session(&c, ResetOpts { force: false })
            .await
            .err()
            .unwrap();
        assert!(matches!(err, BridgeError::ConfigInvalid { .. }));
        assert!(manager.status(&c).await.is_none());
        assert_eq!(backend.releases(), vec!["ctx-reset-cfg-fail-g0"]);
    }

    #[tokio::test]
    async fn checkout_and_release_during_resetting_are_deferred() {
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend.clone()));
        let manager = Arc::new(SessionManager::new(registry, Duration::from_secs(30)));
        let c = ctx("reset-defer");
        manager
            .checkout_turn(&c, agent(), None, None, op("op-1"))
            .await
            .unwrap();
        manager
            .finish_turn(&c, SessionGeneration::new(0), &op("op-1"))
            .await;
        let unblock = backend.block_next_configure();
        let in_flight = {
            let (m, c2) = (manager.clone(), c.clone());
            tokio::spawn(async move { m.reset_session(&c2, ResetOpts { force: false }).await })
        };
        loop {
            if manager.status(&c).await.map(|s| s.state) == Some("resetting") {
                break;
            }
            tokio::task::yield_now().await;
        }
        let busy = manager
            .checkout_turn(&c, agent(), None, None, op("op-2"))
            .await
            .err()
            .unwrap();
        assert_eq!(busy, BridgeError::HandleBusy);
        manager.release(&c).await;
        unblock.send(()).unwrap();
        assert_eq!(
            in_flight.await.unwrap().err().unwrap(),
            BridgeError::SessionExpired
        );
        assert!(manager.status(&c).await.is_none());
    }

    #[tokio::test]
    async fn cancel_during_resetting_is_deferred() {
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend.clone()));
        let manager = Arc::new(SessionManager::new(registry, Duration::from_secs(30)));
        let c = ctx("reset-cancel-defer");
        manager
            .checkout_turn(&c, agent(), None, None, op("op-1"))
            .await
            .unwrap();
        manager
            .finish_turn(&c, SessionGeneration::new(0), &op("op-1"))
            .await;
        let unblock = backend.block_next_configure();
        let in_flight = {
            let (m, c2) = (manager.clone(), c.clone());
            tokio::spawn(async move { m.reset_session(&c2, ResetOpts { force: false }).await })
        };
        loop {
            if manager.status(&c).await.map(|s| s.state) == Some("resetting") {
                break;
            }
            tokio::task::yield_now().await;
        }
        manager.cancel(&c).await.unwrap();
        unblock.send(()).unwrap();
        assert_eq!(
            in_flight.await.unwrap().err().unwrap(),
            BridgeError::SessionExpired
        );
        assert!(manager.status(&c).await.is_none());
    }

    #[tokio::test]
    async fn force_reset_cancels_and_releases_old_id() {
        let (manager, backend, _r) = manager();
        let c = ctx("reset-force");
        manager
            .checkout_turn(&c, agent(), None, None, op("op-1"))
            .await
            .unwrap();
        let out = manager
            .reset_session(&c, ResetOpts { force: true })
            .await
            .unwrap();
        assert!(matches!(out, ResetOutcome::Cleared { generation: 1 }));
        assert!(backend
            .cancels()
            .contains(&"ctx-reset-force-g0".to_string()));
        assert!(backend
            .releases()
            .contains(&"ctx-reset-force-g0".to_string()));
        assert_eq!(manager.status(&c).await.unwrap().generation, 1);
    }

    #[tokio::test]
    async fn finish_turn_applies_on_matching_generation_and_running() {
        let (manager, _b, _r) = manager();
        let c = ctx("ft");
        let turn = manager
            .checkout_turn(&c, agent(), None, None, op("op-1"))
            .await
            .unwrap();
        manager.finish_turn(&c, turn.generation, &op("op-1")).await;
        assert_eq!(manager.status(&c).await.unwrap().state, "idle");
    }

    #[tokio::test]
    async fn finish_turn_noops_on_stale_generation() {
        let (manager, _b, _r) = manager();
        let c = ctx("ft-stale");
        manager
            .checkout_turn(&c, agent(), None, None, op("op-1"))
            .await
            .unwrap();
        manager
            .finish_turn(&c, SessionGeneration::new(99), &op("op-1"))
            .await;
        assert_eq!(manager.status(&c).await.unwrap().state, "running");
    }

    #[tokio::test]
    async fn record_usage_noops_on_stale_generation() {
        let (manager, _b, _r) = manager();
        let c = ctx("ru-stale");
        manager
            .checkout_turn(&c, agent(), None, None, op("op-1"))
            .await
            .unwrap();
        manager
            .record_usage(
                &c,
                SessionGeneration::new(99),
                &op("op-1"),
                UsageSnapshot {
                    used: Some(5),
                    size: Some(9),
                    cost: None,
                    at_ms: 0,
                },
            )
            .await;
        assert_eq!(manager.status(&c).await.unwrap().usage.used, None);
    }

    #[tokio::test]
    async fn cancel_refreshes_idle_ttl() {
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend));
        let clock = ManualClock::new();
        let manager =
            SessionManager::new_with_clock(registry, Duration::from_secs(5), clock.reader());
        let c = ctx("cancel-ttl");
        let turn = manager
            .checkout_turn(&c, agent(), None, None, op("op-1"))
            .await
            .unwrap();
        clock.advance(Duration::from_secs(6));
        manager.cancel(&c).await.unwrap();
        manager.finish_turn(&c, turn.generation, &op("op-1")).await;
        manager.reap_idle().await;
        assert!(
            manager.status(&c).await.is_some(),
            "cancel refreshed idle ttl - not reaped"
        );
        clock.advance(Duration::from_secs(6));
        manager.reap_idle().await;
        assert!(manager.status(&c).await.is_none(), "now past ttl -> reaped");
    }

    #[tokio::test]
    async fn stale_finish_turn_after_cancel_does_not_idle_new_same_generation_turn() {
        let (manager, _b, _r) = manager();
        let c = ctx("cancel-race");
        let t1 = manager
            .checkout_turn(&c, agent(), None, None, op("op-1"))
            .await
            .unwrap();
        manager.cancel(&c).await.unwrap();
        let t2 = manager
            .checkout_turn(&c, agent(), None, None, op("op-2"))
            .await
            .unwrap();

        manager.finish_turn(&c, t1.generation, &t1.op).await;
        assert_eq!(manager.status(&c).await.unwrap().state, "running");

        manager
            .record_usage(
                &c,
                t1.generation,
                &t1.op,
                UsageSnapshot {
                    used: Some(99),
                    size: Some(100),
                    cost: None,
                    at_ms: 0,
                },
            )
            .await;
        assert_eq!(manager.status(&c).await.unwrap().usage.used, None);

        manager.finish_turn(&c, t2.generation, &t2.op).await;
        assert_eq!(manager.status(&c).await.unwrap().state, "idle");
    }

    #[tokio::test]
    async fn resumes_same_backend_session_after_finish() {
        let (manager, _backend, _registry) = manager();
        let ctx = ctx("abc");

        let first = manager
            .checkout_turn(&ctx, agent(), None, None, op("op-1"))
            .await
            .unwrap();
        manager
            .finish_turn(&ctx, first.generation, &op("op-1"))
            .await;
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
        manager
            .finish_turn(&ctx, SessionGeneration::new(0), &op("op-1"))
            .await;
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

        manager
            .finish_turn(&ctx, SessionGeneration::new(0), &op("op-2"))
            .await;
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
        manager
            .finish_turn(&ctx, SessionGeneration::new(0), &op("op-1"))
            .await;

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
        manager
            .finish_turn(&ctx, SessionGeneration::new(0), &op("op-1"))
            .await;
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
        manager
            .finish_turn(&ctx, SessionGeneration::new(0), &op("op-1"))
            .await;
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
        manager
            .finish_turn(&ctx, SessionGeneration::new(0), &op("op-1"))
            .await;

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
        manager
            .finish_turn(&ctx, SessionGeneration::new(0), &op("op-1"))
            .await;

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
        manager
            .finish_turn(&ctx, SessionGeneration::new(0), &op("op-1"))
            .await;

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
        manager
            .finish_turn(&model_ctx, SessionGeneration::new(0), &op("op-1"))
            .await;
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
        manager
            .finish_turn(&effort_ctx, SessionGeneration::new(0), &op("op-3"))
            .await;
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
        manager
            .finish_turn(&ctx, SessionGeneration::new(0), &op("op-1"))
            .await;
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
        manager
            .finish_turn(&ctx, SessionGeneration::new(0), &op("op-1"))
            .await;
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
    async fn record_usage_latest_wins_stamps_at_ms() {
        let (manager, _b, _r) = manager();
        let c = ctx("u");
        let turn = manager
            .checkout_turn(&c, agent(), None, None, op("op-1"))
            .await
            .unwrap();
        manager
            .record_usage(
                &c,
                turn.generation,
                &op("op-1"),
                UsageSnapshot {
                    used: Some(10),
                    size: Some(100),
                    cost: None,
                    at_ms: 0,
                },
            )
            .await;
        manager
            .record_usage(
                &c,
                turn.generation,
                &op("op-1"),
                UsageSnapshot {
                    used: Some(42),
                    size: Some(100),
                    cost: None,
                    at_ms: 0,
                },
            )
            .await;
        let s = manager.status(&c).await.unwrap();
        assert_eq!(s.usage.used, Some(42));
        assert!(s.usage.at_ms > 0);
    }

    #[tokio::test]
    async fn session_status_window_fraction_degrades_without_window() {
        let (manager, _b, _r) = manager();
        let c = ctx("missing-usage-window");
        manager
            .checkout_turn(&c, agent(), None, None, op("op-1"))
            .await
            .unwrap();

        let s = manager.status(&c).await.unwrap();
        assert_eq!(s.window_fraction(), None);
    }

    #[tokio::test]
    async fn checkout_warns_when_carried_usage_at_or_above_fraction() {
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend));
        let manager =
            SessionManager::new(registry, Duration::from_secs(30)).with_warn_fraction(Some(0.8));
        let c = ctx("warn");
        manager
            .checkout_turn(&c, agent(), None, None, op("op-1"))
            .await
            .unwrap();
        manager
            .record_usage(
                &c,
                SessionGeneration::new(0),
                &op("op-1"),
                UsageSnapshot {
                    used: Some(90),
                    size: Some(100),
                    cost: None,
                    at_ms: 0,
                },
            )
            .await;
        manager
            .finish_turn(&c, SessionGeneration::new(0), &op("op-1"))
            .await;
        let turn = manager
            .checkout_turn(&c, agent(), None, None, op("op-2"))
            .await
            .unwrap();
        let w = turn.usage_warning.expect("0.90 >= 0.80 warns");
        assert_eq!((w.used, w.size), (90, 100));
        assert_eq!(manager.status(&c).await.unwrap().over_threshold, Some(true));
    }

    #[tokio::test]
    async fn mint_never_warns_and_below_threshold_is_none() {
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend));
        let manager =
            SessionManager::new(registry, Duration::from_secs(30)).with_warn_fraction(Some(0.8));
        let c = ctx("below");
        let mint = manager
            .checkout_turn(&c, agent(), None, None, op("op-1"))
            .await
            .unwrap();
        assert!(mint.usage_warning.is_none(), "mint has no carried usage");
        manager
            .record_usage(
                &c,
                mint.generation,
                &op("op-1"),
                UsageSnapshot {
                    used: Some(10),
                    size: Some(100),
                    cost: None,
                    at_ms: 0,
                },
            )
            .await;
        manager.finish_turn(&c, mint.generation, &op("op-1")).await;
        let turn = manager
            .checkout_turn(&c, agent(), None, None, op("op-2"))
            .await
            .unwrap();
        assert!(turn.usage_warning.is_none(), "0.10 < 0.80");
        assert_eq!(
            manager.status(&c).await.unwrap().over_threshold,
            Some(false)
        );
    }

    #[tokio::test]
    async fn warn_disabled_and_degraded_are_none() {
        let (manager, _b, _r) = manager();
        let c = ctx("disabled");
        let first = manager
            .checkout_turn(&c, agent(), None, None, op("op-1"))
            .await
            .unwrap();
        manager
            .record_usage(
                &c,
                first.generation,
                &op("op-1"),
                UsageSnapshot {
                    used: Some(99),
                    size: Some(100),
                    cost: None,
                    at_ms: 0,
                },
            )
            .await;
        manager.finish_turn(&c, first.generation, &op("op-1")).await;
        let turn = manager
            .checkout_turn(&c, agent(), None, None, op("op-2"))
            .await
            .unwrap();
        assert!(turn.usage_warning.is_none());
        assert_eq!(manager.status(&c).await.unwrap().over_threshold, None);

        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend));
        let manager =
            SessionManager::new(registry, Duration::from_secs(30)).with_warn_fraction(Some(0.8));
        let c = ctx("degraded");
        let first = manager
            .checkout_turn(&c, agent(), None, None, op("op-1"))
            .await
            .unwrap();
        manager
            .record_usage(
                &c,
                first.generation,
                &op("op-1"),
                UsageSnapshot {
                    used: Some(99),
                    size: None,
                    cost: None,
                    at_ms: 0,
                },
            )
            .await;
        manager.finish_turn(&c, first.generation, &op("op-1")).await;
        let turn = manager
            .checkout_turn(&c, agent(), None, None, op("op-2"))
            .await
            .unwrap();
        assert!(turn.usage_warning.is_none());
        assert_eq!(manager.status(&c).await.unwrap().over_threshold, None);
    }

    #[tokio::test]
    async fn record_usage_noops_unknown_ctx() {
        let (manager, _b, _r) = manager();
        manager
            .record_usage(
                &ctx("nope"),
                SessionGeneration::new(0),
                &op("op-1"),
                UsageSnapshot::default(),
            )
            .await;
        assert!(manager.status(&ctx("nope")).await.is_none());
    }

    #[tokio::test]
    async fn record_usage_does_not_refresh_idle_ttl() {
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend));
        let clock = ManualClock::new();
        let manager =
            SessionManager::new_with_clock(registry, Duration::from_secs(5), clock.reader());
        let c = ctx("idle-usage");
        let turn = manager
            .checkout_turn(&c, agent(), None, None, op("op-1"))
            .await
            .unwrap();
        manager.finish_turn(&c, turn.generation, &op("op-1")).await;
        clock.advance(Duration::from_secs(6));
        manager
            .record_usage(
                &c,
                turn.generation,
                &op("op-1"),
                UsageSnapshot {
                    used: Some(1),
                    size: Some(2),
                    cost: None,
                    at_ms: 0,
                },
            )
            .await;
        manager.reap_idle().await;
        assert!(
            manager.status(&c).await.is_none(),
            "record_usage must NOT have refreshed last_used"
        );
    }

    #[tokio::test]
    async fn release_removes_status_and_releases_backend_session() {
        let (manager, backend, _registry) = manager();
        let ctx = ctx("release");

        let turn = manager
            .checkout_turn(&ctx, agent(), None, None, op("op-1"))
            .await
            .unwrap();
        manager
            .finish_turn(&ctx, turn.generation, &op("op-1"))
            .await;
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

        let idle_turn = manager
            .checkout_turn(&idle, agent(), None, None, op("op-1"))
            .await
            .unwrap();
        manager
            .finish_turn(&idle, idle_turn.generation, &op("op-1"))
            .await;
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
    async fn compact_rejects_when_seed_pending() {
        // Whole-branch review: a second compact before the seed is delivered would summarize the empty
        // gen N+1 session and overwrite the good summary. Reject it; the original seed must survive.
        let (m, _f, _r) = manager();
        let c = ctx("c");
        let turn = m
            .checkout_turn(&c, agent(), None, None, op("t1"))
            .await
            .unwrap();
        m.finish_turn(&c, turn.generation, &turn.op).await;
        m.compact_session(&c, |_b, _s| async { Ok("GOOD SUMMARY".to_string()) })
            .await
            .unwrap();
        let err = m
            .compact_session(&c, |_b, _s| async {
                Ok("EMPTY SESSION SUMMARY".to_string())
            })
            .await
            .unwrap_err();
        assert_eq!(err, BridgeError::HandleBusy);
        // The good summary is preserved (delivered on the next checkout).
        let t2 = m
            .checkout_turn(&c, agent(), None, None, op("t2"))
            .await
            .unwrap();
        assert_eq!(t2.seed.as_deref(), Some("GOOD SUMMARY"));
    }

    #[tokio::test]
    async fn reap_idle_does_not_reap_compacting_handle() {
        // Whole-branch review: a handle claimed Compacting must survive reap_idle even past the TTL (the
        // claim owns the lifecycle; the reaper must not defer-expire it).
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend));
        let clock = ManualClock::new();
        let manager = Arc::new(SessionManager::new_with_clock(
            registry,
            Duration::from_secs(5),
            clock.reader(),
        ));
        let c = ctx("c");
        let turn = manager
            .checkout_turn(&c, agent(), None, None, op("t1"))
            .await
            .unwrap();
        manager.finish_turn(&c, turn.generation, &turn.op).await;

        // Start a compact whose summarize blocks until signalled -> the handle stays Compacting.
        let gate = Arc::new(Notify::new());
        let (m2, c2, g2) = (manager.clone(), c.clone(), gate.clone());
        let compact = tokio::spawn(async move {
            m2.compact_session(&c2, move |_b, _s| {
                let g2 = g2.clone();
                async move {
                    g2.notified().await;
                    Ok("SUMMARY".to_string())
                }
            })
            .await
        });
        for _ in 0..1000 {
            if manager.status(&c).await.map(|s| s.state) == Some("compacting") {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(manager.status(&c).await.unwrap().state, "compacting");

        clock.advance(Duration::from_secs(10));
        manager.reap_idle().await;
        assert!(
            manager.status(&c).await.is_some(),
            "reap must not touch a Compacting handle"
        );

        gate.notify_one();
        let out = compact.await.unwrap().unwrap();
        assert_eq!(out, ResetOutcome::Cleared { generation: 1 });
    }

    #[tokio::test]
    async fn retired_lease_expires_next_checkout() {
        let (manager, _backend, registry) = manager();
        let ctx = ctx("retired");

        let turn = manager
            .checkout_turn(&ctx, agent(), None, None, op("op-1"))
            .await
            .unwrap();
        manager
            .finish_turn(&ctx, turn.generation, &op("op-1"))
            .await;
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
