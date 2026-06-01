mod harness;
use bridge_claude::proc::{spawn_proc, TurnEvent};
use harness::{fake, fake_default, FakeSpec};
use std::sync::atomic::Ordering;

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

/// Regression lock for the `pending_terminal` stash invariant in proc.rs.
///
/// When the reader hits EOF before any turn sender is registered (the
/// spawn→first-turn window), it stashes the terminal event in `pending_terminal`.
/// `begin_turn()` takes-and-replays it exactly ONCE — a second `begin_turn()` must
/// NOT replay the same event (the stash is consumed on first take).
///
/// Determinism: we poll `proc.terminated` (set by the reader *before* routing) to
/// learn when the stash has been written — no fixed sleeps.
#[tokio::test]
async fn stashed_terminal_replays_once() {
    // Spawn a fake that exits immediately before emitting init or reading any input.
    // spawn_proc returns immediately (deferred init), so we have a live Arc<SessionProc>
    // with the reader task already running in the background.
    let (cmd, cfg) = fake(
        "stash-replay",
        FakeSpec {
            exit_before_init: true,
            ..FakeSpec::new()
        },
    );
    let proc = spawn_proc(&cmd, &cfg)
        .await
        .expect("spawn returns immediately (deferred init)");

    // Wait until the reader has observed EOF and set `proc.terminated`. The reader
    // sets `terminated` BEFORE calling `route()`, so once this is true the stash
    // is guaranteed to be populated. We yield+sleep in a tight loop so the spawned
    // reader task gets scheduled; the fake exits immediately so this is fast.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if proc.terminated.load(Ordering::SeqCst) {
            break;
        }
        if std::time::Instant::now() >= deadline {
            panic!("reader should have hit EOF and set terminated within 5s");
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    // --- First begin_turn: stash MUST be replayed ---
    let _g = proc.turn_lock.clone().lock_owned().await;
    let mut rx1 = proc.begin_turn();

    // The stashed terminal must be the first (and only) event on the channel.
    let ev1 = tokio::time::timeout(std::time::Duration::from_secs(1), rx1.recv())
        .await
        .expect("recv should not time out (stash was pre-populated)")
        .expect("channel must yield the stashed event");

    assert!(
        matches!(
            ev1,
            TurnEvent::Failed(bridge_core::error::BridgeError::AgentNotAuthenticated)
        ),
        "first begin_turn must replay Failed(AgentNotAuthenticated) from stash, got {ev1:?}"
    );
    proc.end_turn();

    // --- Second begin_turn: stash MUST be empty (consumed once) ---
    let mut rx2 = proc.begin_turn();

    // The channel should be immediately empty (no replay). Use try_recv to avoid
    // blocking, and confirm Nothing is queued.
    let result2 = rx2.try_recv();
    assert!(
        result2.is_err(),
        "second begin_turn must NOT replay the already-consumed stash event, got {result2:?}"
    );
    proc.end_turn();
}
