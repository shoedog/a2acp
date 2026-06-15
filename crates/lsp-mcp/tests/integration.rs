use std::path::PathBuf;
use std::time::Duration;

fn sample_repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample")
}

// Guarded: only run where rust-analyzer actually WORKS. The bridge's verify container ships a rustup
// *proxy* at this path that spawns but exits non-zero (the component isn't installed), so we check
// status.success() — not merely that the spawn succeeded — or these would run against a non-functional
// server in-container. Host-side (real rust-analyzer on PATH) they run for real. Do NOT remove the guard.
fn ra_available() -> bool {
    std::process::Command::new("rust-analyzer")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn session_reaches_ready_and_resolves_a_symbol() {
    if !ra_available() {
        eprintln!("skip: rust-analyzer not on PATH");
        return;
    }
    let mut s = lsp_mcp::lsp::LspClient::start(&sample_repo(), None).expect("start");
    s.wait_ready(Duration::from_secs(60)).expect("ready");
    let hits = s.workspace_symbol("add").expect("query");
    assert!(
        hits.iter()
            .any(|h| h.signature.as_deref().unwrap_or("").contains("add")),
        "workspace/symbol must find `add`, got {hits:?}"
    );
    s.shutdown();
}

#[test]
fn references_finds_the_caller() {
    if !ra_available() {
        eprintln!("skip");
        return;
    }
    let mut s = lsp_mcp::lsp::LspClient::start(&sample_repo(), None).unwrap();
    s.wait_ready(Duration::from_secs(60)).unwrap();
    let refs = s.references("add", true).unwrap();
    assert!(
        refs.iter().any(|h| h.line == 5),
        "references must include the call site (line 5, `add(1, 2)` in `caller`), got {refs:?}"
    );
    s.shutdown();
}

#[test]
fn implementations_finds_the_impl() {
    if !ra_available() {
        eprintln!("skip");
        return;
    }
    let mut s = lsp_mcp::lsp::LspClient::start(&sample_repo(), None).unwrap();
    s.wait_ready(Duration::from_secs(60)).unwrap();
    let impls = s.implementations("Greet").unwrap();
    assert!(!impls.is_empty(), "Greet must have an implementor (En)");
    s.shutdown();
}

#[test]
fn evict_then_query_reindexes() {
    if !ra_available() {
        eprintln!("skip");
        return;
    }
    let mut s = lsp_mcp::lsp::LspClient::start(&sample_repo(), None).unwrap();
    s.ensure_ready(Duration::from_secs(120)).unwrap();
    assert!(!s.workspace_symbol("add").unwrap().is_empty());
    s.evict();
    s.ensure_ready(Duration::from_secs(120)).unwrap();
    assert!(
        !s.workspace_symbol("add").unwrap().is_empty(),
        "RA respawned + re-indexed after evict"
    );
    s.shutdown();
}

#[test]
fn rust_document_symbols_includes_nested_trait_method() {
    if !ra_available() {
        eprintln!("skip: rust-analyzer not on PATH");
        return;
    }
    let mut s = lsp_mcp::lsp::LspClient::start(&sample_repo(), None).unwrap();
    s.ensure_ready(std::time::Duration::from_secs(120)).unwrap();
    let syms = s.document_symbols(&sample_repo().join("lib.rs")).unwrap();
    let names: Vec<&str> = syms.iter().filter_map(|h| h.signature.as_deref()).collect();
    // Top-level items still present (additive, not a replacement).
    assert!(names.contains(&"add"), "top-level fn add, got {names:?}");
    assert!(names.contains(&"Greet"), "trait Greet, got {names:?}");
    // NEW: the trait method `hi` is now extracted via children recursion (it was DROPPED by the old flat
    // parse). This LOCKS the additive recursive output for Rust — the change is intended, not byte-for-byte.
    assert!(
        names.contains(&"hi"),
        "nested trait method `hi` (recursion), got {names:?}"
    );
    s.shutdown();
}
