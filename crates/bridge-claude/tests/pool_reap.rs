mod harness;
use bridge_claude::ClaudeCliBackend;
use bridge_core::domain::Part;
use bridge_core::ids::SessionId;
use bridge_core::ports::{AgentBackend, Update};
use futures::StreamExt;
use harness::fake_default;

fn sid(s: &str) -> SessionId {
    SessionId::parse(s).unwrap()
}
fn tp(s: &str) -> Vec<Part> {
    vec![Part { text: s.into() }]
}

async fn one_turn(be: &ClaudeCliBackend, s: &SessionId) {
    let mut st = be.prompt(s, tp("hi")).await.unwrap();
    while let Some(i) = st.next().await {
        if matches!(i, Ok(Update::Done { .. }) | Err(_)) {
            break;
        }
    }
}

#[tokio::test]
async fn idle_ttl_reaps_idle_not_busy() {
    let (cmd, mut cfg) = fake_default("pool-idle");
    cfg.idle_ttl = std::time::Duration::from_millis(1); // reap almost immediately
    let be = ClaudeCliBackend::spawn(&cmd, cfg).await.unwrap();
    let s = sid("session-idle");
    one_turn(&be, &s).await; // proc now warm + idle
    assert_eq!(be.live_session_count().await, 1);
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    be.reap_now().await; // idle past TTL → reaped
    assert_eq!(be.live_session_count().await, 0, "idle proc reaped");
    // A follow-up respawns cold and still works.
    one_turn(&be, &s).await;
    assert_eq!(be.live_session_count().await, 1, "respawned on next prompt");
}

#[tokio::test]
async fn max_warm_lru_evicts_over_cap() {
    let (cmd, mut cfg) = fake_default("pool-lru");
    cfg.max_warm = 1;
    cfg.idle_ttl = std::time::Duration::from_secs(9999); // disable TTL path
    let be = ClaudeCliBackend::spawn(&cmd, cfg).await.unwrap();
    one_turn(&be, &sid("session-a")).await;
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    one_turn(&be, &sid("session-b")).await; // b is newer; a is more idle
    assert_eq!(be.live_session_count().await, 2);
    be.reap_now().await; // over cap → evict the most-idle (a)
    assert_eq!(
        be.live_session_count().await,
        1,
        "LRU evicted one over the cap"
    );
}
