//! The per-language server registry: `LangServerConfig` parameterizes the language-agnostic `LspClient`.
//! `Readiness` absorbs ONLY the reader-thread NOTIFICATION parsing (id-routing stays in LspClient).
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The language the shim drives. Chosen by `--lang` (explicit) or `detect_lang` (`--lang auto`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    Python,
    Go,
    TypeScript,
}

impl Lang {
    pub fn as_str(&self) -> &'static str {
        match self {
            Lang::Rust => "rust",
            Lang::Python => "python",
            Lang::Go => "go",
            Lang::TypeScript => "typescript",
        }
    }
}

/// Typed outcome of root-marker detection (spec §1): branch on the variant, never parse error strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detection {
    Detected(Lang),
    None,
    Ambiguous,
}

/// Typed root-marker detection (spec §1): single unambiguous marker → Detected; zero → None;
/// two-or-more → Ambiguous. No bail / no error strings — callers branch on the variant.
pub fn detect(repo: &Path) -> Detection {
    let is_rust = repo.join("Cargo.toml").is_file();
    let is_python = python_markers(repo) || has_real_pyproject(repo) || shallow_py_scan(repo);
    let is_go = repo.join("go.mod").is_file();
    // Detection marker for TS is tsconfig.json ONLY — NOT package.json (a tooling package.json is
    // present in a huge fraction of rust/python repos and would flip them to Ambiguous; tsconfig.json
    // is a strong, low-noise TS signal; tsconfig-less pure-JS needs explicit `--lang typescript`).
    let is_ts = repo.join("tsconfig.json").is_file();
    match [is_rust, is_python, is_go, is_ts]
        .iter()
        .filter(|b| **b)
        .count()
    {
        0 => Detection::None,
        1 if is_rust => Detection::Detected(Lang::Rust),
        1 if is_python => Detection::Detected(Lang::Python),
        1 if is_go => Detection::Detected(Lang::Go),
        1 => Detection::Detected(Lang::TypeScript),
        _ => Detection::Ambiguous,
    }
}

