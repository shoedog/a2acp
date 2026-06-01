mod harness;
use bridge_claude::ClaudeCliBackend;
use bridge_core::domain::Part;
use bridge_core::ids::SessionId;
use bridge_core::ports::{AgentBackend, Update};
use futures::StreamExt;
use harness::fake_default;
use std::sync::Arc;

fn sid(s: &str) -> SessionId {
    SessionId::parse(s).unwrap()
}
fn tp(s: &str) -> Vec<Part> {
    vec![Part { text: s.into() }]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reap_racing_a_followup_respawns_not_crashes() {
    let (cmd, cfg) = fake_default("reap-race");
    let be = Arc::new(ClaudeCliBackend::spawn(&cmd, cfg).await.unwrap());
    let s = sid("session-race");
    // Warm the proc with one completed turn.
    {
        let mut st = be.prompt(&s, tp("Remember the number 7")).await.unwrap();
        while let Some(i) = st.next().await {
            if matches!(i, Ok(Update::Done { .. }) | Err(_)) {
                break;
            }
        }
    }
    // Arm the gate: the NEXT prompt parks at the seam after cloning the proc.
    let (entered, release) = be.arm_race_gate();
    let be2 = be.clone();
    let s2 = s.clone();
    let prompt_task = tokio::spawn(async move {
        let mut st = be2.prompt(&s2, tp("What number?")).await.unwrap();
        let mut out = Vec::new();
        while let Some(i) = st.next().await {
            match i {
                Ok(Update::Text(t)) => out.push(t),
                Ok(Update::Done { .. }) => break,
                Err(e) => panic!("spurious crash: {e}"),
                _ => {}
            }
        }
        out
    });
    // DETERMINISTIC: wait until the prompt has actually PARKED at the seam.
    entered.notified().await;
    // Reap the proc out from under the parked prompt.
    be.reap_now_force(&s).await;
    // Release the parked prompt: it acquires the stale lock, observes terminated,
    // respawns a fresh proc, and the turn SUCCEEDS (no spurious crash).
    release.notify_one();
    let out = prompt_task.await.unwrap();
    assert!(
        !out.is_empty(),
        "respawned turn produced output (no spurious crash)"
    );
}
