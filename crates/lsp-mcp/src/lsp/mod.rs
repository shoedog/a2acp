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

pub fn should_evict(idle_secs: u64, timeout_secs: u64) -> bool {
    timeout_secs > 0 && idle_secs >= timeout_secs
}

/// The no-progress settle window for the Pyright path: basedpyright emits NO `pyright/*Progress` for a
/// typical analysis (Task-1 spike), so readiness is reached by SETTLING — settings applied + this window
/// elapsed with no progress seen. Independent OR-branch of `Readiness::is_ready`; only the Pyright variant
/// is affected (RustRa's `settled_no_progress` equivalent never fires). Lands fully wired in Task 6.
const PYRIGHT_SETTLE: Duration = Duration::from_millis(800);

/// LOAD-BEARING Pyright readiness branch, evaluated by `wait_ready` (it owns the runtime settle Duration
/// that the pure `Readiness::is_ready` predicate can't carry). Returns true only for a `Pyright` machine
/// that has settled with no progress; always false for `RustRa`.
fn pyright_settled(r: &crate::lang::Readiness) -> bool {
    match r {
        crate::lang::Readiness::Pyright(p) => p.settled_no_progress(PYRIGHT_SETTLE),
        crate::lang::Readiness::RustRa(_) => false,
    }
}

pub struct LspClient {
    child: Arc<Mutex<Option<Child>>>,
    repo: PathBuf,
    cfg: Arc<crate::lang::LangServerConfig>,
    last_activity: Arc<Mutex<Instant>>,
    evicted: Arc<AtomicBool>,
    stdin: ChildStdin,
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
    ) -> anyhow::Result<(Child, ChildStdin, PendingRequests, SharedReady)> {
        let mut cmd = Command::new(&cfg.program_argv[0]);
        cmd.args(&cfg.program_argv[1..])
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
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        let pending: PendingRequests = Arc::new(Mutex::new(HashMap::new()));
        let ready = Arc::new(Mutex::new((cfg.new_readiness)()));
        {
            let pending = pending.clone();
            let ready = ready.clone();
            std::thread::spawn(move || {
                let mut r = BufReader::new(stdout);
                while let Ok(Some(body)) = codec::read_frame(&mut r) {
                    let msg: Value = match serde_json::from_slice(&body) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    // id-routing is language-AGNOSTIC and STAYS here (review: don't drag it into Readiness).
                    if let Some(id) = msg.get("id").and_then(|i| i.as_i64()) {
                        if let Some(tx) = pending.lock().unwrap().remove(&id) {
                            let _ = tx.send(msg);
                        }
                    } else if let Some(method) = msg.get("method").and_then(|m| m.as_str()) {
                        ready
                            .lock()
                            .unwrap()
                            .on_notification(method, &msg["params"]);
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
        *self.child.lock().unwrap() = Some(child);
        self.stdin = stdin;
        self.pending = pending;
        self.ready = ready;
        self.next_id = 0;
        self.readied = false;
        // Re-init BEFORE clearing `evicted`: if the handshake fails the session stays marked evicted so the
        // NEXT call retries respawn rather than driving a half-dead server (review MAJOR: respawn-failure
        // path). `handshake()` now re-sends initialize + initialized + post_init_config (a Python venv/
        // settings survive a respawn). Its initialize request touch()es → the idle clock is fresh before we
        // re-arm the watcher.
        self.handshake()?;
        self.evicted.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn send(&mut self, msg: &Value) -> anyhow::Result<()> {
        codec::write_frame(&mut self.stdin, serde_json::to_vec(msg)?.as_slice())?;
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
    /// the bound). The Pyright no-progress settle is OR'd in here because the settle window is a runtime
    /// Duration the pure `Readiness::is_ready` predicate doesn't carry.
    pub fn wait_ready(&mut self, timeout: Duration) -> anyhow::Result<()> {
        let t0 = Instant::now();
        loop {
            // An in-progress index wait is active use — touch so the watcher can't evict the server
            // mid-index (a slow in-container cold/re-index can exceed the idle timeout otherwise).
            self.touch();
            {
                let g = self.ready.lock().unwrap();
                if g.is_ready() || pyright_settled(&g) {
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
        Ok(v["contents"]["value"]
            .as_str()
            .map(|s| s.to_string())
            .or_else(|| v["contents"].as_str().map(|s| s.to_string())))
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
                if let Some(name) = it["name"].as_str() {
                    let line = it["range"]["start"]["line"].as_u64().unwrap_or(0) as u32 + 1;
                    out.push(NavHit {
                        file: file.to_string_lossy().into_owned(),
                        line,
                        signature: Some(name.to_string()),
                        context: it["detail"].as_str().map(|s| s.to_string()),
                    });
                }
            }
        }
        Ok(out)
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
    use super::should_evict;

    #[test]
    fn should_evict_after_idle_timeout() {
        assert!(should_evict(120, 60));
        assert!(!should_evict(30, 60));
        assert!(!should_evict(120, 0), "timeout 0 disables eviction");
    }
}
