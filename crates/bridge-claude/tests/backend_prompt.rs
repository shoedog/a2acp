mod harness;
use bridge_claude::ClaudeCliBackend;
use bridge_core::domain::Part;
use bridge_core::ids::SessionId;
use bridge_core::ports::{AgentBackend, Update};
use futures::StreamExt;
use harness::{fake, fake_default, FakeSpec};

fn sid(s: &str) -> SessionId {
    SessionId::parse(s).unwrap()
}
fn tp(s: &str) -> Vec<Part> {
    vec![Part { text: s.into() }]
}

async fn drain(be: &ClaudeCliBackend, s: &SessionId, msg: &str) -> Vec<String> {
    let mut stream = be.prompt(s, tp(msg)).await.unwrap();
    let mut out = Vec::new();
    while let Some(item) = stream.next().await {
        match item.unwrap() {
            Update::Text(t) => out.push(t),
            Update::Done { .. } => break,
            _ => {}
        }
    }
    out
}

#[tokio::test]
async fn prompt_streams_text_then_done() {
    let (cmd, cfg) = fake_default("prompt-basic");
    let be = ClaudeCliBackend::spawn(&cmd, cfg).await.unwrap();
    let out = drain(&be, &sid("session-t1"), "Remember the number 7").await;
    assert_eq!(out, vec!["7".to_string()]);
}

#[tokio::test]
async fn forget_session_does_not_kill_proc() {
    // The headline blocker-fix unit: forget_session must NOT tear down the warm
    // proc — a follow-up on the same session must reuse it (continuity).
    let (cmd, cfg) = fake_default("prompt-forget");
    let be = ClaudeCliBackend::spawn(&cmd, cfg).await.unwrap();
    let s = sid("session-keep");
    let _ = drain(&be, &s, "Remember the number 7").await; // turn 1
    be.forget_session(&s).await; // per-turn eviction
    let out = drain(&be, &s, "What number?").await; // turn 2 — same proc
    assert_eq!(
        out,
        vec!["7".to_string()],
        "warm proc survived forget_session"
    );
}

#[tokio::test]
async fn turn_timeout_surfaces_failed() {
    let (cmd, mut cfg) = fake(
        "prompt-hang",
        FakeSpec {
            hang: true,
            ..FakeSpec::new()
        },
    );
    cfg.turn_timeout = std::time::Duration::from_millis(300);
    let be = ClaudeCliBackend::spawn(&cmd, cfg).await.unwrap();
    let mut stream = be.prompt(&sid("session-hang"), tp("hang")).await.unwrap();
    let mut saw_err = false;
    while let Some(item) = stream.next().await {
        if item.is_err() {
            saw_err = true;
            break;
        }
    }
    assert!(
        saw_err,
        "hung turn must surface an Err within the turn timeout"
    );
}

#[tokio::test]
async fn dropping_stream_midturn_tears_down_proc() {
    // B3: a STARTED turn (assistant text emitted, no result yet) whose stream is
    // dropped must tear the proc down (TurnGuard) so no warm proc leaks / leaks stale
    // output. `stall` makes the fake emit text then withhold the result; we poll the
    // first Text (proving the turn is genuinely mid-flight) BEFORE dropping.
    let (cmd, cfg) = fake(
        "prompt-drop",
        FakeSpec {
            stall: true,
            ..FakeSpec::new()
        },
    );
    let be = ClaudeCliBackend::spawn(&cmd, cfg).await.unwrap();
    let s = sid("session-drop");
    {
        let mut stream = be.prompt(&s, tp("go")).await.unwrap();
        let first = stream.next().await; // wait until the turn has actually started
        assert!(
            matches!(first, Some(Ok(Update::Text(_)))),
            "turn started: {first:?}"
        );
        drop(stream); // mid-turn drop → TurnGuard teardown (async)
    }
    let mut torn = false;
    for _ in 0..200 {
        if be.live_session_count().await == 0 {
            torn = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        torn,
        "mid-turn stream drop must tear the proc down (no leaked warm proc)"
    );
}
