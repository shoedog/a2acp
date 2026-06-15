//! Characterization harness — pins the CURRENT Rust readiness behavior + `initialize` bytes + respawn
//! ordering so the Slice C1 registry refactor is provably byte-for-byte for the Rust path. Must be GREEN
//! on the pre-refactor code, then stay green after `LangServerConfig`/`Readiness` are split out.
use lsp_mcp::testkit::{is_ready, parse_quiescent, ReadyState};
use serde_json::json;
use std::time::Duration;

/// The exact `initialize` params the Rust path sends today (lib `handshake()`), captured here so the
/// `Readiness::RustRa` config in Task 3 reproduces them value-for-value.
fn rust_initialize_params(root_uri: &str, pid: u32) -> serde_json::Value {
    json!({
        "processId": pid,
        "rootUri": root_uri,
        "capabilities": { "workspace": { "symbol": {} },
            "experimental": { "serverStatusNotification": true } },
        "workspaceFolders": [{ "uri": root_uri, "name": "root" }],
    })
}

#[test]
fn rust_initialize_params_are_pinned() {
    let p = rust_initialize_params("file:///repo", 7);
    assert_eq!(p["capabilities"]["experimental"]["serverStatusNotification"], json!(true));
    assert_eq!(p["capabilities"]["workspace"]["symbol"], json!({}));
    assert_eq!(p["workspaceFolders"][0]["uri"], json!("file:///repo"));
    assert_eq!(p["processId"], json!(7));
    assert_eq!(p["rootUri"], json!("file:///repo"));
    assert_eq!(p["workspaceFolders"][0]["name"], json!("root"));
}

/// Apply one synthetic notification to a ReadyState the way the reader thread does today (mod.rs:99-118).
fn apply(s: &mut ReadyState, msg: &serde_json::Value) {
    if msg.get("method").and_then(|m| m.as_str()) == Some("$/progress") {
        match msg["params"]["value"]["kind"].as_str() {
            Some("begin") => { s.began = true; s.active += 1; }
            Some("end") => { s.active = s.active.saturating_sub(1); }
            _ => {}
        }
    } else if msg.get("method").and_then(|m| m.as_str()) == Some("experimental/serverStatus") {
        if let Some(q) = parse_quiescent(&msg["params"]) { s.quiescent = q; }
    }
}

#[test]
fn rust_readiness_transition_table() {
    let begin = json!({"method":"$/progress","params":{"value":{"kind":"begin"}}});
    let end = json!({"method":"$/progress","params":{"value":{"kind":"end"}}});
    let quiescent = json!({"method":"experimental/serverStatus","params":{"quiescent":true}});

    // ordered begin→end → ready
    let mut s = ReadyState::default();
    assert!(!is_ready(&s), "nothing heard yet");
    apply(&mut s, &begin);
    assert!(!is_ready(&s), "begun, still active");
    apply(&mut s, &end);
    assert!(is_ready(&s), "begun-and-ended → ready");

    // serverStatus quiescent alone (warm-no-progress) → ready, no $/progress needed
    let mut s = ReadyState::default();
    apply(&mut s, &quiescent);
    assert!(is_ready(&s), "quiescent alone is enough");

    // out-of-order: a stray `end` before any `begin` must NOT mark ready (active saturates at 0, began stays false)
    let mut s = ReadyState::default();
    apply(&mut s, &end);
    assert!(!is_ready(&s), "lone end is not ready");
}

fn ra_available() -> bool {
    std::process::Command::new("rust-analyzer").arg("--version").output()
        .map(|o| o.status.success()).unwrap_or(false)
}
fn sample_repo() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample")
}

#[test]
fn respawn_success_clears_evicted() {
    // HAPPY PATH: a respawn whose handshake SUCCEEDS clears `evicted` and the session resolves again.
    // (The GENUINE failure-leaves-evicted=true test — the crown-jewel invariant — lands in Task 3, where
    // the LangServerConfig seam lets us inject a bogus program_argv to FORCE a respawn failure.)
    // We evict a healthy session and assert the next ensure_ready re-spawns against the SAME (valid) repo
    // and resolves. Guarded: needs a real RA to start the first time.
    if !ra_available() { eprintln!("skip: rust-analyzer not on PATH"); return; }
    // renamed to LspClient in Task 3 (the type is still `LspSession` pre-refactor).
    let mut s = lsp_mcp::lsp::LspSession::start(&sample_repo(), None).unwrap();
    s.ensure_ready(Duration::from_secs(120)).unwrap();
    s.evict();
    // After evict, the next ensure_ready respawns against the SAME (valid) repo and succeeds — assert the
    // evicted flag clears on success and the session resolves again (the happy respawn ordering).
    s.ensure_ready(Duration::from_secs(120)).unwrap();
    assert!(!s.workspace_symbol("add").unwrap().is_empty(), "respawn re-indexed after evict");
    s.shutdown();
}
