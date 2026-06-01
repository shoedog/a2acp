mod harness;
use bridge_claude::proc::{spawn_proc, TurnEvent};
use harness::{fake, fake_default, FakeSpec};

#[tokio::test]
async fn spawn_reads_init_then_serves_a_turn() {
    let (cmd, cfg) = fake_default("proc-serves");
    let proc = spawn_proc(&cmd, &cfg)
        .await
        .expect("spawn + init within timeout");
    // claude_session_id captured from the init line.
    assert_eq!(
        proc.claude_session_id.lock().unwrap().as_deref(),
        Some("fake-sid")
    );
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
}

#[tokio::test]
async fn init_timeout_maps_to_not_authenticated() {
    // no_init: the fake never emits an init line → reader sees no init → bounded error.
    let (cmd, mut cfg) = fake(
        "proc-noinit",
        FakeSpec {
            no_init: true,
            ..FakeSpec::new()
        },
    );
    cfg.init_timeout = std::time::Duration::from_millis(300);
    let r = spawn_proc(&cmd, &cfg).await;
    assert!(matches!(
        r,
        Err(bridge_core::error::BridgeError::AgentNotAuthenticated)
    ));
}
