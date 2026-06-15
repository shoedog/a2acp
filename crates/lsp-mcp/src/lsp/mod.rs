pub mod codec;

use crate::shape::{self, NavHit};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type PendingRequests = Arc<Mutex<HashMap<i64, Sender<Value>>>>;
type SharedReady = Arc<Mutex<crate::lang::Readiness>>;
/// The LS stdin writer, shared between the main thread (`request`/`notify`) and the reader thread (which
/// must REPLY to server-initiated requests, e.g. `workspace/configuration`). Locked only for the brief
/// write of a single frame — NEVER held across the reader's blocking `read_frame` (no deadlock; reads come
/// from the separate stdout fd).
type SharedStdin = Arc<Mutex<ChildStdin>>;

/// Classification of one inbound JSON-RPC message from the language server. JSON-RPC overloads `id`:
/// a CLIENT response carries `id` and NO `method`, but a SERVER-INITIATED REQUEST carries BOTH `id` AND
/// `method` (e.g. basedpyright's `workspace/configuration`). Routing on `id` alone (the pre-fix bug)
/// dropped server requests — or, on an `id` collision with a pending client request, mis-delivered the
/// server request AS that client's response, corrupting it (Finding 1). Classify on the (`id`,`method`)
/// pair instead.
#[derive(Debug, PartialEq)]
enum Inbound {
    /// Has `id` AND `method` → a server→client request we must answer (carries the RAW id to echo back
    /// verbatim — a JSON-RPC id may be a number OR a string — plus the method name).
    ServerRequest { id: Value, method: String },
    /// Has `id`, NO `method` → a response to one of our client requests; route to `pending` by i64 id.
    Response { id: i64 },
    /// Has `method`, NO `id` → a notification; hand to the `Readiness` machine.
    Notification { method: String },
    /// Neither a usable id nor a method (or a non-i64 response id we can't route) → ignore.
    Ignore,
}

/// PURE classifier (Finding 1). See `Inbound`. `method`-AND-`id` is checked BEFORE the bare-`id` response
/// path so a server request is never mistaken for a client response.
fn classify(msg: &Value) -> Inbound {
    let has_id = msg.get("id").map(|v| !v.is_null()).unwrap_or(false);
    let method = msg.get("method").and_then(|m| m.as_str());
    match (has_id, method) {
        (true, Some(method)) => Inbound::ServerRequest {
            id: msg["id"].clone(),
            method: method.to_string(),
        },
        (true, None) => match msg.get("id").and_then(|i| i.as_i64()) {
            Some(id) => Inbound::Response { id },
            None => Inbound::Ignore,
        },
        (false, Some(method)) => Inbound::Notification {
            method: method.to_string(),
        },
        (false, None) => Inbound::Ignore,
    }
}

/// PURE builder of the JSON-RPC reply to a server-initiated request (Finding 1). `id` is echoed verbatim.
///
/// - `workspace/configuration`: reply with a `result` ARRAY of length `params.items.len()` (basedpyright
///   expects `result: [<config-per-item>]`). The `python`-section item carries `{ pythonPath }` when known
///   (else `{}`); every other item is `null`. This satisfies basedpyright without hanging it and without
///   touching `pending`.
/// - any OTHER method: a JSON-RPC `-32601` method-not-found error so the server isn't left waiting.
fn build_server_reply(
    id: &Value,
    method: &str,
    params: &Value,
    python_path: Option<&str>,
) -> Value {
    if method == "workspace/configuration" {
        let n = params
            .get("items")
            .and_then(|i| i.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        let items = params.get("items").and_then(|i| i.as_array());
        let result: Vec<Value> = (0..n)
            .map(|i| {
                let section = items
                    .and_then(|a| a.get(i))
                    .and_then(|it| it.get("section"))
                    .and_then(|s| s.as_str())
                    .unwrap_or("");
                if section == "python" || section.starts_with("python") {
                    match python_path {
                        Some(p) => json!({ "pythonPath": p }),
                        None => json!({}),
                    }
                } else {
                    Value::Null
                }
            })
            .collect();
        json!({ "jsonrpc": "2.0", "id": id, "result": result })
    } else {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": format!("method not found: {method}") }
        })
    }
}

