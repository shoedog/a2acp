//! Warm-pool reaper: idle-TTL + max_warm LRU eviction. Every termination path
//! goes through the reaper protocol — `try_lock` the proc's turn lock and HOLD it
//! while invalidating (so a racing prompt observes `terminated` after it finally
//! gets the lock). A proc mid-turn (try_lock fails) is skipped this pass. Every
//! `invalidate_slot` call passes the EXACT slot Arc (identity check, B1).
use super::{ClaudeCliBackend, Inner};
use crate::proc::{SessionProc, SessionSlot};
use bridge_core::ids::SessionId;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

/// One candidate: session id + the slot Arc (for the identity-checked invalidation)
/// + its proc + idle duration.
type Candidate = (SessionId, Arc<SessionSlot>, Arc<SessionProc>, Duration);

fn snapshot_live(map: &std::collections::HashMap<SessionId, Arc<SessionSlot>>) -> Vec<Candidate> {
    map.iter()
        .filter_map(|(sid, slot)| {
            slot.proc.get().and_then(|p| {
                if p.terminated.load(Ordering::SeqCst) {
                    None
                } else {
                    Some((sid.clone(), Arc::clone(slot), Arc::clone(p), p.idle_for()))
                }
            })
        })
        .collect()
}

/// Attempt to invalidate this slot's proc under the reaper protocol. Returns true
/// if reaped (turn lock was free), false if mid-turn (skipped). Passes the exact
/// `slot` Arc so a stale reap can't remove a freshly-respawned slot (B1).
async fn try_reap(
    inner: &Inner,
    session: &SessionId,
    slot: &Arc<SessionSlot>,
    proc: &Arc<SessionProc>,
) -> bool {
    let guard = match proc.turn_lock.try_lock() {
        Ok(g) => g,
        Err(_) => return false, // mid-turn → skip
    };
    proc.terminated.store(true, Ordering::SeqCst);
    ClaudeCliBackend::invalidate_slot(inner, session, slot).await;
    drop(guard);
    true
}

/// Reap exactly one idle proc (used by the admission gate to make room). Picks the
/// most-idle reapable proc.
pub(crate) async fn reap_one_idle(inner: &Inner) -> bool {
    let mut cands = {
        let map = inner.sessions.lock().await;
        snapshot_live(&map)
    };
    cands.sort_by_key(|(_, _, _, idle)| std::cmp::Reverse(*idle));
    for (sid, slot, proc, _) in cands {
        if try_reap(inner, &sid, &slot, &proc).await {
            return true;
        }
    }
    false
}

async fn reap_pass(inner: &Inner) {
    // 1) Idle-TTL: reap anything idle past the TTL.
    let cands = {
        let map = inner.sessions.lock().await;
        snapshot_live(&map)
    };
    let ttl = inner.config.idle_ttl;
    for (sid, slot, proc, idle) in &cands {
        if *idle > ttl {
            let _ = try_reap(inner, sid, slot, proc).await;
        }
    }
    // 2) max_warm is an IDLE-retention cap (spec §3.3): count + evict ONLY idle
    // procs. A busy turn must never be evicted NOR inflate the count (review #4) —
    // otherwise a burst of concurrent turns would evict a legitimately-warm idle proc.
    let mut idle: Vec<_> = {
        let map = inner.sessions.lock().await;
        snapshot_live(&map)
            .into_iter()
            .filter(|(_, _, proc, _)| proc.is_idle())
            .collect()
    };
    if idle.len() > inner.config.max_warm {
        idle.sort_by_key(|(_, _, _, idle_dur)| std::cmp::Reverse(*idle_dur));
        let over = idle.len() - inner.config.max_warm;
        let mut evicted = 0;
        for (sid, slot, proc, _) in idle {
            if evicted >= over {
                break;
            }
            if try_reap(inner, &sid, &slot, &proc).await {
                evicted += 1;
            }
        }
    }
}

pub(crate) fn spawn_reaper(inner: Arc<Inner>) -> tokio::task::JoinHandle<()> {
    let interval = inner.config.reaper_interval;
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            reap_pass(&inner).await;
        }
    })
}

/// Test-only: run a single reap pass synchronously.
#[doc(hidden)]
pub async fn reap_pass_for_test(inner: &Inner) {
    reap_pass(inner).await
}
