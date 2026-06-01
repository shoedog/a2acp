//! ClaudeCliBackend: warm `claude` process per SessionId behind the AgentBackend
//! trait, with a bounded warm-pool (idle-TTL / max_warm LRU / hard max_sessions)
//! and a single `invalidate_slot` teardown reused by reap/LRU/cancel/timeout.
use crate::config::ClaudeConfig;
use crate::proc::{spawn_proc, SessionProc, SessionSlot, TurnEvent};
use async_trait::async_trait;
use bridge_core::domain::{EffectiveConfig, Part};
use bridge_core::error::BridgeError;
use bridge_core::ids::SessionId;
use bridge_core::ports::{AgentBackend, BackendStream, Update, STOP_REASON_CANCELLED};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::Mutex;

pub(crate) struct Inner {
    pub cmd: String,
    pub config: ClaudeConfig,
    pub sessions: Mutex<HashMap<SessionId, Arc<SessionSlot>>>,
    pub session_cfg: StdMutex<HashMap<SessionId, EffectiveConfig>>,
}

pub struct ClaudeCliBackend {
    pub(crate) inner: Arc<Inner>,
    #[allow(dead_code)] // held to keep the reaper task alive; Task 11 will abort it in retire()
    reaper: StdMutex<Option<tokio::task::JoinHandle<()>>>,
}

impl ClaudeCliBackend {
    /// Build the backend and start the warm-pool reaper.
    pub async fn spawn(cmd: &str, config: ClaudeConfig) -> Result<Self, BridgeError> {
        let inner = Arc::new(Inner {
            cmd: cmd.to_string(),
            config,
            sessions: Mutex::new(HashMap::new()),
            session_cfg: StdMutex::new(HashMap::new()),
        });
        let reaper = crate::backend::reaper::spawn_reaper(Arc::clone(&inner));
        Ok(Self {
            inner,
            reaper: StdMutex::new(Some(reaper)),
        })
    }

    /// Count live (non-terminated, minted) warm procs.
    pub(crate) async fn live_count(inner: &Inner) -> usize {
        let map = inner.sessions.lock().await;
        map.values()
            .filter(|s| {
                s.proc
                    .get()
                    .map(|p| !p.terminated.load(Ordering::SeqCst))
                    .unwrap_or(false)
            })
            .count()
    }

    /// Test observability: number of live warm procs.
    #[doc(hidden)]
    pub async fn live_session_count(&self) -> usize {
        Self::live_count(&self.inner).await
    }

    /// Remove the map entry for `session` ONLY if its current value is the SAME
    /// Arc the caller holds (`Arc::ptr_eq`), then terminate that slot's proc. THE
    /// single teardown primitive (reap/LRU/cancel/turn-timeout/abandoned-turn).
    /// The identity check closes the ABA race: a stale reap must not remove a
    /// freshly-respawned slot inserted under the same SessionId after the old one
    /// was invalidated (mirrors 3b's `Arc::ptr_eq` retire). The caller owns the
    /// turn-lock discipline (reaper holds it; cancel sets the latch first).
    pub(crate) async fn invalidate_slot(
        inner: &Inner,
        session: &SessionId,
        expected: &Arc<SessionSlot>,
    ) {
        let removed = {
            let mut map = inner.sessions.lock().await;
            match map.get(session) {
                Some(cur) if Arc::ptr_eq(cur, expected) => map.remove(session),
                _ => None, // a different (fresh) slot is installed — do not touch it
            }
        };
        if let Some(slot) = removed {
            if let Some(proc) = slot.proc.get() {
                proc.terminate(inner.config.cancel_grace).await;
            }
        }
    }

    /// Get-or-insert the slot for a session. The hard `max_sessions` admission gate
    /// is enforced ATOMICALLY under one map-lock guard: the live-or-pending count
    /// (an un-minted slot counts as a reservation, `unwrap_or(true)`) and the insert
    /// happen without releasing the lock, so concurrent new sessions cannot all see
    /// capacity and oversubscribe (B2).
    async fn slot_for(&self, session: &SessionId) -> Result<Arc<SessionSlot>, BridgeError> {
        for _ in 0..2 {
            let mut map = self.inner.sessions.lock().await;
            if let Some(s) = map.get(session) {
                return Ok(Arc::clone(s)); // existing session (follow-up) — never gated
            }
            let occupied = map
                .values()
                .filter(|s| {
                    s.proc
                        .get()
                        .map(|p| !p.terminated.load(Ordering::SeqCst))
                        .unwrap_or(true) // un-minted slot = a pending reservation
                })
                .count();
            if occupied < self.inner.config.max_sessions {
                let slot = Arc::new(SessionSlot::new());
                map.insert(session.clone(), Arc::clone(&slot));
                return Ok(slot);
            }
            drop(map); // at cap — release before the (locking) reap attempt
            if !reaper::reap_one_idle(&self.inner).await {
                return Err(BridgeError::AgentOverloaded);
            }
            // a reap freed a seat → loop once to re-acquire + insert
        }
        Err(BridgeError::AgentOverloaded)
    }