/// Extract the `pythonPath` the bridge configured (from `post_init_config`'s
/// `{settings:{python:{pythonPath}}}`) so the reader thread can answer a `workspace/configuration` request
/// with the right interpreter. None for languages (Rust) that send no python config.
fn python_path_from_cfg(cfg: &crate::lang::LangServerConfig) -> Option<String> {
    let (_, params) = cfg.post_init_config.as_ref()?;
    params["settings"]["python"]["pythonPath"]
        .as_str()
        .map(str::to_string)
}

pub fn should_evict(idle_secs: u64, timeout_secs: u64) -> bool {
    timeout_secs > 0 && idle_secs >= timeout_secs
}

/// The no-progress settle window for settle-based language servers: basedpyright/gopls emit NO progress for
/// a typical analysis/load, so readiness is reached by SETTLING — settings/init applied + this window
/// elapsed with no progress seen. Independent OR-branch of `Readiness::is_ready`; RustRa is unaffected.
const PYRIGHT_SETTLE: Duration = Duration::from_millis(1500);

/// LOAD-BEARING settle branch for the no-progress languages (basedpyright + gopls), evaluated by
/// `wait_ready` (it owns the runtime settle Duration the pure `Readiness::is_ready` can't carry). True
/// only for a settle-based machine that has settled with no progress; false for RustRa.
fn settled_no_progress(r: &crate::lang::Readiness) -> bool {
    match r {
        crate::lang::Readiness::Pyright(p) => p.settled_no_progress(PYRIGHT_SETTLE),
        crate::lang::Readiness::Gopls(g) => g.settled_no_progress(PYRIGHT_SETTLE),
        crate::lang::Readiness::RustRa(_) => false,
    }
}

pub struct LspClient {
    child: Arc<Mutex<Option<Child>>>,
    repo: PathBuf,
    cfg: Arc<crate::lang::LangServerConfig>,
    last_activity: Arc<Mutex<Instant>>,
    evicted: Arc<AtomicBool>,
    stdin: SharedStdin,
    next_id: i64,
    pending: PendingRequests,
    ready: SharedReady,
    readied: bool,
}

