//! The per-language server registry: `LangServerConfig` parameterizes the language-agnostic `LspClient`.
//! `Readiness` absorbs ONLY the reader-thread NOTIFICATION parsing (id-routing stays in LspClient).
use serde_json::{json, Value};
use std::path::Path;
use std::time::{Duration, Instant};

/// The language the shim drives. Chosen by `--lang` (explicit) or `detect_lang` (`--lang auto`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    Python,
}

impl Lang {
    pub fn as_str(&self) -> &'static str {
        match self {
            Lang::Rust => "rust",
            Lang::Python => "python",
        }
    }
}

/// Detect the language from `repo`'s root markers, single-unambiguous-root ONLY (spec §1).
/// Errors on BOTH-markers (ambiguous) or NEITHER (cannot detect → require explicit --lang).
pub fn detect_lang(repo: &Path) -> anyhow::Result<Lang> {
    let is_rust = repo.join("Cargo.toml").is_file();
    let is_python = python_markers(repo) || has_real_pyproject(repo) || shallow_py_scan(repo);
    match (is_rust, is_python) {
        (true, true) => anyhow::bail!(
            "ambiguous repo root (both Rust and Python markers) at {repo:?}; pass an explicit --lang"
        ),
        (true, false) => Ok(Lang::Rust),
        (false, true) => Ok(Lang::Python),
        (false, false) => anyhow::bail!(
            "could not detect language at {repo:?} (no Cargo.toml / setup.py / setup.cfg / requirements*.txt / \
             pyproject project section / .py files); pass an explicit --lang"
        ),
    }
}

fn python_markers(repo: &Path) -> bool {
    if repo.join("setup.py").is_file() || repo.join("setup.cfg").is_file() {
        return true;
    }
    // requirements*.txt
    if let Ok(rd) = std::fs::read_dir(repo) {
        for e in rd.flatten() {
            let n = e.file_name();
            let n = n.to_string_lossy();
            if n.starts_with("requirements") && n.ends_with(".txt") {
                return true;
            }
        }
    }
    false
}

/// A pyproject.toml with a REAL project/dep section — not merely a tooling table ([tool.black]/[tool.ruff]).
fn has_real_pyproject(repo: &Path) -> bool {
    let p = repo.join("pyproject.toml");
    let Ok(text) = std::fs::read_to_string(&p) else {
        return false;
    };
    const REAL: [&str; 5] = [
        "[project]",
        "[tool.poetry]",
        "[tool.pdm]",
        "[build-system]",
        "[project.dependencies]",
    ];
    text.lines().any(|l| {
        let l = l.trim();
        // `starts_with("dynamic")` matches a `dynamic = [...]` key (PEP 621) but NOT an arbitrary bare
        // `dynamic` token mid-line; the old `l == "dynamic"` matched nothing useful and a bare-`dynamic`
        // arm would over-match — anchor to the line start.
        REAL.iter().any(|m| l.starts_with(m)) || l.starts_with("dynamic")
    })
}

/// Directories excluded from the shallow `.py` scan. `.venv`/`venv` are Python virtualenvs;
/// `target` is the Rust build cache; `node_modules` is JS; `build` and `dist` are common
/// build-output directories (e.g. `setuptools`/`wheel` output) excluded so generated `.py`
/// artifacts don't spuriously trigger Python detection; `vendor` is vendored deps.
const EXCLUDED_SCAN_DIRS: [&str; 8] = [
    ".venv",
    "venv",
    ".git",
    "target",
    "node_modules",
    "build",
    "dist",
    "vendor",
];

/// Shallow recursive scan for any `*.py`, excluding venv/build/vendor/hidden dirs. Bounded depth (3) so a
/// huge tree doesn't stall startup; a real Python project has a `.py` within a few levels of the root.
fn shallow_py_scan(repo: &Path) -> bool {
    fn walk(dir: &Path, depth: u8) -> bool {
        if depth == 0 {
            return false;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return false;
        };
        for e in rd.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            let ty = e.file_type();
            if ty.as_ref().map(|t| t.is_dir()).unwrap_or(false) {
                if name.starts_with('.') || EXCLUDED_SCAN_DIRS.contains(&name.as_ref()) {
                    continue;
                }
                if walk(&e.path(), depth - 1) {
                    return true;
                }
            } else if name.ends_with(".py") {
                return true;
            }
        }
        false
    }
    walk(repo, 3)
}

