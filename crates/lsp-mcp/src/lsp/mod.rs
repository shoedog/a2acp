pub mod codec;

use crate::shape::{self, NavHit};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::BufReader;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Build a `file://` request URI from an absolute path (lsp-types 0.97 has no `Url::from_file_path`).
fn file_uri(p: &Path) -> String {
    format!("file://{}", p.display())
}

pub struct LspSession {
    child: Child,
    stdin: ChildStdin,
    next_id: i64,
    pending: Arc<Mutex<HashMap<i64, Sender<Value>>>>,
    ready: Arc<Mutex<ReadyState>>,
}

#[derive(Default)]
struct ReadyState {
    began: bool,
    active: u32,
}

impl LspSession {
    /// Spawn rust-analyzer rooted at `repo` (CARGO_TARGET_DIR=`target_cache` when given) and run the
    /// LSP initialize handshake. A background thread routes responses by id and tracks `$/progress`.
    pub fn start(repo: &Path, target_cache: Option<&Path>) -> anyhow::Result<Self> {
        let mut cmd = Command::new("rust-analyzer");
        cmd.current_dir(repo)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(tc) = target_cache {
            cmd.env("CARGO_TARGET_DIR", tc);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("failed to spawn rust-analyzer: {e}"))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        let pending: Arc<Mutex<HashMap<i64, Sender<Value>>>> = Arc::new(Mutex::new(HashMap::new()));
        let ready = Arc::new(Mutex::new(ReadyState::default()));
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
                    if let Some(id) = msg.get("id").and_then(|i| i.as_i64()) {
                        if let Some(tx) = pending.lock().unwrap().remove(&id) {
                            let _ = tx.send(msg);
                        }
                    } else if msg.get("method").and_then(|m| m.as_str()) == Some("$/progress") {
                        let mut g = ready.lock().unwrap();
                        match msg["params"]["value"]["kind"].as_str() {
                            Some("begin") => {
                                g.began = true;
                                g.active += 1;
                            }
                            Some("end") => g.active = g.active.saturating_sub(1),
                            _ => {}
                        }
                    }
                }
            });
        }

        let mut s = LspSession {
            child,
            stdin,
            next_id: 0,
            pending,
            ready,
        };
        let root = file_uri(repo);
        s.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": root,
                "capabilities": { "workspace": { "symbol": {} },
                    "experimental": { "serverStatusNotification": true } },
                "workspaceFolders": [{ "uri": root, "name": "root" }],
            }),
            Duration::from_secs(30),
        )?;
        s.notify("initialized", json!({}));
        Ok(s)
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

    /// Block until indexing has begun-and-ended, or `timeout` (best-effort past the bound).
    pub fn wait_ready(&mut self, timeout: Duration) -> anyhow::Result<()> {
        let t0 = Instant::now();
        loop {
            {
                let g = self.ready.lock().unwrap();
                if g.began && g.active == 0 {
                    return Ok(());
                }
            }
            if t0.elapsed() >= timeout {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
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
        let uri = file_uri(file);
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
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for LspSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
