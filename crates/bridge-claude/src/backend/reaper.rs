//! Warm-pool reaper: idle-TTL + max_warm LRU eviction, reaper-holds-turn-lock.
use super::Inner;
use std::sync::Arc;

/// Spawn the background reaper task. Filled with real logic in Task 9; for Task 8
/// it is a no-op loop so the backend compiles and prompt tests run.
pub(crate) fn spawn_reaper(inner: Arc<Inner>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let _ = inner; // Task 9 replaces this body.
        futures::future::pending::<()>().await;
    })
}

/// Try to reap exactly one idle proc to make admission room. Stub: returns false.
pub(crate) async fn reap_one_idle(inner: &Inner) -> bool {
    let _ = inner;
    false
}
