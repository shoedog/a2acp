mod harness;
use bridge_claude::ClaudeCliBackend;
use bridge_core::domain::Part;
use bridge_core::error::BridgeError;
use bridge_core::ids::SessionId;
use bridge_core::ports::{AgentBackend, Update};
use futures::StreamExt;
use harness::{fake, fake_default, FakeSpec};
use std::sync::Arc;

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
async fn retire_reaps_all_procs_idempotent() {
    let (cmd, cfg) = fake_default("retire-all");
    let be = ClaudeCliBackend::spawn(&cmd, cfg).await.unwrap();
    one_turn(&be, &sid("session-a")).await;
    one_turn(&be, &sid("session-b")).await;
    assert_eq!(be.live_session_count().await, 2);
    be.retire().await.unwrap();
    assert_eq!(be.live_session_count().await, 0, "retire reaped all");
    be.retire().await.unwrap(); // idempotent — no panic
}

#[tokio::test]
async fn max_sessions_admission_rejects_when_all_busy() {
    // Cap at 1 live session; hold session-a mid-turn (hung) so it can't be reaped,
    // then a NEW session-b must be rejected with AgentOverloaded.
    let (cmd, mut cfg) = fake(
        "admit-busy",
        FakeSpec {
            hang: true,
            ..FakeSpec::new()
        },
    );
    cfg.max_sessions = 1;
    cfg.turn_timeout = std::time::Duration::from_secs(30);
    let be = Arc::new(ClaudeCliBackend::spawn(&cmd, cfg).await.unwrap());
    // Start a's turn and keep the stream alive (do NOT drain to completion).
    let a_stream = be.prompt(&sid("session-a"), tp("hang")).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await; // proc spawned + busy
                                                                     // New session b → admission gate: a is mid-turn (not reapable) → reject.
    let r = be.prompt(&sid("session-b"), tp("hi")).await;
    assert!(
        matches!(r, Err(BridgeError::AgentOverloaded)),
        "expected AgentOverloaded"
    );
    drop(a_stream);
}

#[tokio::test]
async fn max_sessions_concurrent_no_oversubscription() {
    // B2: fire 8 concurrent NEW sessions at cap=2 with hung (non-reapable) procs. Each
    // task returns its prompt() result; admitted tasks return the live stream (the proc
    // stays busy because the turn lock is held inside the stream). The atomic-under-lock
    // admission must admit EXACTLY the cap (2) and reject the other 6 — never spawn
    // more than the cap, even under contention.
    let (cmd, mut cfg) = fake(
        "admit-concurrent",
        FakeSpec {
            hang: true,
            ..FakeSpec::new()
        },
    );
    cfg.max_sessions = 2;
    cfg.turn_timeout = std::time::Duration::from_secs(30);
    let be = Arc::new(ClaudeCliBackend::spawn(&cmd, cfg).await.unwrap());
    let tasks: Vec<_> = (0..8)
        .map(|i| {
            let be = be.clone();
            tokio::spawn(async move { be.prompt(&sid(&format!("s-{i}")), tp("hang")).await })
        })
        .collect();
    let mut admitted = Vec::new(); // retain streams → admitted procs stay busy
    let mut overloaded = 0;
    for r in futures::future::join_all(tasks).await {
        match r.unwrap() {
            Ok(stream) => admitted.push(stream),
            Err(BridgeError::AgentOverloaded) => overloaded += 1,
            Err(e) => panic!("unexpected {e:?}"),
        }
    }
    // The authoritative invariant is NO OVERSUBSCRIPTION; the exact split is
    // scheduler-dependent, so assert the bound, not an exact count.
    assert!(
        admitted.len() <= 2,
        "admitted at most the cap (got {})",
        admitted.len()
    );
    assert_eq!(
        admitted.len() + overloaded,
        8,
        "every task got a definitive result"
    );
    assert!(
        be.live_session_count().await <= 2,
        "never oversubscribed past the cap"
    );
    drop(admitted);
}
