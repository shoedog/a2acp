//! Python (basedpyright) fixture tests. Guarded on a working `basedpyright-langserver` like the Rust
//! integration tests guard on rust-analyzer. The fixture (tests/fixtures/pysample) is built in Task 8.
use lsp_mcp::lang::pyright_config;
use lsp_mcp::lsp::LspClient;
use std::path::PathBuf;
use std::time::Duration;

fn pyright_available() -> bool {
    std::process::Command::new("basedpyright-langserver")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
fn pysample() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pysample")
}
fn pysample_venv_python() -> PathBuf {
    pysample().join(".venv/bin/python")
}
/// Skip unless basedpyright AND the fixture venv (with the third-party dep) both exist.
fn ready() -> bool {
    pyright_available() && pysample_venv_python().is_file()
}

fn start() -> LspClient {
    let repo = pysample();
    let cfg = pyright_config(&repo, Some(&pysample_venv_python())).unwrap();
    LspClient::start_with(&repo, cfg).expect("start basedpyright")
}

#[test]
fn post_eviction_still_resolves_third_party_def() {
    if !ready() {
        eprintln!("skip: basedpyright or fixture venv missing");
        return;
    }
    // TODO Task 8: replace third_party_symbol with the fixture's real third-party import (e.g. BaseModel).
    let mut s = start();
    s.ensure_ready(Duration::from_secs(60)).unwrap();
    // First resolution of a third-party symbol works (the venv is applied via didChangeConfiguration).
    let before = s.definition("third_party_symbol").unwrap();
    assert!(
        !before.is_empty(),
        "third-party def must resolve before eviction"
    );
    // Evict, then the NEXT call must respawn AND re-send didChangeConfiguration so the venv survives.
    s.evict();
    s.ensure_ready(Duration::from_secs(60)).unwrap();
    let after = s.definition("third_party_symbol").unwrap();
    assert!(
        !after.is_empty(),
        "third-party def must STILL resolve after respawn (config re-sent)"
    );
    s.shutdown();
}
