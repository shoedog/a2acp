// heartbeat_cli.rs — `a2a-bridge submit` against a stub serve (activity liveness I3).
//
// The stub answers the unary `SendMessage` after a few seconds and `TaskLiveness` like a
// bridge that predates it ("method not found"). The heartbeat must beat on stderr only,
// elapsed-only, while stdout keeps exactly the locator lines and the answer; the request asks
// for a watched turn exactly while the heartbeat is on.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

/// The `SendMessage` metadata the stub received.
type Received = Arc<Mutex<Vec<Value>>>;

async fn stub_serve(answer_after: Duration) -> (String, Received) {
    let received: Received = Arc::default();
    let seen = received.clone();
    let app = axum::Router::new().route(
        "/",
        axum::routing::post(move |axum::Json(request): axum::Json<Value>| {
            let seen = seen.clone();
            async move {
                match request["method"].as_str() {
                    Some("SendMessage") => {
                        seen.lock()
                            .unwrap()
                            .push(request["params"]["message"]["metadata"].clone());
                        tokio::time::sleep(answer_after).await;
                        axum::Json(json!({
                            "jsonrpc": "2.0", "id": 1,
                            "result": {
                                "task": { "id": "t", "state": "TASK_STATE_COMPLETED" },
                                "artifact": { "text": "ANSWER" },
                                "status": [],
                            }
                        }))
                    }
                    _ => axum::Json(json!({
                        "jsonrpc": "2.0", "id": 1,
                        "error": { "code": -32601, "message": "method not found" }
                    })),
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });
    (url, received)
}

async fn submit(url: &str, heartbeat: &[&str]) -> (Vec<String>, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("brief.md");
    std::fs::write(&input, "hello").unwrap();
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .arg("submit")
        .arg("--input")
        .arg(&input)
        .arg("--url")
        .arg(url)
        .args(heartbeat)
        .env_remove("A2A_BRIDGE_HEARTBEAT_SECS")
        .output()
        .await
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let lines = |bytes: &[u8]| {
        String::from_utf8(bytes.to_vec())
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    (lines(&output.stdout), lines(&output.stderr))
}

#[tokio::test]
async fn submit_beats_on_stderr_only_and_asks_for_a_watched_turn() {
    let (url, received) = stub_serve(Duration::from_millis(2_600)).await;
    let (stdout, stderr) = submit(&url, &["--heartbeat-secs", "1"]).await;

    assert_eq!(stdout.len(), 3, "{stdout:?}");
    assert!(stdout[0].starts_with("execution_id=exec-"));
    assert!(stdout[1].starts_with("attempt_id=attempt-"));
    assert_eq!(stdout[2], "ANSWER");
    let beats: Vec<_> = stderr
        .iter()
        .filter(|line| line.starts_with("[heartbeat] "))
        .collect();
    assert!(!beats.is_empty(), "{stderr:?}");
    assert!(
        beats
            .iter()
            .all(|line| line.ends_with(" liveness=unavailable")),
        "an older server leaves every beat elapsed-only: {beats:?}"
    );
    assert!(
        beats[0].starts_with("[heartbeat] elapsed=1s "),
        "the first beat comes after one interval: {beats:?}"
    );
    assert_eq!(
        received.lock().unwrap()[0]["a2a-bridge.client_watching"],
        "true"
    );
}

#[tokio::test]
async fn no_heartbeat_is_silent_and_leaves_the_turn_unwatched() {
    let (url, received) = stub_serve(Duration::from_millis(1_600)).await;
    let (stdout, stderr) = submit(&url, &["--no-heartbeat"]).await;

    assert_eq!(stdout.len(), 3, "{stdout:?}");
    assert_eq!(stdout[2], "ANSWER");
    assert!(
        !stderr.iter().any(|line| line.contains("[heartbeat]")),
        "{stderr:?}"
    );
    assert!(received.lock().unwrap()[0]
        .get("a2a-bridge.client_watching")
        .is_none());
}
