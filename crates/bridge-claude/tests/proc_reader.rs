mod harness;
use bridge_claude::proc::{spawn_proc, TurnEvent};
use harness::{fake, fake_default, FakeSpec};

#[tokio::test]
async fn spawn_reads_init_then_serves_a_turn() {
    let (cmd, cfg) = fake_default("proc-serves");
    // Deferred init: spawn returns immediately, before init is captured.
    let proc = spawn_proc(&cmd, &cfg)
        .await
        .expect("spawn returns immediately");
    // Drive one turn manually.
    let _g = proc.turn_lock.clone().lock_owned().await;
    let mut rx = proc.begin_turn();
    proc.write_turn("Remember the number 7").await.unwrap();
    let mut got_text = false;
    let mut done = false;
    while let Some(ev) = rx.recv().await {
        match ev {
            TurnEvent::Text(t) => {
                assert_eq!(t, "7");
                got_text = true;
            }
            TurnEvent::Done { .. } => {
                done = true;
                break;
            }
            TurnEvent::Failed(e) => panic!("unexpected failure: {e}"),
        }
    }
    proc.end_turn();
    assert!(got_text && done);
    // By now the reader has processed the init line emitted during the turn.
    assert_eq!(
        proc.claude_session_id.lock().unwrap().as_deref(),
        Some("fake-sid")
    );
}

#[tokio::test]
async fn eof_before_init_maps_to_not_authenticated() {
    // exit_before_init: the fake closes stdout before emitting init or reading.
    // Deferred init means spawn_proc now SUCCEEDS; the not-authenticated signal
    // surfaces on the first turn via the reader's EOF-before-init path.
    let (cmd, cfg) = fake(
        "proc-exit-before-init",
        FakeSpec {
            exit_before_init: true,
            ..FakeSpec::new()
        },
    );
    let proc = spawn_proc(&cmd, &cfg)
        .await
        .expect("spawn returns immediately (deferred init)");
    let _g = proc.turn_lock.clone().lock_owned().await;
    let mut rx = proc.begin_turn();
    // write may race the process exit; either way the reader routes the failure
    // (replayed by begin_turn if it landed before the turn was registered).
    let _ = proc.write_turn("hello").await;
    // Drain to the first terminal event.
    let mut terminal = None;
    while let Some(ev) = rx.recv().await {
        match ev {
            TurnEvent::Text(_) => continue,
            ev => {
                terminal = Some(ev);
                break;
            }
        }
    }
    proc.end_turn();
    assert!(
        matches!(
            terminal,
            Some(TurnEvent::Failed(
                bridge_core::error::BridgeError::AgentNotAuthenticated
            ))
        ),
        "expected Failed(AgentNotAuthenticated), got {terminal:?}"
    );
}