    /// Get the slot's warm proc, lazily spawning exactly once.
    async fn proc_for(
        &self,
        slot: &Arc<SessionSlot>,
        eff: Option<&EffectiveConfig>,
    ) -> Result<Arc<SessionProc>, BridgeError> {
        let mut cfg = self.inner.config.clone();
        if let Some(e) = eff {
            if let Some(m) = &e.model {
                cfg.model = Some(m.clone());
            }
        }
        let cmd = self.inner.cmd.clone();
        slot.proc
            .get_or_try_init(|| async { spawn_proc(&cmd, &cfg).await })
            .await
            .cloned()
    }
}

#[async_trait]
impl AgentBackend for ClaudeCliBackend {
    async fn prompt(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
    ) -> Result<BackendStream, BridgeError> {
        let text = parts_to_text(&parts);
        let eff = self
            .inner
            .session_cfg
            .lock()
            .ok()
            .and_then(|m| m.get(session).cloned());

        // Resolve slot + proc, then the post-lock revalidation loop (§3.2): if a reap
        // terminated the proc in the window before we held the turn lock, drop the
        // dead handle, invalidate THAT slot (by identity), and respawn — bounded.
        let mut attempts = 0;
        let (slot, proc, turn_lock) = loop {
            attempts += 1;
            let slot = self.slot_for(session).await?;
            let proc = match self.proc_for(&slot, eff.as_ref()).await {
                Ok(p) => p,
                Err(e) => {
                    // Spawn failed: drop the empty reservation slot so it doesn't hold
                    // an admission seat forever, then surface the error.
                    Self::invalidate_slot(&self.inner, session, &slot).await;
                    return Err(e);
                }
            };
            let lock = Arc::clone(&proc.turn_lock).lock_owned().await;
            // Cancel that landed during setup: the SLOT-level latch survives the
            // no-proc-yet window, so check it FIRST (before terminated/respawn) and
            // end the turn Canceled without running it (review #2a). We hold the same
            // slot Arc cancel() flagged, even after its invalidate removed the entry.
            if slot.cancel_requested.load(Ordering::SeqCst) {
                drop(lock);
                Self::invalidate_slot(&self.inner, session, &slot).await;
                return Ok(Box::pin(futures::stream::once(async {
                    Ok(Update::Done {
                        stop_reason: STOP_REASON_CANCELLED.into(),
                    })
                })));
            }
            if !proc.terminated.load(Ordering::SeqCst) {
                break (slot, proc, lock);
            }
            drop(lock);
            Self::invalidate_slot(&self.inner, session, &slot).await;
            if attempts >= 3 {
                return Err(BridgeError::AgentCrashed);
            }
            // loop → fresh slot + cold respawn
        };

        // TurnGuard holds the turn lock for the whole turn and, if the stream is
        // dropped before a terminal (client disconnect mid-turn), tears the proc down
        // so no stale output leaks into the next turn and the lock isn't left over a
        // zombie turn (B3). On a clean terminal we call complete() → proc stays warm.
        let guard = TurnGuard {
            _lock: turn_lock,
            proc: Arc::clone(&proc),
            slot: Arc::clone(&slot),
            inner: Arc::clone(&self.inner),
            session: session.clone(),
            completed: AtomicBool::new(false),
        };
        let inner = Arc::clone(&self.inner);
        let session_id = session.clone();
        let turn_timeout = self.inner.config.turn_timeout;

        let stream = async_stream::stream! {
            // `guard` is moved in (held for the whole turn). NOTE: we do NOT reset
            // proc.cancel_requested here — a cancelled proc is always invalidated and
            // removed, so a stale latch cannot leak into a later turn; resetting it
            // would instead drop a cancel that landed between prompt() returning and
            // this first poll, surfacing it as Failed (review #2b / Cl#1).
            let mut rx = proc.begin_turn();
            if let Err(e) = proc.write_turn(&text).await {
                if proc.cancel_requested.load(Ordering::SeqCst) {
                    guard.complete();
                    yield Ok(Update::Done { stop_reason: STOP_REASON_CANCELLED.into() });
                } else {
                    ClaudeCliBackend::invalidate_slot(&inner, &session_id, &slot).await;
                    guard.complete();
                    yield Err(e);
                }
                return;
            }
            proc.touch();
            let deadline = tokio::time::sleep(turn_timeout);
            tokio::pin!(deadline);
            loop {
                tokio::select! {
                    maybe = rx.recv() => match maybe {
                        Some(TurnEvent::Text(t)) => yield Ok(Update::Text(t)),
                        Some(TurnEvent::Done { stop_reason }) => {
                            proc.touch();
                            guard.complete(); // proc stays warm
                            yield Ok(Update::Done { stop_reason });
                            break;
                        }
                        Some(TurnEvent::Failed(e)) => {
                            ClaudeCliBackend::invalidate_slot(&inner, &session_id, &slot).await;
                            guard.complete();
                            yield Err(e);
                            break;
                        }
                        None => {
                            ClaudeCliBackend::invalidate_slot(&inner, &session_id, &slot).await;
                            guard.complete();
                            yield Err(BridgeError::AgentCrashed);
                            break;
                        }
                    },
                    _ = &mut deadline => {
                        // Per-turn timeout: tear the proc down + drop the slot so the
                        // next prompt respawns; surface Failed.
                        ClaudeCliBackend::invalidate_slot(&inner, &session_id, &slot).await;
                        guard.complete();
                        yield Err(BridgeError::AgentCrashed);
                        break;
                    }
                }
            }
            // guard drops here; complete() was called → no extra teardown.
        };
        Ok(Box::pin(stream))
    }