/// Per-language readiness: the notification-parsing half of the reader thread + the ready predicate.
/// `RustRa` reproduces the current `$/progress` + `experimental/serverStatus` machine byte-for-byte.
#[derive(Debug)]
pub enum Readiness {
    RustRa(RustReady),
    Pyright(PyrightReady),
}

/// Rust path readiness state — the old `ReadyState`, unchanged fields/semantics.
#[derive(Debug, Default)]
pub struct RustReady {
    pub began: bool,
    pub active: u32,
    /// Latest `experimental/serverStatus { quiescent }` from rust-analyzer — true once it has finished
    /// loading/indexing and has no background work in flight. A reliable readiness signal even when the
    /// `$/progress` begin/end pair never fires (warm/fast index), which otherwise stalled the first tool
    /// call for the full `ensure_ready` timeout (~30s). RA sends it because we advertise
    /// `serverStatusNotification` in `initialize`.
    pub quiescent: bool,
}

/// Python (basedpyright) readiness: `pyright/{begin,end}Progress` + a short no-progress settle.
#[derive(Debug, Default)]
pub struct PyrightReady {
    pub began: bool,
    pub active: u32,
    /// Set (`Some(Instant::now())`) the moment `initialized`+settings are applied in `handshake`. This is
    /// the CORRECT settle-clock origin: the no-progress settle is timed from when settings were applied,
    /// NOT from `wait_ready` entry (Opus H2). Timing from `wait_ready` entry made a begin-without-end
    /// server pay the FULL timeout — the exact FU3 stall §2 forbids. None until settings are applied.
    pub settled_at: Option<Instant>,
}

impl PyrightReady {
    /// PURE no-progress settle gate: settings applied, no progress begun, and the settle window has
    /// elapsed since `settled_at`. Timed from `settled_at` (settings-applied), not from any wait entry.
    pub fn settled_no_progress(&self, settle: Duration) -> bool {
        !self.began
            && self
                .settled_at
                .map(|t| t.elapsed() >= settle)
                .unwrap_or(false)
    }
}

impl Readiness {
    /// Parse one inbound NOTIFICATION (never a response — id-routing is the caller's job). Mutates state.
    pub fn on_notification(&mut self, method: &str, params: &Value) {
        match self {
            Readiness::RustRa(s) => match method {
                "$/progress" => match params["value"]["kind"].as_str() {
                    Some("begin") => {
                        s.began = true;
                        s.active += 1;
                    }
                    Some("end") => {
                        s.active = s.active.saturating_sub(1);
                    }
                    _ => {}
                },
                "experimental/serverStatus" => {
                    if let Some(q) = params.get("quiescent").and_then(|q| q.as_bool()) {
                        s.quiescent = q;
                    }
                }
                _ => {}
            },
            Readiness::Pyright(s) => match method {
                "pyright/beginProgress" => {
                    s.began = true;
                    s.active += 1;
                }
                "pyright/endProgress" => {
                    s.active = s.active.saturating_sub(1);
                }
                _ => {}
            },
        }
    }

    /// PURE ready predicate. RustRa: quiescent OR begun-and-ended. Pyright: begun-and-ended, OR
    /// settings applied with no progress seen (the no-progress settle is timed by LspClient::wait_ready
    /// via `PyrightReady::settled_no_progress`). The settle branch is the LOAD-BEARING path because
    /// basedpyright emits NO `pyright/*Progress` for a typical analysis (Task-1 spike): it is an
    /// INDEPENDENT OR-branch, NOT gated behind having seen `begin`. begin/end parsing is harmless
    /// belt-and-suspenders for servers that DO emit progress.
    pub fn is_ready(&self) -> bool {
        match self {
            Readiness::RustRa(s) => s.quiescent || (s.began && s.active == 0),
            // The pure predicate covers the begun-and-ended branch; the no-progress settle branch is
            // OR'd in by LspClient::wait_ready, which owns the settle window (a Duration, not in scope here).
            Readiness::Pyright(s) => s.began && s.active == 0,
        }
    }
}

