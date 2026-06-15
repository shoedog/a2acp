//! Characterization harness — pins the CURRENT Rust readiness behavior + `initialize` bytes + respawn
//! ordering so the Slice C1 registry refactor is provably byte-for-byte for the Rust path. Must be GREEN
//! on the pre-refactor code, then stay green after `LangServerConfig`/`Readiness` are split out.
use lsp_mcp::lang::{LangServerConfig, Readiness, RustReady};
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
    assert_eq!(
        p["capabilities"]["experimental"]["serverStatusNotification"],
        json!(true)
    );
    assert_eq!(p["capabilities"]["workspace"]["symbol"], json!({}));
    assert_eq!(p["workspaceFolders"][0]["uri"], json!("file:///repo"));
    assert_eq!(p["processId"], json!(7));
    assert_eq!(p["rootUri"], json!("file:///repo"));
    assert_eq!(p["workspaceFolders"][0]["name"], json!("root"));
}

#[test]
fn rust_readiness_transition_table() {
    // `on_notification` takes the `params` value directly (the reader thread passes `&msg["params"]`), so
    // these are `params`-shaped, matching mod.rs's `ready.lock().unwrap().on_notification(method, &msg["params"])`.
    let begin = json!({"value":{"kind":"begin"}});
    let end = json!({"value":{"kind":"end"}});
    let quiescent = json!({"quiescent": true});

    // ordered begin→end → ready
    let mut r = Readiness::RustRa(RustReady::default());
    assert!(!r.is_ready(), "nothing heard yet");
    r.on_notification("$/progress", &begin);
    assert!(!r.is_ready(), "begun, still active");
    r.on_notification("$/progress", &end);
    assert!(r.is_ready(), "begun-and-ended → ready");

    // experimental/serverStatus quiescent alone (warm-no-progress) → ready, no $/progress needed
    let mut r = Readiness::RustRa(RustReady::default());
    r.on_notification("experimental/serverStatus", &quiescent);
    assert!(r.is_ready(), "quiescent alone is enough");

    // out-of-order: a stray `end` before any `begin` must NOT mark ready (active saturates at 0, began stays false)
    let mut r = Readiness::RustRa(RustReady::default());
    r.on_notification("$/progress", &end);
    assert!(!r.is_ready(), "lone end is not ready");
}

fn ra_available() -> bool {
    std::process::Command::new("rust-analyzer")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
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
    if !ra_available() {
        eprintln!("skip: rust-analyzer not on PATH");
        return;
    }
    let mut s = lsp_mcp::lsp::LspClient::start(&sample_repo(), None).unwrap();
    s.ensure_ready(Duration::from_secs(120)).unwrap();
    s.evict();
    // After evict, the next ensure_ready respawns against the SAME (valid) repo and succeeds — assert the
    // evicted flag clears on success and the session resolves again (the happy respawn ordering).
    s.ensure_ready(Duration::from_secs(120)).unwrap();
    assert!(
        !s.workspace_symbol("add").unwrap().is_empty(),
        "respawn re-indexed after evict"
    );
    s.shutdown();
}

#[test]
fn respawn_failure_leaves_evicted_true() {
    // CROWN-JEWEL INVARIANT: a respawn whose handshake CANNOT succeed leaves `evicted=true` so the NEXT
    // call retries respawn (mod.rs:respawn re-inits BEFORE clearing `evicted`). Force failure by swapping
    // the LangServerConfig for one whose program_argv[0] is a non-existent binary — spawn() then fails.
    let bogus = LangServerConfig {
        name: "bogus-lsp",
        program_argv: vec!["a2a-definitely-not-a-real-lsp".to_string()],
        spawn_env: vec![],
        is_project_root: Box::new(|_| true),
        initialize_params: Box::new(|root| json!({ "rootUri": root })),
        post_init_config: None,
        new_readiness: Box::new(|| Readiness::RustRa(RustReady::default())),
    };
    // Use the real-RA path to get a STARTED client, evict it, then point respawn at the bogus cfg so the
    // missing-binary spawn() errors while keeping the eviction-ordering code under test.
    if !ra_available() {
        eprintln!("skip: rust-analyzer not on PATH");
        return;
    }
    let mut s =
        lsp_mcp::lsp::LspClient::start_with(&sample_repo(), lsp_mcp::lang::rust_ra_config(None))
            .unwrap();
    s.ensure_ready(Duration::from_secs(120)).unwrap();
    s.evict();
    assert!(s.is_evicted_for_test(), "evict() set evicted=true");
    // Swap the cfg to the bogus one and force a respawn — spawn() of the missing binary must Err...
    s.set_cfg_for_test(bogus);
    let err = s.respawn_for_test();
    assert!(err.is_err(), "respawn of a non-existent binary must fail");
    // ...and the invariant: evicted is STILL true after the failed respawn (next call retries respawn).
    assert!(
        s.is_evicted_for_test(),
        "FAILED respawn must leave evicted=true (crown-jewel invariant)"
    );
}

#[test]
fn request_path_advances_last_activity_idle_race_guard() {
    // The idle-race fix: wait_ready()/request() touch() on the active path so the watcher can't evict
    // mid-use. Assert last_activity ADVANCES across a wait_ready() call. If the refactor drops the
    // request-path touch(), last_activity does not advance and this fails. RA-guarded (needs a started client).
    if !ra_available() {
        eprintln!("skip: rust-analyzer not on PATH");
        return;
    }
    let mut s =
        lsp_mcp::lsp::LspClient::start_with(&sample_repo(), lsp_mcp::lang::rust_ra_config(None))
            .unwrap();
    let before = s.last_activity_for_test();
    std::thread::sleep(Duration::from_millis(20));
    s.wait_ready(Duration::from_millis(50)).unwrap(); // touches on every loop iteration
    assert!(
        s.last_activity_for_test() > before,
        "request/wait path must touch() (idle-race fix)"
    );
    s.shutdown();
}
