#![cfg(unix)]

use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

#[tokio::test]
async fn wrapper_propagates_bridge_stdin_eof_to_child() {
    let mut wrapper = Command::new(env!("CARGO_BIN_EXE_codex-acp-attested"))
        .arg("--codex-acp")
        .arg("/bin/sh")
        .arg("--")
        .arg("-c")
        .arg("cat >/dev/null")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn attested wrapper");

    drop(wrapper.stdin.take().expect("wrapper stdin pipe"));

    let status = match tokio::time::timeout(Duration::from_secs(5), wrapper.wait()).await {
        Ok(result) => result.expect("wait for attested wrapper"),
        Err(_) => {
            let _ = wrapper.kill().await;
            panic!("attested wrapper did not propagate stdin EOF to its child");
        }
    };
    assert!(status.success(), "wrapper exited with {status}");
}

#[tokio::test]
async fn wrapper_preserves_ordinary_crlf_and_final_eof_bytes_through_both_directions() {
    let mut wrapper = Command::new(env!("CARGO_BIN_EXE_codex-acp-attested"))
        .arg("--codex-acp")
        .arg("/bin/cat")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn attested wrapper");

    let input = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\r\nnot-json\r\nfinal-without-newline";
    let mut stdin = wrapper.stdin.take().expect("wrapper stdin pipe");
    stdin.write_all(input).await.expect("write wrapper input");
    drop(stdin);

    let output = tokio::time::timeout(Duration::from_secs(5), wrapper.wait_with_output())
        .await
        .expect("wrapper should settle")
        .expect("collect attested wrapper output");
    assert!(
        output.status.success(),
        "wrapper exited with {}; stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, input);
}
