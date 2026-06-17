//! Go (gopls) fixture tests. Guarded on a working `gopls` like the Rust integration tests guard on
//! rust-analyzer and the Python tests guard on basedpyright. The fixture (tests/fixtures/gosample) needs
//! `go mod download` run once (its deps fetched into the module cache) for third-party resolution.
use lsp_mcp::lang::go_config;
use lsp_mcp::lsp::LspClient;
use std::path::PathBuf;
use std::time::Duration;

fn gopls_available() -> bool {
    // Probe the SAME binary go_config will spawn — `resolve_lsp_server` also searches `$GOPATH/bin` and
    // `$(go env GOROOT)/bin`, so an off-PATH gopls the bridge CAN run does not make the guard falsely
    // Skip (or Require-fail under LSP_MCP_REQUIRE_GO=1). Falls back to the bare name if config fails.
    let bin = go_config(&gosample())
        .ok()
        .and_then(|c| c.program_argv.first().cloned())
        .unwrap_or_else(|| "gopls".to_string());
    std::process::Command::new(bin)
        .arg("version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
fn gosample() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gosample")
}
fn go_src() -> PathBuf {
    gosample().join("gosample.go")
}
fn deps_fetched() -> bool {
    // A committed `go.sum` does NOT populate `$GOMODCACHE` (only `go mod download` does), and gopls needs
    // the module in the cache to resolve third-party nav. So require BOTH go.sum AND the uuid module present
    // in the cache — go.sum presence alone would false-green a fresh checkout whose module cache is empty.
    if !gosample().join("go.sum").is_file() {
        return false;
    }
    let modcache = std::process::Command::new("go")
        .args(["env", "GOMODCACHE"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());
    match modcache {
        Some(dir) => std::path::Path::new(&dir)
            .join("github.com/google/uuid@v1.6.0")
            .is_dir(),
        None => false,
    }
}

/// What the Go liveness guard should do, given the require-flag + the presence of gopls + the deps.
#[derive(Debug, PartialEq, Eq)]
enum GoGate {
    Run,
    Skip,
    /// `LSP_MCP_REQUIRE_GO=1` is set but a prerequisite is missing → the guard MUST fail (CI enforces
    /// real Go coverage instead of silently green-ing). Carries the human-readable reason.
    Require(String),
}

fn go_gate(require: bool, gopls: bool, deps: bool) -> GoGate {
    match (gopls, deps) {
        (true, true) => GoGate::Run,
        _ if require => {
            let mut missing = Vec::new();
            if !gopls {
                missing.push("gopls not on PATH");
            }
            if !deps {
                missing.push(
                    "fixture deps not resolvable (go.sum missing or uuid not in the module cache) — run `go mod download` in tests/fixtures/gosample",
                );
            }
            GoGate::Require(missing.join("; "))
        }
        _ => GoGate::Skip,
    }
}

fn ready() -> bool {
    let require = std::env::var("LSP_MCP_REQUIRE_GO").as_deref() == Ok("1");
    match go_gate(require, gopls_available(), deps_fetched()) {
        GoGate::Run => true,
        GoGate::Skip => false,
        GoGate::Require(why) => panic!(
            "LSP_MCP_REQUIRE_GO=1 but Go coverage cannot run: {why}. \
             Install gopls + run `go mod download` in the fixture, or unset LSP_MCP_REQUIRE_GO."
        ),
    }
}

fn start() -> LspClient {
    let repo = gosample();
    let cfg = go_config(&repo).unwrap();
    LspClient::start_with(&repo, cfg).expect("start gopls")
}

// ---- the pure gate decision (hermetic — no real gopls needed), mirroring python_gate's tests ----

#[test]
fn go_gate_runs_when_both_present() {
    assert_eq!(go_gate(false, true, true), GoGate::Run);
    assert_eq!(go_gate(true, true, true), GoGate::Run);
}

#[test]
fn go_gate_skips_when_not_required_and_absent() {
    assert_eq!(go_gate(false, false, false), GoGate::Skip);
    assert_eq!(go_gate(false, true, false), GoGate::Skip);
    assert_eq!(go_gate(false, false, true), GoGate::Skip);
}

#[test]
fn go_gate_requires_when_required_and_absent() {
    assert!(matches!(go_gate(true, false, true), GoGate::Require(_)));
    assert!(matches!(go_gate(true, true, false), GoGate::Require(_)));
    match go_gate(true, false, true) {
        GoGate::Require(why) => assert!(why.contains("gopls"), "got: {why}"),
        other => panic!("expected Require, got {other:?}"),
    }
    match go_gate(true, true, false) {
        GoGate::Require(why) => assert!(why.contains("go.sum"), "got: {why}"),
        other => panic!("expected Require, got {other:?}"),
    }
    // BOTH prerequisites missing → Require, and the reason names BOTH.
    match go_gate(true, false, false) {
        GoGate::Require(why) => {
            assert!(why.contains("gopls"), "got: {why}");
            assert!(why.contains("go.sum"), "got: {why}");
        }
        other => panic!("expected Require, got {other:?}"),
    }
}

// ---- the live 7-tool coverage (skips unless gopls + deps present) ----

#[test]
fn workspace_symbol_finds_func() {
    if !ready() {
        eprintln!("skip");
        return;
    }
    let mut s = start();
    assert!(
        s.ensure_ready(Duration::from_secs(60)).unwrap(),
        "server not ready"
    );
    assert!(!s.workspace_symbol("Add").unwrap().is_empty());
    s.shutdown();
}

#[test]
fn document_symbols_extracts_type_and_method_recursively() {
    if !ready() {
        eprintln!("skip");
        return;
    }
    let mut s = start();
    assert!(
        s.ensure_ready(Duration::from_secs(60)).unwrap(),
        "server not ready"
    );
    let syms = s.document_symbols(&go_src()).unwrap();
    assert!(
        syms.iter()
            .any(|h| h.signature.as_deref() == Some("En") && h.line == 16),
        "type En at :16, got {syms:?}"
    );
    // The interface method `Greet` is a CHILD of `Greeter` (gosample.go:12). A non-recursive walk would
    // only surface the top-level decls (Add / Greeter / En / (En).Greet / NewID); finding `Greet` AT the
    // interface line proves `collect_doc_symbols` recurses into `children`. A name-only `contains("Greet")`
    // would false-green — the concrete method surfaces separately as top-level `(En).Greet` at :20.
    assert!(
        syms.iter()
            .any(|h| h.signature.as_deref() == Some("Greet") && h.line == 12),
        "interface method `Greet` (child of Greeter at :12) must surface via children recursion, got {syms:?}"
    );
    s.shutdown();
}

#[test]
fn definition_and_references_and_hover() {
    if !ready() {
        eprintln!("skip");
        return;
    }
    let mut s = start();
    assert!(
        s.ensure_ready(Duration::from_secs(60)).unwrap(),
        "server not ready"
    );
    // Workspace-local nav: `Add` resolves + hovers.
    assert!(
        !s.definition("Add").unwrap().is_empty(),
        "definition of Add"
    );
    let add_hover = s.hover("Add").unwrap();
    assert!(
        add_hover.as_deref().map(|x| !x.is_empty()).unwrap_or(false),
        "hover(Add) must return non-empty content, got {add_hover:?}"
    );
    // THIRD-PARTY nav — the reason the fixture imports github.com/google/uuid (spike Gate 4b: gopls
    // resolves third-party symbols by name). definition jumps into the module cache; hover returns the
    // upstream doc; references include the local usage site (gosample.go).
    let uuid_def = s.definition("uuid.New").unwrap();
    assert!(
        uuid_def.iter().any(|h| h.file.contains("uuid@v1.6.0")),
        "definition(uuid.New) must jump into the uuid@v1.6.0 module cache, got {uuid_def:?}"
    );
    let uuid_hover = s.hover("uuid.New").unwrap();
    assert!(
        uuid_hover
            .as_deref()
            .map(|x| x.contains("UUID"))
            .unwrap_or(false),
        "hover(uuid.New) must return the third-party doc, got {uuid_hover:?}"
    );
    let uuid_refs = s.references("uuid.New", true).unwrap();
    assert!(
        uuid_refs.iter().any(|h| h.file.ends_with("gosample.go")),
        "references(uuid.New) must include the local usage in gosample.go, got {uuid_refs:?}"
    );
    s.shutdown();
}

#[test]
fn implementations_of_interface() {
    if !ready() {
        eprintln!("skip");
        return;
    }
    let mut s = start();
    assert!(
        s.ensure_ready(Duration::from_secs(60)).unwrap(),
        "server not ready"
    );
    // `Greeter` is an interface; `En` implements it. Assert gopls actually finds the implementer in the
    // fixture (not just "no error" — that would pass on an empty result and prove nothing).
    let impls = s.implementations("Greeter").unwrap();
    assert!(
        impls.iter().any(|h| h.file.ends_with("gosample.go")),
        "implementations(Greeter) must find En in gosample.go, got {impls:?}"
    );
    s.shutdown();
}

#[test]
fn call_hierarchy_finds_incoming_caller() {
    if !ready() {
        eprintln!("skip");
        return;
    }
    let mut s = start();
    assert!(
        s.ensure_ready(Duration::from_secs(60)).unwrap(),
        "server not ready"
    );
    // `Double` calls `Add` (fixture) — incoming call hierarchy must surface that caller, not merely
    // "no error" (Add with no callers would always-empty-pass and prove nothing about result parsing).
    let calls = s.call_hierarchy("Add", true).unwrap();
    assert!(
        calls
            .iter()
            .any(|h| h.signature.as_deref() == Some("Double")),
        "call_hierarchy(Add, incoming) must find caller Double, got {calls:?}"
    );
    s.shutdown();
}