/// Detect the language from `repo`'s root markers, single-unambiguous-root ONLY (spec §1).
/// `go.mod` → go; the existing rust (`Cargo.toml`) / python (markers) predicates are unchanged.
/// ANY two-or-more markers present → ambiguous→refuse; NONE → cannot-detect → require explicit --lang.
pub fn detect_lang(repo: &Path) -> anyhow::Result<Lang> {
    match detect(repo) {
        Detection::Detected(l) => Ok(l),
        Detection::Ambiguous => {
            let is_rust = repo.join("Cargo.toml").is_file();
            let is_python = python_markers(repo) || has_real_pyproject(repo) || shallow_py_scan(repo);
            let is_go = repo.join("go.mod").is_file();
            let is_ts = repo.join("tsconfig.json").is_file();
            anyhow::bail!(
                "ambiguous repo root (multiple language markers: rust={is_rust} python={is_python} go={is_go} typescript={is_ts}) \
                 at {repo:?}; pass an explicit --lang"
            )
        }
        Detection::None => anyhow::bail!(
            "could not detect language at {repo:?} (no Cargo.toml / setup.py / setup.cfg / requirements*.txt / \
             pyproject project section / .py files / go.mod / tsconfig.json); pass an explicit --lang"
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
    Pyright(SettleReady),
    Gopls(SettleReady),
    Ts(SettleReady),
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

/// Shared settle-based readiness state for Pyright, Gopls, and TypeScript. All three use the same
/// `{began, active, settled_at}` fields and the same `settled_no_progress` predicate — extracted here to
/// avoid byte-identical duplication (was `PyrightReady` + `GoplsReady`; TS would have been a 3rd copy).
///
/// Pyright uses `pyright/{begin,end}Progress`; Gopls and Ts use `$/progress` begin/end. The notification
/// dispatch lives in `Readiness::on_notification`; this struct is PURE state + the settle predicate.
#[derive(Debug, Default)]
pub struct SettleReady {
    pub began: bool,
    pub active: u32,
    /// Set (`Some(Instant::now())`) when the server is considered "settings applied" in `handshake`:
    /// for Pyright, after `post_init_config`; for Gopls and Ts, right after `initialized` (no post-init).
    /// None until that point. The no-progress settle is timed from here, NOT from `wait_ready` entry (Opus H2).
    pub settled_at: Option<Instant>,
}

impl SettleReady {
    /// PURE no-progress settle gate: no progress begun and the settle window has elapsed since `settled_at`.
    /// Timed from `settled_at` (settings-applied / initialized), not from any wait entry.
    pub fn settled_no_progress(&self, settle: Duration) -> bool {
        !self.began
            && self
                .settled_at
                .map(|t| t.elapsed() >= settle)
                .unwrap_or(false)
    }
}

/// Type alias kept for backward compatibility with the external characterization harness
/// (`crate::testkit` re-exports it as `PyrightReady`).
pub type PyrightReady = SettleReady;
/// Type alias kept for backward compatibility (was a separate struct, now unified as `SettleReady`).
pub type GoplsReady = SettleReady;

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
            Readiness::Gopls(s) | Readiness::Ts(s) => {
                if method == "$/progress" {
                    match params["value"]["kind"].as_str() {
                        Some("begin") => {
                            s.began = true;
                            s.active += 1;
                        }
                        Some("end") => {
                            s.active = s.active.saturating_sub(1);
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// PURE ready predicate. RustRa: quiescent OR begun-and-ended. Pyright/Gopls/Ts: begun-and-ended, OR
    /// settings applied with no progress seen (the no-progress settle is timed by LspClient::wait_ready
    /// via `settled_no_progress`). The settle branch is the LOAD-BEARING path because basedpyright/gopls/tsls
    /// emit NO progress for a typical analysis/load (Task-1 spikes): it is an
    /// INDEPENDENT OR-branch, NOT gated behind having seen `begin`. begin/end parsing is harmless
    /// belt-and-suspenders for servers that DO emit progress.
    pub fn is_ready(&self) -> bool {
        match self {
            Readiness::RustRa(s) => s.quiescent || (s.began && s.active == 0),
            // The pure predicate covers the begun-and-ended branch; the no-progress settle branch is
            // OR'd in by LspClient::wait_ready, which owns the settle window (a Duration, not in scope here).
            Readiness::Pyright(s) | Readiness::Gopls(s) | Readiness::Ts(s) => {
                s.began && s.active == 0
            }
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
                    "textDocument": { "documentSymbol": { "hierarchicalDocumentSymbolSupport": true } },
                    "experimental": { "serverStatusNotification": true } },
                "workspaceFolders": [{ "uri": root_uri, "name": "root" }],
            })
        }),
        post_init_config: None,
        new_readiness: Box::new(|| Readiness::RustRa(RustReady::default())),
    }
}

/// Validate a candidate interpreter exists AND is executable. A regular file alone is NOT enough — a
/// non-executable path would make basedpyright fail to launch the interpreter at use-time (spec §2 says
/// the chosen path is validated exists+executable before use). Unix: check the file mode's execute bits.
fn is_usable_interpreter(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(p) {
        Ok(m) => m.is_file() && (m.permissions().mode() & 0o111 != 0),
        Err(_) => false,
    }
}

/// Outcome of interpreter discovery. `Hard` = an EXPLICIT override (flag/env) that is missing or
/// non-executable → the caller MUST `anyhow::bail!` (never silently fall back to `python3` — that would
/// mask a typo and silently degrade third-party resolution). `Resolved(p)` = a usable venv interpreter.
/// `Fallback` = no explicit override AND no venv found → caller uses `python3` on PATH + a LOGGED WARNING.
pub enum PyResolve {
    Resolved(PathBuf),
    Fallback,
    Hard(PathBuf),
}

/// Ordered interpreter discovery (spec §2). Precedence:
/// (1) explicit flag / LSP_MCP_PYTHON_PATH — if INVALID (missing/non-executable) → `Hard` (caller bails);
/// (2) $VIRTUAL_ENV/bin/python, (3) <repo>/.venv/bin/python, (4) <repo>/venv/bin/python — first usable wins;
/// (5) none → `Fallback` (caller uses `python3` on PATH with a logged warning).
/// NOTE: a bad EXPLICIT override is a HARD ERROR, NOT a silent `python3` fallback. The silent+warned
/// `python3` fallback is ONLY the no-explicit-override / no-venv case.
pub fn resolve_python_path(
    repo: &Path,
    explicit: Option<&Path>,
    virtual_env: Option<&Path>,
) -> PyResolve {
    if let Some(p) = explicit {
        // A RELATIVE explicit path must be resolved against `repo` (basedpyright is spawned with
        // `current_dir(repo)` so it consumes `pythonPath` relative to the repo, not the process cwd).
        // Join onto `repo` so validation and consumption agree; absolute paths pass through unchanged.
        let resolved = if p.is_relative() {
            repo.join(p)
        } else {
            p.to_path_buf()
        };
        return if is_usable_interpreter(&resolved) {
            PyResolve::Resolved(resolved)
        } else {
            PyResolve::Hard(resolved) // bad explicit override → HARD ERROR (no silent fallback)
        };
    }
    let candidates = [
        virtual_env.map(|v| v.join("bin/python")),
        Some(repo.join(".venv/bin/python")),
        Some(repo.join("venv/bin/python")),
    ];
    for c in candidates.into_iter().flatten() {
        if is_usable_interpreter(&c) {
            return PyResolve::Resolved(c);
        }
    }
    PyResolve::Fallback // no venv found → python3 fallback (warned), only because there was no explicit override
}

/// Resolve a bare server program name to an absolute path suitable for `Command::program`.
///
/// Search order:
/// 1. If `name` contains a path separator, return it unchanged — it is already a path.
/// 2. Each `:` -separated directory in `path_var` (the value of `$PATH`): `<dir>/<name>`.
/// 3. `home_dir/.local/bin/<name>` — the standard `uv tool install` shim location.
/// 4. `home_dir/.cargo/bin/<name>` — the standard `cargo install` location.
/// 5. Each dir in `go_candidates` (e.g. `$GOPATH/bin`, `$(go env GOROOT)/bin`) — for off-PATH gopls.
///
/// The first candidate that `is_usable_interpreter` accepts (exists + executable) is returned as an
/// absolute path string. If none match, `name` is returned unchanged so the caller degrades to the
/// existing "not found" OS error rather than panicking.
///
/// Factored to accept explicit `path_var`, `home_dir`, and `go_candidates` so tests can be hermetic.
fn resolve_lsp_server_with_env(
    name: &str,
    path_var: Option<&str>,
    home_dir: Option<&str>,
    go_candidates: &[PathBuf],
) -> String {
    use std::path::MAIN_SEPARATOR;
    // If the name already contains a separator it is already a concrete path — return as-is.
    if name.contains(MAIN_SEPARATOR) {
        return name.to_string();
    }
    // Search PATH directories.
    if let Some(path) = path_var {
        for dir in path.split(':').filter(|d| !d.is_empty()) {
            let candidate = std::path::Path::new(dir).join(name);
            if is_usable_interpreter(&candidate) {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }
    // Fallback: HOME/.local/bin (uv shims) then HOME/.cargo/bin.
    if let Some(home) = home_dir {
        let home = std::path::Path::new(home);
        for subdir in &[".local/bin", ".cargo/bin"] {
            let candidate = home.join(subdir).join(name);
            if is_usable_interpreter(&candidate) {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }
    // Go toolchain locations (for a bare `gopls` not on PATH, e.g. in a container).
    for dir in go_candidates {
        let candidate = dir.join(name);
        if is_usable_interpreter(&candidate) {
            return candidate.to_string_lossy().into_owned();
        }
    }
    // Nothing found — return bare name so the caller gets the normal OS "not found" error.
    name.to_string()
}

/// Best-effort Go toolchain bin dirs from `go env` (`$GOPATH/bin`, `$(go env GOROOT)/bin`). Skipped
/// (empty) if `go` is absent — never panics. Used only to find a bare `gopls` that isn't on PATH.
fn go_bin_candidates() -> Vec<PathBuf> {
    ["GOPATH", "GOROOT"]
        .into_iter()
        .filter_map(go_env_bin)
        .collect()
}

fn go_env_bin(var: &str) -> Option<PathBuf> {
    let output = std::process::Command::new("go")
        .args(["env", var])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() {
        return None;
    }
    Some(PathBuf::from(path).join("bin"))
}

/// Thin wrapper around `resolve_lsp_server_with_env` that reads the real `$PATH` and `$HOME`.
fn resolve_lsp_server(name: &str) -> String {
    let path = std::env::var("PATH").ok();
    let home = std::env::var("HOME").ok();
    // The Go toolchain dirs are only relevant for gopls. Building them shells `go env GOPATH`/`GOROOT`, so
    // gate on the name — resolving rust-analyzer / basedpyright must NOT spawn those Go subprocesses.
    let go = if name == "gopls" {
        go_bin_candidates()
    } else {
        Vec::new()
    };
    let resolved = resolve_lsp_server_with_env(name, path.as_deref(), home.as_deref(), &go);
    // Log a hint when resolution falls back to the bare name (binary not found in any search location).
    if resolved == name && !name.contains(std::path::MAIN_SEPARATOR) {
        eprintln!(
            "[lsp-mcp] WARNING: {name} not found on PATH / ~/.local/bin / ~/.cargo/bin / Go toolchain bin; \
             install it (basedpyright: 'uv tool install basedpyright'; gopls: 'go install golang.org/x/tools/gopls@latest')"
        );
    }
    resolved
}

/// True if the repo pins its OWN basedpyright/pyright config, which OVERRIDES the pushed `pythonPath`
/// (so a warmed venv may be ignored). Checks `pyrightconfig.json` or a `[tool.pyright]`/`[tool.basedpyright]`
/// section in `pyproject.toml`. Cheap, best-effort (a read failure → false).
pub fn repo_has_pyright_config(repo: &Path) -> bool {
    if repo.join("pyrightconfig.json").is_file() {
        return true;
    }
    std::fs::read_to_string(repo.join("pyproject.toml"))
        .map(|s| s.contains("[tool.pyright]") || s.contains("[tool.basedpyright]"))
        .unwrap_or(false)
}

/// Python / basedpyright config. Resolves the interpreter, advertises NO `window/workDoneProgress` (so the
/// no-progress settle — not a progress cycle — is the readiness signal; Task-1 spike Gate 2), and sends the
/// WRAPPED `didChangeConfiguration { settings: { python: { pythonPath } } }` envelope (spike Gate 1a).
pub fn pyright_config(
    repo: &Path,
    explicit_python: Option<&Path>,
) -> anyhow::Result<LangServerConfig> {
    let virtual_env = std::env::var_os("VIRTUAL_ENV").map(PathBuf::from);
    let explicit = explicit_python
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("LSP_MCP_PYTHON_PATH").map(PathBuf::from));
    let python_path = match resolve_python_path(repo, explicit.as_deref(), virtual_env.as_deref()) {
        PyResolve::Resolved(p) => {
            let s = p.display().to_string();
            eprintln!("[lsp-mcp] python interpreter: {s}");
            s
        }
        // An EXPLICIT --python-path / LSP_MCP_PYTHON_PATH that is missing or non-executable is a HARD ERROR:
        // never silently fall back to `python3` (that would mask a typo and silently degrade resolution).
        PyResolve::Hard(p) => anyhow::bail!(
            "explicit python interpreter {:?} is missing or not executable — fix --python-path / \
             LSP_MCP_PYTHON_PATH (no silent fallback to python3 for an explicit override)",
            p
        ),
        // No explicit override AND no venv → degrade to `python3` on PATH with a LOGGED WARNING (stdlib
        // still resolves; third-party may be incomplete; spike Gate 1c). This is the ONLY warned-fallback case.
        PyResolve::Fallback => {
            eprintln!(
                "[lsp-mcp] WARNING: no venv interpreter found for {repo:?}; using `python3` on PATH — \
                 third-party (site-packages) resolution may be incomplete. Pass --python-path to fix."
            );
            "python3".to_string()
        }
    };
    if repo_has_pyright_config(repo) {
        eprintln!(
            "[lsp-mcp] WARNING: {repo:?} has a pyrightconfig.json / [tool.(based)pyright] section — \
             basedpyright may honor it OVER the pushed pythonPath {python_path:?}, so a warmed venv \
             could be ignored (third-party resolution may differ). See docs/containerized-mcp-env-trap.md."
        );
    }
    // The LSP-standard WRAPPED `settings` envelope (spike Gate 1a) — the proven form that resolves
    // third-party defs into the venv's site-packages. Do NOT regress to a bare `{ "python": {…} }` form.
    let post = (
        "workspace/didChangeConfiguration".to_string(),
        json!({ "settings": { "python": { "pythonPath": python_path } } }),
    );
    let server_bin = resolve_lsp_server("basedpyright-langserver");
    eprintln!("[lsp-mcp] basedpyright server: {server_bin}");
    Ok(LangServerConfig {
        name: "basedpyright",
        program_argv: vec![server_bin, "--stdio".to_string()],
        spawn_env: vec![],
        // The Python root predicates (reuse the Task-4 detection helpers) — a setup.py/setup.cfg/
        // requirements*.txt marker, a REAL pyproject section, or a shallow `.py` scan. `run()` validates an
        // explicit `--lang python` against this before starting basedpyright.
        is_project_root: Box::new(|root: &Path| {
            python_markers(root) || has_real_pyproject(root) || shallow_py_scan(root)
        }),
        initialize_params: Box::new(|root_uri: &str| {
            // Advertise NO window/workDoneProgress → readiness is reached via the no-progress settle, not a
            // progress cycle (basedpyright emits no `pyright/*Progress` for typical analyses; spike Gate 2).
            json!({
                "processId": std::process::id(),
                "rootUri": root_uri,
                "capabilities": { "workspace": { "symbol": {}, "configuration": false },
                    "textDocument": { "documentSymbol": { "hierarchicalDocumentSymbolSupport": true } } },
                "workspaceFolders": [{ "uri": root_uri, "name": "root" }],
            })
        }),
        post_init_config: Some(post),
        new_readiness: Box::new(|| Readiness::Pyright(SettleReady::default())),
    })
}

/// Go / gopls config. gopls auto-configures from `go.mod` — NO interpreter discovery, NO
/// didChangeConfiguration (`post_init_config = None`), `spawn_env = []` (gopls inherits GOPATH/GOROOT from
/// the env; Task-1 spike Gate 2). `program_argv = [resolve_lsp_server("gopls"), "serve"]` (the spike's
/// proven stdio invocation; Gate 1). Readiness is the no-progress settle (Gate 3).
pub fn go_config(repo: &Path) -> anyhow::Result<LangServerConfig> {
    let _ = repo; // gopls needs no per-repo discovery; `repo` kept for signature symmetry / future use.
    let server_bin = resolve_lsp_server("gopls");
    eprintln!("[lsp-mcp] gopls server: {server_bin}");
    Ok(LangServerConfig {
        name: "gopls",
        program_argv: vec![server_bin, "serve".to_string()],
        spawn_env: vec![],
        is_project_root: Box::new(|root: &Path| root.join("go.mod").exists()),
        initialize_params: Box::new(|root_uri: &str| {
            json!({
                "processId": std::process::id(),
                "rootUri": root_uri,
                // Advertise hierarchical documentSymbol so methods on a struct/interface surface via the
                // recursive `collect_doc_symbols` walk. NO window/workDoneProgress → readiness is the
                // no-progress settle (gopls emits no progress for a typical load; spike Gate 3).
                "capabilities": { "workspace": { "symbol": {} },
                    "textDocument": { "documentSymbol": { "hierarchicalDocumentSymbolSupport": true } } },
                "workspaceFolders": [{ "uri": root_uri, "name": "root" }],
            })
        }),
        post_init_config: None,
        new_readiness: Box::new(|| Readiness::Gopls(SettleReady::default())),
    })
}

/// TypeScript / typescript-language-server config. The server is swappable via `LSP_MCP_TS_SERVER` (env)
/// so the container profile can override it without a rebuild — set `lsp_env = { LSP_MCP_TS_SERVER = "vtsls" }`
/// in the TS `[[languages]]` profile. Detection marker = `tsconfig.json` (NOT bare `package.json` — too noisy).
/// `is_project_root` is MORE lenient (package.json suffices for explicit `--lang typescript`). Readiness =
/// no-progress settle (typescript-language-server's `$/progress` is unreliable; same signal as gopls).
pub fn ts_config(repo: &Path) -> anyhow::Result<LangServerConfig> {
    let _ = repo;
    let server =
        std::env::var("LSP_MCP_TS_SERVER").unwrap_or_else(|_| "typescript-language-server".into());
    let server_bin = resolve_lsp_server(&server);
    eprintln!("[lsp-mcp] typescript server: {server_bin}");
    Ok(LangServerConfig {
        name: "typescript-language-server",
        program_argv: vec![server_bin, "--stdio".to_string()],
        spawn_env: vec![],
        // Explicit `--lang typescript` at a package.json-only repo (no tsconfig.json) is valid — the user
        // is choosing the language explicitly. Auto-detect requires tsconfig.json; explicit is more lenient.
        is_project_root: Box::new(|root: &Path| {
            root.join("tsconfig.json").exists() || root.join("package.json").exists()
        }),
        initialize_params: Box::new(|root_uri: &str| {
            json!({
                "processId": std::process::id(),
                "rootUri": root_uri,
                "capabilities": { "workspace": { "symbol": {} },
                    "textDocument": { "documentSymbol": { "hierarchicalDocumentSymbolSupport": true } } },
                "workspaceFolders": [{ "uri": root_uri, "name": "root" }],
                // If impl finds tsserver isn't auto-discovered under the stripped env, add:
                // "initializationOptions": { "tsserver": { "path": "<global typescript lib tsserver.js>" } }
            })
        }),
        post_init_config: None,
        new_readiness: Box::new(|| Readiness::Ts(SettleReady::default())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_typed_classifies_none_rust_go_ambiguous() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(detect(d.path()), Detection::None);
        std::fs::write(d.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        assert_eq!(detect(d.path()), Detection::Detected(Lang::Rust));
        std::fs::write(d.path().join("go.mod"), "module x\n").unwrap();
        assert_eq!(detect(d.path()), Detection::Ambiguous);
        std::fs::remove_file(d.path().join("Cargo.toml")).unwrap();
        assert_eq!(detect(d.path()), Detection::Detected(Lang::Go));
    }

    // ---------------------------------------------------------------------------
    // resolve_lsp_server hermetic unit tests (TDD RED → GREEN)
    // ---------------------------------------------------------------------------

    /// Helper: make a temp dir with an executable `name` binary (mode 0o755) and return its path.
    #[cfg(unix)]
    fn make_fake_executable(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(&p, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    /// A bare name found on PATH → returns its absolute path.
    #[test]
    fn resolve_lsp_server_finds_name_on_path() {
        let dir = tempfile::tempdir().unwrap();
        make_fake_executable(dir.path(), "basedpyright-langserver");
        let resolved = resolve_lsp_server_with_env(
            "basedpyright-langserver",
            Some(dir.path().to_str().unwrap()),
            None,
            &[],
        );
        assert_eq!(
            resolved,
            dir.path()
                .join("basedpyright-langserver")
                .to_str()
                .unwrap()
                .to_string(),
            "should find the executable on the provided PATH"
        );
    }

    /// PATH lacks the binary but HOME/.local/bin has it → returns that absolute path.
    #[test]
    fn resolve_lsp_server_falls_back_to_home_local_bin() {
        let home = tempfile::tempdir().unwrap();
        let local_bin = home.path().join(".local/bin");
        std::fs::create_dir_all(&local_bin).unwrap();
        make_fake_executable(&local_bin, "basedpyright-langserver");

        let resolved = resolve_lsp_server_with_env(
            "basedpyright-langserver",
            Some("/nonexistent_dir_that_does_not_exist"),
            Some(home.path().to_str().unwrap()),
            &[],
        );
        assert_eq!(
            resolved,
            local_bin
                .join("basedpyright-langserver")
                .to_str()
                .unwrap()
                .to_string(),
            "should find the executable in HOME/.local/bin when not on PATH"
        );
    }

    /// gopls is not on PATH / ~/.local/bin / ~/.cargo/bin but IS in $GOPATH/bin -> resolved via go_env.
    #[test]
    fn resolve_lsp_server_finds_gopls_in_gopath_bin() {
        let gopath = tempfile::tempdir().unwrap();
        let gobin = gopath.path().join("bin");
        std::fs::create_dir_all(&gobin).unwrap();
        make_fake_executable(&gobin, "gopls");
        let resolved = resolve_lsp_server_with_env(
            "gopls",
            Some("/nonexistent_dir_that_does_not_exist"),
            Some("/another_nonexistent_home"),
            &[gopath.path().join("bin"), gopath.path().join("goroot/bin")],
        );
        assert_eq!(
            resolved,
            gobin.join("gopls").to_str().unwrap().to_string(),
            "gopls should resolve from a $GOPATH/bin candidate"
        );
    }

    /// Nothing matches → returns the bare name unchanged (degrades gracefully, no panic).
    #[test]
    fn resolve_lsp_server_returns_bare_name_when_nothing_matches() {
        let resolved = resolve_lsp_server_with_env(
            "basedpyright-langserver",
            Some("/nonexistent_dir_that_does_not_exist"),
            Some("/another_nonexistent_home"),
            &[],
        );
        assert_eq!(
            resolved, "basedpyright-langserver",
            "should return bare name when no match found"
        );
    }

    /// A name that already contains a path separator → returned unchanged, no resolution attempted.
    #[test]
    fn resolve_lsp_server_passes_through_names_with_path_separator() {
        let resolved = resolve_lsp_server_with_env(
            "/usr/local/bin/basedpyright-langserver",
            Some("/some/path"),
            Some("/some/home"),
            &[],
        );
        assert_eq!(
            resolved, "/usr/local/bin/basedpyright-langserver",
            "names with a path separator should be returned unchanged"
        );
    }

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
    fn pyright_no_progress_is_ready_after_settle_not_full_bound() {
        let settle = Duration::from_millis(50);
        // Simulate "settings applied, no progress notification arrives": settled_at = now, began = false.
        let mut p = PyrightReady {
            began: false,
            active: 0,
            settled_at: Some(Instant::now()),
        };
        // Immediately after settings applied (within the settle window): NOT yet ready, but also NOT a full
        // timeout — the settle is timed from settled_at, not from any wait entry.
        assert!(
            !p.settled_no_progress(settle),
            "not ready before the settle window elapses"
        );
        assert!(
            !Readiness::Pyright(std::mem::take(&mut p)).is_ready(),
            "no progress + no settle ⇒ is_ready() false"
        );
        // After the settle window with NO progress → ready (the first call returns fast, not at the full bound).
        let p = PyrightReady {
            began: false,
            active: 0,
            settled_at: Some(Instant::now() - Duration::from_millis(60)),
        };
        assert!(
            p.settled_no_progress(settle),
            "settle elapsed with no progress → ready (no full-bound wait)"
        );
        // settled_at = None (settings not yet applied) is never settle-ready.
        let p = PyrightReady {
            began: false,
            active: 0,
            settled_at: None,
        };
        assert!(
            !p.settled_no_progress(settle),
            "no settled_at (settings not applied) → not settle-ready"
        );
        // A begin-without-end does NOT settle (began == true) — documented begin-without-end ceiling.
        let p = PyrightReady {
            began: true,
            active: 1,
            settled_at: Some(Instant::now() - Duration::from_secs(1)),
        };
        assert!(
            !p.settled_no_progress(settle),
            "begin seen ⇒ no no-progress settle (needs a matching end)"
        );
    }

    #[test]
    fn go_config_has_no_post_init_and_uses_serve() {
        let d = tempfile::tempdir().unwrap();
        let cfg = go_config(d.path()).unwrap();
        assert_eq!(cfg.name, "gopls");
        assert!(
            cfg.post_init_config.is_none(),
            "gopls auto-configures from go.mod — NO post-init config"
        );
        assert!(
            cfg.spawn_env.is_empty(),
            "gopls inherits the env — no spawn_env injection"
        );
        assert_eq!(
            cfg.program_argv.last().map(String::as_str),
            Some("serve"),
            "gopls is spawned with the proven stdio invocation"
        );

        let d2 = tempfile::tempdir().unwrap();
        std::fs::write(d2.path().join("go.mod"), "module example.com/x\n").unwrap();
        assert!(
            (cfg.is_project_root)(d2.path()),
            "a go.mod dir is a Go project root"
        );
        assert!(
            !(cfg.is_project_root)(d.path()),
            "a dir without go.mod is NOT a Go project root"
        );
    }

    #[test]
    fn go_config_advertises_hierarchical_document_symbols() {
        let d = tempfile::tempdir().unwrap();
        let cfg = go_config(d.path()).unwrap();
        let p = (cfg.initialize_params)("file:///repo");
        assert_eq!(
            p["capabilities"]["textDocument"]["documentSymbol"]
                ["hierarchicalDocumentSymbolSupport"],
            json!(true),
            "hierarchical documentSymbol must be advertised so methods surface via children recursion"
        );
    }

    #[test]
    fn gopls_no_progress_is_ready_after_settle() {
        let settle = Duration::from_millis(50);
        let p = GoplsReady {
            began: false,
            active: 0,
            settled_at: Some(Instant::now()),
        };
        assert!(
            !p.settled_no_progress(settle),
            "not ready before the settle window elapses"
        );

        let p = GoplsReady {
            began: false,
            active: 0,
            settled_at: Some(Instant::now() - Duration::from_millis(60)),
        };
        assert!(
            p.settled_no_progress(settle),
            "settle elapsed with no progress → ready"
        );

        let p = GoplsReady {
            began: false,
            active: 0,
            settled_at: None,
        };
        assert!(
            !p.settled_no_progress(settle),
            "no settled_at → not settle-ready"
        );

        let p = GoplsReady {
            began: true,
            active: 1,
            settled_at: Some(Instant::now() - Duration::from_secs(1)),
        };
        assert!(
            !p.settled_no_progress(settle),
            "begin seen ⇒ no no-progress settle"
        );
    }

    #[test]
    fn gopls_ready_via_progress() {
        let mut r = Readiness::Gopls(GoplsReady::default());
        assert!(!r.is_ready());
        r.on_notification("$/progress", &json!({"value":{"kind":"begin"}}));
        assert!(!r.is_ready(), "begun, still active");
        r.on_notification("$/progress", &json!({"value":{"kind":"end"}}));
        assert!(r.is_ready(), "begun-and-ended is ready");
    }

    #[test]
    fn pyright_config_carries_resendable_post_init() {
        let d = tempfile::tempdir().unwrap();
        let cfg = pyright_config(d.path(), None).unwrap();
        let (method, params) = cfg
            .post_init_config
            .expect("python MUST send post-init config");
        assert_eq!(method, "workspace/didChangeConfiguration");
        // The pythonPath key must be present (the venv that respawn re-applies).
        let s = serde_json::to_string(&params).unwrap();
        assert!(
            s.contains("pythonPath"),
            "post-init config must set pythonPath, got {s}"
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
        // LOAD-BEARING: hierarchical documentSymbol support MUST be advertised. With it, rust-analyzer (and
        // basedpyright) return the nested `DocumentSymbol{children}` form so `collect_doc_symbols` recursion
        // surfaces nested methods (e.g. the `hi` trait method); WITHOUT it, servers fall back to the flat
        // `SymbolInformation` form and the recursion is dead code (nested methods never reach the children walk).
        assert_eq!(
            p["capabilities"]["textDocument"]["documentSymbol"]
                ["hierarchicalDocumentSymbolSupport"],
            json!(true)
        );
        assert!(
            cfg.post_init_config.is_none(),
            "Rust sends NO post-init config"
        );
    }

    #[test]
    fn pyright_advertises_hierarchical_document_symbols() {
        let d = tempfile::tempdir().unwrap();
        let cfg = pyright_config(d.path(), None).unwrap();
        let p = (cfg.initialize_params)("file:///repo");
        // LOAD-BEARING (Python side): without `hierarchicalDocumentSymbolSupport`, basedpyright returns the
        // flat `SymbolInformation` form and class methods (e.g. `Greeter.greet`) are dropped from the children
        // walk — the live `document_symbols_extracts_class_and_method_recursively` test only fails-then-passes
        // because this capability is advertised. Locks it so the recursion stays genuinely exercised.
        assert_eq!(
            p["capabilities"]["textDocument"]["documentSymbol"]
                ["hierarchicalDocumentSymbolSupport"],
            json!(true)
        );
    }

    #[test]
    fn detects_repo_pyright_config() {
        let d = std::env::temp_dir().join(format!("lspmcp-pyrcfg-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        assert!(!repo_has_pyright_config(&d)); // none
        std::fs::write(d.join("pyrightconfig.json"), "{}").unwrap();
        assert!(repo_has_pyright_config(&d)); // explicit file
        let _ = std::fs::remove_file(d.join("pyrightconfig.json"));
        std::fs::write(d.join("pyproject.toml"), "[tool.basedpyright]\n").unwrap();
        assert!(repo_has_pyright_config(&d)); // pyproject section
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn detect_typescript_via_tsconfig_only() {
        let d = tempfile::tempdir().unwrap();
        // a tooling-only package.json must NOT trigger TS (no rust/python regression — Opus m2)
        std::fs::write(d.path().join("package.json"), "{}").unwrap();
        assert_eq!(detect(d.path()), Detection::None);
        std::fs::write(d.path().join("tsconfig.json"), "{}").unwrap();
        assert_eq!(detect(d.path()), Detection::Detected(Lang::TypeScript));
    }

    #[test]
    fn ts_config_returns_typescript_language_server_config() {
        let d = tempfile::tempdir().unwrap();
        let cfg = ts_config(d.path()).unwrap();
        assert_eq!(cfg.name, "typescript-language-server");
        assert!(cfg.program_argv.iter().any(|a| a == "--stdio"));
        assert!(cfg.post_init_config.is_none(), "TS has no post-init config");
        assert!(cfg.spawn_env.is_empty());
        // is_project_root: tsconfig.json or package.json
        let d2 = tempfile::tempdir().unwrap();
        assert!(
            !(cfg.is_project_root)(d2.path()),
            "empty dir is not TS root"
        );
        std::fs::write(d2.path().join("package.json"), "{}").unwrap();
        assert!(
            (cfg.is_project_root)(d2.path()),
            "package.json is enough for explicit --lang ts"
        );
        std::fs::write(d2.path().join("tsconfig.json"), "{}").unwrap();
        assert!((cfg.is_project_root)(d2.path()), "tsconfig.json is TS root");
        // readiness is settle-based
        let ready = (cfg.new_readiness)();
        assert!(
            matches!(ready, Readiness::Ts(_)),
            "TS uses Ts(SettleReady) readiness"
        );
    }
}
