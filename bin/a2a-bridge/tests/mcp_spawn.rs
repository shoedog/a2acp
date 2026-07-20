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
    // Slice 9 added the `inject` + `permit` tools (6 -> 8).
    assert_eq!(frames[1]["result"]["tools"].as_array().unwrap().len(), 8);
    assert_eq!(frames[2]["id"], 3);
    assert_eq!(frames[2]["result"]["isError"], true);

    let status = tokio::time::timeout(Duration::from_secs(30), child.wait())
        .await
        .expect("timed out waiting for mcp child")
        .unwrap();
    assert!(status.success(), "mcp child exited nonzero on EOF");
}

#[tokio::test]
async fn managed_agent_depth_refuses_before_config_or_store_work() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.toml");
    let output = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .arg("mcp")
        .arg("--config")
        .arg(&missing)
        .env("A2A_BRIDGE_MCP_CALL_DEPTH", "1")
        .output()
        .await
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("managed-agent MCP loopback is unsupported"),
        "guard must own the failure before config resolution: {stderr}"
    );
    assert!(
        !missing.exists(),
        "guard must not create the missing config"
    );
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "guard must create no config, store, lease, or other artifact"
    );
}

#[tokio::test]
async fn external_depth_zero_keeps_existing_mcp_startup_path() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.toml");
    let output = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .arg("mcp")
        .arg("--config")
        .arg(&missing)
        .env("A2A_BRIDGE_MCP_CALL_DEPTH", "0")
        .output()
        .await
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("run `a2a-bridge init`"), "got: {stderr}");
    assert!(
        !stderr.contains("managed-agent MCP loopback is unsupported"),
        "depth zero is the supported external-controller path: {stderr}"
    );
}

#[tokio::test]
async fn malformed_managed_depth_fails_closed_before_config_work() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.toml");
    let output = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .arg("mcp")
        .arg("--config")
        .arg(&missing)
        .env("A2A_BRIDGE_MCP_CALL_DEPTH", "not-a-depth")
        .output()
        .await
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid A2A_BRIDGE_MCP_CALL_DEPTH"),
        "malformed lineage must fail closed: {stderr}"
    );
    assert!(!missing.exists());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn non_unicode_managed_depth_fails_closed_before_config_work() {
    use std::os::unix::ffi::OsStringExt;

    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.toml");
    let output = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .arg("mcp")
        .arg("--config")
        .arg(&missing)
        .env(
            "A2A_BRIDGE_MCP_CALL_DEPTH",
            std::ffi::OsString::from_vec(vec![0xff]),
        )
        .output()
        .await
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid non-Unicode A2A_BRIDGE_MCP_CALL_DEPTH"),
        "non-Unicode lineage must fail closed: {stderr}"
    );
    assert!(!missing.exists());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn managed_depth_does_not_hide_side_effect_free_mcp_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .arg("mcp")
        .arg("--help")
        .env("A2A_BRIDGE_MCP_CALL_DEPTH", "1")
        .output()
        .await
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("usage: a2a-bridge mcp"), "got: {stdout}");
    assert!(stdout.contains("external-controller MCP"), "got: {stdout}");
    assert!(
        stdout.contains("Managed-agent loopback is refused"),
        "got: {stdout}"
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("managed-agent MCP loopback"),
        "help returns before the runtime loopback guard"
    );
}
