//! Serve-side warm-session manager (Slice 0). Sibling to the registry + TaskStore. Owns the
//! contextId->handle table + the registry lease that pins the warm backend. Keyed by A2A contextId.

use crate::clock::{Clock, SystemClock};
use bridge_core::domain::{
    effective_config, AgentOverride, InjectRequest, QueuedInject, SessionSpec,
};
use bridge_core::error::BridgeError;
use bridge_core::ids::{
    AgentId, ContextId, OperationId, SessionGeneration, SessionHandleId, SessionId,
};
use bridge_core::orch::{AgentSessionCaps, ReconcileOutcome, UsageSnapshot};
use bridge_core::permission::PermissionRegistry;
use bridge_core::ports::{AgentBackend, AgentRegistry, DiagnosticObserver, Lease};
use bridge_core::session_cwd::SessionCwd;
use bridge_core::session_fingerprint::SessionSpecFingerprint;
use futures::FutureExt;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

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
    Cancelling,
    /// W1-B: a fresh handle has been claimed (a minted claim id stashed in `op`) and `configure_session`
    /// is running OFF the `by_context` lock, so a slow agent spawn/configure no longer serializes every
    /// context's checkout behind it. Claimed like the other states above: not re-claimable (checkout ->
    /// HandleBusy) and not removable out from under the settle — cancel/release/force-clear set
    /// `expire_after_reconcile` instead (same mechanism the other claimed states use) until
    /// `checkout_turn_inner`'s settle (lock #3) resolves the EXACT claim. No `turn_abort` exists yet (the
    /// real turn only mints at settle), so deferred expiry during this state must never fire one.
    Configuring,
}

fn is_claimed(s: SessionState) -> bool {
    matches!(
        s,
        SessionState::Reconciling
            | SessionState::Expiring
            | SessionState::Resetting
            | SessionState::Compacting
            | SessionState::Cancelling
            | SessionState::Configuring
    )
}

/// cancel-tokens F2 (whole-branch review): fire any abort token still lingering on a handle whose backend
/// session is about to be RELEASED. A keep-warm `SessionCancel` deliberately leaves the in-flight turn's
/// token on the (Idle) handle — see `cancel_inner` — so a pre-first-poll producer stays cancellable rather
/// than stranding the ACP cancel latch. The invariant that makes that safe: EVERY path that then releases
/// that backend session must fire the lingering token FIRST, or the producer could re-mint the released
/// session. Call this under the `by_context` lock immediately before `backend.release_session(&old_id)`
/// (or before pushing a removed handle to a deferred release). `cancel()` is synchronous → lock-safe.
/// Latch-safe: `release_session` removes the ACP entry, so the cancel latch dies with it.
fn fire_lingering_turn_abort(h: &mut WarmHandle) {
    for abort in h.turn_aborts.drain(..) {
        abort.token.cancel();
    }
}

fn has_armed_expiry_intent(h: &WarmHandle, op: &OperationId) -> bool {
    h.turn_aborts
        .iter()
        .any(|turn| turn.op == *op && turn.expiry_intent.is_armed())
}

fn first_armed_idle_expiry(h: &WarmHandle) -> Option<OperationId> {
    if h.state != SessionState::Idle || h.op.is_some() {
        return None;
    }
    h.turn_aborts
        .iter()
        .find(|turn| turn.expiry_intent.is_armed())
        .map(|turn| turn.op.clone())
}

fn reserve_idle_successor_or_armed(h: &WarmHandle) -> Option<OperationId> {
    debug_assert_eq!(h.state, SessionState::Idle);
    debug_assert!(h.op.is_none());
    h.turn_aborts
        .iter()
        .find(|turn| !turn.expiry_intent.reserve_successor())
        .map(|turn| turn.op.clone())
}

struct TurnAbort {
    op: OperationId,
    token: CancellationToken,
    expiry_intent: WarmExpiryIntent,
}

/// Exact-operation signal shared by the synchronous completion observer and
/// the session-table owner. A structured failure can arm this without awaiting
/// the Tokio table lock, so concurrent cancel settlement cannot reopen the
/// poisoned backend session before the expiry claim arrives.
#[derive(Clone)]
pub struct WarmExpiryIntent(Arc<std::sync::atomic::AtomicU8>);

impl WarmExpiryIntent {
    const OPEN: u8 = 0;
    const ARMED: u8 = 1;
    const SUCCESSOR_RESERVED: u8 = 2;

    pub(crate) fn new() -> Self {
        Self(Arc::new(std::sync::atomic::AtomicU8::new(Self::OPEN)))
    }

    pub(crate) fn arm(&self) {
        let _ = self.0.compare_exchange(
            Self::OPEN,
            Self::ARMED,
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
        );
    }

    fn is_armed(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Acquire) == Self::ARMED
    }

    /// Atomically linearize successor admission against late failure
    /// observation for the retained operation. `false` means expiry armed
    /// first and the caller must claim cleanup instead of minting a successor.
    fn reserve_successor(&self) -> bool {
        match self.0.compare_exchange(
            Self::OPEN,
            Self::SUCCESSOR_RESERVED,
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
        ) {
            Ok(_) | Err(Self::SUCCESSOR_RESERVED) => true,
            Err(Self::ARMED) => false,
            Err(other) => unreachable!("unknown warm expiry intent state: {other}"),
        }
    }
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
    /// All not-yet-settled turn abort tokens (cancel-tokens F2). Fired by every backend-session-release path via
    /// `fire_lingering_turn_abort` so a pre-first-poll producer aborts instead of re-minting the released
    /// session. A keep-warm `SessionCancel` deliberately LEAVES the token here (rather than firing it — that
    /// would strand the ACP cancel latch) so a later reset/release can still fire it. A subsequent checkout
    /// appends rather than overwrites; `finish_turn` removes only its exact operation token, closing the
    /// cancel-A -> checkout-B -> release/reset orphan window.
    turn_aborts: Vec<TurnAbort>,
    pending_seed: Option<String>,
    pending_injects: Vec<QueuedInject>,
    last_used: Instant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ExpiringTombstoneState {
    Expiring,
    CleanupFailed { code: &'static str },
}

/// Resource-free marker retained while the exact warm-session cleanup owner is
/// running. Generation + operation reject stale completion and the claim id
/// prevents an older flight from clearing a newer marker.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ExpiringTombstone {
    generation: SessionGeneration,
    op: OperationId,
    cleanup_claim_id: u64,
    state: ExpiringTombstoneState,
}

/// Minimal capability retained after checked cleanup fails. The warm handle,
/// lease, operation observer, and task-owned state have already been dropped;
/// this pair is sufficient to re-enter the backend's exact per-session cleanup
/// cell when an operator retries SessionRelease or SessionClear.
#[derive(Clone)]
struct CleanupRetryOwner {
    backend: Arc<dyn AgentBackend>,
    backend_session: SessionId,
}

#[derive(Default)]
struct SessionTable {
    live: HashMap<ContextId, WarmHandle>,
    tombstones: HashMap<ContextId, ExpiringTombstone>,
    cleanup_retries: HashMap<ContextId, CleanupRetryOwner>,
}

impl SessionTable {
    fn get(&self, ctx: &ContextId) -> Option<&WarmHandle> {
        self.live.get(ctx)
    }

    fn get_mut(&mut self, ctx: &ContextId) -> Option<&mut WarmHandle> {
        self.live.get_mut(ctx)
    }

    fn insert(&mut self, ctx: ContextId, handle: WarmHandle) -> Option<WarmHandle> {
        debug_assert!(!self.tombstones.contains_key(&ctx));
        debug_assert!(!self.cleanup_retries.contains_key(&ctx));
        self.live.insert(ctx, handle)
    }

    fn remove(&mut self, ctx: &ContextId) -> Option<WarmHandle> {
        self.live.remove(ctx)
    }

    fn contains_key(&self, ctx: &ContextId) -> bool {
        self.live.contains_key(ctx) || self.tombstones.contains_key(ctx)
    }

    fn iter(&self) -> impl Iterator<Item = (&ContextId, &WarmHandle)> {
        self.live.iter()
    }

    fn keys(&self) -> impl Iterator<Item = &ContextId> {
        self.live.keys().chain(self.tombstones.keys())
    }