impl LspClient {
    /// Spawn the configured language server rooted at `repo` (with `cfg.spawn_env`). A background thread
    /// routes responses by id (language-AGNOSTIC, stays here) and delegates NOTIFICATION parsing to the
    /// per-language `Readiness::on_notification` machine.
    fn spawn(
        repo: &Path,
        cfg: &crate::lang::LangServerConfig,
    ) -> anyhow::Result<(Child, SharedStdin, PendingRequests, SharedReady)> {
        // `LangServerConfig` is public/test-constructible and could carry an EMPTY `program_argv`;
        // index `[0]` would PANIC. Validate non-empty and degrade to a normal error (Finding 4).
        let (program, args) = cfg
            .program_argv
            .split_first()
            .ok_or_else(|| anyhow::anyhow!("empty program_argv for {}", cfg.name))?;
        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(repo)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (k, v) in &cfg.spawn_env {
            cmd.env(k, v);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("failed to spawn {}: {e}", cfg.name))?;
        // Share the stdin writer with the reader thread so it can REPLY to server-initiated requests
        // (Finding 1). Locked only for the brief write of one frame, never across the blocking read.
        let stdin: SharedStdin = Arc::new(Mutex::new(child.stdin.take().unwrap()));
        let stdout = child.stdout.take().unwrap();
        // The interpreter the bridge configured — answered back if the server asks for `python` config.
        let python_path = python_path_from_cfg(cfg);

        let pending: PendingRequests = Arc::new(Mutex::new(HashMap::new()));
        let ready = Arc::new(Mutex::new((cfg.new_readiness)()));
        {
            let pending = pending.clone();
            let ready = ready.clone();
            let reply_stdin = stdin.clone();
            std::thread::spawn(move || {
                let mut r = BufReader::new(stdout);
                while let Ok(Some(body)) = codec::read_frame(&mut r) {
                    let msg: Value = match serde_json::from_slice(&body) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    // Classify on the (id, method) PAIR — id-AND-method is a server REQUEST, not a client
                    // response (Finding 1). id-routing is language-AGNOSTIC and STAYS here.
                    match classify(&msg) {
                        Inbound::Response { id } => {
                            if let Some(tx) = pending.lock().unwrap().remove(&id) {
                                let _ = tx.send(msg);
                            }
                        }
                        Inbound::ServerRequest { id, method } => {
                            // Answer it so the server isn't left waiting (e.g. basedpyright's
                            // `workspace/configuration`). NEVER touch `pending` — that would corrupt an
                            // in-flight client request whose id happens to collide with this one.
                            let reply = build_server_reply(
                                &id,
                                &method,
                                &msg["params"],
                                python_path.as_deref(),
                            );
                            if let Ok(bytes) = serde_json::to_vec(&reply) {
                                // Brief lock just to write one frame; released before the next read.
                                if let Ok(mut w) = reply_stdin.lock() {
                                    let _ = codec::write_frame(&mut *w, &bytes);
                                }
                            }
                        }
                        Inbound::Notification { method } => {
                            ready
                                .lock()
                                .unwrap()
                                .on_notification(&method, &msg["params"]);
                        }
                        Inbound::Ignore => {}
                    }
                }
            });
        }

        Ok((child, stdin, pending, ready))
    }

    fn handshake(&mut self) -> anyhow::Result<()> {
        let root = shape::file_uri(&self.repo);
        let params = (self.cfg.initialize_params)(&root);
        self.request("initialize", params, Duration::from_secs(30))?;
        self.notify("initialized", json!({}));
        if let Some((method, params)) = self.cfg.post_init_config.clone() {
            self.notify(&method, params);
            // Stamp the settle-clock origin: settings are applied NOW. The Pyright no-progress settle is
            // timed from `settled_at` (settings-applied), NOT from `wait_ready` entry (Opus H2) — so a
            // begin-without-end server is ready ~settle after settings, never paying the full timeout. This
            // is the LOAD-BEARING readiness path: basedpyright emits no `pyright/*Progress` for typical
            // analyses (Task-1 spike Gate 2), so the settle — not a begin/end cycle — is what makes it ready.
            if let crate::lang::Readiness::Pyright(s) = &mut *self.ready.lock().unwrap() {
                s.settled_at = Some(Instant::now());
            }
        }
        // Gopls has no post_init_config → stamp its settle clock right after `initialized`.
        if let crate::lang::Readiness::Gopls(s) = &mut *self.ready.lock().unwrap() {
            s.settled_at = Some(Instant::now());
        }
        Ok(())
    }

    /// Slice-A/test-compat: start with the Rust config (optional CARGO_TARGET_DIR), matching the old
    /// signature. Existing integration/characterization call sites use `start(&repo, None)`.
    pub fn start(repo: &Path, target_cache: Option<&Path>) -> anyhow::Result<Self> {
        Self::start_with(repo, crate::lang::rust_ra_config(target_cache))
    }

    /// Start any language server from its config. Spawns the server rooted at `repo`, runs the LSP
    /// initialize handshake (+ optional post-init config notification), and arms the idle watcher. A
    /// background thread routes responses by id and tracks readiness via the config's `Readiness` machine.
    pub fn start_with(repo: &Path, cfg: crate::lang::LangServerConfig) -> anyhow::Result<Self> {
        let cfg = Arc::new(cfg);
        let (child, stdin, pending, ready) = Self::spawn(repo, &cfg)?;
        let mut s = LspClient {
            child: Arc::new(Mutex::new(Some(child))),
            repo: repo.to_path_buf(),
            cfg,
            last_activity: Arc::new(Mutex::new(Instant::now())),
            evicted: Arc::new(AtomicBool::new(false)),
            stdin,
            next_id: 0,
            pending,
            ready,
            readied: false,
        };
        s.handshake()?;
        s.start_idle_watcher();
        Ok(s)
    }

