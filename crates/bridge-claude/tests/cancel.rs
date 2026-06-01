mod harness;
use bridge_claude::proc::{spawn_proc, TurnEvent};
use bridge_claude::ClaudeCliBackend;
use bridge_core::domain::Part;
use bridge_core::ids::SessionId;
use bridge_core::ports::{AgentBackend, Update};
use futures::StreamExt;
use harness::{fake, FakeSpec};
use std::sync::atomic::Ordering;

fn sid(s: &str) -> SessionId {
    SessionId::parse(s).unwrap()
}
fn tp(s: &str) -> Vec<Part> {
    vec![Part { text: s.into() }]
}

#[tokio::test]
async fn cancel_during_hung_turn_yields_cancelled_not_failed() {
    let (cmd, mut cfg) = fake(
        "cancel-hang",
        FakeSpec {
            hang: true,
            ..FakeSpec::new()
        },
    );
    cfg.turn_timeout = std::time::Duration::from_secs(30); // don't let the timeout race us
    let be = std::sync::Arc::new(ClaudeCliBackend::spawn(&cmd, cfg).await.unwrap());
    let s = sid("session-cancel");
    let mut stream = be.prompt(&s, tp("hang")).await.unwrap();
    let be2 = be.clone();
    let s2 = s.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        be2.cancel(&s2).await.unwrap();
    });
    let mut terminal = None;
    while let Some(item) = stream.next().await {
        match item {
            Ok(Update::Done { stop_reason }) => {
                terminal = Some(stop_reason);
                break;
            }
            Err(_) => {
                terminal = Some("__failed__".into());
                break;
            }
            _ => {}
        }
    }
    assert_eq!(
        terminal.as_deref(),
        Some("cancelled"),
        "cancel (kill->EOF) maps to Canceled, not Failed"
    );
}

#[tokio::test]
async fn reader_maps_error_result_to_cancelled_when_latch_set() {
    // Proc-level precedence test (spec section 4, review #6): with cancel_requested
    // set, a terminal result carrying an ERROR subtype must map to Done{cancelled},
    // NOT Failed. Drive the proc directly so the reader's `ResultErr if
    // cancel_requested` branch is exercised deterministically (the EOF path is
    // covered by the previous test).
    let (cmd, cfg) = fake(
        "cancel-errsub",
        FakeSpec {
            result_err: Some("error_during_execution".into()),
            ..FakeSpec::new()
        },
    );
    let proc = spawn_proc(&cmd, &cfg).await.unwrap();
    let _g = proc.turn_lock.clone().lock_owned().await;
    proc.cancel_requested.store(true, Ordering::SeqCst); // latch set BEFORE the turn
    let mut rx = proc.begin_turn();
    proc.write_turn("go").await.unwrap();
    let mut terminal = None;
    while let Some(ev) = rx.recv().await {
        match ev {
            TurnEvent::Done { stop_reason } => {
                terminal = Some(stop_reason);
                break;
            }
            TurnEvent::Failed(_) => {
                terminal = Some("__failed__".into());
                break;
            }
            TurnEvent::Text(_) => {}
        }
    }
    proc.end_turn();
    assert_eq!(
        terminal.as_deref(),
        Some("cancelled"),
        "error-subtype result with the cancel latch -> cancelled (precedence over Failed)"
    );
}