/// What `LspClient` needs to drive any language server.
pub struct LangServerConfig {
    /// Display name for the startup log + errors (e.g. "rust-analyzer", "basedpyright").
    pub name: &'static str,
    /// argv[0] + args to spawn the server (e.g. ["rust-analyzer"], ["basedpyright-langserver","--stdio"]).
    pub program_argv: Vec<String>,
    /// Extra spawn env (rust: CARGO_TARGET_DIR when a target cache is given).
    pub spawn_env: Vec<(String, String)>,
    /// Predicate: is `root` a valid project root for THIS language? Used by `run()` to validate an
    /// EXPLICIT `--lang rust|python` against the repo before starting (spec §1/§2 root markers). For
    /// `--lang auto` the language is already chosen by `detect_lang`, so this is a redundant-but-cheap
    /// re-check; for explicit `--lang` it is the ONLY guard against pointing the wrong server at a repo.
    pub is_project_root: Box<dyn Fn(&Path) -> bool + Send + Sync>,
    /// The `initialize` params for this language (rooted at `root_uri`).
    pub initialize_params: Box<dyn Fn(&str) -> Value + Send + Sync>,
    /// Notification sent immediately after `initialized` (Python: didChangeConfiguration). None for Rust.
    pub post_init_config: Option<(String, Value)>,
    /// A fresh readiness machine for this language (one per spawn).
    pub new_readiness: Box<dyn Fn() -> Readiness + Send + Sync>,
}

/// Rust / rust-analyzer config — reproduces the pre-refactor `spawn_ra`+`handshake` exactly.
pub fn rust_ra_config(target_cache: Option<&Path>) -> LangServerConfig {
    let spawn_env = target_cache
        .map(|tc| vec![("CARGO_TARGET_DIR".to_string(), tc.display().to_string())])
        .unwrap_or_default();
    LangServerConfig {
        name: "rust-analyzer",
        program_argv: vec!["rust-analyzer".to_string()],
        spawn_env,
        is_project_root: Box::new(|root: &Path| root.join("Cargo.toml").exists()),
        initialize_params: Box::new(|root_uri: &str| {
            json!({
                "processId": std::process::id(),
                "rootUri": root_uri,
                "capabilities": { "workspace": { "symbol": {} },
                    "experimental": { "serverStatusNotification": true } },
                "workspaceFolders": [{ "uri": root_uri, "name": "root" }],
            })
        }),
        post_init_config: None,
        new_readiness: Box::new(|| Readiness::RustRa(RustReady::default())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_ready_via_quiescent_or_progress() {
        let mut r = Readiness::RustRa(RustReady::default());
        assert!(!r.is_ready());
        r.on_notification("experimental/serverStatus", &json!({"quiescent": true}));
        assert!(r.is_ready(), "quiescent alone is ready");

        let mut r = Readiness::RustRa(RustReady::default());
        r.on_notification("$/progress", &json!({"value":{"kind":"begin"}}));
        assert!(!r.is_ready(), "begun, still active");
        r.on_notification("$/progress", &json!({"value":{"kind":"end"}}));
        assert!(r.is_ready(), "begun-and-ended is ready");
    }

    #[test]
    fn rust_ignores_wrong_typed_quiescent() {
        let mut r = Readiness::RustRa(RustReady::default());
        r.on_notification("experimental/serverStatus", &json!({"quiescent":"yes"}));
        assert!(
            !r.is_ready(),
            "non-bool quiescent is ignored (keeps prior false)"
        );
    }

    #[test]
    fn pyright_ready_via_progress() {
        let mut r = Readiness::Pyright(PyrightReady::default());
        r.on_notification("pyright/beginProgress", &json!({}));
        assert!(!r.is_ready());
        r.on_notification("pyright/endProgress", &json!({}));
        assert!(r.is_ready());
    }

    #[test]
    fn pyright_settles_with_no_progress() {
        // The LOAD-BEARING basedpyright path: NO progress ever fires, ready via settle (Task-1 spike).
        let mut r = PyrightReady::default();
        assert!(
            !r.settled_no_progress(Duration::from_millis(0)),
            "settled_at unset → not settled"
        );
        r.settled_at = Some(Instant::now() - Duration::from_secs(5));
        assert!(
            r.settled_no_progress(Duration::from_secs(1)),
            "settings applied + window elapsed + no progress → settled"
        );
        // begin seen → the settle branch must NOT fire (begin/end governs instead).
        r.began = true;
        assert!(
            !r.settled_no_progress(Duration::from_secs(1)),
            "begin seen suppresses the no-progress settle"
        );
    }

    #[test]
    fn rust_initialize_params_match_pinned_handshake() {
        let cfg = rust_ra_config(None);
        let p = (cfg.initialize_params)("file:///repo");
        assert_eq!(
            p["capabilities"]["experimental"]["serverStatusNotification"],
            json!(true)
        );
        assert_eq!(p["capabilities"]["workspace"]["symbol"], json!({}));
        assert!(
            cfg.post_init_config.is_none(),
            "Rust sends NO post-init config"
        );
    }
}