    /// Doc-hidden test accessors: drive `respawn` / read `evicted` / read the idle clock / swap the config
    /// from the external `tests/` crate WITHOUT widening the real API (the fields stay private). Used by the
    /// crown-jewel `respawn_failure_leaves_evicted_true` + request-touch idle-race tests in characterization.
    #[doc(hidden)]
    pub fn respawn_for_test(&mut self) -> anyhow::Result<()> {
        self.respawn()
    }

    #[doc(hidden)]
    pub fn is_evicted_for_test(&self) -> bool {
        self.evicted.load(Ordering::SeqCst)
    }

    #[doc(hidden)]
    pub fn last_activity_for_test(&self) -> std::time::Instant {
        *self.last_activity.lock().unwrap()
    }

    #[doc(hidden)]
    pub fn set_cfg_for_test(&mut self, cfg: crate::lang::LangServerConfig) {
        self.cfg = std::sync::Arc::new(cfg);
    }

    /// True iff a child-process handle is currently held. Used by the respawn-failure tests to assert a
    /// failed respawn does NOT leak the newly-spawned child (Finding 2): the handle must be cleared (the
    /// orphan reaped), not left installed for the next respawn to overwrite.
    #[doc(hidden)]
    pub fn child_present_for_test(&self) -> bool {
        self.child.lock().unwrap().is_some()
    }

    fn do_evict(child: &Arc<Mutex<Option<Child>>>, evicted: &Arc<AtomicBool>) {
        if let Some(mut c) = child.lock().unwrap().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        evicted.store(true, Ordering::SeqCst);
    }

    pub fn touch(&self) {
        *self.last_activity.lock().unwrap() = Instant::now();
    }

    pub fn evict(&mut self) {
        Self::do_evict(&self.child, &self.evicted);
        self.readied = false;
    }

