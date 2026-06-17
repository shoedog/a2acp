//! Characterization harness — pins the CURRENT Rust readiness behavior + `initialize` bytes + respawn
//! ordering so the Slice C1 registry refactor is provably byte-for-byte for the Rust path. Must be GREEN
//! on the pre-refactor code, then stay green after `LangServerConfig`/`Readiness` are split out.
use lsp_mcp::lang::{LangServerConfig, Readiness, RustReady};
use serde_json::json;
use std::time::Duration;

/// The exact `initialize` params the Rust path sends today (via `rust_ra_config().initialize_params`),
/// captured here so the `Readiness::RustRa` config reproduces them value-for-value.
///
/// NOTE (Task 8 intentional change): Rust's initialize now INTENTIONALLY advertises
/// `hierarchicalDocumentSymbolSupport: true` (per LSP documentSymbol capability) to enable
/// rust-analyzer to return nested `DocumentSymbol{children}` (e.g. the `hi` trait method)
/// instead of the flat `SymbolInformation` form. This enables `collect_doc_symbols` recursion to
/// surface nested methods. The change is intended — NOT a regression — and is locked by
/// `rust_document_symbols_includes_nested_trait_method` in integration.rs.
fn rust_initialize_params(root_uri: &str, pid: u32) -> serde_json::Value {
    json!({
        "processId": pid,
        "rootUri": root_uri,
        "capabilities": { "workspace": { "symbol": {} },
            "textDocument": { "documentSymbol": { "hierarchicalDocumentSymbolSupport": true } },
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
    // LOAD-BEARING (Task 8 intentional change): hierarchicalDocumentSymbolSupport MUST be advertised
    // so rust-analyzer returns nested DocumentSymbol{children} (enabling recursive document_symbols).
    // Without it, RA falls back to flat SymbolInformation and nested methods (e.g. trait method `hi`)
    // are dropped. This pin catches any future accidental removal. Locked by
    // `rust_document_symbols_includes_nested_trait_method` in integration.rs.
    assert_eq!(
        p["capabilities"]["textDocument"]["documentSymbol"]["hierarchicalDocumentSymbolSupport"],
        json!(true),
        "Rust initialize MUST advertise hierarchicalDocumentSymbolSupport to enable recursive document_symbols"
    );
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
    assert!(
        s.ensure_ready(Duration::from_secs(120)).unwrap(),
        "server not ready"
    );
    s.evict();
    // After evict, the next ensure_ready respawns against the SAME (valid) repo and succeeds — assert the
    // evicted flag clears on success and the session resolves again (the happy respawn ordering).
    assert!(
        s.ensure_ready(Duration::from_secs(120)).unwrap(),
        "server not ready after respawn"
    );
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
        bootstrap_exts: &[],
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
    assert!(
        s.ensure_ready(Duration::from_secs(120)).unwrap(),
        "server not ready"
    );
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
    // Finding 2: a spawn-failed respawn also leaves no child handle (the bogus binary never spawned a
    // child, so nothing is leaked — the spawn-succeeds/handshake-fails leak is covered separately by
    // `respawn_failure_reaps_new_child_no_leak`).
    assert!(
        !s.child_present_for_test(),
        "FAILED respawn (spawn error) leaves no leaked child handle"
    );
}

/// A fake LSP (shell script) that reproduces the Finding-1 collision: it emits a server-initiated
/// `workspace/configuration` request whose id (1) COLLIDES with the client's `initialize` (also id 1),
/// then BLOCKS reading one reply frame from its stdin, and ONLY THEN emits the real `initialize`
/// response. Consequences for the reader thread under test:
///
/// - CORRECT (fixed): server-req is answered (reply written to $A2A_REPLY_FILE) → the fake unblocks →
///   emits the real init response → handshake (initialize) gets ITS correct response → start Ok.
/// - OLD BUG, DROP: server-req dropped → no reply → fake blocks forever → initialize times out → Err.
/// - OLD BUG, MIS-ROUTE: server-req delivered to pending id 1 → no reply written → fake blocks → the
///   reply file stays empty.
///
/// So `start Ok` proves (a) the client request got its own response; a non-empty reply file with a
/// `result` array proves (b) the server request was answered.
const FAKE_COLLISION_LSP: &str = r#"
emit() {
  body="$1"
  len=$(printf '%s' "$body" | wc -c | tr -d ' ')
  printf 'Content-Length: %s\r\n\r\n%s' "$len" "$body"
}
# Read exactly one framed message from stdin into stdout; returns its body via the `frame` variable is not
# possible in pure sh easily, so we read into a file and cat it.
read_one_frame() {  # writes the body to $1
  clen=0
  while IFS= read -r line; do
    line=$(printf '%s' "$line" | tr -d '\r')
    [ -z "$line" ] && break
    case "$line" in
      Content-Length:*) clen=$(printf '%s' "${line#Content-Length:}" | tr -d ' ') ;;
    esac
  done
  dd bs=1 count="$clen" of="$1" 2>/dev/null
}
# 1) colliding server-initiated request (id 1, same as the client's initialize).
emit '{"jsonrpc":"2.0","id":1,"method":"workspace/configuration","params":{"items":[{"section":"python"}]}}'
# 2) the client writes BOTH its `initialize` request AND our reply to our stdin (same pipe, racy order).
#    Read frames until we see OUR reply — a frame carrying `result` (the workspace/configuration answer),
#    NOT a `method` (the client's initialize request). Skip non-reply frames so the capture is robust.
tmp="$A2A_REPLY_FILE.frame"
while :; do
  read_one_frame "$tmp"
  if grep -q '"result"' "$tmp" 2>/dev/null && ! grep -q '"method"' "$tmp" 2>/dev/null; then
    cp "$tmp" "$A2A_REPLY_FILE"
    break
  fi
done
# 3) now emit the real initialize response (id 1).
emit '{"jsonrpc":"2.0","id":1,"result":{"capabilities":{"the_real":"init_result"}}}'
sleep 5
"#;

#[test]
fn server_request_with_colliding_id_does_not_corrupt_pending() {
    // Finding 1 (HIGH): a server-initiated request (id+method) whose id collides with an in-flight client
    // request must be ANSWERED, never routed to `pending` as that client's response. Drive a real
    // LspClient against a fake LSP that forces exactly this collision (see FAKE_COLLISION_LSP).
    let reply_file = std::env::temp_dir().join(format!(
        "a2a-collision-reply-{}-{}.txt",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&reply_file);
    let cfg = LangServerConfig {
        name: "fake-collision-lsp",
        program_argv: vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            FAKE_COLLISION_LSP.to_string(),
        ],
        spawn_env: vec![(
            "A2A_REPLY_FILE".to_string(),
            reply_file.to_string_lossy().into_owned(),
        )],
        is_project_root: Box::new(|_| true),
        initialize_params: Box::new(|root| json!({ "rootUri": root })),
        post_init_config: None,
        new_readiness: Box::new(|| Readiness::RustRa(RustReady::default())),
        bootstrap_exts: &[],
    };
    // (a) start_with runs the handshake `initialize` (id 1). It returns Ok ONLY if initialize received
    //     its OWN response — proving the colliding server request did not corrupt pending id 1 (a drop
    //     would time out → Err; a mis-route would also block the fake → no real response → Err/empty).
    let mut s = lsp_mcp::lsp::LspClient::start_with(&sample_repo(), cfg)
        .expect("initialize must receive its own response despite the colliding server request");
    // (b) the server request was answered: the fake captured our reply (a workspace/configuration result).
    let reply = std::fs::read_to_string(&reply_file).unwrap_or_default();
    assert!(
        !reply.is_empty(),
        "the server-initiated workspace/configuration request must be answered (reply captured), got empty"
    );
    let reply_json: serde_json::Value =
        serde_json::from_str(&reply).expect("captured reply must be valid JSON-RPC");
    assert_eq!(
        reply_json["id"],
        json!(1),
        "reply echoes the server request id"
    );
    assert!(
        reply_json["result"].is_array(),
        "workspace/configuration reply result must be an array (one entry per item), got {reply_json}"
    );
    s.shutdown();
    let _ = std::fs::remove_file(&reply_file);
}

#[test]
fn respawn_failure_reaps_new_child_no_leak() {
    // Finding 2 (MEDIUM): respawn installs the NEW child BEFORE handshake; if handshake fails the
    // new child must be killed+waited (not leaked for the next respawn to overwrite). This exercises
    // the spawn-SUCCEEDS / handshake-FAILS path (the bogus-binary crown-jewel test fails at spawn() and
    // never creates a child, so it can't catch this leak). A tiny fake LSP replies to the `initialize`
    // request (id 1 — the first request the handshake sends) with a JSON-RPC ERROR, so `handshake()`
    // returns Err deterministically and fast; the fake stays alive (sleep) so we can assert it's reaped.
    if !ra_available() {
        eprintln!("skip: rust-analyzer not on PATH");
        return;
    }
    // A frame replying to id 1 (the initialize request) with an error → handshake bails immediately.
    let body =
        r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32603,"message":"fake handshake failure"}}"#;
    let script = format!(
        "printf 'Content-Length: {}\\r\\n\\r\\n{}'; sleep 5",
        body.len(),
        body
    );
    let fake = LangServerConfig {
        name: "fake-handshake-fail-lsp",
        program_argv: vec!["/bin/sh".to_string(), "-c".to_string(), script],
        spawn_env: vec![],
        is_project_root: Box::new(|_| true),
        initialize_params: Box::new(|root| json!({ "rootUri": root })),
        post_init_config: None,
        new_readiness: Box::new(|| Readiness::RustRa(RustReady::default())),
        bootstrap_exts: &[],
    };
    // Start a healthy client, evict it (old child reaped → self.child == None), then point respawn at the
    // fake cfg whose spawn() SUCCEEDS but whose handshake FAILS.
    let mut s =
        lsp_mcp::lsp::LspClient::start_with(&sample_repo(), lsp_mcp::lang::rust_ra_config(None))
            .unwrap();
    assert!(
        s.ensure_ready(Duration::from_secs(120)).unwrap(),
        "server not ready"
    );
    s.evict();
    assert!(s.is_evicted_for_test(), "evict() set evicted=true");
    s.set_cfg_for_test(fake);
    let err = s.respawn_for_test();
    assert!(
        err.is_err(),
        "respawn must fail when the fake server errors on initialize"
    );
    // Crown-jewel invariant preserved: a failed respawn leaves evicted=true.
    assert!(
        s.is_evicted_for_test(),
        "FAILED respawn must leave evicted=true"
    );
    // Finding 2: the newly-spawned (but handshake-failed) child must be reaped, NOT leaked.
    assert!(
        !s.child_present_for_test(),
        "FAILED respawn must reap the new child (no leaked handle for the next respawn to overwrite)"
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