    async fn cancel(&self, session: &SessionId) -> Result<(), BridgeError> {
        // Filled in Task 10.
        let _ = session;
        Ok(())
    }

    async fn configure_session(
        &self,
        session: &SessionId,
        cfg: &EffectiveConfig,
    ) -> Result<(), BridgeError> {
        if let Ok(mut m) = self.inner.session_cfg.lock() {
            m.insert(session.clone(), cfg.clone());
        }
        Ok(())
    }

    /// DROPS ONLY THE CONFIG STASH — never kills the process (mirrors AcpBackend
    /// acp_backend.rs:1370). The warm proc survives across same-TaskId turns; only
    /// the pool reaps it. THIS is the blocker fix (§3.1).
    async fn forget_session(&self, session: &SessionId) {
        if let Ok(mut m) = self.inner.session_cfg.lock() {
            m.remove(session);
        }
    }

    async fn retire(&self) -> Result<(), BridgeError> {
        // Filled in Task 11.
        Ok(())
    }
}

fn parts_to_text(parts: &[Part]) -> String {
    // `Part` is `struct Part { pub text: String }` (bridge-core/src/domain.rs:6-8).
    // The backend only sends text envelopes; concatenate the parts' text.
    parts
        .iter()
        .map(|p| p.text.as_str())
        .collect::<Vec<_>>()
        .join("")
}

/// Holds the per-session turn lock for the duration of a turn. On drop, if the
/// turn did not reach a terminal (`complete()` never called — e.g. the consumer
/// dropped the BackendStream mid-turn on client disconnect), it terminates the proc
/// and invalidates the slot so no stale agent output routes into a later turn (B3).
struct TurnGuard {
    _lock: tokio::sync::OwnedMutexGuard<()>,
    proc: Arc<SessionProc>,
    slot: Arc<SessionSlot>,
    inner: Arc<Inner>,
    session: SessionId,
    completed: AtomicBool,
}
impl TurnGuard {
    fn complete(&self) {
        self.completed.store(true, Ordering::SeqCst);
    }
}
impl Drop for TurnGuard {
    fn drop(&mut self) {
        self.proc.end_turn();
        if !self.completed.load(Ordering::SeqCst) {
            self.proc.terminated.store(true, Ordering::SeqCst);
            let inner = Arc::clone(&self.inner);
            let session = self.session.clone();
            let slot = Arc::clone(&self.slot);
            tokio::spawn(async move {
                ClaudeCliBackend::invalidate_slot(&inner, &session, &slot).await;
            });
        }
    }
}

pub(crate) mod reaper;
