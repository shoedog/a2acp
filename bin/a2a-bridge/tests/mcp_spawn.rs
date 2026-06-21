use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

#[tokio::test]
async fn mcp_subcommand_handshake_and_tool_call_over_real_pipes() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("a2a-bridge.toml");
    let store_path = dir.path().join("tasks.sqlite");
    std::fs::write(
        &cfg_path,
        format!(
            "default = \"codex\"\n\n\
             [server]\naddr = \"127.0.0.1:0\"\n\n\
             [store]\npath = {store:?}\n\n\
             [[agents]]\nid = \"codex\"\ncmd = \"codex\"\nkind = \"acp\"\ndefault = true\n",
            store = store_path,
        ),
    )
    .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .arg("mcp")
        .arg("--config")
        .arg(&cfg_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();

    let reqs = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"status","arguments":{"task_id":"task-nope"}}}"#,
    ];
    for r in reqs {
        stdin.write_all(r.as_bytes()).await.unwrap();
        stdin.write_all(b"\n").await.unwrap();
    }
    stdin.flush().await.unwrap();
    drop(stdin);

    let mut out = String::new();
    tokio::time::timeout(Duration::from_secs(30), stdout.read_to_string(&mut out))
        .await
        .expect("timed out reading mcp stdout")
        .unwrap();
    let frames: Vec<serde_json::Value> = out
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();

    assert_eq!(frames.len(), 3, "frames: {out}");
    assert_eq!(frames[0]["id"], 1);
    assert_eq!(frames[0]["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(frames[1]["id"], 2);
    assert_eq!(frames[1]["result"]["tools"].as_array().unwrap().len(), 6);
    assert_eq!(frames[2]["id"], 3);
    assert_eq!(frames[2]["result"]["isError"], true);

    let status = tokio::time::timeout(Duration::from_secs(30), child.wait())
        .await
        .expect("timed out waiting for mcp child")
        .unwrap();
    assert!(status.success(), "mcp child exited nonzero on EOF");
}