    fn start_idle_watcher(&self) {
        let timeout = std::env::var("LSP_MCP_IDLE_EVICT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(60);
        if timeout == 0 {
            return;
        }

        let child = self.child.clone();
        let last_activity = self.last_activity.clone();
        let evicted = self.evicted.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            let idle = last_activity.lock().unwrap().elapsed().as_secs();
            if !evicted.load(Ordering::SeqCst) && should_evict(idle, timeout) {
                Self::do_evict(&child, &evicted);
            }
        });
    }

    fn respawn(&mut self) -> anyhow::Result<()> {
        let (child, stdin, pending, ready) = Self::spawn(&self.repo, &self.cfg)?;
        // Wire the new I/O into `self` so `handshake()` (which uses self.stdin/pending/ready) can run, but
        // hold the new CHILD in a local until the handshake SUCCEEDS. If it fails we kill+wait this child
        // here — installing it into `self.child` first (the old behavior) leaked it: `evicted` stayed true,
        // the orphan was never reaped, and the next respawn overwrote the handle (Finding 2). `self.child`
        // is already None here (respawn runs only after `evict`/`do_evict` took the old child).
        self.stdin = stdin;
        self.pending = pending;
        self.ready = ready;
        self.next_id = 0;
        self.readied = false;
        let mut child = child;
        // Re-init BEFORE clearing `evicted`: if the handshake fails the session stays marked evicted so the
        // NEXT call retries respawn rather than driving a half-dead server (review MAJOR: respawn-failure
        // path). `handshake()` now re-sends initialize + initialized + post_init_config (a Python venv/
        // settings survive a respawn). Its initialize request touch()es → the idle clock is fresh before we
        // re-arm the watcher.
        if let Err(e) = self.handshake() {
            // Reap the just-spawned child so a failed respawn can't detach the process (Finding 2). Leave
            // `self.child` as None and `evicted` true so the next call cleanly retries respawn.
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        // Handshake OK → commit the new child and clear `evicted`.
        *self.child.lock().unwrap() = Some(child);
        self.evicted.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn send(&mut self, msg: &Value) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec(msg)?;
        // Brief lock on the shared stdin writer (shared with the reader thread for server-request replies,
        // Finding 1). `recv_timeout` blocks AFTER this returns and the guard is dropped → no deadlock.
        let mut w = self
            .stdin
            .lock()
            .map_err(|_| anyhow::anyhow!("LSP stdin lock poisoned"))?;
        codec::write_frame(&mut *w, bytes.as_slice())?;
        Ok(())
    }

    fn notify(&mut self, method: &str, params: Value) {
        let _ = self.send(&json!({"jsonrpc":"2.0","method":method,"params":params}));
    }

    /// Send a request and block for its response (correlated by id).
    pub fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> anyhow::Result<Value> {
        // Any LSP request is activity → reset the idle clock so the watcher never evicts RA mid-use (or
        // mid-handshake during respawn). The review's MAJOR "idle-boundary race" surfaced in-container:
        // a respawn re-armed `evicted=false` against a STALE last_activity and was re-evicted within 1s,
        // breaking the next query with a Broken pipe. Touching here keeps every active path self-sustaining.
        self.touch();
        self.next_id += 1;
        let id = self.next_id;
        let (tx, rx): (Sender<Value>, Receiver<Value>) = channel();
        self.pending.lock().unwrap().insert(id, tx);
        self.send(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))?;
        let msg = rx
            .recv_timeout(timeout)
            .map_err(|_| anyhow::anyhow!("LSP request `{method}` timed out"))?;
        if let Some(e) = msg.get("error") {
            anyhow::bail!("LSP error on `{method}`: {e}");
        }
        Ok(msg.get("result").cloned().unwrap_or(Value::Null))
    }

    /// Block until the server reports ready (per its `Readiness` machine), or `timeout` (best-effort past
    /// the bound). The no-progress settle for the settle-based servers (Pyright/Gopls) is OR'd in here via
    /// `settled_no_progress` because the settle window is a runtime Duration the pure `Readiness::is_ready`
    /// predicate doesn't carry.
    pub fn wait_ready(&mut self, timeout: Duration) -> anyhow::Result<()> {
        let t0 = Instant::now();
        loop {
            // An in-progress index wait is active use — touch so the watcher can't evict the server
            // mid-index (a slow in-container cold/re-index can exceed the idle timeout otherwise).
            self.touch();
            {
                let g = self.ready.lock().unwrap();
                if g.is_ready() || settled_no_progress(&g) {
                    return Ok(());
                }
            }
            if t0.elapsed() >= timeout {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Lazily ensure the index is ready — waits only on the first call (idempotent after).
    pub fn ensure_ready(&mut self, timeout: std::time::Duration) -> anyhow::Result<()> {
        if self.evicted.load(Ordering::SeqCst) {
            self.respawn()?;
        }
        if !self.readied {
            self.wait_ready(timeout)?;
            self.readied = true;
        }
        Ok(())
    }

    fn locations_to_hits(v: &Value) -> Vec<NavHit> {
        let arr = match v {
            Value::Array(a) => a.clone(),
            Value::Null => vec![],
            other => vec![other.clone()],
        };
        arr.iter()
            .filter_map(|it| {
                // `Location` or `LocationLink` (targetUri/targetRange).
                let loc_val = if it.get("targetUri").is_some() {
                    json!({"uri": it["targetUri"], "range": it["targetRange"]})
                } else {
                    it.clone()
                };
                serde_json::from_value::<lsp_types::Location>(loc_val)
                    .ok()
                    .map(|l| NavHit::from_location(&l, None))
            })
            .collect()
    }

    pub fn workspace_symbol(&mut self, query: &str) -> anyhow::Result<Vec<NavHit>> {
        let res = self.request(
            "workspace/symbol",
            json!({ "query": query }),
            Duration::from_secs(20),
        )?;
        let mut out = Vec::new();
        if let Some(arr) = res.as_array() {
            for it in arr {
                if let Ok(loc) =
                    serde_json::from_value::<lsp_types::Location>(it["location"].clone())
                {
                    out.push(NavHit::from_location(
                        &loc,
                        it["name"].as_str().map(|s| s.to_string()),
                    ));
                }
            }
        }
        Ok(out)
    }

    /// Resolve a symbol name → (uri string, position value) via workspace/symbol (first hit).
    fn resolve_pos(&mut self, name: &str) -> anyhow::Result<(String, Value)> {
        let res = self.request(
            "workspace/symbol",
            json!({ "query": name }),
            Duration::from_secs(20),
        )?;
        let first = res
            .as_array()
            .and_then(|a| a.first())
            .ok_or_else(|| anyhow::anyhow!("symbol `{name}` not found"))?;
        let uri = first["location"]["uri"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("symbol `{name}` location has no uri"))?
            .to_string();
        let pos = first["location"]["range"]["start"].clone();
        Ok((uri, pos))
    }

    fn positional(&mut self, method: &str, name: &str) -> anyhow::Result<Value> {
        let (uri, pos) = self.resolve_pos(name)?;
        self.request(
            method,
            json!({ "textDocument": { "uri": uri }, "position": pos }),
            Duration::from_secs(20),
        )
    }

    pub fn definition(&mut self, name: &str) -> anyhow::Result<Vec<NavHit>> {
        Ok(Self::locations_to_hits(
            &self.positional("textDocument/definition", name)?,
        ))
    }

    pub fn references(&mut self, name: &str, include_decl: bool) -> anyhow::Result<Vec<NavHit>> {
        let (uri, pos) = self.resolve_pos(name)?;
        let v = self.request(
            "textDocument/references",
            json!({
                "textDocument": { "uri": uri }, "position": pos,
                "context": { "includeDeclaration": include_decl },
            }),
            Duration::from_secs(30),
        )?;
        Ok(Self::locations_to_hits(&v))
    }

    pub fn implementations(&mut self, name: &str) -> anyhow::Result<Vec<NavHit>> {
        Ok(Self::locations_to_hits(
            &self.positional("textDocument/implementation", name)?,
        ))
    }

    pub fn hover(&mut self, name: &str) -> anyhow::Result<Option<String>> {
        let v = self.positional("textDocument/hover", name)?;
        // MarkupContent { value } | a bare MarkedString string | MarkedString[] (array of strings/objects).
        let s = v["contents"]["value"]
            .as_str()
            .map(str::to_string)
            .or_else(|| v["contents"].as_str().map(str::to_string))
            .or_else(|| {
                v["contents"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|e| {
                                e.as_str()
                                    .map(str::to_string)
                                    .or_else(|| e["value"].as_str().map(str::to_string))
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .filter(|s| !s.is_empty())
            });
        Ok(s)
    }

    pub fn document_symbols(&mut self, file: &Path) -> anyhow::Result<Vec<NavHit>> {
        let uri = shape::file_uri(file);
        let v = self.request(
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": uri } }),
            Duration::from_secs(20),
        )?;
        let mut out = Vec::new();
        if let Some(arr) = v.as_array() {
            for it in arr {
                Self::collect_doc_symbols(it, file, &mut out);
            }
        }
        Ok(out)
    }

    /// Recursively flatten a DocumentSymbol tree (`children`) into NavHits. Also handles the flat
    /// SymbolInformation form (no `children`). Required so Python class methods aren't dropped (spec §1):
    /// with `hierarchicalDocumentSymbolSupport` advertised, both basedpyright and rust-analyzer return the
    /// nested `DocumentSymbol{children}` form, so a flat top-level parse drops nested methods (e.g. `greet`
    /// under `Greeter`, `hi` under the `Greet` trait). The walk surfaces them additively.
    fn collect_doc_symbols(it: &Value, file: &Path, out: &mut Vec<NavHit>) {
        if let Some(name) = it["name"].as_str() {
            // DocumentSymbol uses `range`; SymbolInformation uses `location.range`.
            let start = if it.get("range").is_some() {
                &it["range"]["start"]
            } else {
                &it["location"]["range"]["start"]
            };
            let line = start["line"].as_u64().unwrap_or(0) as u32 + 1;
            out.push(NavHit {
                file: file.to_string_lossy().into_owned(),
                line,
                signature: Some(name.to_string()),
                context: it["detail"].as_str().map(|s| s.to_string()),
            });
        }
        if let Some(children) = it["children"].as_array() {
            for c in children {
                Self::collect_doc_symbols(c, file, out);
            }
        }
    }

    pub fn call_hierarchy(&mut self, name: &str, incoming: bool) -> anyhow::Result<Vec<NavHit>> {
        let (uri, pos) = self.resolve_pos(name)?;
        let prep = self.request(
            "textDocument/prepareCallHierarchy",
            json!({ "textDocument": { "uri": uri }, "position": pos }),
            Duration::from_secs(20),
        )?;
        let item = prep
            .as_array()
            .and_then(|a| a.first())
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no call-hierarchy item for `{name}`"))?;
        let method = if incoming {
            "callHierarchy/incomingCalls"
        } else {
            "callHierarchy/outgoingCalls"
        };
        let v = self.request(method, json!({ "item": item }), Duration::from_secs(30))?;
        let key = if incoming { "from" } else { "to" };
        let mut out = Vec::new();
        if let Some(arr) = v.as_array() {
            for it in arr {
                if let Ok(node) =
                    serde_json::from_value::<lsp_types::CallHierarchyItem>(it[key].clone())
                {
                    out.push(NavHit {
                        file: shape::file_path_from_uri(&node.uri).unwrap_or_default(),
                        line: node.range.start.line + 1,
                        signature: Some(node.name),
                        context: node.detail,
                    });
                }
            }
        }
        Ok(out)
    }

    pub fn shutdown(&mut self) {
        let _ = self.request("shutdown", Value::Null, Duration::from_secs(5));
        self.notify("exit", Value::Null);
        Self::do_evict(&self.child, &self.evicted);
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        Self::do_evict(&self.child, &self.evicted);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::{Readiness, RustReady};

    #[test]
    fn should_evict_after_idle_timeout() {
        assert!(should_evict(120, 60));
        assert!(!should_evict(30, 60));
        assert!(!should_evict(120, 0), "timeout 0 disables eviction");
    }

    // -----------------------------------------------------------------------
    // Finding 1: server-request classification + reply shape (pure)
    // -----------------------------------------------------------------------

    #[test]
    fn classify_distinguishes_server_request_from_response() {
        // id + method → SERVER REQUEST (the pre-fix bug routed this as a response by id).
        let server_req =
            json!({"jsonrpc":"2.0","id":1,"method":"workspace/configuration","params":{}});
        assert_eq!(
            classify(&server_req),
            Inbound::ServerRequest {
                id: json!(1),
                method: "workspace/configuration".to_string()
            }
        );
        // id, NO method → client response.
        let resp = json!({"jsonrpc":"2.0","id":1,"result":[]});
        assert_eq!(classify(&resp), Inbound::Response { id: 1 });
        // method, NO id → notification.
        let notif = json!({"jsonrpc":"2.0","method":"$/progress","params":{}});
        assert_eq!(
            classify(&notif),
            Inbound::Notification {
                method: "$/progress".to_string()
            }
        );
        // A server request with a STRING id is preserved verbatim (echoed back as-is).
        let str_id = json!({"jsonrpc":"2.0","id":"abc","method":"window/showMessageRequest"});
        assert_eq!(
            classify(&str_id),
            Inbound::ServerRequest {
                id: json!("abc"),
                method: "window/showMessageRequest".to_string()
            }
        );
        // Neither id nor method → ignore. A null id with no method → ignore.
        assert_eq!(classify(&json!({"jsonrpc":"2.0"})), Inbound::Ignore);
    }

    #[test]
    fn workspace_configuration_reply_is_array_per_item_with_pythonpath() {
        let params = json!({"items":[{"section":"python"},{"section":"python.analysis"},{"section":"other"}]});
        let reply = build_server_reply(
            &json!(7),
            "workspace/configuration",
            &params,
            Some("/venv/bin/python"),
        );
        assert_eq!(reply["jsonrpc"], json!("2.0"));
        assert_eq!(reply["id"], json!(7), "id echoed verbatim");
        let result = reply["result"].as_array().expect("result is an array");
        assert_eq!(result.len(), 3, "one entry per requested item");
        assert_eq!(
            result[0],
            json!({ "pythonPath": "/venv/bin/python" }),
            "python section carries pythonPath"
        );
        assert_eq!(
            result[1],
            json!({ "pythonPath": "/venv/bin/python" }),
            "python.* section carries pythonPath"
        );
        assert_eq!(result[2], Value::Null, "non-python section is null");
    }

    #[test]
    fn workspace_configuration_reply_handles_no_pythonpath_and_empty_items() {
        // No pythonPath known → python item is `{}` (still an object, never omitted).
        let params = json!({"items":[{"section":"python"}]});
        let reply = build_server_reply(&json!(1), "workspace/configuration", &params, None);
        assert_eq!(reply["result"][0], json!({}));
        // No items → empty array (length matches), not an error.
        let reply2 = build_server_reply(&json!(2), "workspace/configuration", &json!({}), None);
        assert_eq!(reply2["result"], json!([]));
    }

    #[test]
    fn other_server_request_gets_method_not_found_error() {
        let reply = build_server_reply(
            &json!("xyz"),
            "window/workDoneProgress/create",
            &json!({}),
            None,
        );
        assert_eq!(reply["id"], json!("xyz"));
        assert_eq!(reply["error"]["code"], json!(-32601));
        assert!(reply.get("result").is_none(), "error reply has no result");
    }

    #[test]
    fn python_path_from_cfg_extracts_configured_interpreter() {
        let d = tempfile::tempdir().unwrap();
        let cfg = crate::lang::pyright_config(d.path(), None).unwrap();
        // Rust sends no post-init config → None.
        assert_eq!(
            python_path_from_cfg(&crate::lang::rust_ra_config(None)),
            None
        );
        // Python's post-init config carries pythonPath → extracted.
        let p = python_path_from_cfg(&cfg);
        assert!(
            p.is_some(),
            "python config must surface a pythonPath for the configuration reply"
        );
    }

    /// Finding 4 (LOW): a `LangServerConfig` is public/test-constructible and may carry an EMPTY
    /// `program_argv`. `spawn` must turn that into a normal `Err` (validated non-empty), NOT panic on
    /// `program_argv[0]` indexing. Assert an empty argv yields Err and does not panic.
    #[test]
    fn spawn_empty_program_argv_errors_not_panics() {
        let cfg = crate::lang::LangServerConfig {
            name: "empty-argv-lsp",
            program_argv: vec![],
            spawn_env: vec![],
            is_project_root: Box::new(|_| true),
            initialize_params: Box::new(|root| json!({ "rootUri": root })),
            post_init_config: None,
            new_readiness: Box::new(|| Readiness::RustRa(RustReady::default())),
        };
        let dir = std::env::temp_dir();
        let res = LspClient::spawn(&dir, &cfg);
        assert!(
            res.is_err(),
            "empty program_argv must yield Err (not panic on [0] indexing)"
        );
        let msg = res.err().unwrap().to_string();
        assert!(
            msg.contains("program_argv"),
            "error should mention the empty program_argv, got: {msg}"
        );
    }
}
