# LSP-over-MCP semantic nav (L3 Slice A) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a host-side `lsp-mcp` shim that wraps `rust-analyzer` and exposes 7 type-resolved nav tools over MCP, wire it to the bridge's host-side claude+codex reviewers (beside prism), and ship the paired `lsp-nav` skill — per `docs/superpowers/specs/2026-06-13-lsp-mcp-nav-design.md`.

**Architecture:** A new binary crate `crates/lsp-mcp` in the a2a-bridge workspace. It speaks MCP over stdio to the agent (hand-rolled JSON-RPC, mirroring prism's `src/mcp/` module layout) and LSP over a child `rust-analyzer`'s stdio (hand-rolled Content-Length framing + a background reader thread; `lsp-types` for payloads). One `rust-analyzer` per MCP session, held warm; `CARGO_TARGET_DIR` set to a per-repo shared cache (keyed by `git origin.url`). It is name-addressed: tools accept symbol names and resolve them to positions via `workspace/symbol`. Wired purely via the existing `[[agents.mcp]]` config seam (one small `bridge-core::mcp` change for the kiro skill `resources` field). The `lsp-nav` skill lives in the already-bootstrapped `~/knowledge-ref/skills/` library.

**Tech Stack:** Rust (edition 2021, toolchain 1.94.0), `lsp-types`, `serde`/`serde_json`, `clap`, `anyhow`, `sha2`-free FNV hashing (no new hash dep). Sync design with one background LSP reader thread (no tokio), matching prism's lightweight style.

---

## File structure

```
crates/lsp-mcp/
  Cargo.toml                 # new workspace member; bin "lsp-mcp"
  src/
    main.rs                  # clap CLI (--repo, --lang, --target-cache) → run()
    lib.rs                   # module wiring + run()
    mcp/
      mod.rs                 # run loop: read agent stdin → dispatch → write stdout
      transport.rs           # stdio JSON-RPC framing + lifecycle (initialize/initialized gating)
      registry.rs            # the 7 tool schemas (tools/list) + dispatch table
      error.rs               # JSON-RPC error codes (-32600/-32601/-32602/-32603)
    lsp/
      mod.rs                 # LspSession: spawn rust-analyzer, handshake, readiness, request()
      codec.rs               # Content-Length read/write framing (pure, testable)
    tools.rs                 # the 7 tools: MCP args → LSP request → NavHit shaping
    shape.rs                 # NavHit + result shaping (pure, testable)
    cache_key.rs             # per-repo CARGO_TARGET_DIR derivation (pure, testable)
  tests/
    fixtures/sample/         # tiny cargo crate with known symbols for integration tests
    integration.rs           # end-to-end against a real rust-analyzer (guarded)

crates/bridge-core/src/mcp.rs   # MODIFY: render_kiro_agent_config gains `resources` skill:// field
examples/a2a-bridge.containerized.toml   # MODIFY: [[agents.mcp]] lsp for claude+codex
examples/a2a-bridge.containerized.podman.toml  # MODIFY: mirror
prompts/review-implement.md     # MODIFY: inline lsp-nav pointer
~/knowledge-ref/skills/lsp-nav/  # NEW skill (external library)
```

---

## Task 1: Scaffold the `lsp-mcp` crate

**Files:**
- Create: `crates/lsp-mcp/Cargo.toml`, `crates/lsp-mcp/src/main.rs`, `crates/lsp-mcp/src/lib.rs`
- Modify: `Cargo.toml` (workspace already globs `crates/*` — no edit needed; verify)

- [ ] **Step 1: Write `Cargo.toml`**

```toml
[package]
name = "lsp-mcp"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "lsp-mcp"
path = "src/main.rs"

[dependencies]
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
lsp-types = "0.97"
anyhow = "1"
```

- [ ] **Step 2: Write `src/lib.rs` with the CLI struct and a stub `run`**

```rust
//! lsp-mcp: an LSP-over-MCP shim. Wraps a language server (rust-analyzer in Slice A) and exposes
//! type-resolved navigation as MCP tools. See docs/superpowers/specs/2026-06-13-lsp-mcp-nav-design.md.
use std::path::PathBuf;

pub mod cache_key;
pub mod shape;

#[derive(clap::Parser, Debug)]
#[command(name = "lsp-mcp")]
pub struct Cli {
    /// Repo root the language server is rooted at (the session cwd).
    #[arg(long)]
    pub repo: PathBuf,
    /// Language server to drive. Slice A supports only "rust".
    #[arg(long, default_value = "rust")]
    pub lang: String,
    /// Base dir for the per-repo shared build cache (CARGO_TARGET_DIR). Optional.
    #[arg(long)]
    pub target_cache: Option<PathBuf>,
}

pub fn run(cli: Cli) -> anyhow::Result<()> {
    anyhow::ensure!(cli.lang == "rust", "Slice A supports only --lang rust (got {:?})", cli.lang);
    // mcp::run(cli) — implemented in later tasks
    let _ = cli;
    Ok(())
}
```

- [ ] **Step 3: Write `src/main.rs`**

```rust
fn main() -> anyhow::Result<()> {
    let cli = <lsp_mcp::Cli as clap::Parser>::parse();
    lsp_mcp::run(cli)
}
```

- [ ] **Step 4: Add empty module files so later tasks compile incrementally**

Create `src/cache_key.rs` and `src/shape.rs` each containing `// filled in a later task`. (lib.rs already declares them.)

- [ ] **Step 5: Verify it builds and the binary parses args**

Run: `cargo build -p lsp-mcp && cargo run -p lsp-mcp -- --repo . --lang rust`
Expected: builds; exits 0 (stub run).
Run: `cargo run -p lsp-mcp -- --repo . --lang go`
Expected: exits non-zero with "Slice A supports only --lang rust".

- [ ] **Step 6: Commit**

```bash
git add crates/lsp-mcp Cargo.lock
git commit -m "feat(lsp-mcp): scaffold crate + CLI (--repo/--lang/--target-cache)"
```

---

## Task 2: Per-repo cache-key derivation (pure)

**Files:**
- Modify: `crates/lsp-mcp/src/cache_key.rs`
- Test: same file (`#[cfg(test)]`)

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn same_origin_same_dir_regardless_of_clone_path() {
        let base = Path::new("/cache");
        let a = cache_dir(base, Path::new("/clones/A"), Some("git@github.com:me/repo.git"));
        let b = cache_dir(base, Path::new("/clones/B"), Some("git@github.com:me/repo.git"));
        assert_eq!(a, b, "clones of the same origin must share one target dir");
        assert!(a.starts_with("/cache/ra-"));
    }

    #[test]
    fn different_origin_different_dir() {
        let base = Path::new("/cache");
        let a = cache_dir(base, Path::new("/x"), Some("git@github.com:me/one.git"));
        let b = cache_dir(base, Path::new("/x"), Some("git@github.com:me/two.git"));
        assert_ne!(a, b);
    }

    #[test]
    fn blank_or_missing_origin_falls_back_to_path() {
        let base = Path::new("/cache");
        let by_path = cache_dir(base, Path::new("/clones/A"), None);
        let by_blank = cache_dir(base, Path::new("/clones/A"), Some("   "));
        assert_eq!(by_path, by_blank, "blank origin must behave like no origin");
        let other = cache_dir(base, Path::new("/clones/B"), None);
        assert_ne!(by_path, other, "path-keyed dirs differ by path");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p lsp-mcp cache_key`
Expected: FAIL — `cache_dir` not found.

- [ ] **Step 3: Implement**

```rust
//! Per-repo CARGO_TARGET_DIR derivation. The key is a *reuse boundary*, not a re-index trigger:
//! clones of the same repo (same origin) share one warm build cache → ~0.7s usable index vs ~9s cold.
use std::path::{Path, PathBuf};

/// Per-repo cache dir under `base`, keyed by `origin_url` (git remote.origin.url) when present and
/// non-blank, else by `repo_root`'s path. Dir name is FS-safe hex.
pub fn cache_dir(base: &Path, repo_root: &Path, origin_url: Option<&str>) -> PathBuf {
    let key = match origin_url {
        Some(u) if !u.trim().is_empty() => u.trim().to_string(),
        _ => repo_root.to_string_lossy().into_owned(),
    };
    base.join(format!("ra-{}", fnv1a_hex(&key)))
}

fn fnv1a_hex(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p lsp-mcp cache_key`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/lsp-mcp/src/cache_key.rs
git commit -m "feat(lsp-mcp): per-repo CARGO_TARGET_DIR cache key (origin.url, path fallback)"
```

---

## Task 3: NavHit + result shaping (pure)

**Files:**
- Modify: `crates/lsp-mcp/src/shape.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn location_shapes_to_one_based_line_with_path() {
        // lsp-types Location uses 0-based lines; NavHit must present 1-based.
        let loc = lsp_types::Location {
            uri: lsp_types::Url::parse("file:///repo/src/foo.rs").unwrap(),
            range: lsp_types::Range {
                start: lsp_types::Position { line: 41, character: 4 },
                end: lsp_types::Position { line: 41, character: 10 },
            },
        };
        let hit = NavHit::from_location(&loc, Some("fn build_cfg".into()));
        assert_eq!(hit.file, "/repo/src/foo.rs");
        assert_eq!(hit.line, 42, "0-based 41 → 1-based 42");
        assert_eq!(hit.signature.as_deref(), Some("fn build_cfg"));
    }

    #[test]
    fn renders_compact_json_array() {
        let hits = vec![NavHit { file: "/a.rs".into(), line: 1, signature: None, context: None }];
        let v = render_hits(&hits);
        assert_eq!(v["count"], 1);
        assert_eq!(v["hits"][0]["file"], "/a.rs");
        assert_eq!(v["hits"][0]["line"], 1);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p lsp-mcp shape`
Expected: FAIL — `NavHit` not found.

- [ ] **Step 3: Implement**

```rust
//! Compact, agent-friendly result shaping. Never hand an agent a raw LSP Location blob.
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct NavHit {
    /// Filesystem path (from the file:// URI).
    pub file: String,
    /// 1-based line (LSP is 0-based).
    pub line: u32,
    /// The enclosing item's signature/name, when known.
    pub signature: Option<String>,
    /// Optional short surrounding snippet.
    pub context: Option<String>,
}

impl NavHit {
    pub fn from_location(loc: &lsp_types::Location, signature: Option<String>) -> Self {
        NavHit {
            file: loc.uri.to_file_path().map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| loc.uri.to_string()),
            line: loc.range.start.line + 1,
            signature,
            context: None,
        }
    }
}

/// Shape a list of hits into the JSON an MCP `tools/call` returns (content text is this, stringified).
pub fn render_hits(hits: &[NavHit]) -> Value {
    json!({
        "count": hits.len(),
        "hits": hits.iter().map(|h| json!({
            "file": h.file, "line": h.line,
            "signature": h.signature, "context": h.context,
        })).collect::<Vec<_>>(),
    })
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p lsp-mcp shape`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/lsp-mcp/src/shape.rs
git commit -m "feat(lsp-mcp): NavHit + compact result shaping (1-based lines)"
```

---

## Task 4: LSP framing codec (pure)

**Files:**
- Create: `crates/lsp-mcp/src/lsp/mod.rs` (module decl only this task), `crates/lsp-mcp/src/lsp/codec.rs`
- Modify: `crates/lsp-mcp/src/lib.rs` (add `pub mod lsp;`)

- [ ] **Step 1: Declare the module**

In `lib.rs` add `pub mod lsp;`. In `src/lsp/mod.rs` add `pub mod codec;`.

- [ ] **Step 2: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn writes_content_length_frame() {
        let mut buf = Vec::new();
        write_frame(&mut buf, br#"{"jsonrpc":"2.0"}"#).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s, "Content-Length: 16\r\n\r\n{\"jsonrpc\":\"2.0\"}");
    }

    #[test]
    fn reads_a_frame_back() {
        let wire = "Content-Length: 16\r\n\r\n{\"jsonrpc\":\"2.0\"}";
        let mut r = Cursor::new(wire.as_bytes());
        let body = read_frame(&mut r).unwrap().unwrap();
        assert_eq!(body, br#"{"jsonrpc":"2.0"}"#);
    }

    #[test]
    fn read_frame_eof_returns_none() {
        let mut r = Cursor::new(&b""[..]);
        assert!(read_frame(&mut r).unwrap().is_none());
    }
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p lsp-mcp codec`
Expected: FAIL — `write_frame`/`read_frame` not found.

- [ ] **Step 4: Implement**

```rust
//! LSP/MCP wire framing: `Content-Length: N\r\n\r\n<body>`. Shared by both the MCP (agent) side and
//! the LSP (rust-analyzer) side.
use std::io::{self, BufRead, Read, Write};

pub fn write_frame<W: Write>(w: &mut W, body: &[u8]) -> io::Result<()> {
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(body)?;
    w.flush()
}

/// Read one frame. Returns `Ok(None)` on clean EOF before any header.
pub fn read_frame<R: BufRead>(r: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut len: Option<usize> = None;
    let mut saw_any = false;
    loop {
        let mut line = String::new();
        let n = r.read_line(&mut line)?;
        if n == 0 {
            return if saw_any {
                Err(io::Error::new(io::ErrorKind::UnexpectedEof, "eof mid-header"))
            } else {
                Ok(None)
            };
        }
        saw_any = true;
        let t = line.trim_end_matches(['\r', '\n']);
        if t.is_empty() {
            break; // end of headers
        }
        if let Some(v) = t.strip_prefix("Content-Length:") {
            len = Some(v.trim().parse().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "bad Content-Length")
            })?);
        }
    }
    let len = len.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no Content-Length"))?;
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    Ok(Some(body))
}
```

- [ ] **Step 5: Run to verify pass**

Run: `cargo test -p lsp-mcp codec`
Expected: PASS (3 tests).

- [ ] **Step 6: Commit**

```bash
git add crates/lsp-mcp/src/lsp/ crates/lsp-mcp/src/lib.rs
git commit -m "feat(lsp-mcp): Content-Length frame codec (shared MCP+LSP wire)"
```

---

## Task 5: LspSession — spawn rust-analyzer, handshake, readiness, request()

**Files:**
- Modify: `crates/lsp-mcp/src/lsp/mod.rs`
- Test: `crates/lsp-mcp/tests/integration.rs` (+ fixture crate)

**Note on readiness:** rust-analyzer answers `workspace/symbol` with *empty* results until indexing finishes, so the session must gate the first query. Primary signal: advertise `experimental.serverStatusNotification` and wait for a `serverStatus`/`$/progress`-end with quiescence. Robust fallback (always implemented, so this is not a placeholder): track `$/progress` begin/end tokens and consider ready once at least one progress has begun-and-ended, OR after a `ready_timeout` (default 30s) — then answer best-effort.

- [ ] **Step 1: Create the fixture crate**

Create `crates/lsp-mcp/tests/fixtures/sample/Cargo.toml`:
```toml
[package]
name = "sample"
version = "0.0.0"
edition = "2021"
[lib]
path = "lib.rs"
```
Create `crates/lsp-mcp/tests/fixtures/sample/lib.rs`:
```rust
pub fn add(a: i32, b: i32) -> i32 { a + b }
pub fn caller() -> i32 { add(1, 2) }
pub trait Greet { fn hi(&self) -> &'static str; }
pub struct En;
impl Greet for En { fn hi(&self) -> &'static str { "hi" } }
```
Create `crates/lsp-mcp/tests/fixtures/sample/Cargo.lock` by running `cargo generate-lockfile --manifest-path crates/lsp-mcp/tests/fixtures/sample/Cargo.toml` (committed so `--locked` verify works).

- [ ] **Step 2: Write the failing integration test**

```rust
// crates/lsp-mcp/tests/integration.rs
use std::path::PathBuf;

fn sample_repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample")
}

// Guarded: only runs where rust-analyzer is on PATH (host gate has it; hermetic container verify skips).
fn ra_available() -> bool {
    std::process::Command::new("rust-analyzer").arg("--version").output().is_ok()
}

#[test]
fn session_reaches_ready_and_resolves_a_symbol() {
    if !ra_available() { eprintln!("skip: rust-analyzer not on PATH"); return; }
    let mut s = lsp_mcp::lsp::LspSession::start(&sample_repo(), None).expect("start");
    s.wait_ready(std::time::Duration::from_secs(60)).expect("ready");
    let hits = s.workspace_symbol("add").expect("query");
    assert!(hits.iter().any(|h| h.signature.as_deref().unwrap_or("").contains("add")),
            "workspace/symbol must find `add`, got {hits:?}");
    s.shutdown();
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p lsp-mcp --test integration`
Expected: FAIL — `LspSession` not found.

- [ ] **Step 4: Implement `LspSession`**

```rust
//! One rust-analyzer child per MCP session, held warm. Sync design: a background thread reads the
//! child's stdout and routes responses (by id) to waiting callers and progress to a readiness flag.
pub mod codec;

use crate::shape::NavHit;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::BufReader;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct LspSession {
    child: Child,
    stdin: ChildStdin,
    next_id: i64,
    pending: Arc<Mutex<HashMap<i64, Sender<Value>>>>,
    ready: Arc<Mutex<ReadyState>>,
}

#[derive(Default)]
struct ReadyState { began: bool, active: u32 }

impl LspSession {
    /// Spawn rust-analyzer rooted at `repo`, with CARGO_TARGET_DIR set to `target_cache` when given,
    /// and run the LSP initialize handshake.
    pub fn start(repo: &Path, target_cache: Option<&Path>) -> anyhow::Result<Self> {
        let mut cmd = Command::new("rust-analyzer");
        cmd.current_dir(repo).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        if let Some(tc) = target_cache { cmd.env("CARGO_TARGET_DIR", tc); }
        let mut child = cmd.spawn()
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
                    let Ok(msg): Result<Value, _> = serde_json::from_slice(&body) else { continue };
                    if let Some(id) = msg.get("id").and_then(|i| i.as_i64()) {
                        if let Some(tx) = pending.lock().unwrap().remove(&id) { let _ = tx.send(msg); }
                    } else if msg.get("method").and_then(|m| m.as_str()) == Some("$/progress") {
                        let mut g = ready.lock().unwrap();
                        match msg["params"]["value"]["kind"].as_str() {
                            Some("begin") => { g.began = true; g.active += 1; }
                            Some("end") => { g.active = g.active.saturating_sub(1); }
                            _ => {}
                        }
                    }
                }
            });
        }

        let mut s = LspSession { child, stdin, next_id: 0, pending, ready };
        let root = lsp_types::Url::from_file_path(repo)
            .map_err(|_| anyhow::anyhow!("repo path not absolute: {}", repo.display()))?;
        s.request("initialize", json!({
            "processId": std::process::id(),
            "rootUri": root,
            "capabilities": { "workspace": { "symbol": {} },
                "experimental": { "serverStatusNotification": true } },
            "workspaceFolders": [{ "uri": root, "name": "root" }],
        }), Duration::from_secs(30))?;
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

    /// Send a request and block for its response (by id).
    pub fn request(&mut self, method: &str, params: Value, timeout: Duration) -> anyhow::Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let (tx, rx): (Sender<Value>, Receiver<Value>) = channel();
        self.pending.lock().unwrap().insert(id, tx);
        self.send(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))?;
        let msg = rx.recv_timeout(timeout)
            .map_err(|_| anyhow::anyhow!("LSP request `{method}` timed out"))?;
        if let Some(e) = msg.get("error") {
            anyhow::bail!("LSP error on `{method}`: {e}");
        }
        Ok(msg.get("result").cloned().unwrap_or(Value::Null))
    }

    /// Block until indexing has begun-and-ended, or `timeout`. Best-effort past the bound.
    pub fn wait_ready(&mut self, timeout: Duration) -> anyhow::Result<()> {
        let t0 = Instant::now();
        loop {
            { let g = self.ready.lock().unwrap(); if g.began && g.active == 0 { return Ok(()); } }
            if t0.elapsed() >= timeout { return Ok(()); } // best-effort: answer anyway
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn workspace_symbol(&mut self, query: &str) -> anyhow::Result<Vec<NavHit>> {
        let res = self.request("workspace/symbol", json!({"query": query}), Duration::from_secs(20))?;
        let mut out = Vec::new();
        if let Some(arr) = res.as_array() {
            for it in arr {
                let loc: lsp_types::Location = match serde_json::from_value(it["location"].clone()) {
                    Ok(l) => l, Err(_) => continue,
                };
                let name = it["name"].as_str().map(|s| s.to_string());
                out.push(NavHit::from_location(&loc, name));
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
    fn drop(&mut self) { let _ = self.child.kill(); let _ = self.child.wait(); }
}
```

(Remove the standalone `pub mod codec;` added in Task 4's `mod.rs` if it now lives at the top of this file; keep exactly one declaration.)

- [ ] **Step 5: Run to verify pass**

Run: `cargo test -p lsp-mcp --test integration`
Expected: PASS where rust-analyzer is present (the `add` symbol is found); prints "skip" otherwise.

- [ ] **Step 6: Commit**

```bash
git add crates/lsp-mcp
git commit -m "feat(lsp-mcp): LspSession — spawn rust-analyzer, handshake, readiness, workspace_symbol"
```

---

## Task 6: The remaining six tools on LspSession

**Files:**
- Modify: `crates/lsp-mcp/src/lsp/mod.rs`, `crates/lsp-mcp/tests/integration.rs`

**Name-addressing:** position-based tools (`definition`/`references`/`hover`/`implementations`/`call_hierarchy`) accept a symbol *name*; the session resolves name→position via `workspace/symbol` (first hit), then issues the positional LSP request. `document_symbols` takes a file path.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn references_finds_the_caller() {
    if !ra_available() { eprintln!("skip"); return; }
    let mut s = lsp_mcp::lsp::LspSession::start(&sample_repo(), None).unwrap();
    s.wait_ready(std::time::Duration::from_secs(60)).unwrap();
    let refs = s.references("add", true).unwrap();
    // `add` is referenced by `caller` (line 2 in lib.rs) and declared on line 1.
    assert!(refs.iter().any(|h| h.line == 2), "references must include the call site in `caller`, got {refs:?}");
    s.shutdown();
}

#[test]
fn implementations_finds_the_impl() {
    if !ra_available() { eprintln!("skip"); return; }
    let mut s = lsp_mcp::lsp::LspSession::start(&sample_repo(), None).unwrap();
    s.wait_ready(std::time::Duration::from_secs(60)).unwrap();
    let impls = s.implementations("Greet").unwrap();
    assert!(!impls.is_empty(), "Greet must have an implementor (En)");
    s.shutdown();
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p lsp-mcp --test integration`
Expected: FAIL — `references`/`implementations` not found.

- [ ] **Step 3: Implement the resolver + six tools**

```rust
impl LspSession {
    /// Resolve a symbol name to its first definition position (for positional requests).
    fn resolve_pos(&mut self, name: &str) -> anyhow::Result<(lsp_types::Url, lsp_types::Position)> {
        let res = self.request("workspace/symbol", json!({"query": name}), Duration::from_secs(20))?;
        let first = res.as_array().and_then(|a| a.first())
            .ok_or_else(|| anyhow::anyhow!("symbol `{name}` not found"))?;
        let loc: lsp_types::Location = serde_json::from_value(first["location"].clone())?;
        Ok((loc.uri, loc.range.start))
    }

    fn positional(&mut self, method: &str, name: &str) -> anyhow::Result<Value> {
        let (uri, pos) = self.resolve_pos(name)?;
        self.request(method, json!({
            "textDocument": {"uri": uri}, "position": pos,
        }), Duration::from_secs(20))
    }

    fn hits_from_locations(v: &Value) -> Vec<NavHit> {
        let arr = match v { Value::Array(a) => a.clone(),
            Value::Null => vec![], other => vec![other.clone()] };
        arr.iter().filter_map(|it| {
            // `Location` or `LocationLink`
            let loc = it.get("targetUri").map(|u| json!({"uri": u, "range": it["targetRange"]}))
                .unwrap_or_else(|| it.clone());
            serde_json::from_value::<lsp_types::Location>(loc).ok()
                .map(|l| NavHit::from_location(&l, None))
        }).collect()
    }

    pub fn definition(&mut self, name: &str) -> anyhow::Result<Vec<NavHit>> {
        Ok(Self::hits_from_locations(&self.positional("textDocument/definition", name)?))
    }
    pub fn references(&mut self, name: &str, include_decl: bool) -> anyhow::Result<Vec<NavHit>> {
        let (uri, pos) = self.resolve_pos(name)?;
        let v = self.request("textDocument/references", json!({
            "textDocument": {"uri": uri}, "position": pos,
            "context": {"includeDeclaration": include_decl},
        }), Duration::from_secs(30))?;
        Ok(Self::hits_from_locations(&v))
    }
    pub fn implementations(&mut self, name: &str) -> anyhow::Result<Vec<NavHit>> {
        Ok(Self::hits_from_locations(&self.positional("textDocument/implementation", name)?))
    }
    pub fn hover(&mut self, name: &str) -> anyhow::Result<Option<String>> {
        let v = self.positional("textDocument/hover", name)?;
        Ok(v["contents"]["value"].as_str().map(|s| s.to_string())
            .or_else(|| v["contents"].as_str().map(|s| s.to_string())))
    }
    pub fn document_symbols(&mut self, file: &Path) -> anyhow::Result<Vec<NavHit>> {
        let uri = lsp_types::Url::from_file_path(file)
            .map_err(|_| anyhow::anyhow!("file path not absolute"))?;
        let v = self.request("textDocument/documentSymbol", json!({"textDocument": {"uri": uri}}),
            Duration::from_secs(20))?;
        let mut out = Vec::new();
        if let Some(arr) = v.as_array() {
            for it in arr {
                if let Some(name) = it["name"].as_str() {
                    let line = it["range"]["start"]["line"].as_u64().unwrap_or(0) as u32 + 1;
                    out.push(NavHit { file: file.to_string_lossy().into(), line,
                        signature: Some(name.to_string()), context: it["detail"].as_str().map(Into::into) });
                }
            }
        }
        Ok(out)
    }
    pub fn call_hierarchy(&mut self, name: &str, incoming: bool) -> anyhow::Result<Vec<NavHit>> {
        let (uri, pos) = self.resolve_pos(name)?;
        let prep = self.request("textDocument/prepareCallHierarchy",
            json!({"textDocument": {"uri": uri}, "position": pos}), Duration::from_secs(20))?;
        let item = prep.as_array().and_then(|a| a.first()).cloned()
            .ok_or_else(|| anyhow::anyhow!("no call-hierarchy item for `{name}`"))?;
        let method = if incoming { "callHierarchy/incomingCalls" } else { "callHierarchy/outgoingCalls" };
        let v = self.request(method, json!({"item": item}), Duration::from_secs(30))?;
        let key = if incoming { "from" } else { "to" };
        let mut out = Vec::new();
        if let Some(arr) = v.as_array() {
            for it in arr {
                if let Ok(node) = serde_json::from_value::<lsp_types::CallHierarchyItem>(it[key].clone()) {
                    out.push(NavHit { file: node.uri.to_file_path().map(|p| p.to_string_lossy().into())
                        .unwrap_or_default(), line: node.range.start.line + 1,
                        signature: Some(node.name), context: node.detail });
                }
            }
        }
        Ok(out)
    }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p lsp-mcp --test integration`
Expected: PASS (references finds line 2; implementations non-empty).

- [ ] **Step 5: Commit**

```bash
git add crates/lsp-mcp
git commit -m "feat(lsp-mcp): definition/references/hover/implementations/document_symbols/call_hierarchy"
```

---

## Task 7: MCP transport + lifecycle gating

**Files:**
- Create: `crates/lsp-mcp/src/mcp/mod.rs`, `crates/lsp-mcp/src/mcp/transport.rs`, `crates/lsp-mcp/src/mcp/error.rs`
- Modify: `crates/lsp-mcp/src/lib.rs` (`pub mod mcp;`)

**Lifecycle (mirror prism):** `tools/call` before `initialized` → error -32600; unknown method → -32601; bad params → -32602; tool failure → -32603 (or an MCP `isError` content). A repeat `initialize` must not downgrade.

- [ ] **Step 1: Write the failing transport tests**

```rust
// in transport.rs #[cfg(test)]
fn drive(msgs: &[&str]) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let mut lc = Lifecycle::default();
    for m in msgs {
        let v: serde_json::Value = serde_json::from_str(m).unwrap();
        if let Some(reply) = lc.handle_meta(&v) { out.push(reply); }
    }
    out
}

#[test]
fn tools_call_before_initialized_is_32600() {
    let out = drive(&[r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"definition","arguments":{}}}"#]);
    assert_eq!(out[0]["error"]["code"], -32600);
}

#[test]
fn initialize_then_tools_list_ok() {
    let out = drive(&[
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
    ]);
    assert_eq!(out[0]["id"], 1);                 // initialize result
    assert!(out.iter().any(|m| m["id"] == 2 && m["result"]["tools"].is_array()));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p lsp-mcp transport`
Expected: FAIL — `Lifecycle` not found.

- [ ] **Step 3: Implement `error.rs` and the `Lifecycle` meta-handler**

```rust
// error.rs
use serde_json::{json, Value};
pub fn err(id: &Value, code: i64, msg: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":msg}})
}
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL: i64 = -32603;
```

```rust
// transport.rs — protocol lifecycle only; tool dispatch is injected by mcp/mod.rs.
use crate::mcp::error::*;
use serde_json::{json, Value};

#[derive(Default)]
pub struct Lifecycle { initialized: bool }

impl Lifecycle {
    /// Handle initialize/initialized/tools/list and lifecycle errors. Returns a reply for
    /// request messages, or None for handled notifications. `tools/call` returns None here — the
    /// caller routes it to the tool dispatcher only when `self.initialized`.
    pub fn handle_meta(&mut self, msg: &Value) -> Option<Value> {
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        match method {
            "initialize" => Some(json!({"jsonrpc":"2.0","id":id,"result":{
                "protocolVersion":"2025-11-25",
                "capabilities":{"tools":{}},
                "serverInfo":{"name":"lsp-mcp","version":env!("CARGO_PKG_VERSION")}}})),
            "notifications/initialized" => { self.initialized = true; None }
            "tools/list" => Some(json!({"jsonrpc":"2.0","id":id,"result":{
                "tools": crate::mcp::tool_schemas()}})),
            "tools/call" if !self.initialized =>
                Some(err(&id, INVALID_REQUEST, "received tools/call before initialized")),
            "ping" => Some(json!({"jsonrpc":"2.0","id":id,"result":{}})),
            _ => None, // tools/call (when initialized) and unknown handled by the caller
        }
    }
    pub fn is_initialized(&self) -> bool { self.initialized }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p lsp-mcp transport`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/lsp-mcp/src/mcp/ crates/lsp-mcp/src/lib.rs
git commit -m "feat(lsp-mcp): MCP stdio lifecycle (initialize gating, error codes)"
```

---

## Task 8: Tool registry/schemas + dispatch + the run loop

**Files:**
- Modify: `crates/lsp-mcp/src/mcp/mod.rs`, `crates/lsp-mcp/src/lib.rs` (`run` wires it up)

- [ ] **Step 1: Write the failing test (schemas present, names match)**

```rust
// in mcp/mod.rs #[cfg(test)]
#[test]
fn exposes_the_seven_tools() {
    let names: Vec<String> = tool_schemas().iter()
        .map(|t| t["name"].as_str().unwrap().to_string()).collect();
    for n in ["workspace_symbol","document_symbols","definition","references",
              "hover","implementations","call_hierarchy"] {
        assert!(names.contains(&n.to_string()), "missing tool {n}");
    }
    assert_eq!(names.len(), 7);
    // every tool advertises an object inputSchema
    for t in tool_schemas() { assert_eq!(t["inputSchema"]["type"], "object"); }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p lsp-mcp exposes_the_seven_tools`
Expected: FAIL — `tool_schemas` not found.

- [ ] **Step 3: Implement `tool_schemas`, `dispatch`, and the `run` loop**

```rust
// mcp/mod.rs
pub mod error;
pub mod transport;

use crate::lsp::LspSession;
use crate::mcp::error::*;
use crate::mcp::transport::Lifecycle;
use crate::shape::render_hits;
use serde_json::{json, Value};
use std::io::{BufReader, Write};

fn name_arg(a: &Value) -> Result<&str, Value> {
    a.get("name").and_then(|v| v.as_str())
        .ok_or_else(|| json!({"error":"missing required string arg `name`"}))
}

pub fn tool_schemas() -> Vec<Value> {
    let name_only = json!({"type":"object","properties":{"name":{"type":"string",
        "description":"symbol name to resolve"}},"required":["name"]});
    vec![
        json!({"name":"workspace_symbol","description":"Find a symbol by name across the repo (entry point).",
            "inputSchema":json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"]})}),
        json!({"name":"document_symbols","description":"Outline of a file's symbols.",
            "inputSchema":json!({"type":"object","properties":{"file":{"type":"string"}},"required":["file"]})}),
        json!({"name":"definition","description":"Type-resolved go-to-definition of a symbol.","inputSchema":name_only}),
        json!({"name":"references","description":"All references to a symbol (blast radius); resolves generics/traits.",
            "inputSchema":json!({"type":"object","properties":{"name":{"type":"string"},
                "include_declaration":{"type":"boolean"}},"required":["name"]})}),
        json!({"name":"hover","description":"Resolved type + signature + docs at a symbol.","inputSchema":name_only}),
        json!({"name":"implementations","description":"Trait impls / who implements a trait or type.","inputSchema":name_only}),
        json!({"name":"call_hierarchy","description":"Type-resolved callers/callees of a symbol.",
            "inputSchema":json!({"type":"object","properties":{"name":{"type":"string"},
                "direction":{"type":"string","enum":["incoming","outgoing"]}},"required":["name"]})}),
    ]
}

fn ok(id: &Value, body: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":body.to_string()}]}})
}

fn dispatch(id: &Value, params: &Value, s: &mut LspSession) -> Value {
    let tool = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let a = params.get("arguments").cloned().unwrap_or(json!({}));
    let res: anyhow::Result<Value> = (|| Ok(match tool {
        "workspace_symbol" => {
            let q = a["query"].as_str().ok_or_else(|| anyhow::anyhow!("missing `query`"))?;
            render_hits(&s.workspace_symbol(q)?)
        }
        "document_symbols" => {
            let f = a["file"].as_str().ok_or_else(|| anyhow::anyhow!("missing `file`"))?;
            render_hits(&s.document_symbols(std::path::Path::new(f))?)
        }
        "definition" => render_hits(&s.definition(name_arg(&a).map_err(|e| anyhow::anyhow!(e.to_string()))?)?),
        "references" => render_hits(&s.references(name_arg(&a).map_err(|e| anyhow::anyhow!(e.to_string()))?,
            a["include_declaration"].as_bool().unwrap_or(true))?),
        "hover" => json!({"hover": s.hover(name_arg(&a).map_err(|e| anyhow::anyhow!(e.to_string()))?)?}),
        "implementations" => render_hits(&s.implementations(name_arg(&a).map_err(|e| anyhow::anyhow!(e.to_string()))?)?),
        "call_hierarchy" => render_hits(&s.call_hierarchy(name_arg(&a).map_err(|e| anyhow::anyhow!(e.to_string()))?,
            a["direction"].as_str().unwrap_or("incoming") == "incoming")?),
        other => return Err(anyhow::anyhow!("unknown tool `{other}`")),
    }))();
    match res {
        Ok(body) => ok(id, body),
        // tool failures are reported as content with isError, so the agent sees the reason and degrades.
        Err(e) => json!({"jsonrpc":"2.0","id":id,"result":{"isError":true,
            "content":[{"type":"text","text":format!("lsp-mcp error: {e}")}]}}),
    }
}

/// Block on stdin, driving the MCP loop against a warm `LspSession`.
pub fn serve(mut session: LspSession) -> anyhow::Result<()> {
    let stdin = std::io::stdin();
    let mut r = BufReader::new(stdin.lock());
    let mut out = std::io::stdout();
    let mut lc = Lifecycle::default();
    while let Some(body) = transport::read_frame_stdin(&mut r)? {
        let msg: Value = match serde_json::from_slice(&body) { Ok(v) => v, Err(_) => continue };
        let reply = if msg.get("method").and_then(|m| m.as_str()) == Some("tools/call") && lc.is_initialized() {
            Some(dispatch(&msg["id"], &msg["params"], &mut session))
        } else if let Some(r) = lc.handle_meta(&msg) {
            Some(r)
        } else if msg.get("id").is_some() && msg.get("method").is_some() {
            Some(err(&msg["id"], METHOD_NOT_FOUND, "unknown method"))
        } else { None };
        if let Some(reply) = reply {
            crate::lsp::codec::write_frame(&mut out, serde_json::to_vec(&reply)?.as_slice())?;
        }
    }
    session.shutdown();
    Ok(())
}
```

Add to `transport.rs`: `pub fn read_frame_stdin<R: std::io::BufRead>(r: &mut R) -> std::io::Result<Option<Vec<u8>>> { crate::lsp::codec::read_frame(r) }` (reuse the codec).

- [ ] **Step 4: Wire `run` in `lib.rs`**

```rust
pub fn run(cli: Cli) -> anyhow::Result<()> {
    anyhow::ensure!(cli.lang == "rust", "Slice A supports only --lang rust (got {:?})", cli.lang);
    let repo = cli.repo.canonicalize()
        .map_err(|e| anyhow::anyhow!("repo {:?}: {e}", cli.repo))?;
    anyhow::ensure!(repo.join("Cargo.toml").exists(), "not a cargo repo (no Cargo.toml): {:?}", repo);
    let target = cli.target_cache.as_deref().map(|base| {
        let origin = git_origin(&repo);
        cache_key::cache_dir(base, &repo, origin.as_deref())
    });
    let mut session = lsp::LspSession::start(&repo, target.as_deref())?;
    session.wait_ready(std::time::Duration::from_secs(30))?;
    mcp::serve(session)
}

fn git_origin(repo: &std::path::Path) -> Option<String> {
    let out = std::process::Command::new("git").current_dir(repo)
        .args(["config","--get","remote.origin.url"]).output().ok()?;
    if !out.status.success() { return None; }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}
```

Add `pub mod lsp; pub mod mcp;` to `lib.rs` if not already present.

- [ ] **Step 5: Run to verify pass + full build**

Run: `cargo test -p lsp-mcp && cargo build -p lsp-mcp`
Expected: PASS; binary builds.

- [ ] **Step 6: End-to-end smoke (manual, where rust-analyzer present)**

Run:
```bash
printf 'Content-Length: 52\r\n\r\n{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' | \
  cargo run -q -p lsp-mcp -- --repo crates/lsp-mcp/tests/fixtures/sample | head -c 200; echo
```
Expected: a framed `initialize` result with `serverInfo.name == "lsp-mcp"`.

- [ ] **Step 7: Commit**

```bash
git add crates/lsp-mcp
git commit -m "feat(lsp-mcp): tool registry + dispatch + stdin serve loop (7 tools live)"
```

---

## Task 9: clippy/fmt clean + workspace gate

**Files:** none (hygiene)

- [ ] **Step 1: Format and lint**

Run: `cargo fmt --all && cargo clippy -p lsp-mcp --all-targets -- -D warnings`
Expected: no diffs after fmt; clippy clean. Fix any `-D warnings` (likely: unused imports, `Default` derive nits).

- [ ] **Step 2: Full workspace build + test (the gate floor)**

Run: `cargo build --locked && cargo test --workspace --locked --exclude bridge-container -- --skip process::tests::terminate_reaps_child_no_zombie --skip process::tests::term_ignoring_loop_forces_group_sigkill --skip process::tests::drop_group_kills_descendants`
Expected: green (matches the `[verify]` command set in the containerized config).

- [ ] **Step 3: Commit (if fmt/clippy changed anything)**

```bash
git add -A
git commit -m "style(lsp-mcp): fmt + clippy clean"
```

---

## Task 10: kiro skill `resources` field in `bridge-core::mcp`

**Files:**
- Modify: `crates/bridge-core/src/mcp.rs` (`render_kiro_agent_config`)
- Test: same file (`#[cfg(test)]`)

This is the only `bridge-core` change. Custom kiro agents must opt into skill discovery via a `resources` field with `skill://` URIs (default agents auto-discover; the bridge writes *custom* agents).

- [ ] **Step 1: Read the current function and its tests**

Run: `grep -n "fn render_kiro_agent_config" crates/bridge-core/src/mcp.rs` and read the function + the nearest existing test.

- [ ] **Step 2: Write the failing test**

```rust
#[test]
fn kiro_agent_config_advertises_skill_resources() {
    let cfg = render_kiro_agent_config(&[], "/repo", "a2a-mcp-kiro");
    let v: serde_json::Value = serde_json::from_str(&cfg).unwrap();
    let res = v["resources"].as_array().expect("resources array");
    let joined = res.iter().filter_map(|r| r.as_str()).collect::<Vec<_>>().join(" ");
    assert!(joined.contains("skill://"), "must advertise skill:// resources, got {joined}");
    assert!(joined.contains("~/.kiro/skills/"), "must include the global skills glob");
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p bridge-core kiro_agent_config_advertises_skill_resources`
Expected: FAIL — `resources` absent or empty.

- [ ] **Step 4: Implement**

In `render_kiro_agent_config`, after building the existing `Map`, insert:
```rust
config.insert("resources".into(), json!([
    "skill://.kiro/skills/*/SKILL.md",
    "skill://~/.kiro/skills/*/SKILL.md",
]));
```
(Match the existing variable name for the root config object; the function already returns serialized JSON.)

- [ ] **Step 5: Run to verify pass + the existing kiro tests still pass**

Run: `cargo test -p bridge-core mcp`
Expected: PASS (new test + existing render tests).

- [ ] **Step 6: Commit**

```bash
git add crates/bridge-core/src/mcp.rs
git commit -m "feat(mcp): kiro custom agents opt into skill discovery via resources field"
```

---

## Task 11: Author the `lsp-nav` skill in `~/knowledge-ref`

**Files:**
- Create: `~/knowledge-ref/skills/lsp-nav/SKILL.md`, `~/knowledge-ref/skills/lsp-nav/references/lsp-vs-prism.md`

Not a Rust task — a content artifact. Acceptance: valid frontmatter (`name`, `description` ≤1024 chars), description packed with triggers (best practice), the playbook body, and it installs cleanly.

- [ ] **Step 1: Write `SKILL.md`**

Frontmatter `name: lsp-nav`; `description` (pushy, trigger-rich) e.g.:
> "Type-resolved code navigation with the `lsp` MCP tools (workspace_symbol, document_symbols, definition, references, hover, implementations, call_hierarchy). Use when reviewing a change or planning an edit and you need the EXACT type at a point, all references to a symbol (blast radius, resolving generics/traits that grep/prism miss), what implements a trait, or a type-resolved call graph — 'what is this type', 'find all references', 'who implements X', 'go to definition', 'callers of Y'. Semantic counterpart to prism-nav (structural graph) and the slicing CLI (diff-driven). Tools are mcp__lsp__* for Claude/Codex, bare names for Kiro."

Body: the tool-picker table (lsp vs prism vs grep/read), the `workspace_symbol → position → query` chain, the review heuristics (every changed `pub` item → `references`; changed trait/impl → `implementations`; unfamiliar type → `hover`; changed contract → `call_hierarchy incoming`), and budget discipline (query the diff's symbols, don't spider). Keep it concise (progressive disclosure).

- [ ] **Step 2: Write `references/lsp-vs-prism.md`** — the deeper "when LSP beats prism beats grep" guide with worked examples.

- [ ] **Step 3: Install + verify**

Run: `~/knowledge-ref/install-skills.sh && for d in ~/.claude/skills ~/.agents/skills ~/.kiro/skills; do test -f "$d/lsp-nav/SKILL.md" && echo "$d OK" || echo "$d MISSING"; done`
Expected: three `OK` lines.

- [ ] **Step 4: Validate frontmatter**

Run: `head -20 ~/knowledge-ref/skills/lsp-nav/SKILL.md`
Expected: well-formed YAML frontmatter; `description` is a single block scalar, no stray `VERDICT`-style contradictions, length under 1024 chars.

- [ ] **Step 5: Commit (in the knowledge-ref repo)**

```bash
cd ~/knowledge-ref && git add skills/lsp-nav && \
  git -c user.name="Wesley Jinks" -c user.email="wesley.jinks@gmail.com" \
  commit -m "Add lsp-nav skill (type-resolved navigation, sibling to prism-nav)" && cd -
```

---

## Task 12: Wire `lsp` into the bridge config + review prompt

**Files:**
- Modify: `examples/a2a-bridge.containerized.toml`, `examples/a2a-bridge.containerized.podman.toml`, `prompts/review-implement.md`

- [ ] **Step 1: Add the `lsp` MCP server to the claude reviewer**

In `examples/a2a-bridge.containerized.toml`, under the `claude` agent's existing `[[agents.mcp]]` (prism), add a second:
```toml
[[agents.mcp]]
name = "lsp"
command = "/Users/wesleyjinks/code/a2a-bridge/target/release/lsp-mcp"
args = ["--repo", "{cwd}", "--lang", "rust", "--target-cache", "/Users/wesleyjinks/.local/share/a2a/lsp-target-cache"]
```

- [ ] **Step 2: Mirror it under the `codex` reviewer** (same block).

- [ ] **Step 3: Mirror both into `…podman.toml`** (byte-identical except the documented comment/runtime/allowed_cmds differences the parity test enforces).

- [ ] **Step 4: Run the podman parity test**

Run: `cargo test -p a2a-bridge podman_example_parses_validates_and_mirrors_docker`
Expected: PASS (the two configs still mirror).

- [ ] **Step 5: Add the always-on fallback pointer to the review prompt**

In `prompts/review-implement.md`, add one line near the navigation guidance:
> "For type-resolved questions (exact types, all references/blast-radius, trait implementors, call graph) use the `lsp` MCP tools; for structural questions use `prism`. See the `lsp-nav` skill."

- [ ] **Step 6: Build the release binary the config points at**

Run: `cargo build --release -p lsp-mcp && test -x target/release/lsp-mcp && echo OK`
Expected: `OK`.

- [ ] **Step 7: Commit**

```bash
git add examples/a2a-bridge.containerized.toml examples/a2a-bridge.containerized.podman.toml prompts/review-implement.md
git commit -m "feat(config): wire lsp MCP nav into claude+codex reviewers + review prompt pointer"
```

---

## Task 13: Live dogfood DoD gate

**Files:** none (validation), then memory.

This is the spec's Definition of Done — validate through the bridge itself.

- [ ] **Step 1: Start serve with the containerized config**

Run: `a2a-bridge serve --config examples/a2a-bridge.containerized.toml` (in one shell; ensure `target/release/lsp-mcp` exists from Task 12).

- [ ] **Step 2: Run an `implement-review` on a small a2a-bridge change** (e.g., the change under review is this very branch's diff), capturing reviewer agent logs.

- [ ] **Step 3: Confirm the DoD**

- A host-side reviewer issues at least one `lsp` tool call (e.g. `references` on a changed `pub fn`) — grep the bridge logs under `agent_stderr`/tool-call traces for `mcp__lsp__` (claude/codex) usage.
- Confirm the `lsp-nav` skill activated OR the inlined prompt pointer drove the same usage.
- Confirm the per-repo target cache dir was created under `~/.local/share/a2a/lsp-target-cache/ra-*`.

Expected: all three observed. If the reviewer never calls `lsp`, strengthen the skill `description` triggers and/or the prompt pointer and re-run (the spec flags activation reliability as the key risk).

- [ ] **Step 4: Record memory**

Write a memory file noting: L3 Slice A shipped; the spike findings that shaped it (clone-reuse 0.72s, OrbStack UDS boundary, ~50ms incremental); the cross-agent skills library at `~/knowledge-ref`; and the gotcha that the cache key is a reuse boundary not a re-index trigger. Add the one-line MEMORY.md index entry.

- [ ] **Step 5: Finish the branch**

REQUIRED SUB-SKILL: Use superpowers:finishing-a-development-branch.

---

## Self-review notes

- **Spec coverage:** shim (Tasks 1–9), 7 tools (Tasks 5–6, 8), name-addressing (Task 6 `resolve_pos`), per-repo cache key (Task 2, wired Task 8), skills library (already bootstrapped; lsp-nav Task 11), kiro `resources` (Task 10), config wiring (Task 12), DoD live gate (Task 13). Non-goals (Slice B/C, idle-evict, gateway) are not tasked — correct.
- **Readiness** is implemented with a real progress-tracking fallback, not a placeholder; the rust-analyzer `serverStatus` capability is advertised as the primary signal but the code does not depend on it.
- **Hermetic verify:** the integration tests are guarded by `ra_available()` so the container `verify` (no rust-analyzer) skips them; the host gate runs them.
- **`lsp-types` version** (0.97) and the `CallHierarchyItem`/`Location` deserialization shapes should be confirmed against the locked version on first build; the JSON access is defensive (`from_value(...).ok()`), so a field mismatch degrades to fewer hits rather than a panic.
