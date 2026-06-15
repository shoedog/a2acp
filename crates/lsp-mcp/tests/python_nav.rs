//! Python (basedpyright) fixture tests. Guarded on a working `basedpyright-langserver` like the Rust
//! integration tests guard on rust-analyzer. The fixture (tests/fixtures/pysample) is built in Task 8.
use lsp_mcp::lang::pyright_config;
use lsp_mcp::lsp::LspClient;
use std::path::PathBuf;
use std::time::Duration;

fn pyright_available() -> bool {
    // The liveness guard runs the `basedpyright` CLI, NOT `basedpyright-langserver`: the language SERVER
    // binary needs a stdin connection and exits NON-ZERO on `--version` ("Connection input stream is not
    // set"), which would silently SKIP every live test. Only the CLI answers `--version` cleanly. (The
    // language server is still correctly spawned as `basedpyright-langserver --stdio` inside `pyright_config`.)
    std::process::Command::new("basedpyright")
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

fn models_py() -> PathBuf {
    pysample().join("pysample/models.py")
}

/// Positional go-to-definition at the `BaseModel` usage on `class Greeter(BaseModel):`. Returns the set of
/// resolved target URIs. **API adaptation (host-empirical):** basedpyright's `workspace/symbol` indexes ONLY
/// workspace symbols — it returns NOTHING for site-packages names like `BaseModel`, so the bridge's name-only
/// `definition("BaseModel")` cannot reach third-party defs. Third-party resolution works POSITIONALLY (the
/// venv applied via `didChangeConfiguration` makes go-to-def at the usage site jump into site-packages). This
/// helper exercises that real path so the post-eviction test validates the venv genuinely survives a respawn
/// — instead of asserting a name lookup that structurally can't resolve on basedpyright.
fn basemodel_def_targets(s: &mut LspClient) -> Vec<String> {
    let src = std::fs::read_to_string(models_py()).unwrap();
    // `class Greeter(BaseModel):` — find that line + the column of the `BaseModel` token (0-based).
    let (line, col) = src
        .lines()
        .enumerate()
        .find_map(|(i, l)| l.find("(BaseModel").map(|c| (i as u64, (c + 1) as u64)))
        .expect("models.py must use `(BaseModel`");
    let uri = format!("file://{}", models_py().display());
    let res = s
        .request(
            "textDocument/definition",
            serde_json::json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": col } }),
            Duration::from_secs(20),
        )
        .unwrap();
    res.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|it| {
                    it.get("uri")
                        .or_else(|| it.get("targetUri"))
                        .and_then(|u| u.as_str())
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn post_eviction_still_resolves_third_party_def() {
    if !ready() {
        eprintln!("skip: basedpyright or fixture venv missing");
        return;
    }
    let mut s = start();
    s.ensure_ready(Duration::from_secs(60)).unwrap();
    // First resolution of a third-party symbol works (the venv is applied via didChangeConfiguration):
    // positional go-to-def at the `BaseModel` usage jumps into the venv's pydantic site-packages.
    let before = basemodel_def_targets(&mut s);
    assert!(
        before.iter().any(|u| u.contains("site-packages/pydantic")),
        "third-party def must resolve into the venv's pydantic site-packages before eviction, got {before:?}"
    );
    // Evict, then the NEXT call must respawn AND re-send didChangeConfiguration so the venv survives.
    s.evict();
    s.ensure_ready(Duration::from_secs(60)).unwrap();
    let after = basemodel_def_targets(&mut s);
    assert!(
        after.iter().any(|u| u.contains("site-packages/pydantic")),
        "third-party def must STILL resolve into site-packages after respawn (config re-sent), got {after:?}"
    );
    s.shutdown();
}

#[test]
fn document_symbols_extracts_class_and_method_recursively() {
    if !ready() {
        eprintln!("skip");
        return;
    }
    let mut s = start();
    s.ensure_ready(Duration::from_secs(60)).unwrap();
    let syms = s.document_symbols(&models_py()).unwrap();
    let names: Vec<&str> = syms.iter().filter_map(|h| h.signature.as_deref()).collect();
    assert!(names.contains(&"Greeter"), "class Greeter, got {names:?}");
    // `greet` exists ONLY as a child of `Greeter` (no module-level `greet`). `Vec::contains` is EXACT element
    // equality (not substring) — it proves child extraction: this test FAILS if `children` recursion is
    // dropped (the flat top-level parse never sees `greet`), and `module_greet` can't false-green it.
    assert!(
        names.contains(&"greet"),
        "method `greet` must appear via Greeter.children recursion, got {names:?}"
    );
    s.shutdown();
}

#[test]
fn resolve_pos_handles_duplicate_name() {
    if !ready() {
        eprintln!("skip");
        return;
    }
    let mut s = start();
    s.ensure_ready(Duration::from_secs(60)).unwrap();
    // `module_greet` exists as BOTH a method (Greeter.module_greet) and a module function. First-hit must
    // resolve to a real location (degradation documented: we keep the name-only API; basedpyright ranks
    // the hits). (`greet` is the nested-only name used by the recursion test — distinct from this pair.)
    let def = s.definition("module_greet").unwrap();
    assert!(
        !def.is_empty(),
        "duplicate `module_greet` must resolve to some def, got {def:?}"
    );
    s.shutdown();
}

#[test]
fn hover_is_non_empty() {
    if !ready() {
        eprintln!("skip");
        return;
    }
    let mut s = start();
    s.ensure_ready(Duration::from_secs(60)).unwrap();
    let h = s.hover("Greeter").unwrap();
    assert!(
        h.as_deref().map(|x| !x.is_empty()).unwrap_or(false),
        "hover must return non-empty content (MarkupContent or MarkedString[]), got {h:?}"
    );
    s.shutdown();
}

#[test]
fn workspace_symbol_finds_class() {
    if !ready() {
        eprintln!("skip");
        return;
    }
    let mut s = start();
    s.ensure_ready(Duration::from_secs(60)).unwrap();
    assert!(!s.workspace_symbol("Greeter").unwrap().is_empty());
    s.shutdown();
}

#[test]
fn references_and_callhierarchy_and_definition() {
    if !ready() {
        eprintln!("skip");
        return;
    }
    let mut s = start();
    s.ensure_ready(Duration::from_secs(60)).unwrap();
    assert!(!s.definition("Greeter").unwrap().is_empty(), "definition");
    let _refs = s.references("greet", true).unwrap(); // must not error
    let _calls = s.call_hierarchy("module_greet", true).unwrap(); // incoming callers, must not error
    let _impls = s.implementations("Greeter").unwrap(); // basedpyright may return [] — must not error
    s.shutdown();
}