    fn tombstone(&self, ctx: &ContextId) -> Option<&ExpiringTombstone> {
        self.tombstones.get(ctx)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CleanupReport {
    pub(crate) result: Result<(), BridgeError>,
}

/// Exact-claim capability retained by both the detached cleanup worker and
/// its joiner. The worker uses it after any caught panic; the joiner uses it
/// when Tokio reports cancellation or an otherwise escaped panic. A stale
/// capability cannot settle a replacement tombstone because every mutation
/// rechecks generation, operation, and cleanup-claim identity.
#[derive(Clone)]
struct CleanupClaimSettlement {
    by_context: Arc<Mutex<SessionTable>>,
    ctx: ContextId,
    generation: SessionGeneration,
    op: OperationId,
    cleanup_claim_id: u64,
    retry: CleanupRetryOwner,
}

impl CleanupClaimSettlement {
    async fn settle(&self, result: Result<(), BridgeError>) -> CleanupReport {
        let mut table = self.by_context.lock().await;
        let matches_claim = matches!(
            table.tombstones.get(&self.ctx),
            Some(tombstone)
                if tombstone.generation == self.generation
                    && tombstone.op == self.op
                    && tombstone.cleanup_claim_id == self.cleanup_claim_id
        );
        if matches_claim {
            if result.is_ok() {
                table.tombstones.remove(&self.ctx);
                table.cleanup_retries.remove(&self.ctx);
            } else if let Some(tombstone) = table.tombstones.get_mut(&self.ctx) {
                tombstone.state = ExpiringTombstoneState::CleanupFailed {
                    code: "warm.cleanup.release_failed",
                };
                table
                    .cleanup_retries
                    .insert(self.ctx.clone(), self.retry.clone());
            }
        }
        CleanupReport { result }
    }
}

pub(crate) struct CleanupFlight {
    task: tokio::task::JoinHandle<CleanupReport>,
    settlement: CleanupClaimSettlement,
}

impl CleanupFlight {
    #[cfg(test)]
    fn abort(&self) {
        self.task.abort();
    }
}

/// Owns the removed warm handle until its resources are synchronously moved
/// into one detached cleanup task. Dropping the claim before explicit start
/// starts that same flight unobserved.
pub(crate) struct ExpiryClaim {
    by_context: Arc<Mutex<SessionTable>>,
    children: Arc<Mutex<HashMap<ContextId, HashSet<ContextId>>>>,
    ctx: ContextId,
    generation: SessionGeneration,
    op: OperationId,
    cleanup_claim_id: u64,
    prune_children: bool,
    handle: Option<WarmHandle>,
    retry: Option<CleanupRetryOwner>,
}

impl ExpiryClaim {
    fn start_flight(&mut self) -> Option<CleanupFlight> {
        let handle = self.handle.take();
        let retry = match handle.as_ref() {
            Some(handle) => CleanupRetryOwner {
                backend: handle.backend.clone(),
                backend_session: handle.backend_session.clone(),
            },
            None => self.retry.take()?,
        };
        let by_context = self.by_context.clone();
        let children = self.children.clone();
        let ctx = self.ctx.clone();
        let generation = self.generation;
        let op = self.op.clone();
        let cleanup_claim_id = self.cleanup_claim_id;
        let prune_children = self.prune_children;
        let settlement = CleanupClaimSettlement {
            by_context,
            ctx: ctx.clone(),
            generation,
            op,
            cleanup_claim_id,
            retry: retry.clone(),
        };
        let worker_settlement = settlement.clone();
        let task = tokio::spawn(async move {
            let recovery = worker_settlement.clone();
            let worker = AssertUnwindSafe(async move {
                let result = retry
                    .backend
                    .release_session_checked(&retry.backend_session)
                    .await;
                // The lease and every remaining handle-owned value belong to
                // this task and are dropped exactly once even when the joining
                // waiter is canceled. The whole worker is unwind-protected so
                // a public Lease implementation cannot strand the tombstone.
                drop(handle);
                if prune_children {
                    let mut registrations = children.lock().await;
                    for set in registrations.values_mut() {
                        set.retain(|child| child != &ctx);
                    }
                    registrations.retain(|_, set| !set.is_empty());
                }
                worker_settlement.settle(result).await
            })
            .catch_unwind()
            .await;
            match worker {
                Ok(report) => report,
                Err(_) => {
                    recovery
                        .settle(Err(BridgeError::agent_crashed(
                            "warm cleanup task panicked",
                        )))
                        .await
                }
            }
        });
        Some(CleanupFlight { task, settlement })
    }

    /// Consume this claim into its detached cleanup flight without awaiting.
    /// The returned join handle observes the report; dropping it detaches from
    /// the already-owned task and cannot restart cleanup.
    pub(crate) fn into_flight(mut self) -> Option<CleanupFlight> {
        self.start_flight()
    }

    pub(crate) async fn join_flight(flight: CleanupFlight) -> CleanupReport {
        let CleanupFlight { task, settlement } = flight;
        match task.await {
            Ok(report) => report,
            Err(_) => {
                settlement
                    .settle(Err(BridgeError::agent_crashed("warm cleanup task failed")))
                    .await
            }
        }
    }

    pub(crate) async fn cleanup(mut self) -> CleanupReport {
        // No await precedes ownership transfer. If the returned future is never
        // polled, ExpiryClaim::drop starts the flight; once polled, dropping the
        // JoinHandle merely detaches the already-owned task.
        let Some(flight) = self.start_flight() else {
            return CleanupReport { result: Ok(()) };
        };
        Self::join_flight(flight).await
    }
}

impl Drop for ExpiryClaim {
    fn drop(&mut self) {
        let _ = self.start_flight();
    }
}

/// What a checked-out warm turn needs to dispatch.
pub struct WarmTurn {
    pub backend: Arc<dyn AgentBackend>,
    pub session: SessionId,
    pub usage_warning: Option<UsageWarning>,
    pub generation: SessionGeneration,
    pub op: OperationId,
    pub expiry_intent: WarmExpiryIntent,
    pub abort: CancellationToken,
    pub seed: Option<String>,
    pub injects: Vec<QueuedInject>,
    pub agent: AgentId,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub mode: Option<String>,
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
    perm_registry: Option<Arc<PermissionRegistry>>,
    by_context: Arc<Mutex<SessionTable>>,
    children: Arc<Mutex<HashMap<ContextId, HashSet<ContextId>>>>,
    idle_ttl: Duration,
    warn_fraction: Option<f64>,
    compact_summarize_timeout: Duration,
    clock: Arc<dyn Clock>,
    seq: std::sync::atomic::AtomicU64,
    turn_op_seq: std::sync::atomic::AtomicU64,
    cleanup_claim_seq: std::sync::atomic::AtomicU64,
    /// Test-only deterministic-interleaving hook (whole-branch review regression test): when armed via
    /// `block_next_lock2`, `checkout_turn_inner` parks here between the off-lock resolve/fingerprint and
    /// lock #2. Compiled out entirely in non-test builds; a no-op in test builds unless armed.
    #[cfg(test)]
    lock2_pause_gate: std::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    #[cfg(test)]
    lock2_pause_started: tokio::sync::Notify,
    #[cfg(test)]
    lock2_pause_started_count: std::sync::atomic::AtomicUsize,
}

impl SessionManager {
    pub fn new(registry: Arc<dyn AgentRegistry>, idle_ttl: Duration) -> Self {
        Self::new_with_clock(registry, idle_ttl, Arc::new(SystemClock))
    }

    pub fn new_with_clock(
        registry: Arc<dyn AgentRegistry>,
        idle_ttl: Duration,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            registry,
            perm_registry: None,
            by_context: Arc::new(Mutex::new(SessionTable::default())),
            children: Arc::new(Mutex::new(HashMap::new())),
            idle_ttl,
            warn_fraction: None,
            compact_summarize_timeout: Duration::from_secs(120),
            clock,
            seq: std::sync::atomic::AtomicU64::new(0),
            turn_op_seq: std::sync::atomic::AtomicU64::new(1),
            cleanup_claim_seq: std::sync::atomic::AtomicU64::new(1),
            #[cfg(test)]
            lock2_pause_gate: std::sync::Mutex::new(None),
            #[cfg(test)]
            lock2_pause_started: tokio::sync::Notify::new(),
            #[cfg(test)]
            lock2_pause_started_count: std::sync::atomic::AtomicUsize::new(0),
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

    pub fn with_permission_registry(mut self, reg: Arc<PermissionRegistry>) -> Self {
        self.perm_registry = Some(reg);
        self
    }

    pub async fn inject(&self, req: InjectRequest) -> Result<usize, BridgeError> {
        const MAX_INJECTS: usize = 32;
        const MAX_INJECT_BYTES: usize = 64 * 1024;

        let mut tab = self.by_context.lock().await;
        let Some(h) = tab.get_mut(&req.context) else {
            return Err(BridgeError::SessionNotFound);
        };
        if !matches!(h.state, SessionState::Idle | SessionState::Running) {
            return Err(BridgeError::HandleBusy);
        }

        let mut candidate = h.pending_injects.clone();
        let replacement = req.dedupe_key.as_ref().and_then(|key| {
            candidate
                .iter()
                .position(|entry| entry.dedupe_key.as_ref() == Some(key))
        });
        let queued = QueuedInject {
            text: req.text,
            mode: req.mode,
            dedupe_key: req.dedupe_key,
        };
        if let Some(idx) = replacement {
            candidate[idx] = queued;
        } else {
            candidate.push(queued);
        }

        let total_bytes: usize = candidate.iter().map(|entry| entry.text.len()).sum();
        if candidate.len() > MAX_INJECTS || total_bytes > MAX_INJECT_BYTES {
            return Err(BridgeError::HandleBusy);
        }

        h.pending_injects = candidate;
        Ok(h.pending_injects.len())
    }

    pub async fn pending_inject_count(&self, ctx: &ContextId) -> usize {
        self.by_context
            .lock()
            .await
            .get(ctx)
            .map(|h| h.pending_injects.len())
            .unwrap_or(0)
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

    #[cfg(test)]
    async fn child_registered(&self, parent: &ContextId, child: &ContextId) -> bool {
        self.children
            .lock()
            .await
            .get(parent)
            .is_some_and(|children| children.contains(child))
    }

    #[cfg(test)]
    async fn child_parent_registered(&self, parent: &ContextId) -> bool {
        self.children.lock().await.contains_key(parent)
    }

    /// Test-only: parks the caller here (between the off-lock resolve/fingerprint and lock #2 in
    /// `checkout_turn_inner`) if a test has armed the gate via `block_next_lock2`; otherwise a no-op.
    /// Mirrors `FakeBackend`'s `block_next_*`/`wait_for_*` gate style (started-counter + `Notify` to
    /// avoid a lost-wakeup race, `oneshot` to resume).
    #[cfg(test)]
    async fn pause_before_lock2(&self) {
        let gate = self.lock2_pause_gate.lock().unwrap().take();
        self.lock2_pause_started_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.lock2_pause_started.notify_waiters();
        if let Some(gate) = gate {
            let _ = gate.await;
        }
    }

    #[cfg(test)]
    fn block_next_lock2(&self) -> tokio::sync::oneshot::Sender<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        *self.lock2_pause_gate.lock().unwrap() = Some(rx);
        tx
    }

    #[cfg(test)]
    async fn wait_for_lock2_pause(&self) {
        while self
            .lock2_pause_started_count
            .load(std::sync::atomic::Ordering::SeqCst)
            == 0
        {
            self.lock2_pause_started.notified().await;
        }
    }

    fn warm_turn_from_handle(
        h: &mut WarmHandle,
        usage_warning: Option<UsageWarning>,
        op: OperationId,
        expiry_intent: WarmExpiryIntent,
        abort: CancellationToken,
    ) -> WarmTurn {
        WarmTurn {
            backend: h.backend.clone(),
            session: h.backend_session.clone(),
            usage_warning,
            generation: h.generation,
            op,
            expiry_intent,
            abort,
            seed: h.pending_seed.take(),
            injects: std::mem::take(&mut h.pending_injects),
            agent: h.agent.clone(),
            model: h.fingerprint.config.model.clone(),
            effort: h
                .fingerprint
                .config
                .effort
                .as_ref()
                .map(|effort| format!("{effort:?}").to_ascii_lowercase()),
            mode: h.fingerprint.config.mode.clone(),
        }
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

    fn mint_turn_op(&self) -> OperationId {
        let n = self
            .turn_op_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        OperationId::parse(format!("turn-{n}")).expect("minted turn op is non-empty")
    }

    fn take_expiry_claim_locked(
        &self,
        tab: &mut SessionTable,
        ctx: &ContextId,
        generation: SessionGeneration,
        op: OperationId,
    ) -> ExpiryClaim {
        let mut handle = tab.remove(ctx).expect("matching warm handle exists");
        fire_lingering_turn_abort(&mut handle);
        let cleanup_claim_id = self
            .cleanup_claim_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        tab.tombstones.insert(
            ctx.clone(),
            ExpiringTombstone {
                generation,
                op: op.clone(),
                cleanup_claim_id,
                state: ExpiringTombstoneState::Expiring,
            },
        );
        ExpiryClaim {
            by_context: self.by_context.clone(),
            children: self.children.clone(),
            ctx: ctx.clone(),
            generation,
            op,
            cleanup_claim_id,
            prune_children: true,
            handle: Some(handle),
            retry: None,
        }
    }

    fn claim_armed_idle_locked(
        &self,
        tab: &mut SessionTable,
        ctx: &ContextId,
    ) -> Option<ExpiryClaim> {
        let handle = tab.get(ctx)?;
        let generation = handle.generation;
        let op = first_armed_idle_expiry(handle)?;
        Some(self.take_expiry_claim_locked(tab, ctx, generation, op))
    }

    fn claim_or_reserve_idle_successor_locked(
        &self,
        tab: &mut SessionTable,
        ctx: &ContextId,
    ) -> Option<ExpiryClaim> {
        let handle = tab.get(ctx)?;
        let generation = handle.generation;
        let op = reserve_idle_successor_or_armed(handle)?;
        Some(self.take_expiry_claim_locked(tab, ctx, generation, op))
    }

    async fn prune_child_registration(&self, ctx: &ContextId) {
        let mut children = self.children.lock().await;
        for set in children.values_mut() {
            set.retain(|c| c != ctx);
        }
        children.retain(|_, set| !set.is_empty());
    }

    fn prune_child_registration_locked(
        children: &mut HashMap<ContextId, HashSet<ContextId>>,
        expired: &[ContextId],
    ) {
        if expired.is_empty() {
            return;
        }
        for set in children.values_mut() {
            set.retain(|c| !expired.contains(c));
        }
        children.retain(|_, set| !set.is_empty());
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
    ) -> Result<WarmTurn, BridgeError> {
        self.checkout_turn_observed(
            ctx,
            agent,
            overrides,
            cwd,
            Arc::new(bridge_core::diagnostics::NoopDiagnosticObserver::default()),
        )
        .await
    }

    pub async fn checkout_turn_observed(
        &self,
        ctx: &ContextId,
        agent: AgentId,
        overrides: Option<AgentOverride>,
        cwd: Option<SessionCwd>,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<WarmTurn, BridgeError> {
        let (res, removed) = self
            .checkout_turn_inner(ctx, agent, overrides, cwd, observer)
            .await;
        for ctx in &removed {
            self.prune_child_registration(ctx).await;
        }
        res
    }

    /// Continue an EXISTING warm context, REUSING its stored fingerprint (agent/config/cwd) rather than
    /// re-deriving it from caller params. This is the `continue` semantic: the caller supplies only the
    /// context (+ input), so there is nothing to reconcile — an Idle handle transitions straight to
    /// Running. Mirrors the no-diff reuse branch of [`Self::checkout_turn`], but a context that was
    /// never minted returns `SessionNotFound` (you cannot continue what does not exist) instead of
    /// minting a fresh session. A retired lease → `SessionExpired`; a busy handle → `HandleBusy`.
    pub async fn checkout_existing_turn(&self, ctx: &ContextId) -> Result<WarmTurn, BridgeError> {
        let mut tab = self.by_context.lock().await;
        if tab.tombstone(ctx).is_some() {
            return Err(BridgeError::SessionExpired);
        }
        let Some(h) = tab.get(ctx) else {
            return Err(BridgeError::SessionNotFound);
        };
        if h.lease.is_retired() {
            return Err(BridgeError::SessionExpired);
        }
        if h.state != SessionState::Idle {
            return Err(BridgeError::HandleBusy);
        }
        if let Some(claim) = self.claim_or_reserve_idle_successor_locked(&mut tab, ctx) {
            drop(tab);
            let _ = claim.into_flight();
            return Err(BridgeError::SessionExpired);
        }
        let h = tab
            .get_mut(ctx)
            .expect("idle successor reservation remains live");
        let usage_warning = self.eval_warn(&h.usage);
        let op = self.mint_turn_op();
        let abort = CancellationToken::new();
        let expiry_intent = WarmExpiryIntent::new();
        h.state = SessionState::Running;
        h.op = Some(op.clone());
        h.turn_aborts.push(TurnAbort {
            op: op.clone(),
            token: abort.clone(),
            expiry_intent: expiry_intent.clone(),
        });
        h.last_used = self.clock.now_instant();
        let seed = h.pending_seed.take();
        let injects = std::mem::take(&mut h.pending_injects);
        Ok(WarmTurn {
            seed,
            injects,
            ..Self::warm_turn_from_handle(h, usage_warning, op, expiry_intent, abort)
        })
    }

    async fn checkout_turn_inner(
        &self,
        ctx: &ContextId,
        agent: AgentId,
        overrides: Option<AgentOverride>,
        cwd: Option<SessionCwd>,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> (Result<WarmTurn, BridgeError>, Vec<ContextId>) {
        // W1-B (verified serialization point): `by_context` used to stay held across BOTH
        // `registry.resolve()` (lazy agent spawn) and `configure_session()` on the fresh path, and
        // across `resolve()` on the existing-handle path — one slow spawn blocked every context's
        // checkout serve-wide. Lock #1 below is precedence-only (today's retired-before-busy order,
        // unchanged) and is dropped before either await. An Idle-or-absent handle tells us nothing more
        // under the lock, so we resolve + fingerprint OFF-lock and re-validate at lock #2 (optimistic
        // re-check) before doing anything stateful.
        //
        // v2.1 (whole-branch review MAJOR, codex xhigh): lock #1 also records whether it observed a live
        // handle at all (`saw_handle`), independent of the busy/retired checks below. Lock #2 uses it to
        // distinguish "no handle at #1, none at #2 either" (proceed fresh, unchanged) from "a handle was
        // here at #1 but is gone at #2" (a concurrent release/reap landed in the off-lock window below —
        // must NOT fresh-mint; see the check at lock #2).
        let mut saw_handle = false;
        {
            let mut tab = self.by_context.lock().await;
            if tab.tombstone(ctx).is_some() {
                return (Err(BridgeError::SessionExpired), Vec::new());
            }
            if let Some(h) = tab.get(ctx) {
                saw_handle = true;
                if h.lease.is_retired() {
                    return (Err(BridgeError::SessionExpired), Vec::new());
                }
                if h.state != SessionState::Idle {
                    // Running / Reconciling / Expiring / Resetting / Compacting / Cancelling /
                    // Configuring all mean the handle is busy.
                    return (Err(BridgeError::HandleBusy), Vec::new());
                }
            }
            if let Some(claim) = self.claim_armed_idle_locked(&mut tab, ctx) {
                drop(tab);
                let _ = claim.into_flight();
                return (Err(BridgeError::SessionExpired), Vec::new());
            }
        }

        let resolved = match self.registry.resolve_observed(&agent, observer).await {
            Ok(resolved) => resolved,
            Err(e) => return (Err(e), Vec::new()),
        };
        let eff = effective_config(&resolved.entry, overrides.as_ref());
        let fp = SessionSpecFingerprint {
            agent: agent.clone(),
            config: eff.clone(),
            cwd: cwd.as_ref().map(|c| c.as_str().to_string()),
        };

        // Test-only deterministic-interleaving hook (regression test for the v2.1 fix below): parks the
        // in-flight checkout here, between the off-lock resolve/fingerprint and lock #2, so a test can
        // force a concurrent release to land in this exact window. No-op unless armed via
        // `block_next_lock2`; compiled out entirely in non-test builds.
        #[cfg(test)]
        self.pause_before_lock2().await;

        // Lock #2: re-validate against CURRENT state — it may have appeared, disappeared, or changed
        // state while we were off-lock. Same guards as lock #1, now correct against fresh state (may
        // now return HandleBusy/SessionExpired where lock #1 saw Idle-or-absent).
        let mut tab = self.by_context.lock().await;
        if tab.tombstone(ctx).is_some() {
            return (Err(BridgeError::SessionExpired), Vec::new());
        }
        if let Some(h) = tab.get_mut(ctx) {
            if h.lease.is_retired() {
                return (Err(BridgeError::SessionExpired), Vec::new());
            }
            if h.state != SessionState::Idle {
                return (Err(BridgeError::HandleBusy), Vec::new());
            }
            // ---- existing-handle path: fingerprint diff -> fast mint, or reconcile/reseed. Unchanged
            // from before the W1-B restructuring except that `resolved`/`eff`/`fp` are precomputed. ----
            let d = h.fingerprint.diff(&fp);
            if d.is_empty() {
                if let Some(expiry_op) = reserve_idle_successor_or_armed(h) {
                    let generation = h.generation;
                    let claim = self.take_expiry_claim_locked(&mut tab, ctx, generation, expiry_op);
                    drop(tab);
                    let _ = claim.into_flight();
                    return (Err(BridgeError::SessionExpired), Vec::new());
                }
                let usage_warning = self.eval_warn(&h.usage);
                let op = self.mint_turn_op();
                let abort = CancellationToken::new();
                let expiry_intent = WarmExpiryIntent::new();
                h.state = SessionState::Running;
                h.op = Some(op.clone());
                h.turn_aborts.push(TurnAbort {
                    op: op.clone(),
                    token: abort.clone(),
                    expiry_intent: expiry_intent.clone(),
                });
                h.last_used = self.clock.now_instant();
                let seed = h.pending_seed.take();
                let injects = std::mem::take(&mut h.pending_injects);
                return (
                    Ok(WarmTurn {
                        seed,
                        injects,
                        ..Self::warm_turn_from_handle(h, usage_warning, op, expiry_intent, abort)
                    }),
                    Vec::new(),
                );
            }
            if d.contains(&"agent") {
                return (
                    Err(BridgeError::ConfigMismatch { field: "agent" }),
                    Vec::new(),
                );
            }
            if d.contains(&"cwd") {
                return (
                    Err(BridgeError::ConfigMismatch { field: "cwd" }),
                    Vec::new(),
                );
            }
            if d.contains(&"mode") {
                return (
                    Err(BridgeError::ConfigReseedRequired { field: "mode" }),
                    Vec::new(),
                );
            }
            if d.contains(&"model") && fp.config.model.is_none() {
                return (
                    Err(BridgeError::ConfigReseedRequired { field: "model" }),
                    Vec::new(),
                );
            }
            if d.contains(&"effort") && fp.config.effort.is_none() {
                return (
                    Err(BridgeError::ConfigReseedRequired { field: "effort" }),
                    Vec::new(),
                );
            }
            let reseed_field = if d.contains(&"model") {
                "model"
            } else {
                "effort"
            };
            if let Some(expiry_op) = reserve_idle_successor_or_armed(h) {
                let generation = h.generation;
                let claim = self.take_expiry_claim_locked(&mut tab, ctx, generation, expiry_op);
                drop(tab);
                let _ = claim.into_flight();
                return (Err(BridgeError::SessionExpired), Vec::new());
            }
            let claimed_id = h.id.clone();
            let backend = h.backend.clone();
            let backend_session = h.backend_session.clone();
            // Claim the handle as Reconciling: a concurrent checkout is now HandleBusy (no ABA re-claim) and
            // cancel/release defer (set expire_after_reconcile) rather than mutate/remove it.
            h.state = SessionState::Reconciling;
            h.expire_after_reconcile = false;
            h.last_used = self.clock.now_instant();
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
                return (Err(BridgeError::SessionExpired), Vec::new());
            }
            let cancelled_or_released = tab
                .get(ctx)
                .map(|h| h.expire_after_reconcile)
                .unwrap_or(true);
            let clean = matches!(outcome, Ok(ReconcileOutcome::Applied)) && !cancelled_or_released;
            if clean {
                let h = tab.get_mut(ctx).expect("still_ours");
                let usage_warning = self.eval_warn(&h.usage);
                let op = self.mint_turn_op();
                let abort = CancellationToken::new();
                let expiry_intent = WarmExpiryIntent::new();
                h.fingerprint = fp;
                h.state = SessionState::Running;
                h.op = Some(op.clone());
                h.turn_aborts.push(TurnAbort {
                    op: op.clone(),
                    token: abort.clone(),
                    expiry_intent: expiry_intent.clone(),
                });
                h.last_used = self.clock.now_instant();
                let seed = h.pending_seed.take();
                let injects = std::mem::take(&mut h.pending_injects);
                return (
                    Ok(WarmTurn {
                        seed,
                        injects,
                        ..Self::warm_turn_from_handle(h, usage_warning, op, expiry_intent, abort)
                    }),
                    Vec::new(),
                );
            }
            // Non-clean (failed reconcile OR cancel/release arrived mid-window): EXPIRE via an `Expiring`
            // tombstone held across release_session().await so a concurrent checkout (HandleBusy on Expiring)
            // can't re-mint the same backend_session id before release completes.
            // cancel-tokens F2 (whole-branch review round 4): a non-clean reconcile EXPIRES (releases) this
            // session; a keep-warm cancel may have left a lingering token on it → fire before release.
            let h = tab.get_mut(ctx).expect("still_ours");
            fire_lingering_turn_abort(h);
            h.state = SessionState::Expiring;
            drop(tab);
            backend.release_session(&backend_session).await;
            {
                let mut tab = self.by_context.lock().await;
                if let Some(h) = tab.remove(ctx) {
                    drop(h.lease);
                }
            }
            return (
                if cancelled_or_released {
                    Err(BridgeError::SessionExpired)
                } else {
                    Err(BridgeError::ConfigReseedRequired {
                        field: reseed_field,
                    })
                },
                vec![ctx.clone()],
            );
        }

        // v2.1 (whole-branch review MAJOR, codex xhigh, fixed here): lock #1 observed a live Idle handle
        // for this ctx, but it is gone now — a concurrent release (or reap) removed it from `by_context`
        // while we were off-lock (registry.resolve + fingerprint, and/or paused above) and is possibly
        // still awaiting `backend.release_session("ctx-{ctx}-g0")` (`release_inner` drops the lock before
        // that await). Falling into the fresh path below would mint that SAME deterministic backend
        // session id and race `configure_session(g0)` against the in-flight `release_session(g0)` —
        // ACP/container backends insert per-session state on configure and remove it on release, so the
        // interleaving can corrupt the new session. Treat this exactly like the caller's implicit target
        // having been released/reaped out from under them: SessionExpired, nothing local to clean up, the
        // caller's retry starts a clean checkout. (The narrower case — lock #1 saw NO handle, i.e. the
        // checkout started entirely after the release completed — still proceeds fresh below; that
        // exposure is pre-existing on main and out of scope here, see the spec addendum.)
        if saw_handle {
            return (Err(BridgeError::SessionExpired), Vec::new());
        }

        // ---- fresh path: no handle exists at lock #2 either, and none was observed at lock #1. Claim it
        // as `Configuring` with a minted claim id stashed in `op` (a claim, NOT a turn op — the real turn
        // op mints at settle below).
        // `capabilities()` is sync so it is set now; NO `turn_abort` is installed (no real turn exists
        // yet). Drop
        // the lock, THEN run the (possibly slow) `configure_session` off-lock. ----
        let n = self.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let backend_session = match SessionId::parse(format!("ctx-{}-g0", ctx.as_str())) {
            Ok(backend_session) => backend_session,
            Err(_) => {
                return (
                    Err(BridgeError::InvalidRequest { field: "contextId" }),
                    Vec::new(),
                )
            }
        };
        let handle_id = SessionHandleId::parse(format!("h-{n}")).unwrap();
        let claim = self.mint_turn_op();
        let caps = resolved.backend.capabilities();
        let backend = resolved.backend.clone();
        tab.insert(
            ctx.clone(),
            WarmHandle {
                id: handle_id.clone(),
                agent,
                backend: resolved.backend,
                backend_session: backend_session.clone(),
                caps,
                generation: SessionGeneration::new(0),
                fingerprint: fp,
                lease: resolved.lease,
                state: SessionState::Configuring,
                usage: UsageSnapshot::default(),
                expire_after_reconcile: false,
                op: Some(claim.clone()),
                turn_aborts: Vec::new(),
                pending_seed: None,
                pending_injects: Vec::new(),
                last_used: self.clock.now_instant(),
            },
        );
        drop(tab);

        let cfg = backend
            .configure_session(&backend_session, &SessionSpec { config: eff, cwd })
            .await;

        // Lock #3 (settle — ALWAYS, success and failure): validate the EXACT claim (same handle
        // identity, still `Configuring`, `op` still the minted claim). A cancel/release/force-clear
        // arriving during the window never touches identity/state/op directly — it only sets
        // `expire_after_reconcile` (the same deferred-expiry flag the other claimed states use, checked
        // separately below) so settle is the sole owner of tearing the claim down. This is what keeps a
        // deferred release from ever racing the `configure_session` call above.
        let mut tab = self.by_context.lock().await;
        let still_ours = matches!(
            tab.get(ctx),
            Some(h) if h.id == handle_id
                && h.state == SessionState::Configuring
                && h.op.as_ref() == Some(&claim)
        );
        if !still_ours {
            // Defensive: the claim invariants (Configuring blocks re-claim/removal; deferred-expiry
            // never mutates identity/state/op) mean this handle should still be exactly ours. Treat it
            // like the flagged-expired case below (nothing local to remove).
            // INVARIANT PIN: this branch must stay unreachable while a successor handle can exist —
            // the release below targets `ctx-{ctx}-g0`, an id a successor fresh checkout REUSES, so
            // reaching here with a live successor would tear down the successor's backend session.
            drop(tab);
            return match cfg {
                Ok(()) => {
                    backend.release_session(&backend_session).await;
                    (Err(BridgeError::SessionExpired), Vec::new())
                }
                Err(e) => (Err(e), Vec::new()),
            };
        }
        let expired = tab
            .get(ctx)
            .map(|h| h.expire_after_reconcile)
            .unwrap_or(true);
        if expired {
            // Deferred-expiry protocol: a cancel/release/force-clear arrived while Configuring. No
            // `turn_abort` exists to fire (none was ever installed). Tombstone (Configuring already
            // blocks re-claim, but this keeps status() honest during the teardown) and drop the lock
            // before the possibly-slow release so unrelated contexts are never blocked by it.
            tab.get_mut(ctx).expect("still_ours").state = SessionState::Expiring;
            drop(tab);
            if cfg.is_ok() {
                backend.release_session(&backend_session).await;
            }
            {
                let mut tab = self.by_context.lock().await;
                if let Some(h) = tab.remove(ctx) {
                    drop(h.lease);
                }
            }
            return (
                match cfg {
                    Ok(()) => Err(BridgeError::SessionExpired),
                    Err(e) => Err(e),
                },
                vec![ctx.clone()],
            );
        }
        match cfg {
            Ok(()) => {
                // Claim intact: transition to Running and mint the REAL turn (a claim id is not a turn
                // op) — this is the only point a `turn_abort` token is installed for a fresh checkout.
                let h = tab.get_mut(ctx).expect("still_ours");
                let op = self.mint_turn_op();
                let abort = CancellationToken::new();
                let expiry_intent = WarmExpiryIntent::new();
                h.state = SessionState::Running;
                h.op = Some(op.clone());
                h.turn_aborts.push(TurnAbort {
                    op: op.clone(),
                    token: abort.clone(),
                    expiry_intent: expiry_intent.clone(),
                });
                h.last_used = self.clock.now_instant();
                let seed = h.pending_seed.take();
                let injects = std::mem::take(&mut h.pending_injects);
                (
                    Ok(WarmTurn {
                        seed,
                        injects,
                        ..Self::warm_turn_from_handle(h, None, op, expiry_intent, abort)
                    }),
                    Vec::new(),
                )
            }
            Err(e) => {
                if let Some(h) = tab.remove(ctx) {
                    drop(h.lease);
                }
                (Err(e), vec![ctx.clone()])
            }
        }
    }

    pub async fn checkout_child_turn(
        &self,
        parent: &ContextId,
        child: &ContextId,
        agent: AgentId,
        overrides: Option<AgentOverride>,
        cwd: Option<SessionCwd>,
    ) -> Result<WarmTurn, BridgeError> {
        self.checkout_child_turn_observed(
            parent,
            child,
            agent,
            overrides,
            cwd,
            Arc::new(bridge_core::diagnostics::NoopDiagnosticObserver::default()),
        )
        .await
    }

    pub async fn checkout_child_turn_observed(
        &self,
        parent: &ContextId,
        child: &ContextId,
        agent: AgentId,
        overrides: Option<AgentOverride>,
        cwd: Option<SessionCwd>,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<WarmTurn, BridgeError> {
        // PFIX-4 (FIX-2 atomicity): hold `children` ACROSS checkout_turn + insert. A concurrent
        // `*_with_children` sweep (Task 4) takes `children` FIRST, so it WAITS for an in-progress child
        // checkout instead of missing it — closes the register-after-release leak window. Lock order is
        // children -> by_context (checkout_turn locks by_context internally); the sweeps use the same order.
        let mut children = self.children.lock().await;
        let (turn, removed) = self
            .checkout_turn_inner(child, agent, overrides, cwd, observer)
            .await;
        Self::prune_child_registration_locked(&mut children, &removed);
        let turn = turn?;
        children
            .entry(parent.clone())
            .or_default()
            .insert(child.clone());
        Ok(turn)
    }

    pub async fn expire_turn(&self, ctx: &ContextId) {
        self.release(ctx).await;
    }

    /// Atomically replace the exact running generation/operation with a
    /// resource-free expiring tombstone and return ownership of all resources.
    /// A stale guard is a no-op.
    pub(crate) async fn begin_expire_current(
        self: &Arc<Self>,
        ctx: &ContextId,
        generation: SessionGeneration,
        op: &OperationId,
    ) -> Option<ExpiryClaim> {
        let mut tab = self.by_context.lock().await;
        if tab.tombstone(ctx).is_some() {
            return None;
        }
        let matches_current = match tab.get_mut(ctx) {
            Some(handle) if handle.generation == generation => {
                if handle.op.as_ref() == Some(op) && handle.state == SessionState::Cancelling {
                    // The cancel flight owns the handle. Make the exact
                    // structured-failure expiry sticky and let that owner
                    // publish the tombstone after backend.cancel settles.
                    handle.expire_after_reconcile = true;
                    return None;
                }
                (handle.op.as_ref() == Some(op) && handle.state == SessionState::Running)
                    || (handle.op.is_none()
                        && handle.state == SessionState::Idle
                        && has_armed_expiry_intent(handle, op))
            }
            _ => false,
        };
        if !matches_current {
            return None;
        }
        Some(self.take_expiry_claim_locked(&mut tab, ctx, generation, op.clone()))
    }

    pub(crate) fn expire_current_unobserved(
        self: Arc<Self>,
        ctx: ContextId,
        generation: SessionGeneration,
        op: OperationId,
    ) {
        tokio::spawn(async move {
            if let Some(claim) = self.begin_expire_current(&ctx, generation, &op).await {
                let _ = claim.cleanup().await;
            }
        });
    }

    /// Mark the current turn finished -> Idle (keep warm). FIX-3: no-op unless this is the SAME generation
    /// and operation AND the handle is Running (a turn only legitimately idles a Running handle); a stale
    /// (reset-away, cancelled, or claim-state) completion touches NOTHING.
    pub async fn finish_turn(&self, ctx: &ContextId, gen: SessionGeneration, op: &OperationId) {
        if let Some(h) = self.by_context.lock().await.get_mut(ctx) {
            if h.generation == gen {
                h.turn_aborts.retain(|abort| abort.op != *op);
            }
            if h.generation == gen && h.op.as_ref() == Some(op) && h.state == SessionState::Running
            {
                h.state = SessionState::Idle;
                h.op = None;
                h.last_used = self.clock.now_instant();
            }
        }
    }

    pub async fn status(&self, ctx: &ContextId) -> Option<SessionStatusInfo> {
        let tab = self.by_context.lock().await;
        if let Some(tombstone) = tab.tombstone(ctx) {
            return Some(SessionStatusInfo {
                state: match tombstone.state {
                    ExpiringTombstoneState::Expiring => "expiring",
                    ExpiringTombstoneState::CleanupFailed { .. } => "cleanup_failed",
                },
                agent: String::new(),
                generation: tombstone.generation.get(),
                idle_age_ms: 0,
                capabilities: AgentSessionCaps::default(),
                usage: UsageSnapshot::default(),
                over_threshold: None,
            });
        }
        tab.get(ctx).map(|h| SessionStatusInfo {
            state: match h.state {
                SessionState::Idle => "idle",
                SessionState::Running => "running",
                SessionState::Reconciling => "reconciling",
                SessionState::Expiring => "expiring",
                SessionState::Resetting => "resetting",
                SessionState::Compacting => "compacting",
                SessionState::Cancelling => "cancelling",
                SessionState::Configuring => "configuring",
            },
            agent: h.agent.as_str().to_string(),
            generation: h.generation.get(),
            idle_age_ms: self
                .clock
                .now_instant()
                .duration_since(h.last_used)
                .as_millis(),
            capabilities: h.caps.clone(),
            usage: h.usage.clone(),
            over_threshold: self.over_threshold(&h.usage),
        })
    }

    /// Record the latest usage snapshot for a warm handle. Partial snapshots merge with the
    /// previous one so terminal token totals cannot erase the context-window gauge. Stamps
    /// `at_ms` from the injected wall clock. FIX-7: does NOT
    /// touch `last_used` (usage during a turn is already covered by Running + finish_turn's
    /// refresh; bumping it here only races reap_idle). No-ops a missing/removed handle. [Slice 2]
    pub async fn record_usage(
        &self,
        ctx: &ContextId,
        gen: SessionGeneration,
        op: &OperationId,
        mut snap: UsageSnapshot,
    ) {
        snap.at_ms = self.clock.now_ms();
        if let Some(h) = self.by_context.lock().await.get_mut(ctx) {
            if h.generation == gen && h.op.as_ref() == Some(op) && h.state == SessionState::Running
            {
                snap.merge_missing_from(&h.usage);
                h.usage = snap;
            }
        }
    }

    pub async fn release(&self, ctx: &ContextId) {
        self.release_inner(ctx, true).await;
        self.prune_child_registration(ctx).await;
    }

    fn claim_release_locked(
        &self,
        tab: &mut SessionTable,
        ctx: &ContextId,
        prune_children: bool,
    ) -> Option<ExpiryClaim> {
        if let Some(tombstone) = tab.tombstone(ctx).cloned() {
            if !matches!(
                tombstone.state,
                ExpiringTombstoneState::CleanupFailed { .. }
            ) {
                return None;
            }
            let retry = tab.cleanup_retries.remove(ctx)?;
            let cleanup_claim_id = self
                .cleanup_claim_seq
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let current = tab
                .tombstones
                .get_mut(ctx)
                .expect("matching cleanup-failed tombstone exists");
            current.cleanup_claim_id = cleanup_claim_id;
            current.state = ExpiringTombstoneState::Expiring;
            return Some(ExpiryClaim {
                by_context: self.by_context.clone(),
                children: self.children.clone(),
                ctx: ctx.clone(),
                generation: tombstone.generation,
                op: tombstone.op,
                cleanup_claim_id,
                prune_children,
                handle: None,
                retry: Some(retry),
            });
        }
        if let Some(handle) = tab.get_mut(ctx) {
            // A reconcile owns the handle: defer teardown to its resolve (don't remove it out from
            // under the in-flight release / let the backend_session id be reused mid-release).
            if is_claimed(handle.state) {
                handle.expire_after_reconcile = true;
                return None;
            }
            if let Some(registry) = &self.perm_registry {
                registry.resolve_context_cancelled(ctx);
            }
            // SessionRelease owns every outstanding operation token before the live handle leaves the
            // table. The cleanup claim then owns the backend session and lease without another await.
            fire_lingering_turn_abort(handle);
        }
        let handle = tab.remove(ctx)?;
        let generation = handle.generation;
        let op = handle.op.clone().unwrap_or_else(|| self.mint_turn_op());
        let cleanup_claim_id = self
            .cleanup_claim_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        tab.tombstones.insert(
            ctx.clone(),
            ExpiringTombstone {
                generation,
                op: op.clone(),
                cleanup_claim_id,
                state: ExpiringTombstoneState::Expiring,
            },
        );
        Some(ExpiryClaim {
            by_context: self.by_context.clone(),
            children: self.children.clone(),
            ctx: ctx.clone(),
            generation,
            op,
            cleanup_claim_id,
            prune_children,
            handle: Some(handle),
            retry: None,
        })
    }

    async fn release_inner(&self, ctx: &ContextId, prune_children: bool) {
        let claim = {
            let mut tab = self.by_context.lock().await;
            self.claim_release_locked(&mut tab, ctx, prune_children)
        };
        if let Some(claim) = claim {
            let _ = claim.cleanup().await;
        }
    }

    pub async fn release_with_children(&self, ctx: &ContextId) {
        let claims = {
            // The established child-checkout order is children -> by_context. Claim the parent and every
            // registered child in that same order before cleanup's first await. Cancellation after this
            // block drops any unjoined claims into their detached flights instead of leaving live children.
            let mut children = self.children.lock().await;
            let snapshot = children.get(ctx).cloned().unwrap_or_default();
            let mut table = self.by_context.lock().await;
            let mut claims = Vec::with_capacity(snapshot.len() + 1);
            if let Some(claim) = self.claim_release_locked(&mut table, ctx, false) {
                claims.push(claim);
            }
            for child in &snapshot {
                if let Some(claim) = self.claim_release_locked(&mut table, child, false) {
                    claims.push(claim);
                }
            }
            children.remove(ctx);
            claims
        };
        for claim in claims {
            let _ = claim.cleanup().await;
        }
    }

    /// Release EVERY warm context (with children). Called on mcp stdin-EOF (FIX-5).
    pub async fn release_all(&self) {
        let ctxs: Vec<ContextId> = self.by_context.lock().await.keys().cloned().collect();
        for c in ctxs {
            self.release_with_children(&c).await;
        }
    }

    /// Cancel an in-flight turn but keep the session warm (-> Idle).
    pub async fn cancel(&self, ctx: &ContextId) -> Result<(), BridgeError> {
        let (res, expired) = self.cancel_inner(ctx, true).await;
        if expired {
            self.prune_child_registration(ctx).await;
        }
        res
    }

    pub(crate) async fn cancel_turn_current(
        &self,
        ctx: &ContextId,
        generation: SessionGeneration,
        op: &OperationId,
    ) -> Result<(), BridgeError> {
        let (res, expired) = self
            .cancel_inner_expected(ctx, Some((generation, op)), true)
            .await;
        if expired {
            self.prune_child_registration(ctx).await;
        }
        res
    }

    async fn cancel_inner(
        &self,
        ctx: &ContextId,
        prune_children: bool,
    ) -> (Result<(), BridgeError>, bool) {
        self.cancel_inner_expected(ctx, None, prune_children).await
    }

    async fn cancel_inner_expected(
        &self,
        ctx: &ContextId,
        expected: Option<(SessionGeneration, &OperationId)>,
        prune_children: bool,
    ) -> (Result<(), BridgeError>, bool) {
        let (backend, session, claimed_id) = {
            let mut tab = self.by_context.lock().await;
            if tab.tombstone(ctx).is_some() {
                return (Err(BridgeError::SessionExpired), false);
            }
            let Some(h) = tab.get_mut(ctx) else {
                return (Err(BridgeError::SessionNotFound), false);
            };
            if expected.is_some_and(|(generation, op)| {
                h.generation != generation
                    || h.op.as_ref() != Some(op)
                    || h.state != SessionState::Running
            }) {
                return (Ok(()), false);
            }
            // A reconcile owns the handle: flag it to expire on resolve rather than resetting to Idle
            // (which would let a third checkout re-claim it under the in-flight reconcile — the ABA bug).
            if h.state == SessionState::Cancelling {
                return (Ok(()), false);
            }
            if is_claimed(h.state) {
                h.expire_after_reconcile = true;
                return (Ok(()), false);
            }
            if let Some(reg) = &self.perm_registry {
                reg.resolve_context_cancelled(ctx);
            }
            let was_running = h.state == SessionState::Running;
            // cancel-tokens (whole-branch review): a keep-warm SessionCancel must NEITHER fire NOR clear
            // the in-flight turn's abort token here.
            //   - Don't FIRE it: the session stays warm, so the producer must reach its prompt to drain the
            //     ACP cancel latch backend.cancel (below) sets; aborting it pre-first-poll would strand that
            //     latch → a spurious cancel of the next turn that mints this session (round-2 Finding 1).
            //   - Don't CLEAR (orphan) it: a still-pre-first-poll producer holds a live token, so the token
            //     must stay reachable on the handle — else a later reset/release sees None, releases the
            //     session, and the orphaned producer re-mints the cleared context (round-3 BLOCKER).
            // So leave it on the handle: the keep-warm success path below retains that operation's token on
            // the Idle handle (a later reset/release can fire it; the next checkout appends its own), and the EXPIRE
            // branch below — which DOES release the session — fires it first.
            if !was_running {
                h.state = SessionState::Idle;
                h.op = None;
                return (Ok(()), false);
            }
            // was_running is necessarily true here (the !was_running case returned above). Claim the
            // handle across backend.cancel so a failed teardown cannot leave it reusable.
            h.state = SessionState::Cancelling;
            h.last_used = self.clock.now_instant();
            (h.backend.clone(), h.backend_session.clone(), h.id.clone())
        };
        let by_context = self.by_context.clone();
        let children = self.children.clone();
        let flight_ctx = ctx.clone();
        let cleanup_claim_id = self
            .cleanup_claim_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let fallback_op = self.mint_turn_op();
        let mut flight = Box::pin(async move {
            let cancel_result = AssertUnwindSafe(backend.cancel(&session))
                .catch_unwind()
                .await
                .unwrap_or_else(|_| Err(BridgeError::agent_crashed("warm cancel task panicked")));

            let mut expired = None;
            let mut gone = false;
            {
                let mut table = by_context.lock().await;
                let still_ours = matches!(
                    table.get(&flight_ctx),
                    Some(handle)
                        if handle.id == claimed_id && handle.state == SessionState::Cancelling
                );
                if still_ours {
                    let deferred = table
                        .get(&flight_ctx)
                        .map(|handle| {
                            handle.expire_after_reconcile
                                || handle
                                    .op
                                    .as_ref()
                                    .is_some_and(|op| has_armed_expiry_intent(handle, op))
                        })
                        .unwrap_or(true);
                    if cancel_result.is_ok() && !deferred {
                        // Keep-warm: leave this operation's abort token on the Idle handle (reachable
                        // for a later reset/release; a next checkout appends its own). The producer
                        // drains the ACP latch.
                        let handle = table.get_mut(&flight_ctx).expect("still_ours");
                        handle.state = SessionState::Idle;
                        handle.op = None;
                    } else if let Some(mut handle) = table.remove(&flight_ctx) {
                        // EXPIRE: cancel failed/panicked or a deferred release landed on the claim.
                        fire_lingering_turn_abort(&mut handle);
                        let generation = handle.generation;
                        let op = handle.op.clone().unwrap_or(fallback_op);
                        table.tombstones.insert(
                            flight_ctx.clone(),
                            ExpiringTombstone {
                                generation,
                                op: op.clone(),
                                cleanup_claim_id,
                                state: ExpiringTombstoneState::Expiring,
                            },
                        );
                        expired = Some(ExpiryClaim {
                            by_context: by_context.clone(),
                            children: children.clone(),
                            ctx: flight_ctx.clone(),
                            generation,
                            op,
                            cleanup_claim_id,
                            prune_children,
                            handle: Some(handle),
                            retry: None,
                        });
                    }
                } else {
                    gone = !table.contains_key(&flight_ctx);
                }
            }
            let expired_handle = expired.is_some();
            let cleanup_result = match expired {
                Some(claim) => claim.cleanup().await.result,
                None => Ok(()),
            };

            let result = match (cancel_result, cleanup_result) {
                (Err(error), _) if gone || expired_handle => Err(error),
                (Err(_), Ok(())) => Ok(()),
                (Err(_), Err(cleanup)) => Err(cleanup),
                (Ok(()), Err(cleanup)) => Err(cleanup),
                (Ok(()), Ok(())) => Ok(()),
            };
            (result, expired_handle)
        });

        // Preserve the legacy no-yield fast path when cancel + settlement are
        // immediately ready. If any part becomes pending, move that same
        // partially-polled future into a task; dropping the report waiter then
        // detaches rather than cancels it.
        let initial_poll = {
            let mut context = Context::from_waker(futures::task::noop_waker_ref());
            flight.as_mut().poll(&mut context)
        };
        match initial_poll {
            Poll::Ready(result) => result,
            Poll::Pending => match tokio::spawn(flight).await {
                Ok(result) => result,
                Err(_) => (
                    Err(BridgeError::agent_crashed("warm cancel flight failed")),
                    false,
                ),
            },
        }
    }

    pub async fn cancel_with_children(&self, ctx: &ContextId) -> Result<(), BridgeError> {
        let mut children = self.children.lock().await;
        let snapshot = children.get(ctx).cloned().unwrap_or_default();
        let mut expired = Vec::new();

        let parent_found = match self.cancel_inner(ctx, false).await {
            (Ok(()), parent_expired) => {
                if parent_expired {
                    expired.push(ctx.clone());
                }
                true
            }
            (Err(BridgeError::SessionNotFound), _) => false,
            (Err(e), parent_expired) => {
                if parent_expired {
                    expired.push(ctx.clone());
                    Self::prune_child_registration_locked(&mut children, &expired);
                }
                return Err(e);
            }
        };
        for child in &snapshot {
            match self.cancel_inner(child, false).await {
                (Ok(()), child_expired) => {
                    if child_expired {
                        expired.push(child.clone());
                    }
                }
                (Err(BridgeError::SessionNotFound), _) => {}
                (Err(e), child_expired) => {
                    if child_expired {
                        expired.push(child.clone());
                    }
                    Self::prune_child_registration_locked(&mut children, &expired);
                    return Err(e);
                }
            }
        }
        Self::prune_child_registration_locked(&mut children, &expired);

        if parent_found || !snapshot.is_empty() {
            Ok(())
        } else {
            Err(BridgeError::SessionNotFound)
        }
    }

    pub async fn clear_with_children(
        &self,
        ctx: &ContextId,
        force: bool,
    ) -> Result<ResetOutcome, BridgeError> {
        let mut children = self.children.lock().await;
        let snapshot = children.get(ctx).cloned().unwrap_or_default();
        let mut expired = Vec::new();

        let (p, parent_expired) = self.reset_session_inner(ctx, ResetOpts { force }).await;
        expired.extend(parent_expired);
        let p = match p {
            Ok(p) => p,
            Err(e) => {
                Self::prune_child_registration_locked(&mut children, &expired);
                return Err(e);
            }
        };
        for child in &snapshot {
            let (res, child_expired) = self.reset_session_inner(child, ResetOpts { force }).await;
            expired.extend(child_expired);
            match res {
                Ok(_) => {}
                Err(e) => {
                    Self::prune_child_registration_locked(&mut children, &expired);
                    return Err(e);
                }
            }
        }
        Self::prune_child_registration_locked(&mut children, &expired);

        Ok(match p {
            ResetOutcome::Cleared { generation } => ResetOutcome::Cleared { generation },
            ResetOutcome::NotFound if !snapshot.is_empty() => {
                ResetOutcome::Cleared { generation: 0 }
            }
            ResetOutcome::NotFound => ResetOutcome::NotFound,
        })
    }

    pub async fn reset_session(
        &self,
        ctx: &ContextId,
        opts: ResetOpts,
    ) -> Result<ResetOutcome, BridgeError> {
        let (res, expired) = self.reset_session_inner(ctx, opts).await;
        for ctx in &expired {
            self.prune_child_registration(ctx).await;
        }
        res
    }

    async fn reset_session_inner(
        &self,
        ctx: &ContextId,
        opts: ResetOpts,
    ) -> (Result<ResetOutcome, BridgeError>, Vec<ContextId>) {
        // A prior checked teardown may have dropped the warm handle while
        // retaining the minimal backend/session retry capability. SessionClear
        // is an explicit retry surface: atomically replace CleanupFailed with a
        // fresh exact claim, then run the same cancellation-safe detached
        // cleanup flight. Do this before the live-handle reset state machine.
        let failed_cleanup_retry = {
            let mut tab = self.by_context.lock().await;
            let generation = tab.tombstone(ctx).and_then(|tombstone| {
                matches!(
                    tombstone.state,
                    ExpiringTombstoneState::CleanupFailed { .. }
                )
                .then_some(tombstone.generation)
            });
            generation.and_then(|generation| {
                self.claim_release_locked(&mut tab, ctx, false)
                    .map(|claim| (claim, generation))
            })
        };
        if let Some((claim, generation)) = failed_cleanup_retry {
            return match claim.cleanup().await.result {
                Ok(()) => (
                    Ok(ResetOutcome::Cleared {
                        generation: generation.get(),
                    }),
                    vec![ctx.clone()],
                ),
                Err(error) => (Err(error), Vec::new()),
            };
        }

        // (1)+(2)+(3) claim under ONE lock hold (FIX-2: never bounce through Idle, never call self.cancel).
        let (backend, old_id, claimed_id, new_gen, new_id, spec) = {
            let mut tab = self.by_context.lock().await;
            let Some(h) = tab.get_mut(ctx) else {
                return (Ok(ResetOutcome::NotFound), Vec::new());
            };
            match h.state {
                SessionState::Idle => {}
                SessionState::Running if opts.force => {}
                SessionState::Configuring if opts.force => {
                    // W1-B deferred-expiry protocol: a Configuring handle's `backend_session` is the
                    // JUST-minted id whose `configure_session` is still running off-lock in
                    // `checkout_turn_inner`. Do NOT take the `Running if force` path above (cancel +
                    // release + reconfigure `old_id`) — that would race `release_session(old_id)`
                    // against the in-flight `configure_session(old_id)`. Instead mark the claim expired
                    // (the same `expire_after_reconcile` flag the other claimed states use) and let
                    // `checkout_turn_inner`'s settle detect it and best-effort release the session once
                    // `configure_session` resolves. Nothing here has actually cleared yet, so report the
                    // handle's current (about-to-be-superseded) generation, mirroring `cancel_inner`'s
                    // "accepted, takes effect asynchronously" semantics for claimed states.
                    let generation = h.generation.get();
                    h.expire_after_reconcile = true;
                    return (Ok(ResetOutcome::Cleared { generation }), Vec::new());
                }
                _ => return (Err(BridgeError::HandleBusy), Vec::new()),
            }
            let backend = h.backend.clone();
            let old_id = h.backend_session.clone();
            let claimed_id = h.id.clone();
            let new_gen = SessionGeneration::new(h.generation.get() + 1);
            let new_id = SessionId::parse(format!("ctx-{}-g{}", ctx.as_str(), new_gen.get()))
                .map_err(|_| BridgeError::InvalidRequest { field: "contextId" });
            let new_id = match new_id {
                Ok(new_id) => new_id,
                Err(e) => return (Err(e), Vec::new()),
            };
            let cwd = match h.fingerprint.cwd.as_deref() {
                Some(s) => match SessionCwd::parse(s) {
                    Ok(cwd) => Some(cwd),
                    Err(_) => {
                        return (
                            Err(BridgeError::ConfigInvalid {
                                reason: "session cwd".into(),
                            }),
                            Vec::new(),
                        )
                    }
                },
                None => None,
            };
            let spec = SessionSpec {
                config: h.fingerprint.config.clone(),
                cwd,
            };
            if let Some(reg) = &self.perm_registry {
                reg.resolve_context_cancelled(ctx);
            }
            // F2 (cancel-tokens): all fallible validation passed — committed to Resetting. Fire the lingering
            // token UNDER the lock (only here, after the fallible new_id/cwd parses, so an early-return error
            // path can't strand it) so the cancel strictly precedes the lock release + backend teardown below.
            fire_lingering_turn_abort(h);
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
            return (Err(BridgeError::SessionExpired), Vec::new());
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
            let expired = {
                let mut tab = self.by_context.lock().await;
                if let Some(h) = tab.remove(ctx) {
                    drop(h.lease);
                    vec![ctx.clone()]
                } else {
                    Vec::new()
                }
            };
            return (
                match cfg {
                    Err(e) => Err(e),
                    Ok(()) => Err(BridgeError::SessionExpired),
                },
                expired,
            );
        }
        let h = tab.get_mut(ctx).expect("still_ours");
        h.backend_session = new_id;
        h.generation = new_gen;
        h.usage = UsageSnapshot::default();
        h.op = None;
        h.turn_aborts.clear();
        h.pending_seed = None;
        h.pending_injects.clear();
        h.state = SessionState::Idle;
        h.last_used = self.clock.now_instant();
        (
            Ok(ResetOutcome::Cleared {
                generation: new_gen.get(),
            }),
            Vec::new(),
        )
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
            if !h.pending_injects.is_empty() {
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
            // cancel-tokens (whole-branch review round 6): compact does NOT fire a lingering keep-warm-cancel
            // token here. Unlike reset/release (which release the ACP entry immediately, so its cancel latch
            // dies with it), compact PROMPTS old_id to SUMMARIZE before releasing it — so firing the lingering
            // token (which makes its pre-mint producer abort WITHOUT draining the ACP cancel latch the earlier
            // cancel set) would let compact's own summarize prompt drain that stale latch and return cancelled,
            // failing the compact. A lingering pre-mint producer racing compact is part of the Slice-9 deferral
            // (see the WarmHandle.turn_aborts note); compact summarizes the warm gen-N session as-is.
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
            drop(tab);
            self.prune_child_registration(ctx).await;
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
        h.turn_aborts.clear();
        h.pending_seed = Some(summary);
        h.state = SessionState::Idle;
        h.last_used = self.clock.now_instant();
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
        {
            let mut tab = self.by_context.lock().await;
            if let Some(h) = tab.remove(ctx) {
                drop(h.lease);
            }
        }
        self.prune_child_registration(ctx).await;
    }

    /// Reap only idle warm sessions past the TTL (never an active turn).
    pub async fn reap_idle(&self) {
        let now = self.clock.now_instant();
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
        let mut claims = Vec::new();
        {
            let mut children = self.children.lock().await;
            let mut tab = self.by_context.lock().await;
            let mut reaped = HashSet::new();
            for c in expired {
                // Re-validate under the lock and REMOVE atomically: only reap a STILL-Idle,
                // STILL-expired handle. A claim that landed after the snapshot
                // (compact/reset/reconcile flips the state off Idle) OWNS the lifecycle — the
                // reaper must SKIP it, never route through `release` (which would set the
                // deferred-expire flag and make the claim's commit tail kill the handle).
                // [whole-branch review]
                let should_reap = matches!(
                    tab.get(&c),
                    Some(h)
                        if h.state == SessionState::Idle
                            && now.duration_since(h.last_used) >= self.idle_ttl
                );
                if should_reap {
                    if let Some(reg) = &self.perm_registry {
                        reg.resolve_context_cancelled(&c);
                    }
                    if let Some(mut handle) = tab.remove(&c) {
                        fire_lingering_turn_abort(&mut handle);
                        let generation = handle.generation;
                        let op = handle.op.clone().unwrap_or_else(|| self.mint_turn_op());
                        let cleanup_claim_id = self
                            .cleanup_claim_seq
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        tab.tombstones.insert(
                            c.clone(),
                            ExpiringTombstone {
                                generation,
                                op: op.clone(),
                                cleanup_claim_id,
                                state: ExpiringTombstoneState::Expiring,
                            },
                        );
                        reaped.insert(c.clone());
                        claims.push(ExpiryClaim {
                            by_context: self.by_context.clone(),
                            children: self.children.clone(),
                            ctx: c,
                            generation,
                            op,
                            cleanup_claim_id,
                            prune_children: true,
                            handle: Some(handle),
                            retry: None,
                        });
                    }
                }
            }
            if !reaped.is_empty() {
                for set in children.values_mut() {
                    set.retain(|child| !reaped.contains(child));
                }
                children.retain(|_, set| !set.is_empty());
            }
        }
        for claim in claims {
            let _ = claim.cleanup().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ManualClock;
    use crate::dispatch::{WarmCompletionExit, WarmCompletionGuard};
    use async_trait::async_trait;
    use bridge_core::diagnostics::{
        DiagnosticFailureClass, DiagnosticPhase, DiagnosticRedactor, FailureDiagnostic,
        FailureDiagnosticInput, FailureDisposition, NoopDiagnosticObserver,
    };
    use bridge_core::domain::{AgentEntry, AgentKind, Effort, InjectMode, Part, RegistrySnapshot};
    use bridge_core::permission::{
        PendingPermissionView, PermKey, PermissionOptionView, PermissionRegistry,
        PermissionResolution,
    };
    use bridge_core::ports::{BackendStream, Resolved, Update};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;
    use tokio::sync::{oneshot, Notify};

    struct NoopLease;
    impl Lease for NoopLease {}

    struct PanickingLease;

    impl Lease for PanickingLease {}

    impl Drop for PanickingLease {
        fn drop(&mut self) {
            panic!("injected lease drop panic");
        }
    }

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
        release_result: StdMutex<Result<(), BridgeError>>,
        release_panics: AtomicBool,
        release_gate: StdMutex<Option<oneshot::Receiver<()>>>,
        release_started: Notify,
        release_started_count: AtomicUsize,
        cancels: StdMutex<Vec<String>>,
        configured: StdMutex<Vec<String>>,
        reconciled: StdMutex<Vec<(String, SessionSpec)>>,
        cancel_result: StdMutex<Result<(), BridgeError>>,
        cancel_panics: AtomicBool,
        cancel_gate: StdMutex<Option<oneshot::Receiver<()>>>,
        cancel_started: Notify,
        cancel_started_count: AtomicUsize,
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
                release_result: StdMutex::new(Ok(())),
                release_panics: AtomicBool::new(false),
                release_gate: StdMutex::new(None),
                release_started: Notify::new(),
                release_started_count: AtomicUsize::new(0),
                cancels: StdMutex::new(Vec::new()),
                configured: StdMutex::new(Vec::new()),
                reconciled: StdMutex::new(Vec::new()),
                cancel_result: StdMutex::new(Ok(())),
                cancel_panics: AtomicBool::new(false),
                cancel_gate: StdMutex::new(None),
                cancel_started: Notify::new(),
                cancel_started_count: AtomicUsize::new(0),
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

        fn set_release_result(&self, result: Result<(), BridgeError>) {
            *self.release_result.lock().unwrap() = result;
        }

        fn set_release_panics(&self) {
            self.release_panics.store(true, Ordering::SeqCst);
        }

        fn block_next_release(&self) -> oneshot::Sender<()> {
            let (tx, rx) = oneshot::channel();
            *self.release_gate.lock().unwrap() = Some(rx);
            tx
        }

        async fn wait_for_release(&self) {
            while self.release_started_count.load(Ordering::SeqCst) == 0 {
                self.release_started.notified().await;
            }
        }

        async fn wait_for_release_count(&self, expected: usize) {
            while self.release_started_count.load(Ordering::SeqCst) < expected {
                let started = self.release_started.notified();
                if self.release_started_count.load(Ordering::SeqCst) < expected {
                    started.await;
                }
            }
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

        fn set_cancel_result(&self, result: Result<(), BridgeError>) {
            *self.cancel_result.lock().unwrap() = result;
        }

        fn set_cancel_panics(&self) {
            self.cancel_panics.store(true, Ordering::SeqCst);
        }

        fn block_next_cancel(&self) -> oneshot::Sender<()> {
            let (tx, rx) = oneshot::channel();
            *self.cancel_gate.lock().unwrap() = Some(rx);
            tx
        }

        async fn wait_for_cancel(&self) {
            while self.cancel_started_count.load(Ordering::SeqCst) == 0 {
                self.cancel_started.notified().await;
            }
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
            let gate = self.cancel_gate.lock().unwrap().take();
            self.cancel_started_count.fetch_add(1, Ordering::SeqCst);
            self.cancel_started.notify_waiters();
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            assert!(
                !self.cancel_panics.load(Ordering::SeqCst),
                "injected cancel panic"
            );
            self.cancel_result.lock().unwrap().clone()
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
            let gate = self.release_gate.lock().unwrap().take();
            self.release_started_count.fetch_add(1, Ordering::SeqCst);
            self.release_started.notify_waiters();
            if let Some(gate) = gate {
                let _ = gate.await;
            }
        }

        async fn release_session_checked(&self, session: &SessionId) -> Result<(), BridgeError> {
            self.release_session(session).await;
            assert!(
                !self.release_panics.load(Ordering::SeqCst),
                "injected cleanup panic"
            );
            self.release_result.lock().unwrap().clone()
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
        panic_next_lease_drop: AtomicBool,
        observed: StdMutex<Vec<Arc<dyn DiagnosticObserver>>>,
    }

    impl FakeRegistry {
        fn new(entry: AgentEntry, backend: Arc<FakeBackend>) -> Self {
            Self {
                entries: vec![entry],
                backend,
                retired: Arc::new(AtomicBool::new(false)),
                panic_next_lease_drop: AtomicBool::new(false),
                observed: StdMutex::new(Vec::new()),
            }
        }

        fn with_entries(entries: Vec<AgentEntry>, backend: Arc<FakeBackend>) -> Self {
            Self {
                entries,
                backend,
                retired: Arc::new(AtomicBool::new(false)),
                panic_next_lease_drop: AtomicBool::new(false),
                observed: StdMutex::new(Vec::new()),
            }
        }

        fn retire(&self) {
            self.retired.store(true, Ordering::SeqCst);
        }

        fn panic_next_lease_drop(&self) {
            self.panic_next_lease_drop.store(true, Ordering::SeqCst);
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
            let lease: Box<dyn Lease> = if self.panic_next_lease_drop.swap(false, Ordering::SeqCst)
            {
                Box::new(PanickingLease)
            } else {
                Box::new(RetiringLease {
                    retired: self.retired.clone(),
                })
            };
            Ok(Resolved {
                entry: Arc::new(entry.clone()),
                backend: self.backend.clone(),
                lease,
            })
        }

        async fn resolve_observed(
            &self,
            id: &AgentId,
            observer: Arc<dyn DiagnosticObserver>,
        ) -> Result<Resolved, BridgeError> {
            self.observed.lock().unwrap().push(observer);
            self.resolve(id).await
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
            watchdog: None,
            mcp: vec![],
            mcp_delivery: Default::default(),
            auth_method: None,
            pre_authenticated: false,
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

    fn structured_failure(class: DiagnosticFailureClass) -> BridgeError {
        BridgeError::agent_failure(
            FailureDiagnostic::build_static_code(
                FailureDiagnosticInput {
                    failed_phase: DiagnosticPhase::PromptStart,
                    last_completed_phase: Some(DiagnosticPhase::SessionCreate),
                    class,
                    disposition: FailureDisposition::Fatal,
                    code: "ignored".to_owned(),
                    summary: "bounded test failure".to_owned(),
                    causes: Vec::new(),
                    stderr_observed: false,
                    stderr_line_count: 0,
                    stderr_scope: None,
                    stderr_tail: None,
                    stderr_redaction: None,
                    retry_after_ms: None,
                    reset_at_ms: None,
                    prompt_may_have_been_accepted: true,
                },
                "test.warm.failure",
                &DiagnosticRedactor::default(),
            )
            .unwrap(),
        )
    }

    fn pkey(
        context_id: &ContextId,
        generation: SessionGeneration,
        op: &OperationId,
        request_id: &str,
    ) -> PermKey {
        PermKey {
            context_id: context_id.clone(),
            generation: generation.get(),
            op: op.clone(),
            request_id: request_id.into(),
        }
    }

    fn permission_view(
        request_id: &str,
        generation: SessionGeneration,
        op: &OperationId,
    ) -> PendingPermissionView {
        PendingPermissionView {
            request_id: request_id.into(),
            tool_call_id: format!("tool-{request_id}"),
            generation: generation.get(),
            op: op.clone(),
            title: "permission".into(),
            options: vec![PermissionOptionView {
                option_id: "approved".into(),
                name: "Allow".into(),
                kind: "allow_once".into(),
            }],
            raw_input: None,
            timeout_ms: 120_000,
        }
    }

    async fn assert_permission_cancelled(rx: oneshot::Receiver<PermissionResolution>) {
        let resolved = tokio::time::timeout(Duration::from_secs(1), rx)
            .await
            .expect("pending permission should resolve promptly")
            .expect("permission sender should still be live");
        assert!(matches!(resolved, PermissionResolution::Cancelled));
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

    #[derive(Default)]
    struct MarkerDiagnostic;

    #[async_trait]
    impl DiagnosticObserver for MarkerDiagnostic {
        async fn record(
            &self,
            _event: bridge_core::diagnostics::DiagnosticEvent,
        ) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct PendingDiagnostic {
        entered_count: AtomicUsize,
        entered: Notify,
    }

    impl PendingDiagnostic {
        async fn wait_until_entered(&self) {
            while self.entered_count.load(Ordering::SeqCst) == 0 {
                let entered = self.entered.notified();
                if self.entered_count.load(Ordering::SeqCst) == 0 {
                    entered.await;
                }
            }
        }
    }

    #[async_trait]
    impl DiagnosticObserver for PendingDiagnostic {
        async fn record(
            &self,
            _event: bridge_core::diagnostics::DiagnosticEvent,
        ) -> Result<(), BridgeError> {
            self.entered_count.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_waiters();
            std::future::pending().await
        }
    }

    struct RejectingDiagnostic;

    #[async_trait]
    impl DiagnosticObserver for RejectingDiagnostic {
        async fn record(
            &self,
            _event: bridge_core::diagnostics::DiagnosticEvent,
        ) -> Result<(), BridgeError> {
            Err(BridgeError::StoreFailure)
        }
    }

    #[test]
    fn is_claimed_includes_compacting() {
        assert!(super::is_claimed(super::SessionState::Compacting));
        assert!(super::is_claimed(super::SessionState::Cancelling));
    }

    #[test]
    fn expiry_intent_linearizes_failure_against_successor_reservation() {
        let failure_first = WarmExpiryIntent::new();
        failure_first.arm();
        assert!(!failure_first.reserve_successor());
        assert!(failure_first.is_armed());

        let successor_first = WarmExpiryIntent::new();
        assert!(successor_first.reserve_successor());
        successor_first.arm();
        assert!(successor_first.reserve_successor());
        assert!(
            !successor_first.is_armed(),
            "a stale completion cannot arm expiry after successor admission linearizes first"
        );
    }

    #[tokio::test]
    async fn observed_checkout_forwards_exact_observer_to_registry_resolution() {
        let (manager, _backend, registry) = manager();
        let observer: Arc<dyn DiagnosticObserver> = Arc::new(MarkerDiagnostic);

        let turn = manager
            .checkout_turn_observed(
                &ctx("observed-checkout"),
                agent(),
                None,
                None,
                observer.clone(),
            )
            .await
            .unwrap();

        let observed = registry.observed.lock().unwrap();
        assert_eq!(observed.len(), 1);
        assert!(Arc::ptr_eq(&observed[0], &observer));
        drop(turn);
    }

    #[tokio::test]
    async fn observed_child_checkout_forwards_exact_observer_before_registration() {
        let (manager, _backend, registry) = manager();
        let parent = ctx("observed-parent");
        let child = ctx("observed-child");
        let observer: Arc<dyn DiagnosticObserver> = Arc::new(MarkerDiagnostic);

        let turn = manager
            .checkout_child_turn_observed(&parent, &child, agent(), None, None, observer.clone())
            .await
            .unwrap();

        {
            let observed = registry.observed.lock().unwrap();
            assert_eq!(observed.len(), 1);
            assert!(Arc::ptr_eq(&observed[0], &observer));
        }
        assert!(manager.child_registered(&parent, &child).await);
        drop(turn);
    }

    #[tokio::test]
    async fn checkout_mints_unique_op_nonce_per_turn() {
        let (mgr, _backend, _registry) = manager();
        let c = ctx("ctx-nonce");
        let t1 = mgr.checkout_turn(&c, agent(), None, None).await.unwrap();
        let old_op = t1.op.clone();
        mgr.finish_turn(&c, t1.generation, &old_op).await;
        let t2 = mgr.checkout_turn(&c, agent(), None, None).await.unwrap();
        assert_ne!(old_op, t2.op, "each checkout mints a distinct op nonce");
        mgr.finish_turn(&c, t2.generation, &old_op).await;
        assert_eq!(mgr.status(&c).await.unwrap().state, "running");
    }

    #[tokio::test]
    async fn reset_on_idle_bumps_generation_releases_old_configures_new_zeroes_usage() {
        let (manager, backend, _r) = manager();
        let c = ctx("reset");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        manager
            .record_usage(
                &c,
                turn.generation,
                &turn.op,
                UsageSnapshot {
                    used: Some(7),
                    size: Some(9),
                    cost: None,
                    terminal: None,
                    at_ms: 0,
                },
            )
            .await;
        manager.finish_turn(&c, turn.generation, &turn.op).await;
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
            .checkout_turn(&c, agent(), None, None)
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
        let turn = m.checkout_turn(&c, agent(), None, None).await.unwrap();
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
        let _turn = m.checkout_turn(&c, agent(), None, None).await.unwrap(); // Running
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
            let turn = m.checkout_turn(&c, agent(), None, None).await.unwrap();
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
        let turn = m.checkout_turn(&c, agent(), None, None).await.unwrap();
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
        let turn = m.checkout_turn(&c, agent(), None, None).await.unwrap();
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
        let turn = m.checkout_turn(&c, agent(), None, None).await.unwrap(); // configures g0 OK
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
    async fn compact_configure_failure_prunes_child_registration() {
        let (m, fake, _r) = manager();
        let parent = ctx("compact-configure-parent");
        let child = ctx("compact-configure-child");
        let turn = m
            .checkout_child_turn(&parent, &child, agent(), None, None)
            .await
            .unwrap();
        m.finish_turn(&child, turn.generation, &turn.op).await;
        assert!(m.child_registered(&parent, &child).await);

        fake.set_configure_result(Err(BridgeError::ConfigInvalid {
            reason: "test".into(),
        }));
        let err = m
            .compact_session(&child, |_b, _s| async { Ok("good summary".to_string()) })
            .await
            .unwrap_err();

        assert!(matches!(err, BridgeError::ConfigInvalid { .. }));
        assert!(m.status(&child).await.is_none());
        assert!(!m.child_registered(&parent, &child).await);
        assert!(!m.child_parent_registered(&parent).await);
    }

    #[tokio::test]
    async fn checkout_consumes_seed_once() {
        let (m, _f, _r) = manager();
        let c = ctx("c");
        let t = m.checkout_turn(&c, agent(), None, None).await.unwrap();
        m.finish_turn(&c, t.generation, &t.op).await;
        m.compact_session(&c, |_b, _s| async { Ok("SUMMARY".into()) })
            .await
            .unwrap();
        // First checkout after compact carries the seed; clear it; second sees None.
        let t1 = m.checkout_turn(&c, agent(), None, None).await.unwrap();
        assert_eq!(t1.seed.as_deref(), Some("SUMMARY"));
        m.finish_turn(&c, t1.generation, &t1.op).await;
        let t2 = m.checkout_turn(&c, agent(), None, None).await.unwrap();
        assert_eq!(t2.seed, None);
    }

    #[tokio::test]
    async fn seed_delivered_on_reconcile_checkout() {
        // FIX-10: the seed is ALSO taken at the post-reconcile clean resume return (:261-275), not only clean-diff.
        // Mirror the clean-reconcile setup in `model_override_change_reconciles_and_advances_fingerprint` (:1277).
        let (m, fake, _r) = manager();
        let c = ctx("c");
        let t = m.checkout_turn(&c, agent(), None, None).await.unwrap();
        m.finish_turn(&c, t.generation, &t.op).await;
        m.compact_session(&c, |_b, _s| async { Ok("SUMMARY".into()) })
            .await
            .unwrap();
        // A model-override checkout takes the reconcile path; the seed must still be delivered.
        let t1 = m
            .checkout_turn(&c, agent(), Some(model_override("m1")), None)
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
        let t = m.checkout_turn(&c, agent(), None, None).await.unwrap();
        m.finish_turn(&c, t.generation, &t.op).await;
        m.compact_session(&c, |_b, _s| async { Ok("SUMMARY".into()) })
            .await
            .unwrap();
        m.reset_session(&c, ResetOpts { force: false })
            .await
            .unwrap(); // plain clear after compact
        let t1 = m.checkout_turn(&c, agent(), None, None).await.unwrap();
        assert_eq!(t1.seed, None, "clear drops the pending seed");
    }

    #[tokio::test]
    async fn inject_queues_and_drains_once_fifo() {
        let (m, _b, _r) = manager();
        let c = ctx("inject-once");
        let turn = m.checkout_turn(&c, agent(), None, None).await.unwrap();
        m.finish_turn(&c, turn.generation, &turn.op).await;

        assert_eq!(
            m.inject(InjectRequest {
                context: c.clone(),
                text: "A".into(),
                mode: InjectMode::PrependNextTurn,
                dedupe_key: None,
            })
            .await
            .unwrap(),
            1
        );
        assert_eq!(
            m.inject(InjectRequest {
                context: c.clone(),
                text: "B".into(),
                mode: InjectMode::AppendNextTurn,
                dedupe_key: None,
            })
            .await
            .unwrap(),
            2
        );

        let turn = m.checkout_existing_turn(&c).await.unwrap();
        assert_eq!(
            turn.injects
                .iter()
                .map(|i| (i.text.as_str(), i.mode))
                .collect::<Vec<_>>(),
            vec![
                ("A", InjectMode::PrependNextTurn),
                ("B", InjectMode::AppendNextTurn)
            ]
        );
        m.finish_turn(&c, turn.generation, &turn.op).await;

        let turn = m.checkout_existing_turn(&c).await.unwrap();
        assert!(turn.injects.is_empty());
    }

    #[tokio::test]
    async fn inject_dedupe_replaces_in_place() {
        let (m, _b, _r) = manager();
        let c = ctx("inject-dedupe");
        let turn = m.checkout_turn(&c, agent(), None, None).await.unwrap();
        m.finish_turn(&c, turn.generation, &turn.op).await;

        m.inject(InjectRequest {
            context: c.clone(),
            text: "first".into(),
            mode: InjectMode::PrependNextTurn,
            dedupe_key: Some("same".into()),
        })
        .await
        .unwrap();
        m.inject(InjectRequest {
            context: c.clone(),
            text: "second".into(),
            mode: InjectMode::AppendNextTurn,
            dedupe_key: Some("same".into()),
        })
        .await
        .unwrap();

        assert_eq!(m.pending_inject_count(&c).await, 1);
        let turn = m.checkout_existing_turn(&c).await.unwrap();
        assert_eq!(turn.injects[0].text, "second");
        assert_eq!(turn.injects[0].mode, InjectMode::AppendNextTurn);
    }

    #[tokio::test]
    async fn inject_absent_ctx_is_session_not_found() {
        let (m, _b, _r) = manager();
        let err = m
            .inject(InjectRequest {
                context: ctx("missing"),
                text: "x".into(),
                mode: InjectMode::PrependNextTurn,
                dedupe_key: None,
            })
            .await
            .unwrap_err();
        assert_eq!(err, BridgeError::SessionNotFound);
    }

    #[tokio::test]
    async fn inject_cap_rejects_beyond_limit() {
        let (m, _b, _r) = manager();
        let c = ctx("inject-cap");
        let turn = m.checkout_turn(&c, agent(), None, None).await.unwrap();
        m.finish_turn(&c, turn.generation, &turn.op).await;

        for n in 0..32 {
            m.inject(InjectRequest {
                context: c.clone(),
                text: format!("x{n}"),
                mode: InjectMode::PrependNextTurn,
                dedupe_key: None,
            })
            .await
            .unwrap();
        }
        let err = m
            .inject(InjectRequest {
                context: c.clone(),
                text: "too many".into(),
                mode: InjectMode::PrependNextTurn,
                dedupe_key: None,
            })
            .await
            .unwrap_err();
        assert_eq!(err, BridgeError::HandleBusy);

        let c = ctx("inject-byte-cap");
        let turn = m.checkout_turn(&c, agent(), None, None).await.unwrap();
        m.finish_turn(&c, turn.generation, &turn.op).await;
        let err = m
            .inject(InjectRequest {
                context: c,
                text: "x".repeat(64 * 1024 + 1),
                mode: InjectMode::AppendNextTurn,
                dedupe_key: None,
            })
            .await
            .unwrap_err();
        assert_eq!(err, BridgeError::HandleBusy);
    }

    #[tokio::test]
    async fn clear_drops_injects() {
        let (m, _b, _r) = manager();
        let c = ctx("inject-clear");
        let turn = m.checkout_turn(&c, agent(), None, None).await.unwrap();
        m.finish_turn(&c, turn.generation, &turn.op).await;
        m.inject(InjectRequest {
            context: c.clone(),
            text: "drop me".into(),
            mode: InjectMode::PrependNextTurn,
            dedupe_key: None,
        })
        .await
        .unwrap();

        m.reset_session(&c, ResetOpts { force: false })
            .await
            .unwrap();
        let turn = m.checkout_existing_turn(&c).await.unwrap();
        assert!(turn.injects.is_empty());
    }

    #[tokio::test]
    async fn compact_rejects_while_injects_pending() {
        let (m, _b, _r) = manager();
        let c = ctx("inject-compact");
        let turn = m.checkout_turn(&c, agent(), None, None).await.unwrap();
        m.finish_turn(&c, turn.generation, &turn.op).await;
        m.inject(InjectRequest {
            context: c.clone(),
            text: "pending".into(),
            mode: InjectMode::PrependNextTurn,
            dedupe_key: None,
        })
        .await
        .unwrap();

        let err = m
            .compact_session(&c, |_b, _s| async { Ok("summary".into()) })
            .await
            .unwrap_err();
        assert_eq!(err, BridgeError::HandleBusy);
    }

    #[tokio::test]
    async fn stale_completion_during_resetting_window_is_dropped() {
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend.clone()));
        let manager = Arc::new(SessionManager::new(registry, Duration::from_secs(30)));
        let c = ctx("reset-window");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        manager
            .record_usage(
                &c,
                turn.generation,
                &turn.op,
                UsageSnapshot {
                    used: Some(7),
                    size: Some(9),
                    cost: None,
                    terminal: None,
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
        manager.finish_turn(&c, turn.generation, &turn.op).await;
        manager
            .record_usage(
                &c,
                turn.generation,
                &turn.op,
                UsageSnapshot {
                    used: Some(99),
                    size: Some(100),
                    cost: None,
                    terminal: None,
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
    async fn force_reset_cancels_the_inflight_turn_abort() {
        // cancel-tokens F2: a force-reset of a Running handle must cancel the in-flight turn's abort token
        // (the producer's biased select then aborts before/instead of prompting the released session).
        let (manager, _backend, _registry) = manager();
        let c = ctx("ctx-abort");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        assert!(!turn.abort.is_cancelled());
        let out = manager
            .reset_session(&c, ResetOpts { force: true })
            .await
            .unwrap();
        assert!(matches!(out, ResetOutcome::Cleared { generation: 1 }));
        assert!(
            turn.abort.is_cancelled(),
            "force reset must cancel the in-flight turn's abort token"
        );
    }

    #[tokio::test]
    async fn force_reset_cancels_token_before_backend_teardown() {
        // cancel-tokens F2 ordering invariant (whole-branch review): the in-flight turn's abort token must
        // be cancelled BEFORE the backend session is torn down, so a producer can never poll a released
        // session with an un-cancelled token (the ACP lazy-re-mint / resurrection window). On a force reset
        // `backend.cancel(old)` is the first teardown step; gate it, and when it is entered the token must
        // ALREADY be cancelled. Guards against any future regression that moves the cancel past the lock.
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend.clone()));
        let manager = Arc::new(SessionManager::new(registry, Duration::from_secs(30)));
        let c = ctx("ctx-reset-order");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        assert!(!turn.abort.is_cancelled());
        let release = backend.block_next_cancel();
        let reset = tokio::spawn({
            let manager = manager.clone();
            let c = c.clone();
            async move { manager.reset_session(&c, ResetOpts { force: true }).await }
        });
        // The reset has reached backend.cancel — which runs AFTER the under-lock token cancel.
        backend.wait_for_cancel().await;
        assert!(
            turn.abort.is_cancelled(),
            "the abort token must be cancelled before the backend session is torn down"
        );
        let _ = release.send(());
        let out = reset.await.unwrap().unwrap();
        assert!(matches!(out, ResetOutcome::Cleared { generation: 1 }));
    }

    #[tokio::test]
    async fn release_cancels_the_inflight_turn_abort() {
        // cancel-tokens F2 (whole-branch review, Finding 2): SessionRelease releases the backend session,
        // so — like a force-reset — it must cancel the in-flight turn's abort token (the producer aborts
        // instead of re-minting the just-released session). A release is one of the two paths that fire it.
        let (manager, _backend, _registry) = manager();
        let c = ctx("ctx-release-abort");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        assert!(!turn.abort.is_cancelled());
        manager.release(&c).await;
        assert!(
            turn.abort.is_cancelled(),
            "releasing a running handle must cancel the in-flight turn's abort token"
        );
        assert!(
            manager.status(&c).await.is_none(),
            "release removes the handle"
        );
    }

    #[tokio::test]
    async fn session_cancel_does_not_fire_the_turn_abort_token() {
        // cancel-tokens (whole-branch review, round-2 Finding 1): a keep-warm SessionCancel must NOT fire
        // the abort token — the producer must reach its prompt to drain the ACP cancel latch backend.cancel
        // sets; firing it pre-first-poll would strand that latch and spuriously cancel the NEXT turn.
        let (manager, _backend, _registry) = manager();
        let c = ctx("ctx-cancel-nofire");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        manager.cancel(&c).await.unwrap();
        assert!(
            !turn.abort.is_cancelled(),
            "a keep-warm cancel must NOT fire the abort token (it would strand the ACP cancel latch)"
        );
        // The handle stays warm at Idle and a fresh checkout mints a new turn (new op nonce + token).
        let next = manager.checkout_existing_turn(&c).await.unwrap();
        assert_ne!(next.op, turn.op);
    }

    #[tokio::test]
    async fn cancel_keep_warm_leaves_token_reachable_for_release() {
        // cancel-tokens (whole-branch review, round-3 BLOCKER): a keep-warm cancel does not fire the token,
        // but it must NOT orphan it either — a still-pre-first-poll producer keeps a live token, so the
        // token stays on the Idle handle and a SUBSEQUENT reset/release can still fire it. Otherwise the
        // cancel→release sequence would release the session out from under an un-cancellable producer that
        // then re-mints the cleared context.
        let (manager, _backend, _registry) = manager();
        let c = ctx("ctx-cancel-then-release");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        manager.cancel(&c).await.unwrap();
        assert!(!turn.abort.is_cancelled(), "keep-warm cancel does not fire");
        // The token lingers on the Idle handle → a following release fires it (no orphan, no re-mint).
        manager.release(&c).await;
        assert!(
            turn.abort.is_cancelled(),
            "a release after a keep-warm cancel must fire the lingering abort token"
        );
    }

    #[tokio::test]
    async fn cancel_a_checkout_b_then_release_fires_both_operation_tokens() {
        let (manager, _backend, _registry) = manager();
        let c = ctx("ctx-cancel-a-checkout-b-release");
        let first = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        manager.cancel(&c).await.unwrap();
        let second = manager.checkout_existing_turn(&c).await.unwrap();

        assert!(!first.abort.is_cancelled());
        assert!(!second.abort.is_cancelled());
        manager.release(&c).await;
        assert!(
            first.abort.is_cancelled(),
            "checkout B must not overwrite the lingering producer-A abort token"
        );
        assert!(second.abort.is_cancelled());
        assert!(manager.status(&c).await.is_none());
    }

    #[tokio::test]
    async fn cancel_expire_fires_the_turn_abort_token() {
        // cancel-tokens (whole-branch review, round-3): when backend.cancel FAILS, cancel_inner EXPIRES —
        // it releases the session. That release path must fire the in-flight turn's abort token (like a
        // force-reset) so a pre-first-poll producer aborts instead of re-minting the released session.
        let (manager, backend, _registry) = manager();
        let c = ctx("ctx-cancel-expire");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        backend.set_cancel_result(Err(BridgeError::AgentCrashed {
            reason: "cancel failed".into(),
        }));
        let _ = manager.cancel(&c).await;
        assert!(
            turn.abort.is_cancelled(),
            "an expiring (releasing) cancel must fire the in-flight turn's abort token"
        );
        assert!(
            manager.status(&c).await.is_none(),
            "an expiring cancel removes the handle"
        );
    }

    #[tokio::test]
    async fn compact_after_keep_warm_cancel_does_not_fire_lingering_token() {
        // cancel-tokens (whole-branch review round 6): compact must NOT fire a lingering keep-warm-cancel
        // token — it prompts old_id (summarize) before releasing it, so firing would make the pre-mint
        // producer abort without draining the ACP cancel latch, and compact's own summarize would then drain
        // that stale latch and come back cancelled. (A lingering pre-mint producer racing compact is the
        // Slice-9 deferral.) Here the FakeBackend has no latch, so compact still succeeds; we pin that compact
        // leaves the lingering token untouched (does NOT fire it).
        let (manager, _backend, _registry) = manager();
        let c = ctx("ctx-compact-nofire");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        manager.cancel(&c).await.unwrap();
        assert!(!turn.abort.is_cancelled(), "keep-warm cancel does not fire");
        let out = manager
            .compact_session(&c, |_b, _s| async { Ok("THE SUMMARY".to_string()) })
            .await
            .unwrap();
        assert!(matches!(out, ResetOutcome::Cleared { generation: 1 }));
        assert!(
            !turn.abort.is_cancelled(),
            "compact must NOT fire a lingering token (it would latch-poison its own summarize prompt)"
        );
    }

    #[tokio::test]
    async fn reap_after_keep_warm_cancel_fires_lingering_token() {
        // cancel-tokens F2 (whole-branch review round 4): a reaped Idle handle that carries a lingering
        // keep-warm-cancel token must have it fired before the reaper releases the backend session.
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend));
        let clock = Arc::new(ManualClock::new(0));
        let manager =
            SessionManager::new_with_clock(registry, Duration::from_secs(5), clock.clone());
        let c = ctx("ctx-reap-fires");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        manager.cancel(&c).await.unwrap();
        assert!(!turn.abort.is_cancelled());
        clock.advance(Duration::from_secs(6));
        manager.reap_idle().await;
        assert!(manager.status(&c).await.is_none(), "reaped past TTL");
        assert!(
            turn.abort.is_cancelled(),
            "reap must fire the lingering keep-warm-cancel token before releasing"
        );
    }

    #[tokio::test]
    async fn continue_after_force_clear_uses_new_empty_generation() {
        // cancel-tokens DoD / SPEC-FIX-7: a force-clear of a Running turn leaves the context at a NEW empty
        // generation (Idle) — a subsequent continue (checkout_existing_turn) SUCCEEDS at the new generation,
        // it does NOT return SessionNotFound (which is only for a truly unknown context).
        let (manager, _backend, _registry) = manager();
        let c = ctx("ctx-reclear");
        let first = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        assert_eq!(first.generation.get(), 0);
        let out = manager
            .reset_session(&c, ResetOpts { force: true })
            .await
            .unwrap();
        assert!(matches!(out, ResetOutcome::Cleared { generation: 1 }));
        let next = manager.checkout_existing_turn(&c).await.unwrap();
        assert_eq!(next.generation.get(), 1, "continue uses the new generation");
        assert_ne!(next.op, first.op, "the new turn mints a fresh op nonce");
    }

    #[tokio::test]
    async fn reset_configure_failure_expires_handle_and_returns_error() {
        let (manager, backend, _r) = manager();
        let c = ctx("reset-cfg-fail");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        manager.finish_turn(&c, turn.generation, &turn.op).await;
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
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        manager.finish_turn(&c, turn.generation, &turn.op).await;
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
            .checkout_turn(&c, agent(), None, None)
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
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        manager.finish_turn(&c, turn.generation, &turn.op).await;
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
            .checkout_turn(&c, agent(), None, None)
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
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        manager.finish_turn(&c, turn.generation, &turn.op).await;
        assert_eq!(manager.status(&c).await.unwrap().state, "idle");
    }

    #[tokio::test]
    async fn finish_turn_noops_on_stale_generation() {
        let (manager, _b, _r) = manager();
        let c = ctx("ft-stale");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        manager
            .finish_turn(&c, SessionGeneration::new(99), &turn.op)
            .await;
        assert_eq!(manager.status(&c).await.unwrap().state, "running");
    }

    #[tokio::test]
    async fn expiry_claim_blocks_checkout_and_cleanup_survives_waiter_cancellation() {
        let (manager, backend, _registry) = manager();
        let manager = Arc::new(manager);
        let c = ctx("expiry-detach");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        let allow_release = backend.block_next_release();
        let claim = manager
            .begin_expire_current(&c, turn.generation, &turn.op)
            .await
            .expect("matching running turn is claimed");

        assert_eq!(manager.status(&c).await.unwrap().state, "expiring");
        assert_eq!(
            manager
                .checkout_turn(&c, agent(), None, None)
                .await
                .err()
                .unwrap(),
            BridgeError::SessionExpired,
            "the tombstone must reject deterministic g0 remint"
        );

        let waiter = tokio::spawn(async move { claim.cleanup().await });
        backend.wait_for_release().await;
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        assert_eq!(manager.status(&c).await.unwrap().state, "expiring");

        allow_release.send(()).unwrap();
        for _ in 0..100 {
            if manager.status(&c).await.is_none() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(manager.status(&c).await.is_none());
        assert_eq!(backend.releases(), vec!["ctx-expiry-detach-g0"]);

        let replacement = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        assert_eq!(replacement.generation.get(), 0);
        assert_ne!(replacement.op, turn.op);
    }

    #[tokio::test]
    async fn canceling_completion_before_expiry_lock_handoff_still_expires() {
        let (manager, backend, _registry) = manager();
        let manager = Arc::new(manager);
        let c = ctx("expiry-pre-lock-cancel");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        let mut guard = WarmCompletionGuard::finish_owner(
            manager.clone(),
            c.clone(),
            turn.generation,
            turn.op.clone(),
            turn.expiry_intent.clone(),
            Arc::new(NoopDiagnosticObserver::default()),
        );
        let error = structured_failure(DiagnosticFailureClass::Transport);
        guard.observe_exit(WarmCompletionExit::Error(&error));

        let table_lock = manager.by_context.lock().await;
        let completion = tokio::spawn(guard.complete());
        tokio::task::yield_now().await;
        completion.abort();
        assert!(completion.await.unwrap_err().is_cancelled());
        drop(table_lock);

        backend.wait_for_release().await;
        for _ in 0..100 {
            if manager.status(&c).await.is_none() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(manager.status(&c).await.is_none());
        assert_eq!(backend.releases(), vec!["ctx-expiry-pre-lock-cancel-g0"]);
    }

    #[tokio::test]
    async fn structured_failure_expiry_during_cancel_settlement_stays_sticky() {
        let (manager, backend, _registry) = manager();
        let manager = Arc::new(manager);
        let c = ctx("expiry-during-cancel");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        let mut guard = WarmCompletionGuard::finish_owner(
            manager.clone(),
            c.clone(),
            turn.generation,
            turn.op.clone(),
            turn.expiry_intent.clone(),
            Arc::new(NoopDiagnosticObserver::default()),
        );
        let error = structured_failure(DiagnosticFailureClass::Transport);
        guard.observe_exit(WarmCompletionExit::Error(&error));
        let allow_cancel = backend.block_next_cancel();

        let cancel_manager = manager.clone();
        let cancel_ctx = c.clone();
        let cancel =
            tokio::spawn(async move { cancel_manager.cancel_with_children(&cancel_ctx).await });
        backend.wait_for_cancel().await;
        assert_eq!(manager.status(&c).await.unwrap().state, "cancelling");

        guard.complete().await.unwrap();
        allow_cancel.send(()).unwrap();
        cancel.await.unwrap().unwrap();

        assert!(
            manager.status(&c).await.is_none(),
            "the exact structured-failure operation must expire after cancel settles"
        );
        assert_eq!(backend.releases(), vec!["ctx-expiry-during-cancel-g0"]);
    }

    #[tokio::test]
    async fn armed_structured_failure_survives_completed_cancel_settlement() {
        let (manager, backend, _registry) = manager();
        let manager = Arc::new(manager);
        let c = ctx("expiry-after-cancel");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        let mut guard = WarmCompletionGuard::finish_owner(
            manager.clone(),
            c.clone(),
            turn.generation,
            turn.op.clone(),
            turn.expiry_intent.clone(),
            Arc::new(NoopDiagnosticObserver::default()),
        );
        let error = structured_failure(DiagnosticFailureClass::AgentProcess);
        guard.observe_exit(WarmCompletionExit::Error(&error));

        manager.cancel_with_children(&c).await.unwrap();
        guard.complete().await.unwrap();

        assert!(
            manager.status(&c).await.is_none(),
            "cancel success must not reopen a turn already armed for expiry"
        );
        assert_eq!(backend.releases(), vec!["ctx-expiry-after-cancel-g0"]);
    }

    #[tokio::test]
    async fn armed_idle_failure_blocks_successor_existing_checkout() {
        let (manager, backend, _registry) = manager();
        let manager = Arc::new(manager);
        let c = ctx("expiry-before-existing-successor");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        let mut guard = WarmCompletionGuard::finish_owner(
            manager.clone(),
            c.clone(),
            turn.generation,
            turn.op.clone(),
            turn.expiry_intent.clone(),
            Arc::new(NoopDiagnosticObserver::default()),
        );

        manager.cancel_with_children(&c).await.unwrap();
        let error = structured_failure(DiagnosticFailureClass::Transport);
        guard.observe_exit(WarmCompletionExit::Error(&error));

        assert_eq!(
            manager.checkout_existing_turn(&c).await.err(),
            Some(BridgeError::SessionExpired),
            "an armed retained operation must expire before continue can mint a successor"
        );
        guard.complete().await.unwrap();
        backend.wait_for_release().await;
        assert!(manager.status(&c).await.is_none());
        assert_eq!(
            backend.releases(),
            vec!["ctx-expiry-before-existing-successor-g0"]
        );
    }

    #[tokio::test]
    async fn armed_idle_failure_blocks_successor_regular_checkout() {
        let (manager, backend, _registry) = manager();
        let manager = Arc::new(manager);
        let c = ctx("expiry-before-regular-successor");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        let mut guard = WarmCompletionGuard::finish_owner(
            manager.clone(),
            c.clone(),
            turn.generation,
            turn.op.clone(),
            turn.expiry_intent.clone(),
            Arc::new(NoopDiagnosticObserver::default()),
        );

        manager.cancel_with_children(&c).await.unwrap();
        let error = structured_failure(DiagnosticFailureClass::AgentProcess);
        guard.observe_exit(WarmCompletionExit::Error(&error));

        assert_eq!(
            manager.checkout_turn(&c, agent(), None, None).await.err(),
            Some(BridgeError::SessionExpired),
            "an armed retained operation must expire before ordinary checkout can mint a successor"
        );
        guard.complete().await.unwrap();
        backend.wait_for_release().await;
        assert!(manager.status(&c).await.is_none());
        assert_eq!(
            backend.releases(),
            vec!["ctx-expiry-before-regular-successor-g0"]
        );
    }

    #[tokio::test]
    async fn warm_completion_starts_cleanup_before_pending_teardown_observation() {
        let (manager, backend, _registry) = manager();
        let manager = Arc::new(manager);
        let c = ctx("expiry-pending-start-observation");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        let observer = Arc::new(PendingDiagnostic::default());
        let weak_observer = Arc::downgrade(&observer);
        let mut guard = WarmCompletionGuard::finish_owner(
            manager.clone(),
            c.clone(),
            turn.generation,
            turn.op.clone(),
            turn.expiry_intent.clone(),
            observer.clone(),
        );
        let error = structured_failure(DiagnosticFailureClass::Transport);
        guard.observe_exit(WarmCompletionExit::Error(&error));
        let allow_release = backend.block_next_release();

        let completion = tokio::spawn(guard.complete());
        observer.wait_until_entered().await;
        tokio::time::timeout(Duration::from_secs(2), backend.wait_for_release())
            .await
            .expect("warm cleanup must start before teardown observation settles");
        assert_eq!(manager.status(&c).await.unwrap().state, "expiring");

        completion.abort();
        assert!(completion.await.unwrap_err().is_cancelled());
        drop(observer);
        assert!(
            weak_observer.upgrade().is_none(),
            "observer-free cleanup must not retain the pending operation observer"
        );
        allow_release.send(()).unwrap();
        for _ in 0..100 {
            if manager.status(&c).await.is_none() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(manager.status(&c).await.is_none());
        assert_eq!(backend.releases().len(), 1);
    }

    #[tokio::test]
    async fn teardown_start_observation_failure_still_joins_cleanup_before_returning() {
        let (manager, backend, _registry) = manager();
        let manager = Arc::new(manager);
        let c = ctx("expiry-rejected-start-observation");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        let mut guard = WarmCompletionGuard::finish_owner(
            manager.clone(),
            c.clone(),
            turn.generation,
            turn.op.clone(),
            turn.expiry_intent.clone(),
            Arc::new(RejectingDiagnostic),
        );
        let error = structured_failure(DiagnosticFailureClass::AgentProcess);
        guard.observe_exit(WarmCompletionExit::Error(&error));
        let allow_release = backend.block_next_release();

        let completion = tokio::spawn(guard.complete());
        backend.wait_for_release().await;
        assert!(
            !completion.is_finished(),
            "observation failure must not return before the owned cleanup report settles"
        );
        allow_release.send(()).unwrap();
        assert_eq!(completion.await.unwrap(), Err(BridgeError::StoreFailure));
        assert!(manager.status(&c).await.is_none());
        assert_eq!(backend.releases().len(), 1);
    }

    #[tokio::test]
    async fn workflow_cancel_settlement_survives_completion_waiter_cancellation() {
        let (manager, backend, _registry) = manager();
        let manager = Arc::new(manager);
        let c = ctx("cancel-settlement-detach");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        let mut guard = WarmCompletionGuard::workflow_owner(
            manager.clone(),
            c.clone(),
            turn.generation,
            turn.op.clone(),
            turn.expiry_intent.clone(),
            Arc::new(NoopDiagnosticObserver::default()),
        );
        guard.observe_exit(WarmCompletionExit::Canceled);
        let allow_cancel = backend.block_next_cancel();

        let completion = tokio::spawn(guard.complete());
        backend.wait_for_cancel().await;
        completion.abort();
        assert!(completion.await.unwrap_err().is_cancelled());
        assert!(
            allow_cancel.send(()).is_ok(),
            "cancel settlement must be owned by a detached flight, not the report waiter"
        );

        for _ in 0..100 {
            if manager.status(&c).await.map(|status| status.state) == Some("idle") {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(manager.status(&c).await.unwrap().state, "idle");
        assert_eq!(backend.cancels(), vec!["ctx-cancel-settlement-detach-g0"]);
    }

    #[tokio::test]
    async fn detached_cleanup_flight_does_not_retain_operation_observer() {
        let (manager, backend, _registry) = manager();
        let manager = Arc::new(manager);
        let c = ctx("expiry-observer-detach");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        let observer = Arc::new(NoopDiagnosticObserver::default());
        let weak_observer = Arc::downgrade(&observer);
        let mut guard = WarmCompletionGuard::finish_owner(
            manager.clone(),
            c.clone(),
            turn.generation,
            turn.op.clone(),
            turn.expiry_intent.clone(),
            observer.clone(),
        );
        drop(observer);
        let error = structured_failure(DiagnosticFailureClass::AgentProcess);
        guard.observe_exit(WarmCompletionExit::Error(&error));
        let allow_release = backend.block_next_release();
        let waiter = tokio::spawn(guard.complete());
        backend.wait_for_release().await;

        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        assert!(
            weak_observer.upgrade().is_none(),
            "the detached cleanup task must not capture the operation observer"
        );
        allow_release.send(()).unwrap();
        for _ in 0..100 {
            if manager.status(&c).await.is_none() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(manager.status(&c).await.is_none());
        assert!(weak_observer.upgrade().is_none());
    }

    #[tokio::test]
    async fn dropping_unstarted_expiry_claim_starts_exactly_one_cleanup_flight() {
        let (manager, backend, _registry) = manager();
        let manager = Arc::new(manager);
        let c = ctx("expiry-claim-drop");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        let allow_release = backend.block_next_release();
        let claim = manager
            .begin_expire_current(&c, turn.generation, &turn.op)
            .await
            .unwrap();

        drop(claim);
        backend.wait_for_release().await;
        assert_eq!(backend.releases(), vec!["ctx-expiry-claim-drop-g0"]);
        assert_eq!(manager.status(&c).await.unwrap().state, "expiring");
        allow_release.send(()).unwrap();
        for _ in 0..100 {
            if manager.status(&c).await.is_none() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(manager.status(&c).await.is_none());
        assert_eq!(backend.releases().len(), 1);
    }

    #[tokio::test]
    async fn cleanup_failure_leaves_non_reusable_retryable_tombstone() {
        let (manager, backend, _registry) = manager();
        let manager = Arc::new(manager);
        let c = ctx("expiry-failed");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        backend.set_release_result(Err(BridgeError::StoreFailure));
        let report = manager
            .begin_expire_current(&c, turn.generation, &turn.op)
            .await
            .unwrap()
            .cleanup()
            .await;

        assert_eq!(report.result, Err(BridgeError::StoreFailure));
        assert_eq!(manager.status(&c).await.unwrap().state, "cleanup_failed");
        assert_eq!(
            manager
                .checkout_turn(&c, agent(), None, None)
                .await
                .err()
                .unwrap(),
            BridgeError::SessionExpired
        );
        manager.finish_turn(&c, turn.generation, &turn.op).await;
        assert_eq!(manager.status(&c).await.unwrap().state, "cleanup_failed");
        assert!(manager
            .by_context
            .lock()
            .await
            .cleanup_retries
            .contains_key(&c));
        assert_eq!(backend.releases(), vec!["ctx-expiry-failed-g0"]);
    }

    #[tokio::test]
    async fn explicit_release_retries_cleanup_failed_tombstone_after_backend_recovers() {
        let (manager, backend, _registry) = manager();
        let manager = Arc::new(manager);
        let c = ctx("expiry-failed-release-retry");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        backend.set_release_result(Err(BridgeError::StoreFailure));
        let report = manager
            .begin_expire_current(&c, turn.generation, &turn.op)
            .await
            .unwrap()
            .cleanup()
            .await;
        assert_eq!(report.result, Err(BridgeError::StoreFailure));
        assert_eq!(manager.status(&c).await.unwrap().state, "cleanup_failed");

        backend.set_release_result(Ok(()));
        manager.release(&c).await;

        assert!(manager.status(&c).await.is_none());
        assert_eq!(
            backend.releases(),
            vec![
                "ctx-expiry-failed-release-retry-g0",
                "ctx-expiry-failed-release-retry-g0"
            ]
        );
    }

    #[tokio::test]
    async fn failed_cleanup_retry_restores_owner_for_a_later_release() {
        let (manager, backend, _registry) = manager();
        let manager = Arc::new(manager);
        let c = ctx("expiry-failed-release-twice");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        backend.set_release_result(Err(BridgeError::StoreFailure));
        assert_eq!(
            manager
                .begin_expire_current(&c, turn.generation, &turn.op)
                .await
                .unwrap()
                .cleanup()
                .await
                .result,
            Err(BridgeError::StoreFailure)
        );

        manager.release(&c).await;
        assert_eq!(manager.status(&c).await.unwrap().state, "cleanup_failed");
        assert_eq!(backend.releases().len(), 2);

        backend.set_release_result(Ok(()));
        manager.release(&c).await;
        assert!(manager.status(&c).await.is_none());
        assert_eq!(backend.releases().len(), 3);
    }

    #[tokio::test]
    async fn canceled_cleanup_retry_waiter_does_not_cancel_the_owned_flight() {
        let (manager, backend, _registry) = manager();
        let manager = Arc::new(manager);
        let c = ctx("expiry-failed-release-detached");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        backend.set_release_result(Err(BridgeError::StoreFailure));
        assert_eq!(
            manager
                .begin_expire_current(&c, turn.generation, &turn.op)
                .await
                .unwrap()
                .cleanup()
                .await
                .result,
            Err(BridgeError::StoreFailure)
        );

        backend.set_release_result(Ok(()));
        let allow_release = backend.block_next_release();
        let release_manager = manager.clone();
        let release_ctx = c.clone();
        let release = tokio::spawn(async move { release_manager.release(&release_ctx).await });
        tokio::time::timeout(Duration::from_secs(2), backend.wait_for_release_count(2))
            .await
            .expect("explicit release must start the retained cleanup retry");
        release.abort();
        assert!(release.await.unwrap_err().is_cancelled());
        allow_release.send(()).unwrap();

        for _ in 0..100 {
            if manager.status(&c).await.is_none() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(manager.status(&c).await.is_none());
        assert_eq!(backend.releases().len(), 2);
    }

    #[tokio::test]
    async fn clear_retries_cleanup_failed_tombstone_instead_of_returning_not_found() {
        let (manager, backend, _registry) = manager();
        let manager = Arc::new(manager);
        let c = ctx("expiry-failed-clear-retry");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        backend.set_release_result(Err(BridgeError::StoreFailure));
        assert_eq!(
            manager
                .begin_expire_current(&c, turn.generation, &turn.op)
                .await
                .unwrap()
                .cleanup()
                .await
                .result,
            Err(BridgeError::StoreFailure)
        );

        backend.set_release_result(Ok(()));
        assert_eq!(
            manager.clear_with_children(&c, true).await.unwrap(),
            ResetOutcome::Cleared {
                generation: turn.generation.get()
            }
        );
        assert!(manager.status(&c).await.is_none());
        assert_eq!(backend.releases().len(), 2);
    }

    #[tokio::test]
    async fn cleanup_panic_is_caught_and_finalizes_non_reusable_failure_state() {
        let (manager, backend, _registry) = manager();
        let manager = Arc::new(manager);
        let c = ctx("expiry-panicked");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        backend.set_release_panics();
        let report = manager
            .begin_expire_current(&c, turn.generation, &turn.op)
            .await
            .unwrap()
            .cleanup()
            .await;

        assert!(matches!(
            report.result,
            Err(BridgeError::AgentCrashed { .. })
        ));
        assert_eq!(manager.status(&c).await.unwrap().state, "cleanup_failed");
        assert_eq!(backend.releases(), vec!["ctx-expiry-panicked-g0"]);
    }

    #[tokio::test]
    async fn lease_drop_panic_finalizes_retryable_cleanup_failure() {
        let (manager, backend, registry) = manager();
        let manager = Arc::new(manager);
        let c = ctx("expiry-lease-drop-panicked");
        registry.panic_next_lease_drop();
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();

        let report = manager
            .begin_expire_current(&c, turn.generation, &turn.op)
            .await
            .unwrap()
            .cleanup()
            .await;

        assert!(matches!(
            report.result,
            Err(BridgeError::AgentCrashed { .. })
        ));
        assert_eq!(manager.status(&c).await.unwrap().state, "cleanup_failed");
        assert!(manager
            .by_context
            .lock()
            .await
            .cleanup_retries
            .contains_key(&c));
        assert_eq!(
            backend.releases(),
            vec!["ctx-expiry-lease-drop-panicked-g0"]
        );

        manager.release(&c).await;

        assert!(manager.status(&c).await.is_none());
        assert_eq!(backend.releases().len(), 2);
    }

    #[tokio::test]
    async fn worker_panic_recovery_does_not_depend_on_joiner_settlement() {
        let (manager, backend, registry) = manager();
        let manager = Arc::new(manager);
        let c = ctx("expiry-worker-only-panic-recovery");
        registry.panic_next_lease_drop();
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        let allow_release = backend.block_next_release();
        let CleanupFlight { task, settlement } = manager
            .begin_expire_current(&c, turn.generation, &turn.op)
            .await
            .unwrap()
            .into_flight()
            .unwrap();
        backend.wait_for_release().await;

        // Remove the joiner's recovery capability before the lease destructor
        // panics. Only the settlement owner outside the whole-worker unwind
        // boundary can make this raw Tokio task return a CleanupReport and
        // publish retryable state.
        drop(settlement);
        allow_release.send(()).unwrap();
        let report = task
            .await
            .expect("the whole cleanup worker catches lease-drop panic");

        assert!(matches!(
            report.result,
            Err(BridgeError::AgentCrashed { .. })
        ));
        assert_eq!(manager.status(&c).await.unwrap().state, "cleanup_failed");
        assert!(manager
            .by_context
            .lock()
            .await
            .cleanup_retries
            .contains_key(&c));

        manager.release(&c).await;
        assert!(manager.status(&c).await.is_none());
        assert_eq!(backend.releases().len(), 2);
    }

    #[tokio::test]
    async fn aborted_cleanup_task_finalizes_retryable_failure() {
        let (manager, backend, _registry) = manager();
        let manager = Arc::new(manager);
        let c = ctx("expiry-task-aborted");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        let allow_release = backend.block_next_release();
        let flight = manager
            .begin_expire_current(&c, turn.generation, &turn.op)
            .await
            .unwrap()
            .into_flight()
            .unwrap();
        backend.wait_for_release().await;

        flight.abort();
        let report = ExpiryClaim::join_flight(flight).await;

        assert!(matches!(
            report.result,
            Err(BridgeError::AgentCrashed { .. })
        ));
        assert_eq!(manager.status(&c).await.unwrap().state, "cleanup_failed");
        assert!(manager
            .by_context
            .lock()
            .await
            .cleanup_retries
            .contains_key(&c));
        drop(allow_release);

        manager.release(&c).await;

        assert!(manager.status(&c).await.is_none());
        assert_eq!(backend.releases().len(), 2);
    }

    #[tokio::test]
    async fn stale_expiry_operation_cannot_claim_a_newer_turn() {
        let (manager, backend, _registry) = manager();
        let manager = Arc::new(manager);
        let c = ctx("expiry-stale-op");
        let first = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        manager.finish_turn(&c, first.generation, &first.op).await;
        let second = manager.checkout_existing_turn(&c).await.unwrap();

        assert!(manager
            .begin_expire_current(&c, first.generation, &first.op)
            .await
            .is_none());
        assert_eq!(manager.status(&c).await.unwrap().state, "running");
        assert_eq!(backend.releases().len(), 0);
        manager.finish_turn(&c, second.generation, &second.op).await;
    }

    #[tokio::test]
    async fn stale_cleanup_flight_cannot_clear_a_newer_claim_id_tombstone() {
        let (manager, backend, _registry) = manager();
        let manager = Arc::new(manager);
        let c = ctx("expiry-stale-flight");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        let claim = manager
            .begin_expire_current(&c, turn.generation, &turn.op)
            .await
            .unwrap();
        {
            let mut table = manager.by_context.lock().await;
            let tombstone = table.tombstones.get_mut(&c).unwrap();
            tombstone.cleanup_claim_id += 1;
        }

        assert!(claim.cleanup().await.result.is_ok());
        assert_eq!(manager.status(&c).await.unwrap().state, "expiring");
        assert_eq!(backend.releases(), vec!["ctx-expiry-stale-flight-g0"]);
    }

    #[tokio::test]
    async fn record_usage_noops_on_stale_generation() {
        let (manager, _b, _r) = manager();
        let c = ctx("ru-stale");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        manager
            .record_usage(
                &c,
                SessionGeneration::new(99),
                &turn.op,
                UsageSnapshot {
                    used: Some(5),
                    size: Some(9),
                    cost: None,
                    terminal: None,
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
        let clock = Arc::new(ManualClock::new(0));
        let manager =
            SessionManager::new_with_clock(registry, Duration::from_secs(5), clock.clone());
        let c = ctx("cancel-ttl");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        clock.advance(Duration::from_secs(6));
        manager.cancel(&c).await.unwrap();
        manager.finish_turn(&c, turn.generation, &turn.op).await;
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
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        manager.cancel(&c).await.unwrap();
        let t2 = manager
            .checkout_turn(&c, agent(), None, None)
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
                    terminal: None,
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
            .checkout_turn(&ctx, agent(), None, None)
            .await
            .unwrap();
        manager.finish_turn(&ctx, first.generation, &first.op).await;
        let second = manager
            .checkout_turn(&ctx, agent(), None, None)
            .await
            .unwrap();

        assert_eq!(first.session.as_str(), "ctx-abc-g0");
        assert_eq!(first.session, second.session);
    }

    #[tokio::test]
    async fn checkout_child_turn_registers_and_reuses() {
        let (manager, _backend, _registry) = manager();
        let parent = ctx("parent");
        let child = ctx("child");

        let first = manager
            .checkout_child_turn(&parent, &child, agent(), None, None)
            .await
            .unwrap();
        assert_eq!(first.session.as_str(), "ctx-child-g0");
        assert_eq!(first.generation, SessionGeneration::new(0));
        assert!(manager.child_registered(&parent, &child).await);

        manager
            .finish_turn(&child, first.generation, &first.op)
            .await;
        let second = manager
            .checkout_child_turn(&parent, &child, agent(), None, None)
            .await
            .unwrap();

        assert_eq!(second.session, first.session);
        assert_eq!(second.generation, SessionGeneration::new(0));
        assert_ne!(first.op, second.op);
        assert!(manager.child_registered(&parent, &child).await);
    }

    #[tokio::test]
    async fn expire_turn_prunes_child_registration() {
        let (manager, _backend, _registry) = manager();
        let parent = ctx("expire-parent");
        let child = ctx("expire-child");

        manager
            .checkout_child_turn(&parent, &child, agent(), None, None)
            .await
            .unwrap();
        assert!(manager.child_registered(&parent, &child).await);

        manager.expire_turn(&child).await;

        assert!(manager.status(&child).await.is_none());
        assert!(!manager.child_registered(&parent, &child).await);
        assert!(!manager.child_parent_registered(&parent).await);
        assert_eq!(
            manager.cancel_with_children(&parent).await.err(),
            Some(BridgeError::SessionNotFound)
        );
    }

    #[tokio::test]
    async fn release_standalone_prunes_child_registration() {
        let (manager, _backend, _registry) = manager();
        let parent = ctx("release-parent");
        let child = ctx("release-child");

        manager
            .checkout_child_turn(&parent, &child, agent(), None, None)
            .await
            .unwrap();
        assert!(manager.child_registered(&parent, &child).await);

        manager.release(&child).await;

        assert!(manager.status(&child).await.is_none());
        assert!(!manager.child_registered(&parent, &child).await);
        assert!(!manager.child_parent_registered(&parent).await);
    }

    #[tokio::test]
    async fn reconcile_expire_prunes_child_registration() {
        let (manager, backend, _registry) = manager();
        let parent = ctx("reconcile-expire-parent");
        let child = ctx("reconcile-expire-child");

        let turn = manager
            .checkout_child_turn(
                &parent,
                &child,
                agent(),
                Some(model_override("gpt-5.5")),
                None,
            )
            .await
            .unwrap();
        manager.finish_turn(&child, turn.generation, &turn.op).await;
        assert!(manager.child_registered(&parent, &child).await);

        backend.set_reconcile_result(Ok(ReconcileOutcome::Rejected));
        let err = manager
            .checkout_turn(&child, agent(), Some(model_override("gpt-5.4")), None)
            .await
            .err()
            .unwrap();

        assert_eq!(err, BridgeError::ConfigReseedRequired { field: "model" });
        assert!(manager.status(&child).await.is_none());
        assert!(!manager.child_registered(&parent, &child).await);
        assert!(!manager.child_parent_registered(&parent).await);
    }

    #[tokio::test]
    async fn checkout_child_turn_reconcile_expiry_does_not_deadlock() {
        let (manager, backend, _registry) = manager();
        let parent = ctx("child-reconcile-expire-parent");
        let child = ctx("child-reconcile-expire-child");

        let turn = manager
            .checkout_child_turn(
                &parent,
                &child,
                agent(),
                Some(model_override("gpt-5.5")),
                None,
            )
            .await
            .unwrap();
        manager.finish_turn(&child, turn.generation, &turn.op).await;
        assert!(manager.child_registered(&parent, &child).await);

        backend.set_reconcile_result(Ok(ReconcileOutcome::Rejected));
        let err = tokio::time::timeout(
            Duration::from_millis(200),
            manager.checkout_child_turn(
                &parent,
                &child,
                agent(),
                Some(model_override("gpt-5.4")),
                None,
            ),
        )
        .await
        .expect("checkout_child_turn must not deadlock")
        .err()
        .unwrap();

        assert_eq!(err, BridgeError::ConfigReseedRequired { field: "model" });
        assert!(manager.status(&child).await.is_none());
        assert!(!manager.child_registered(&parent, &child).await);
        assert!(!manager.child_parent_registered(&parent).await);
    }

    #[tokio::test]
    async fn reset_reconcile_expire_prunes_child_registration() {
        let (manager, backend, _registry) = manager();
        let parent = ctx("reset-expire-parent");
        let child = ctx("reset-expire-child");

        let turn = manager
            .checkout_child_turn(&parent, &child, agent(), None, None)
            .await
            .unwrap();
        manager.finish_turn(&child, turn.generation, &turn.op).await;
        assert!(manager.child_registered(&parent, &child).await);

        backend.set_configure_result(Err(BridgeError::ConfigInvalid {
            reason: "boom".into(),
        }));
        let err = manager
            .reset_session(&child, ResetOpts { force: false })
            .await
            .err()
            .unwrap();

        assert!(matches!(err, BridgeError::ConfigInvalid { .. }));
        assert!(manager.status(&child).await.is_none());
        assert!(!manager.child_registered(&parent, &child).await);
        assert!(!manager.child_parent_registered(&parent).await);
    }

    #[tokio::test]
    async fn checkout_child_turn_failure_does_not_register() {
        let (manager, backend, _registry) = manager();
        let parent = ctx("parent-fail");
        let missing_child = ctx("missing-child");

        let err = manager
            .checkout_child_turn(
                &parent,
                &missing_child,
                AgentId::parse("missing").unwrap(),
                None,
                None,
            )
            .await
            .err()
            .unwrap();
        assert!(matches!(err, BridgeError::UnknownAgent { .. }));
        assert!(!manager.child_registered(&parent, &missing_child).await);

        backend.set_configure_result(Err(BridgeError::ConfigInvalid {
            reason: "boom".into(),
        }));
        let configure_child = ctx("configure-child");
        let err = manager
            .checkout_child_turn(&parent, &configure_child, agent(), None, None)
            .await
            .err()
            .unwrap();
        assert!(matches!(err, BridgeError::ConfigInvalid { .. }));
        assert!(!manager.child_registered(&parent, &configure_child).await);
    }

    #[tokio::test]
    async fn release_with_children_sweeps() {
        let (manager, backend, _registry) = manager();
        let parent = ctx("sweep-parent");
        let child_a = ctx("sweep-child-a");
        let child_b = ctx("sweep-child-b");

        manager
            .checkout_turn(&parent, agent(), None, None)
            .await
            .unwrap();
        manager
            .checkout_child_turn(&parent, &child_a, agent(), None, None)
            .await
            .unwrap();
        manager
            .checkout_child_turn(&parent, &child_b, agent(), None, None)
            .await
            .unwrap();

        manager.release_with_children(&parent).await;

        assert!(manager.status(&parent).await.is_none());
        assert!(manager.status(&child_a).await.is_none());
        assert!(manager.status(&child_b).await.is_none());
        assert!(!manager.child_registered(&parent, &child_a).await);
        assert!(!manager.child_registered(&parent, &child_b).await);
        let mut releases = backend.releases();
        releases.sort();
        assert_eq!(
            releases,
            vec![
                "ctx-sweep-child-a-g0".to_string(),
                "ctx-sweep-child-b-g0".to_string(),
                "ctx-sweep-parent-g0".to_string(),
            ]
        );

        manager.release_with_children(&parent).await;
    }

    #[tokio::test]
    async fn release_with_children_claims_every_handle_before_cleanup_waiter_can_be_canceled() {
        let (manager, backend, _registry) = manager();
        let manager = Arc::new(manager);
        let parent = ctx("cancel-sweep-parent");
        let child_a = ctx("cancel-sweep-child-a");
        let child_b = ctx("cancel-sweep-child-b");

        manager
            .checkout_turn(&parent, agent(), None, None)
            .await
            .unwrap();
        manager
            .checkout_child_turn(&parent, &child_a, agent(), None, None)
            .await
            .unwrap();
        manager
            .checkout_child_turn(&parent, &child_b, agent(), None, None)
            .await
            .unwrap();

        let allow_first_release = backend.block_next_release();
        let release_manager = manager.clone();
        let release_parent = parent.clone();
        let release = tokio::spawn(async move {
            release_manager.release_with_children(&release_parent).await;
        });
        backend.wait_for_release().await;
        release.abort();
        assert!(release.await.unwrap_err().is_cancelled());

        for _ in 0..100 {
            if backend.releases().len() == 3 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            backend.releases().len(),
            3,
            "canceling the waiter must drop three already-owned claims into their cleanup flights"
        );
        assert!(
            !manager.child_parent_registered(&parent).await,
            "the parent registration must be removed before cleanup's first await"
        );

        allow_first_release.send(()).unwrap();
        for _ in 0..100 {
            if manager.status(&parent).await.is_none()
                && manager.status(&child_a).await.is_none()
                && manager.status(&child_b).await.is_none()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(manager.status(&parent).await.is_none());
        assert!(manager.status(&child_a).await.is_none());
        assert!(manager.status(&child_b).await.is_none());
    }

    #[tokio::test]
    async fn cancel_then_release_frees_children() {
        let (manager, backend, _registry) = manager();
        let parent = ctx("cancel-release-parent");
        let child_a = ctx("cancel-release-child-a");
        let child_b = ctx("cancel-release-child-b");

        manager
            .checkout_turn(&parent, agent(), None, None)
            .await
            .unwrap();
        manager
            .checkout_child_turn(&parent, &child_a, agent(), None, None)
            .await
            .unwrap();
        manager
            .checkout_child_turn(&parent, &child_b, agent(), None, None)
            .await
            .unwrap();

        manager.cancel_with_children(&parent).await.unwrap();

        assert_eq!(manager.status(&parent).await.unwrap().state, "idle");
        assert_eq!(manager.status(&child_a).await.unwrap().state, "idle");
        assert_eq!(manager.status(&child_b).await.unwrap().state, "idle");
        assert!(manager.child_registered(&parent, &child_a).await);
        assert!(manager.child_registered(&parent, &child_b).await);

        manager.release_with_children(&parent).await;

        assert!(manager.status(&parent).await.is_none());
        assert!(manager.status(&child_a).await.is_none());
        assert!(manager.status(&child_b).await.is_none());
        let mut releases = backend.releases();
        releases.sort();
        assert_eq!(
            releases,
            vec![
                "ctx-cancel-release-child-a-g0".to_string(),
                "ctx-cancel-release-child-b-g0".to_string(),
                "ctx-cancel-release-parent-g0".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn clear_then_release_frees_children() {
        let (manager, backend, _registry) = manager();
        let parent = ctx("clear-release-parent");
        let child_a = ctx("clear-release-child-a");
        let child_b = ctx("clear-release-child-b");

        manager
            .checkout_turn(&parent, agent(), None, None)
            .await
            .unwrap();
        manager
            .checkout_child_turn(&parent, &child_a, agent(), None, None)
            .await
            .unwrap();
        manager
            .checkout_child_turn(&parent, &child_b, agent(), None, None)
            .await
            .unwrap();

        let out = manager.clear_with_children(&parent, true).await.unwrap();

        assert_eq!(out, ResetOutcome::Cleared { generation: 1 });
        assert_eq!(manager.status(&parent).await.unwrap().state, "idle");
        assert_eq!(manager.status(&child_a).await.unwrap().state, "idle");
        assert_eq!(manager.status(&child_b).await.unwrap().state, "idle");
        assert!(manager.child_registered(&parent, &child_a).await);
        assert!(manager.child_registered(&parent, &child_b).await);

        manager.release_with_children(&parent).await;

        assert!(manager.status(&parent).await.is_none());
        assert!(manager.status(&child_a).await.is_none());
        assert!(manager.status(&child_b).await.is_none());
        let mut releases = backend.releases();
        releases.sort();
        assert_eq!(
            releases,
            vec![
                "ctx-clear-release-child-a-g0".to_string(),
                "ctx-clear-release-child-a-g1".to_string(),
                "ctx-clear-release-child-b-g0".to_string(),
                "ctx-clear-release-child-b-g1".to_string(),
                "ctx-clear-release-parent-g0".to_string(),
                "ctx-clear-release-parent-g1".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn clear_with_children_unknown_is_not_found() {
        let (manager, _backend, _registry) = manager();
        let out = manager
            .clear_with_children(&ctx("clear-unknown"), false)
            .await
            .unwrap();

        assert_eq!(out, ResetOutcome::NotFound);
    }

    #[tokio::test]
    async fn cancel_with_children_unknown_is_not_found() {
        let (manager, _backend, _registry) = manager();
        let err = manager
            .cancel_with_children(&ctx("cancel-unknown"))
            .await
            .err()
            .unwrap();

        assert_eq!(err, BridgeError::SessionNotFound);
    }

    #[tokio::test]
    async fn clear_with_children_threads_force() {
        let (manager, backend, _registry) = manager();
        let parent = ctx("clear-force-parent");
        let child = ctx("clear-force-child");

        manager
            .checkout_child_turn(&parent, &child, agent(), None, None)
            .await
            .unwrap();

        let out = manager.clear_with_children(&parent, true).await.unwrap();

        assert_eq!(out, ResetOutcome::Cleared { generation: 0 });
        let status = manager.status(&child).await.unwrap();
        assert_eq!(status.state, "idle");
        assert_eq!(status.generation, 1);
        assert_eq!(backend.cancels(), vec!["ctx-clear-force-child-g0"]);
        assert!(backend
            .releases()
            .contains(&"ctx-clear-force-child-g0".to_string()));
    }

    #[tokio::test]
    async fn clear_with_children_running_child_without_force_is_busy() {
        let (manager, _backend, _registry) = manager();
        let parent = ctx("clear-busy-parent");
        let child = ctx("clear-busy-child");

        manager
            .checkout_child_turn(&parent, &child, agent(), None, None)
            .await
            .unwrap();

        let err = manager
            .clear_with_children(&parent, false)
            .await
            .err()
            .unwrap();

        assert_eq!(err, BridgeError::HandleBusy);
        assert_eq!(manager.status(&child).await.unwrap().state, "running");
    }

    #[tokio::test]
    async fn clear_with_children_reset_failure_does_not_deadlock() {
        let (manager, backend, _registry) = manager();
        let parent = ctx("clear-fail-parent");
        let child = ctx("clear-fail-child");

        let turn = manager
            .checkout_child_turn(&parent, &child, agent(), None, None)
            .await
            .unwrap();
        manager.finish_turn(&child, turn.generation, &turn.op).await;
        assert!(manager.child_registered(&parent, &child).await);

        backend.set_configure_result(Err(BridgeError::ConfigInvalid {
            reason: "boom".into(),
        }));
        let out = tokio::time::timeout(
            Duration::from_secs(1),
            manager.clear_with_children(&parent, false),
        )
        .await
        .expect("clear_with_children must not deadlock");

        assert!(matches!(out, Err(BridgeError::ConfigInvalid { .. })));
        assert!(manager.status(&child).await.is_none());
        assert!(!manager.child_registered(&parent, &child).await);
        assert!(!manager.child_parent_registered(&parent).await);
    }

    #[tokio::test]
    async fn cancel_resolves_pending_permission() {
        let reg = PermissionRegistry::new();
        let (manager, backend, _registry) = manager();
        let manager = Arc::new(manager.with_permission_registry(reg.clone()));
        let c = ctx("cancel-perm");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        let (rx, _guard) = reg.register(
            pkey(&c, turn.generation, &turn.op, "r"),
            permission_view("r", turn.generation, &turn.op),
        );
        let unblock = backend.block_next_cancel();

        let cancel = {
            let manager = manager.clone();
            let c = c.clone();
            tokio::spawn(async move { manager.cancel_with_children(&c).await })
        };
        backend.wait_for_cancel().await;
        assert_permission_cancelled(rx).await;

        unblock.send(()).unwrap();
        cancel.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn clear_resolves_pending_permission() {
        let reg = PermissionRegistry::new();
        let (manager, backend, _registry) = manager();
        let manager = Arc::new(manager.with_permission_registry(reg.clone()));
        let c = ctx("clear-perm");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        let (rx, _guard) = reg.register(
            pkey(&c, turn.generation, &turn.op, "r"),
            permission_view("r", turn.generation, &turn.op),
        );
        let unblock = backend.block_next_cancel();

        let clear = {
            let manager = manager.clone();
            let c = c.clone();
            tokio::spawn(async move { manager.clear_with_children(&c, true).await })
        };
        backend.wait_for_cancel().await;
        assert_permission_cancelled(rx).await;

        unblock.send(()).unwrap();
        clear.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn release_resolves_pending_permission() {
        let reg = PermissionRegistry::new();
        let (manager, backend, _registry) = manager();
        let manager = Arc::new(manager.with_permission_registry(reg.clone()));
        let c = ctx("release-perm");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        let (rx, _guard) = reg.register(
            pkey(&c, turn.generation, &turn.op, "r"),
            permission_view("r", turn.generation, &turn.op),
        );
        let unblock = backend.block_next_release();

        let release = {
            let manager = manager.clone();
            let c = c.clone();
            tokio::spawn(async move { manager.release_with_children(&c).await })
        };
        backend.wait_for_release().await;

        assert_permission_cancelled(rx).await;

        unblock.send(()).unwrap();
        release.await.unwrap();
    }

    #[tokio::test]
    async fn keepwarm_cancel_resolves_pending_perm_without_stranding_next_turn() {
        let reg = PermissionRegistry::new();
        let (manager, _backend, _registry) = manager();
        let manager = manager.with_permission_registry(reg.clone());
        let c = ctx("cancel-perm-next-turn");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        let (rx, _guard) = reg.register(
            pkey(&c, turn.generation, &turn.op, "r"),
            permission_view("r", turn.generation, &turn.op),
        );

        manager.cancel(&c).await.unwrap();
        assert_permission_cancelled(rx).await;

        // The permission registry and retained abort tokens are both operation-scoped: resolving the
        // canceled turn's permission does not poison a later warm checkout.
        let next = manager.checkout_existing_turn(&c).await.unwrap();
        assert_ne!(next.op, turn.op);
        assert_eq!(next.generation, turn.generation);
    }

    #[tokio::test]
    async fn claimed_compacting_cancel_does_not_resolve_pending() {
        let reg = PermissionRegistry::new();
        let (manager, _backend, _registry) = manager();
        let manager = Arc::new(manager.with_permission_registry(reg.clone()));
        let c = ctx("claimed-compact-perm");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        manager.finish_turn(&c, turn.generation, &turn.op).await;

        let (entered_tx, entered_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let compact = {
            let manager = manager.clone();
            let c = c.clone();
            tokio::spawn(async move {
                manager
                    .compact_session(&c, |_b, _s| async move {
                        let _ = entered_tx.send(());
                        let _ = release_rx.await;
                        Ok("summary".to_string())
                    })
                    .await
            })
        };
        entered_rx.await.unwrap();
        assert_eq!(manager.status(&c).await.unwrap().state, "compacting");

        let (mut rx, _guard) = reg.register(
            pkey(&c, turn.generation, &turn.op, "r"),
            permission_view("r", turn.generation, &turn.op),
        );

        manager.cancel(&c).await.unwrap();
        let err = manager.clear_with_children(&c, false).await.err().unwrap();
        assert_eq!(err, BridgeError::HandleBusy);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut rx)
                .await
                .is_err(),
            "claimed compacting permissions must not be cancelled by non-force cancel/clear"
        );

        release_tx.send(()).unwrap();
        let out = compact.await.unwrap();
        assert_eq!(out, Err(BridgeError::SessionExpired));
    }

    #[tokio::test]
    async fn cancel_idle_handle_skips_backend_cancel() {
        let (manager, backend, _registry) = manager();
        let c = ctx("cancel-idle");

        manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        manager.cancel(&c).await.unwrap();
        manager.cancel(&c).await.unwrap();

        assert_eq!(backend.cancels(), vec!["ctx-cancel-idle-g0"]);
        assert_eq!(manager.status(&c).await.unwrap().state, "idle");
    }

    #[tokio::test]
    async fn concurrent_cancel_during_cancelling_is_noop() {
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend.clone()));
        let manager = Arc::new(SessionManager::new(registry, Duration::from_secs(30)));
        let parent = ctx("cancel-race-parent");
        let child = ctx("cancel-race-child");

        manager
            .checkout_child_turn(&parent, &child, agent(), None, None)
            .await
            .unwrap();
        assert!(manager.child_registered(&parent, &child).await);
        let unblock = backend.block_next_cancel();

        let first = {
            let manager = manager.clone();
            let child = child.clone();
            tokio::spawn(async move { manager.cancel(&child).await })
        };
        backend.wait_for_cancel().await;
        assert_eq!(manager.status(&child).await.unwrap().state, "cancelling");

        manager.cancel(&child).await.unwrap();
        assert_eq!(backend.cancels(), vec!["ctx-cancel-race-child-g0"]);

        unblock.send(()).unwrap();
        first.await.unwrap().unwrap();

        assert_eq!(backend.cancels(), vec!["ctx-cancel-race-child-g0"]);
        assert!(backend.releases().is_empty());
        assert_eq!(manager.status(&child).await.unwrap().state, "idle");
        assert!(manager.child_registered(&parent, &child).await);

        manager
            .checkout_child_turn(&parent, &child, agent(), None, None)
            .await
            .unwrap();
        assert_eq!(backend.configured(), vec!["ctx-cancel-race-child-g0"]);
        assert_eq!(manager.status(&child).await.unwrap().state, "running");
    }

    #[tokio::test]
    async fn cancel_backend_error_expires_handle() {
        let (manager, backend, _registry) = manager();
        let parent = ctx("cancel-error-expire-parent");
        let child = ctx("cancel-error-expire-child");

        manager
            .checkout_child_turn(&parent, &child, agent(), None, None)
            .await
            .unwrap();
        assert!(manager.child_registered(&parent, &child).await);
        backend.set_cancel_result(Err(BridgeError::AgentCrashed {
            reason: "cancel failed".into(),
        }));

        let err = manager.cancel(&child).await.err().unwrap();

        assert_eq!(
            err,
            BridgeError::AgentCrashed {
                reason: "cancel failed".into()
            }
        );
        assert_eq!(backend.cancels(), vec!["ctx-cancel-error-expire-child-g0"]);
        assert_eq!(backend.releases(), vec!["ctx-cancel-error-expire-child-g0"]);
        assert!(manager.status(&child).await.is_none());
        assert!(!manager.child_registered(&parent, &child).await);
        assert!(!manager.child_parent_registered(&parent).await);
    }

    #[tokio::test]
    async fn cancel_backend_panic_is_caught_and_expires_handle() {
        let (manager, backend, _registry) = manager();
        let c = ctx("cancel-panic-expire");
        manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        backend.set_cancel_panics();

        let error = manager.cancel(&c).await.unwrap_err();

        assert!(matches!(error, BridgeError::AgentCrashed { .. }));
        assert_eq!(backend.cancels(), vec!["ctx-cancel-panic-expire-g0"]);
        assert_eq!(backend.releases(), vec!["ctx-cancel-panic-expire-g0"]);
        assert!(manager.status(&c).await.is_none());
    }

    #[tokio::test]
    async fn cancel_failure_retains_tombstone_until_release_finishes() {
        let (manager, backend, _registry) = manager();
        let manager = Arc::new(manager);
        let c = ctx("cancel-error-tombstone");
        manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        backend.set_cancel_result(Err(BridgeError::CancelTimeout));
        let allow_release = backend.block_next_release();
        let cancel_manager = manager.clone();
        let cancel_ctx = c.clone();
        let cancel = tokio::spawn(async move { cancel_manager.cancel(&cancel_ctx).await });
        backend.wait_for_release().await;

        assert_eq!(manager.status(&c).await.unwrap().state, "expiring");
        assert_eq!(
            manager
                .checkout_turn(&c, agent(), None, None)
                .await
                .err()
                .unwrap(),
            BridgeError::SessionExpired
        );
        allow_release.send(()).unwrap();
        assert_eq!(cancel.await.unwrap(), Err(BridgeError::CancelTimeout));
        assert!(manager.status(&c).await.is_none());
        assert_eq!(backend.releases(), vec!["ctx-cancel-error-tombstone-g0"]);
    }

    #[tokio::test]
    async fn cancel_success_keeps_warm() {
        let (manager, backend, _registry) = manager();
        let c = ctx("cancel-success-warm");

        manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();

        manager.cancel(&c).await.unwrap();

        assert_eq!(backend.cancels(), vec!["ctx-cancel-success-warm-g0"]);
        assert!(backend.releases().is_empty());
        assert_eq!(manager.status(&c).await.unwrap().state, "idle");
        manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        assert_eq!(manager.status(&c).await.unwrap().state, "running");
    }

    #[tokio::test]
    async fn cancel_with_children_propagates_real_child_error() {
        let (manager, backend, _registry) = manager();
        let parent = ctx("cancel-error-parent");
        let stale_child = ctx("cancel-error-stale-child");
        let error_child = ctx("cancel-error-child");

        manager
            .checkout_child_turn(&parent, &stale_child, agent(), None, None)
            .await
            .unwrap();
        manager.release(&stale_child).await;
        manager
            .checkout_child_turn(&parent, &error_child, agent(), None, None)
            .await
            .unwrap();
        backend.set_cancel_result(Err(BridgeError::AgentCrashed {
            reason: "cancel failed".into(),
        }));

        let err = manager.cancel_with_children(&parent).await.err().unwrap();

        assert_eq!(
            err,
            BridgeError::AgentCrashed {
                reason: "cancel failed".into()
            }
        );
    }

    #[tokio::test]
    async fn concurrent_checkout_returns_handle_busy() {
        let (manager, _backend, _registry) = manager();
        let ctx = ctx("busy");

        manager
            .checkout_turn(&ctx, agent(), None, None)
            .await
            .unwrap();
        let err = manager.checkout_turn(&ctx, agent(), None, None).await.err();

        assert_eq!(err, Some(BridgeError::HandleBusy));
    }

    #[tokio::test]
    async fn model_override_change_reconciles_and_advances_fingerprint() {
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend.clone()));
        let manager = SessionManager::new(registry, Duration::from_secs(30));
        let ctx = ctx("model");

        let first = manager
            .checkout_turn(&ctx, agent(), Some(model_override("gpt-5.5")), None)
            .await
            .unwrap();
        manager.finish_turn(&ctx, first.generation, &first.op).await;
        let second = manager
            .checkout_turn(&ctx, agent(), Some(model_override("gpt-5.4")), None)
            .await;

        let second = second.unwrap();
        assert_eq!(backend.reconciled().len(), 1);
        assert_eq!(
            backend.reconciled()[0].1.config.model.as_deref(),
            Some("gpt-5.4")
        );

        manager
            .finish_turn(&ctx, second.generation, &second.op)
            .await;
        backend.set_reconcile_result(Ok(ReconcileOutcome::Rejected));
        manager
            .checkout_turn(&ctx, agent(), Some(model_override("gpt-5.4")), None)
            .await
            .unwrap();
        assert_eq!(backend.reconciled().len(), 1);
    }

    #[tokio::test]
    async fn effort_override_change_reconciles() {
        let (manager, backend, _registry) = manager();
        let ctx = ctx("effort");

        let turn = manager
            .checkout_turn(&ctx, agent(), Some(effort_override(Effort::Low)), None)
            .await
            .unwrap();
        manager.finish_turn(&ctx, turn.generation, &turn.op).await;

        let _turn = manager
            .checkout_turn(&ctx, agent(), Some(effort_override(Effort::High)), None)
            .await
            .unwrap();

        assert_eq!(backend.reconciled().len(), 1);
        assert_eq!(backend.reconciled()[0].1.config.effort, Some(Effort::High));
    }

    #[tokio::test]
    async fn reconcile_not_advertised_expires_handle_and_next_checkout_mints_cold() {
        let (manager, backend, _registry) = manager();
        let ctx = ctx("not-advertised");

        let turn = manager
            .checkout_turn(&ctx, agent(), Some(model_override("gpt-5.5")), None)
            .await
            .unwrap();
        manager.finish_turn(&ctx, turn.generation, &turn.op).await;
        backend.set_reconcile_result(Ok(ReconcileOutcome::NotAdvertised));

        let err = manager
            .checkout_turn(&ctx, agent(), Some(model_override("gpt-5.4")), None)
            .await
            .err()
            .unwrap();

        assert_eq!(err, BridgeError::ConfigReseedRequired { field: "model" });
        assert!(manager.status(&ctx).await.is_none());
        assert_eq!(backend.releases(), vec!["ctx-not-advertised-g0"]);

        manager
            .checkout_turn(&ctx, agent(), Some(model_override("gpt-5.4")), None)
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

        let turn = manager
            .checkout_turn(&ctx, agent(), Some(model_override("gpt-5.5")), None)
            .await
            .unwrap();
        manager.finish_turn(&ctx, turn.generation, &turn.op).await;
        backend.set_reconcile_result(Ok(ReconcileOutcome::Rejected));

        let err = manager
            .checkout_turn(&ctx, agent(), Some(model_override("gpt-5.4")), None)
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

        let turn = manager
            .checkout_turn(&ctx, agent(), Some(mode_override("fast")), None)
            .await
            .unwrap();
        manager.finish_turn(&ctx, turn.generation, &turn.op).await;

        let err = manager
            .checkout_turn(&ctx, agent(), Some(mode_override("slow")), None)
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

        let turn = manager
            .checkout_turn(
                &ctx,
                agent(),
                Some(model_override("gpt-5.5")),
                cwd("/work/a"),
            )
            .await
            .unwrap();
        manager.finish_turn(&ctx, turn.generation, &turn.op).await;

        let err = manager
            .checkout_turn(
                &ctx,
                agent(),
                Some(model_override("gpt-5.4")),
                cwd("/work/b"),
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

        let turn = manager
            .checkout_turn(&ctx, agent(), None, None)
            .await
            .unwrap();
        manager.finish_turn(&ctx, turn.generation, &turn.op).await;

        let err = manager
            .checkout_turn(&ctx, AgentId::parse("claude").unwrap(), None, None)
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

        let model_turn = manager
            .checkout_turn(&model_ctx, agent(), Some(model_override("gpt-5.5")), None)
            .await
            .unwrap();
        manager
            .finish_turn(&model_ctx, model_turn.generation, &model_turn.op)
            .await;
        let err = manager
            .checkout_turn(&model_ctx, agent(), None, None)
            .await
            .err()
            .unwrap();
        assert_eq!(err, BridgeError::ConfigReseedRequired { field: "model" });

        let effort_ctx = ctx("clear-effort");
        let effort_turn = manager
            .checkout_turn(
                &effort_ctx,
                agent(),
                Some(effort_override(Effort::High)),
                None,
            )
            .await
            .unwrap();
        manager
            .finish_turn(&effort_ctx, effort_turn.generation, &effort_turn.op)
            .await;
        let err = manager
            .checkout_turn(&effort_ctx, agent(), None, None)
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

        let turn = manager
            .checkout_turn(&ctx, agent(), Some(model_override("gpt-5.5")), None)
            .await
            .unwrap();
        manager.finish_turn(&ctx, turn.generation, &turn.op).await;
        let unblock = backend.block_next_reconcile();

        let in_flight = {
            let manager = manager.clone();
            let ctx = ctx.clone();
            tokio::spawn(async move {
                manager
                    .checkout_turn(&ctx, agent(), Some(model_override("gpt-5.4")), None)
                    .await
            })
        };
        backend.wait_for_reconcile().await;

        manager.release(&ctx).await;
        // During the reconcile/release window the handle is OWNED (Reconciling): a concurrent checkout must
        // be HandleBusy — no fresh re-mint of the same backend_session id mid-reconcile (closes the reuse race).
        let busy = manager
            .checkout_turn(&ctx, agent(), Some(model_override("gpt-5.3")), None)
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
            .checkout_turn(&ctx, agent(), Some(model_override("gpt-5.3")), None)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn cancel_during_reconcile_expires_claimed_handle() {
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend.clone()));
        let manager = Arc::new(SessionManager::new(registry, Duration::from_secs(30)));
        let ctx = ctx("cancel-race");

        let turn = manager
            .checkout_turn(&ctx, agent(), Some(model_override("gpt-5.5")), None)
            .await
            .unwrap();
        manager.finish_turn(&ctx, turn.generation, &turn.op).await;
        let unblock = backend.block_next_reconcile();

        let in_flight = {
            let manager = manager.clone();
            let ctx = ctx.clone();
            tokio::spawn(async move {
                manager
                    .checkout_turn(&ctx, agent(), Some(model_override("gpt-5.4")), None)
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
            .checkout_turn(&ctx, agent(), None, None)
            .await
            .unwrap();

        assert_eq!(manager.status(&ctx).await.unwrap().capabilities, caps);
    }

    #[tokio::test]
    async fn record_usage_latest_wins_stamps_at_ms() {
        let (manager, _b, _r) = manager();
        let c = ctx("u");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        manager
            .record_usage(
                &c,
                turn.generation,
                &turn.op,
                UsageSnapshot {
                    used: Some(10),
                    size: Some(100),
                    cost: None,
                    terminal: None,
                    at_ms: 0,
                },
            )
            .await;
        manager
            .record_usage(
                &c,
                turn.generation,
                &turn.op,
                UsageSnapshot {
                    used: Some(42),
                    size: Some(100),
                    cost: None,
                    terminal: None,
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
            .checkout_turn(&c, agent(), None, None)
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
        let first = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        manager
            .record_usage(
                &c,
                first.generation,
                &first.op,
                UsageSnapshot {
                    used: Some(90),
                    size: Some(100),
                    cost: None,
                    terminal: None,
                    at_ms: 0,
                },
            )
            .await;
        manager.finish_turn(&c, first.generation, &first.op).await;
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        let w = turn.usage_warning.expect("0.90 >= 0.80 warns");
        assert_eq!((w.used, w.size), (90, 100));
        assert_eq!(manager.status(&c).await.unwrap().over_threshold, Some(true));
    }

    #[tokio::test]
    async fn terminal_usage_merges_without_clobbering_window_usage() {
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend));
        let manager =
            SessionManager::new(registry, Duration::from_secs(30)).with_warn_fraction(Some(0.8));
        let c = ctx("terminal-usage-merge");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        manager
            .record_usage(
                &c,
                turn.generation,
                &turn.op,
                UsageSnapshot {
                    used: Some(90),
                    size: Some(100),
                    cost: None,
                    terminal: None,
                    at_ms: 0,
                },
            )
            .await;
        manager
            .record_usage(
                &c,
                turn.generation,
                &turn.op,
                UsageSnapshot {
                    used: None,
                    size: None,
                    cost: None,
                    terminal: Some(bridge_core::orch::TerminalUsage {
                        total_tokens: 321,
                        input_tokens: 300,
                        output_tokens: 21,
                        thought_tokens: None,
                        cached_read_tokens: None,
                        cached_write_tokens: None,
                    }),
                    at_ms: 0,
                },
            )
            .await;
        manager.finish_turn(&c, turn.generation, &turn.op).await;

        let status = manager.status(&c).await.unwrap();
        assert_eq!(
            (status.usage.used, status.usage.size),
            (Some(90), Some(100))
        );
        assert_eq!(
            status
                .usage
                .terminal
                .as_ref()
                .map(|usage| usage.total_tokens),
            Some(321)
        );
        assert_eq!(status.over_threshold, Some(true));

        let next = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        let warning = next
            .usage_warning
            .expect("merged window usage should still warn");
        assert_eq!((warning.used, warning.size), (90, 100));
    }

    #[tokio::test]
    async fn mint_never_warns_and_below_threshold_is_none() {
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend));
        let manager =
            SessionManager::new(registry, Duration::from_secs(30)).with_warn_fraction(Some(0.8));
        let c = ctx("below");
        let mint = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        assert!(mint.usage_warning.is_none(), "mint has no carried usage");
        manager
            .record_usage(
                &c,
                mint.generation,
                &mint.op,
                UsageSnapshot {
                    used: Some(10),
                    size: Some(100),
                    cost: None,
                    terminal: None,
                    at_ms: 0,
                },
            )
            .await;
        manager.finish_turn(&c, mint.generation, &mint.op).await;
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
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
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        manager
            .record_usage(
                &c,
                first.generation,
                &first.op,
                UsageSnapshot {
                    used: Some(99),
                    size: Some(100),
                    cost: None,
                    terminal: None,
                    at_ms: 0,
                },
            )
            .await;
        manager.finish_turn(&c, first.generation, &first.op).await;
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
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
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        manager
            .record_usage(
                &c,
                first.generation,
                &first.op,
                UsageSnapshot {
                    used: Some(99),
                    size: None,
                    cost: None,
                    terminal: None,
                    at_ms: 0,
                },
            )
            .await;
        manager.finish_turn(&c, first.generation, &first.op).await;
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
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
        let clock = Arc::new(ManualClock::new(0));
        let manager =
            SessionManager::new_with_clock(registry, Duration::from_secs(5), clock.clone());
        let c = ctx("idle-usage");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        manager.finish_turn(&c, turn.generation, &turn.op).await;
        clock.advance(Duration::from_secs(6));
        manager
            .record_usage(
                &c,
                turn.generation,
                &turn.op,
                UsageSnapshot {
                    used: Some(1),
                    size: Some(2),
                    cost: None,
                    terminal: None,
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
            .checkout_turn(&ctx, agent(), None, None)
            .await
            .unwrap();
        manager.finish_turn(&ctx, turn.generation, &turn.op).await;
        manager.release(&ctx).await;

        assert!(manager.status(&ctx).await.is_none());
        assert_eq!(backend.releases(), vec!["ctx-release-g0"]);
    }

    #[tokio::test]
    async fn explicit_release_retains_tombstone_until_gated_cleanup_finishes() {
        let (manager, backend, _registry) = manager();
        let manager = Arc::new(manager);
        let c = ctx("release-tombstone");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        manager.finish_turn(&c, turn.generation, &turn.op).await;
        let allow_release = backend.block_next_release();
        let release_manager = manager.clone();
        let release_ctx = c.clone();
        let release = tokio::spawn(async move { release_manager.release(&release_ctx).await });
        backend.wait_for_release().await;

        assert_eq!(manager.status(&c).await.unwrap().state, "expiring");
        assert_eq!(
            manager
                .checkout_turn(&c, agent(), None, None)
                .await
                .err()
                .unwrap(),
            BridgeError::SessionExpired
        );
        assert_eq!(backend.configured(), vec!["ctx-release-tombstone-g0"]);
        allow_release.send(()).unwrap();
        release.await.unwrap();
        assert!(manager.status(&c).await.is_none());
    }

    #[tokio::test]
    async fn reap_idle_removes_only_idle_sessions_past_ttl() {
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend));
        let clock = Arc::new(ManualClock::new(0));
        let manager =
            SessionManager::new_with_clock(registry, Duration::from_secs(5), clock.clone());
        let idle = ctx("idle");
        let running = ctx("running");

        let idle_turn = manager
            .checkout_turn(&idle, agent(), None, None)
            .await
            .unwrap();
        manager
            .finish_turn(&idle, idle_turn.generation, &idle_turn.op)
            .await;
        manager
            .checkout_turn(&running, agent(), None, None)
            .await
            .unwrap();
        clock.advance(Duration::from_secs(6));

        manager.reap_idle().await;

        assert!(manager.status(&idle).await.is_none());
        assert_eq!(manager.status(&running).await.unwrap().state, "running");
    }

    #[tokio::test]
    async fn idle_reap_retains_tombstone_until_gated_cleanup_finishes() {
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend.clone()));
        let clock = Arc::new(ManualClock::new(0));
        let manager = Arc::new(SessionManager::new_with_clock(
            registry,
            Duration::from_secs(5),
            clock.clone(),
        ));
        let c = ctx("reap-tombstone");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        manager.finish_turn(&c, turn.generation, &turn.op).await;
        clock.advance(Duration::from_secs(6));
        let allow_release = backend.block_next_release();
        let reap_manager = manager.clone();
        let reap = tokio::spawn(async move { reap_manager.reap_idle().await });
        backend.wait_for_release().await;

        assert_eq!(manager.status(&c).await.unwrap().state, "expiring");
        assert_eq!(
            manager
                .checkout_turn(&c, agent(), None, None)
                .await
                .err()
                .unwrap(),
            BridgeError::SessionExpired
        );
        allow_release.send(()).unwrap();
        reap.await.unwrap();
        assert!(manager.status(&c).await.is_none());
        assert_eq!(backend.releases(), vec!["ctx-reap-tombstone-g0"]);
    }

    #[tokio::test]
    async fn reap_idle_resolves_pending_permission() {
        let reg = PermissionRegistry::new();
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend));
        let clock = Arc::new(ManualClock::new(0));
        let manager =
            SessionManager::new_with_clock(registry, Duration::from_secs(5), clock.clone())
                .with_permission_registry(reg.clone());
        let c = ctx("reap-idle-perm");

        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        manager.finish_turn(&c, turn.generation, &turn.op).await;
        let (rx, _guard) = reg.register(
            pkey(&c, turn.generation, &turn.op, "r"),
            permission_view("r", turn.generation, &turn.op),
        );
        clock.advance(Duration::from_secs(6));

        manager.reap_idle().await;

        assert_permission_cancelled(rx).await;
        assert!(manager.status(&c).await.is_none());
    }

    #[tokio::test]
    async fn reap_idle_prunes_child_registration() {
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend));
        let clock = Arc::new(ManualClock::new(0));
        let manager =
            SessionManager::new_with_clock(registry, Duration::from_secs(5), clock.clone());
        let parent = ctx("reap-child-parent");
        let child = ctx("reap-child");

        let turn = manager
            .checkout_child_turn(&parent, &child, agent(), None, None)
            .await
            .unwrap();
        manager.finish_turn(&child, turn.generation, &turn.op).await;
        assert!(manager.child_registered(&parent, &child).await);
        assert!(manager.child_parent_registered(&parent).await);

        clock.advance(Duration::from_secs(6));
        manager.reap_idle().await;

        assert!(manager.status(&child).await.is_none());
        assert!(!manager.child_registered(&parent, &child).await);
        assert!(!manager.child_parent_registered(&parent).await);
        assert_eq!(
            manager.cancel_with_children(&parent).await.err(),
            Some(BridgeError::SessionNotFound)
        );
    }

    #[tokio::test]
    async fn compact_rejects_when_seed_pending() {
        // Whole-branch review: a second compact before the seed is delivered would summarize the empty
        // gen N+1 session and overwrite the good summary. Reject it; the original seed must survive.
        let (m, _f, _r) = manager();
        let c = ctx("c");
        let turn = m.checkout_turn(&c, agent(), None, None).await.unwrap();
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
        let t2 = m.checkout_turn(&c, agent(), None, None).await.unwrap();
        assert_eq!(t2.seed.as_deref(), Some("GOOD SUMMARY"));
    }

    #[tokio::test]
    async fn reap_idle_does_not_reap_compacting_handle() {
        // Whole-branch review: a handle claimed Compacting must survive reap_idle even past the TTL (the
        // claim owns the lifecycle; the reaper must not defer-expire it).
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend));
        let clock = Arc::new(ManualClock::new(0));
        let manager = Arc::new(SessionManager::new_with_clock(
            registry,
            Duration::from_secs(5),
            clock.clone(),
        ));
        let c = ctx("c");
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
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
            .checkout_turn(&ctx, agent(), None, None)
            .await
            .unwrap();
        manager.finish_turn(&ctx, turn.generation, &turn.op).await;
        registry.retire();
        let err = manager.checkout_turn(&ctx, agent(), None, None).await.err();

        assert_eq!(err, Some(BridgeError::SessionExpired));
    }

    #[test]
    fn noop_lease_defaults_to_not_retired() {
        assert!(!NoopLease.is_retired());
    }

    // ---- W1-B: Configuring claim state + optimistic re-check lock-scope fix ----------------------

    #[tokio::test]
    async fn different_ctx_checkout_is_not_blocked_by_a_configuring_peer() {
        // The verified serialization point: ctx A's fresh checkout is gated inside configure_session
        // (off the by_context lock per W1-B); a DIFFERENT context/agent's checkout must complete
        // promptly instead of waiting behind it.
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::with_entries(
            vec![fake_entry("codex"), fake_entry("claude")],
            backend.clone(),
        ));
        let manager = Arc::new(SessionManager::new(registry, Duration::from_secs(30)));
        let a = ctx("cfg-live-a");
        let b = ctx("cfg-live-b");

        let unblock = backend.block_next_configure();
        let in_flight_a = {
            let (m, a2) = (manager.clone(), a.clone());
            tokio::spawn(async move { m.checkout_turn(&a2, agent(), None, None).await })
        };
        backend.wait_for_configure().await;
        assert_eq!(manager.status(&a).await.unwrap().state, "configuring");

        let claude = AgentId::parse("claude").unwrap();
        let turn_b = tokio::time::timeout(
            Duration::from_secs(2),
            manager.checkout_turn(&b, claude, None, None),
        )
        .await
        .expect("a different context's checkout must not be blocked by ctx A's in-flight configure")
        .unwrap();
        assert_eq!(manager.status(&b).await.unwrap().state, "running");
        assert_eq!(
            manager.status(&a).await.unwrap().state,
            "configuring",
            "ctx A is still gated"
        );
        manager.finish_turn(&b, turn_b.generation, &turn_b.op).await;

        let _ = unblock.send(());
        let turn_a = tokio::time::timeout(Duration::from_secs(2), in_flight_a)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(manager.status(&a).await.unwrap().state, "running");
        manager.finish_turn(&a, turn_a.generation, &turn_a.op).await;
    }

    #[tokio::test]
    async fn checkout_does_not_fresh_mint_when_tombstone_replaces_handle_before_lock2() {
        // Whole-branch review MAJOR (codex xhigh, v2.1 fix): lock #1 observes a live Idle handle for ctx
        // but historically did not record that fact. If a concurrent `release` removes the handle from
        // `by_context` and its (off-lock) `backend.release_session("ctx-{ctx}-g0")` is still in flight
        // when lock #2 re-checks, lock #2 used to see NO handle and fall into the fresh path — re-minting
        // the SAME deterministic backend session id and racing `configure_session(g0)` against the
        // still-in-flight `release_session(g0)`. Assert the fixed behavior: the racing checkout returns
        // SessionExpired, and `configure_session` is called exactly once total (the original warm mint) —
        // never a second time to re-mint g0 while the release is still in flight.
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend.clone()));
        let manager = Arc::new(SessionManager::new(registry, Duration::from_secs(30)));
        let c = ctx("lock2-race");

        // Warm the context to Idle (one configure_session call) so lock #1 observes a live handle.
        let turn = manager
            .checkout_turn(&c, agent(), None, None)
            .await
            .unwrap();
        manager.finish_turn(&c, turn.generation, &turn.op).await;
        assert_eq!(manager.status(&c).await.unwrap().state, "idle");
        assert_eq!(backend.configured().len(), 1);

        // Arm the deterministic pause: the next checkout will park immediately after its off-lock resolve
        // (registry.resolve + fingerprint), before lock #2 re-checks `by_context`. Also gate
        // release_session so we can prove it is still in flight when the checkout resumes.
        let resume_checkout = manager.block_next_lock2();
        let unblock_release = backend.block_next_release();

        let checkout = {
            let manager = manager.clone();
            let c = c.clone();
            tokio::spawn(async move { manager.checkout_turn(&c, agent(), None, None).await })
        };
        manager.wait_for_lock2_pause().await;
        // Lock #1 has already run (it observed the Idle handle) and the off-lock resolve has completed;
        // the checkout is now parked immediately before lock #2.

        let release = {
            let manager = manager.clone();
            let c = c.clone();
            tokio::spawn(async move { manager.release(&c).await })
        };
        // release_inner atomically replaces the live handle with an expiring tombstone and THEN awaits
        // backend.release_session — wait_for_release only resolves once that await has been entered, so
        // by this point the live handle is provably gone, the tombstone is visible, and the release is
        // provably still parked on its gate (in flight).
        backend.wait_for_release().await;
        assert_eq!(
            manager.status(&c).await.unwrap().state,
            "expiring",
            "an expiring tombstone must replace the live handle before lock #2 resumes"
        );

        // Let the parked checkout resume into lock #2. It must see the tombstone and must NOT fall into
        // the fresh path while the release is still in flight.
        let _ = resume_checkout.send(());
        let result = tokio::time::timeout(Duration::from_secs(2), checkout)
            .await
            .expect("checkout must not deadlock behind the in-flight release")
            .unwrap();
        assert!(
            matches!(result, Err(BridgeError::SessionExpired)),
            "tombstone-at-lock-#2 must return SessionExpired, not fresh-mint"
        );

        // No second configure_session call happened while release_session(g0) was still in flight — no
        // g0 re-mint race against the in-flight release.
        assert_eq!(
            backend.configured().len(),
            1,
            "configure_session must not be called again while release_session(g0) is still in flight"
        );

        // Finally let the release complete.
        let _ = unblock_release.send(());
        tokio::time::timeout(Duration::from_secs(2), release)
            .await
            .expect("release must complete")
            .unwrap();
        assert!(
            manager.status(&c).await.is_none(),
            "successful cleanup must clear the exact expiring tombstone"
        );
    }

    #[tokio::test]
    async fn same_ctx_checkout_during_configure_is_handle_busy() {
        // Observable-behavior divergence accepted by the v2 design: a same-ctx checkout arriving during
        // the Configuring window is HandleBusy (not blocked-then-fresh-on-failure as before W1-B).
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend.clone()));
        let manager = Arc::new(SessionManager::new(registry, Duration::from_secs(30)));
        let c = ctx("cfg-busy");

        let unblock = backend.block_next_configure();
        let in_flight = {
            let (m, c2) = (manager.clone(), c.clone());
            tokio::spawn(async move { m.checkout_turn(&c2, agent(), None, None).await })
        };
        backend.wait_for_configure().await;
        assert_eq!(manager.status(&c).await.unwrap().state, "configuring");

        let busy = manager.checkout_turn(&c, agent(), None, None).await.err();
        assert_eq!(busy, Some(BridgeError::HandleBusy));
        assert_eq!(
            backend.configured().len(),
            1,
            "exactly one configure_session call must have been observed"
        );

        let _ = unblock.send(());
        let turn = tokio::time::timeout(Duration::from_secs(2), in_flight)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(backend.configured().len(), 1);
        manager.finish_turn(&c, turn.generation, &turn.op).await;
    }

    #[tokio::test]
    async fn configure_failure_settles_by_removing_the_handle_then_next_checkout_is_fresh() {
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend.clone()));
        let manager = Arc::new(SessionManager::new(registry, Duration::from_secs(30)));
        let c = ctx("cfg-fail");

        backend.set_configure_result(Err(BridgeError::ConfigInvalid {
            reason: "boom".into(),
        }));
        let unblock = backend.block_next_configure();
        let in_flight = {
            let (m, c2) = (manager.clone(), c.clone());
            tokio::spawn(async move { m.checkout_turn(&c2, agent(), None, None).await })
        };
        backend.wait_for_configure().await;
        assert_eq!(manager.status(&c).await.unwrap().state, "configuring");

        let _ = unblock.send(());
        let err = tokio::time::timeout(Duration::from_secs(2), in_flight)
            .await
            .unwrap()
            .unwrap()
            .err()
            .unwrap();
        assert!(matches!(err, BridgeError::ConfigInvalid { .. }));
        assert!(
            manager.status(&c).await.is_none(),
            "a failed configure must settle by removing the Configuring handle"
        );

        backend.set_configure_result(Ok(()));
        let turn = tokio::time::timeout(
            Duration::from_secs(2),
            manager.checkout_turn(&c, agent(), None, None),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(manager.status(&c).await.unwrap().state, "running");
        manager.finish_turn(&c, turn.generation, &turn.op).await;
    }

    #[tokio::test]
    async fn cancel_during_configure_defers_without_real_turn_cancel_or_abort_token() {
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend.clone()));
        let manager = Arc::new(SessionManager::new(registry, Duration::from_secs(30)));
        let c = ctx("cfg-cancel");

        let unblock = backend.block_next_configure();
        let in_flight = {
            let (m, c2) = (manager.clone(), c.clone());
            tokio::spawn(async move { m.checkout_turn(&c2, agent(), None, None).await })
        };
        backend.wait_for_configure().await;
        assert_eq!(manager.status(&c).await.unwrap().state, "configuring");

        manager.cancel(&c).await.unwrap();
        // No real-turn cancel path: the fake's `cancel()` (the ACP cancel RPC) is never invoked for a
        // Configuring claim, and no abort token exists to fire — no release has happened yet either
        // (settle hasn't run: configure_session is still gated).
        assert!(
            backend.cancels().is_empty(),
            "cancel during Configuring must not take the real-turn cancel path"
        );
        assert!(
            backend.releases().is_empty(),
            "cancel during Configuring must not release the backend session itself (settle owns that)"
        );
        assert_eq!(
            manager.status(&c).await.unwrap().state,
            "configuring",
            "the deferred-expiry flag does not change the observable claim state"
        );

        let _ = unblock.send(());
        let err = tokio::time::timeout(Duration::from_secs(2), in_flight)
            .await
            .unwrap()
            .unwrap()
            .err()
            .unwrap();
        assert_eq!(err, BridgeError::SessionExpired);
        assert_eq!(
            backend.releases(),
            vec!["ctx-cfg-cancel-g0"],
            "settle must best-effort release the just-configured session for the cancelled claim"
        );
        assert!(
            manager.status(&c).await.is_none(),
            "no WarmTurn escapes a cancelled Configuring claim"
        );
    }

    #[tokio::test]
    async fn force_clear_during_configure_settle_detects_replaced_claim_without_racing_release() {
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend.clone()));
        let manager = Arc::new(SessionManager::new(registry, Duration::from_secs(30)));
        let c = ctx("cfg-force-clear");

        let unblock = backend.block_next_configure();
        let in_flight = {
            let (m, c2) = (manager.clone(), c.clone());
            tokio::spawn(async move { m.checkout_turn(&c2, agent(), None, None).await })
        };
        backend.wait_for_configure().await;
        assert_eq!(manager.status(&c).await.unwrap().state, "configuring");

        let reset_out = manager
            .reset_session(&c, ResetOpts { force: true })
            .await
            .unwrap();
        assert_eq!(reset_out, ResetOutcome::Cleared { generation: 0 });
        // Force-clear during Configuring must NOT take the `Running if force` path: no direct
        // cancel()/release() against the still-being-configured backend session.
        assert!(
            backend.cancels().is_empty(),
            "force-clear during Configuring must not cancel the not-yet-real backend session directly"
        );
        assert!(
            backend.releases().is_empty(),
            "release_session must never race the in-flight configure_session"
        );

        let _ = unblock.send(());
        let err = tokio::time::timeout(Duration::from_secs(2), in_flight)
            .await
            .unwrap()
            .unwrap()
            .err()
            .unwrap();
        assert_eq!(err, BridgeError::SessionExpired);
        // release_session(old) only ran AFTER configure_session(old) had already completed (settle owns
        // it) — no concurrent release-vs-configure race.
        assert_eq!(backend.configured(), vec!["ctx-cfg-force-clear-g0"]);
        assert_eq!(backend.releases(), vec!["ctx-cfg-force-clear-g0"]);
        assert!(manager.status(&c).await.is_none());
    }

    #[tokio::test]
    async fn status_reports_configuring_during_the_window() {
        let backend = Arc::new(FakeBackend::new("ok"));
        let registry = Arc::new(FakeRegistry::new(fake_entry("codex"), backend.clone()));
        let manager = Arc::new(SessionManager::new(registry, Duration::from_secs(30)));
        let c = ctx("cfg-status");

        assert!(manager.status(&c).await.is_none(), "no handle exists yet");
        let unblock = backend.block_next_configure();
        let in_flight = {
            let (m, c2) = (manager.clone(), c.clone());
            tokio::spawn(async move { m.checkout_turn(&c2, agent(), None, None).await })
        };
        backend.wait_for_configure().await;
        assert_eq!(manager.status(&c).await.unwrap().state, "configuring");
        // `inject` accepts only Idle/Running — Configuring is rejected like the other claimed states.
        assert_eq!(
            manager
                .inject(InjectRequest {
                    context: c.clone(),
                    text: "hi".into(),
                    mode: InjectMode::AppendNextTurn,
                    dedupe_key: None,
                })
                .await,
            Err(BridgeError::HandleBusy)
        );

        let _ = unblock.send(());
        let turn = tokio::time::timeout(Duration::from_secs(2), in_flight)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(manager.status(&c).await.unwrap().state, "running");
        manager.finish_turn(&c, turn.generation, &turn.op).await;
        assert_eq!(manager.status(&c).await.unwrap().state, "idle");
    }
}
