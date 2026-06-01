# API Backend (`kind="api"`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a non-process, vendor-neutral OpenAI-compatible HTTP `AgentBackend` (`kind="api"`) — the cheap/free B1 replacement that gives the parked conductor decision its non-process evidence (surface A = lifecycle/transport, surface B = permission/policy).

**Architecture:** A new `crates/bridge-api` crate holds `ApiBackend` (impl `bridge_core::ports::AgentBackend`) over `reqwest`. It owns no child process. The whole prompt turn — stream text, request a tool, **decide the tool permission silently via the injected `PolicyEngine`**, execute/deny, loop to a final answer — runs inside `prompt()` and yields **only `Update::Text`/`Update::Done`** (never `Update::Permission`). Phase A builds + fully tests the crate in isolation (wiremock); Phase B does the surface-A domain ripple (`cmd: String → Option<String>`, new `base_url`/`api_key_env`, `registry::validate` fix, the factory `Api` arm) and wires it in.

**Tech Stack:** Rust, `reqwest` 0.12 (json/stream/rustls-tls), `serde`/`serde_json`, `async-stream`, `tokio`. Dev: `wiremock` (offline mock HTTP). Live gate: local Ollama (`qwen3.5:9b`). Spec: `docs/superpowers/specs/2026-06-01-a2a-bridge-api-backend-design.md` (rev2).

**Conventions (project standing rules):**
- Subagent task commits do **NOT** add a `Co-Authored-By` trailer. Only controller doc commits do (Task 22 only).
- Coverage is measured **after** `cargo llvm-cov clean --workspace`.
- `~/code/a2a-local-bridge` is firewall-black-box; do not read its source.
- Every task ends green: `cargo build`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`, `cargo test` (the touched crate).

---

## File Structure

**New crate `crates/bridge-api/`:**
- `Cargo.toml` — deps: `bridge-core`, `reqwest`, `serde`, `serde_json`, `async-stream`, `futures`, `tokio`, `tracing`, `async-trait`, `tokio-stream`; dev: `wiremock`, `tokio-test`.
- `src/lib.rs` — `pub mod config; pub mod wire; pub mod tool; pub mod backend;` + re-exports `pub use {config::ApiConfig, backend::ApiBackend};`.
- `src/config.rs` — `ApiConfig` (base_url, model, api_key_env, max_tool_rounds, request_timeout, stream).
- `src/wire.rs` — request/response serde types + `SseAccumulator` (tolerant streamed parse) + `parse_nonstream` (the `stream:false` shape).
- `src/tool.rs` — the stub `get_current_time` tool: `tool_def()` (JSON schema) + `run_tool(&ToolCall) -> String`.
- `src/backend.rs` — `ApiBackend`, `SessionState`, the `AgentBackend` impl, the turn loop, the silent policy decision.
- `tests/fixtures/ollama-openai-compat.json` — REAL-CAPTURE frames (the single source of the wiremock stub bodies).
- `tests/wiremock_turns.rs` — DoD-1/2/4/5/5b offline suite.
- `tests/deny_through_translator.rs` — DoD-3 (the B1-catching test).
- `tests/corpus_replay.rs` — DoD-6 provenance + replay-through-parser.
- `tests/live_ollama.rs` — DoD-7 gated `#[ignore]`.

**Modified (Phase B — surface A ripple):**
- `crates/bridge-core/src/domain.rs` — `AgentKind::Api`; `AgentEntry.cmd: Option<String>`; `+ base_url`, `+ api_key_env`; fix literals.
- `crates/bridge-a2a-inbound/src/server.rs:1683`, `bin/a2a-bridge/src/route.rs:95` — `AgentEntry` literals.
- `bin/a2a-bridge/src/config.rs` — raw TOML struct, `into_snapshot`, `parse_kind`, parse-shape validation.
- `crates/bridge-registry/src/registry.rs` — `validate` fix + kind-invariant, reuse-identity `base_url`, test literals.
- `bin/a2a-bridge/src/main.rs` — factory Acp-arm `Some`-guard + new `Api` arm.
- `bin/a2a-bridge/Cargo.toml` — add `bridge-api` dep.
- `bin/a2a-bridge/tests/e2e_registry.rs`, `bin/a2a-bridge/tests/common/mod.rs` — kind-aware spawn factory + literals + DoD-8.
- `.github/workflows/ci.yml` — `bridge-api` 90% floor.
- `docs/adr/0007-api-backend.md` — new ADR.

---

## Task 0: Branch + crate skeleton

**Files:**
- Create: `crates/bridge-api/Cargo.toml`, `crates/bridge-api/src/lib.rs`

- [ ] **Step 1: Branch off main**

```bash
cd /Users/wesleyjinks/code/a2a-bridge
git checkout main && git checkout -b feat/api-backend
```

- [ ] **Step 2: Create the crate manifest**

`crates/bridge-api/Cargo.toml`:
```toml
[package]
name = "bridge-api"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
bridge-core = { path = "../bridge-core" }
reqwest = { version = "0.12", default-features = false, features = ["json", "stream", "rustls-tls"] }
serde.workspace = true
serde_json.workspace = true
async-stream.workspace = true
futures.workspace = true
tokio = { workspace = true }
tokio-stream.workspace = true
tracing.workspace = true
async-trait.workspace = true

[dev-dependencies]
tokio = { workspace = true }
tokio-test = { workspace = true }
wiremock = "0.6"
```

- [ ] **Step 3: Create `src/lib.rs`**

```rust
//! bridge-api — a non-process, OpenAI-compatible HTTP AgentBackend (kind="api").
//! See docs/superpowers/specs/2026-06-01-a2a-bridge-api-backend-design.md.
pub mod backend;
pub mod config;
pub mod tool;
pub mod wire;

pub use backend::ApiBackend;
pub use config::ApiConfig;
```

(Modules are created in later tasks; declare them now as empty files so the crate builds — create `src/config.rs`, `src/wire.rs`, `src/tool.rs`, `src/backend.rs` each containing only a `// placeholder` line for this step, replaced in their tasks.)

- [ ] **Step 4: Verify it builds**

Run: `cargo build -p bridge-api`
Expected: compiles (the crate is auto-included via `members = ["crates/*"]`).

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-api Cargo.lock
git commit -m "feat(api): scaffold bridge-api crate skeleton"
```

---

## Task 1: The stub tool (`tool.rs`)

**Files:**
- Modify: `crates/bridge-api/src/tool.rs`
- Modify: `crates/bridge-api/src/wire.rs` (define `ToolCall` early — Task 3 fills the rest)

- [ ] **Step 1: Write the failing test**

Append to `crates/bridge-api/src/tool.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_def_is_get_current_time_function() {
        let d = tool_def();
        assert_eq!(d["type"], "function");
        assert_eq!(d["function"]["name"], "get_current_time");
        assert!(d["function"]["parameters"]["type"] == "object");
    }

    #[test]
    fn run_tool_returns_deterministic_stub() {
        // Side-effect-free, deterministic (NOT wall-clock) so tests are stable.
        assert_eq!(run_tool(), "2026-01-01T00:00:00Z");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-api tool::`
Expected: FAIL — `tool_def`/`run_tool` not found.

- [ ] **Step 3: Implement**

Replace the placeholder in `crates/bridge-api/src/tool.rs` (above the test module):
```rust
//! The single stub tool. Its only purpose is to make the model emit a tool_call
//! so the permission control-flow (surface B) runs. Side-effect-free + deterministic.
use serde_json::{json, Value};

pub const TOOL_NAME: &str = "get_current_time";

/// The OpenAI `tools[]` entry advertised on every request.
pub fn tool_def() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": TOOL_NAME,
            "description": "Return the current server time as an ISO-8601 string.",
            "parameters": { "type": "object", "properties": {}, "required": [] }
        }
    })
}

/// Execute the stub. Deterministic constant — the value is irrelevant; the
/// control-flow (decide → execute → feed result) is the point.
pub fn run_tool() -> String {
    "2026-01-01T00:00:00Z".to_string()
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p bridge-api tool::`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-api/src/tool.rs
git commit -m "feat(api): stub get_current_time tool"
```

---

## Task 2: `ApiConfig` (`config.rs`)

**Files:**
- Modify: `crates/bridge-api/src/config.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/bridge-api/src/config.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_are_sane() {
        let c = ApiConfig::new("http://localhost:11434/v1");
        assert_eq!(c.base_url, "http://localhost:11434/v1");
        assert_eq!(c.max_tool_rounds, 4);
        assert!(c.stream);
        assert_eq!(c.request_timeout, std::time::Duration::from_secs(120));
        assert!(c.model.is_none() && c.api_key_env.is_none());
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-api config::`
Expected: FAIL — `ApiConfig` not found.

- [ ] **Step 3: Implement**

Replace the placeholder in `crates/bridge-api/src/config.rs` (above the test module):
```rust
//! Configuration for ApiBackend. `model`/`api_key_env` are NOT frozen here — the
//! backend resolves the key per-prompt (env) and the model per-session (stash);
//! `ApiConfig` holds only the construction-time defaults.
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ApiConfig {
    /// OpenAI-compatible base, e.g. "http://localhost:11434/v1". The backend POSTs
    /// to `{base_url}/chat/completions`.
    pub base_url: String,
    /// Default request model id; per-session `configure_session` may override it.
    pub model: Option<String>,
    /// NAME of an env var holding a bearer token (never the secret). Read per-prompt.
    pub api_key_env: Option<String>,
    /// Bounds the tool loop — no infinite tool_call cycles.
    pub max_tool_rounds: usize,
    pub request_timeout: Duration,
    /// Use SSE streaming (default). `false` uses the non-streamed `message.tool_calls` shape.
    pub stream: bool,
}

impl ApiConfig {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            model: None,
            api_key_env: None,
            max_tool_rounds: 4,
            request_timeout: Duration::from_secs(120),
            stream: true,
        }
    }
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p bridge-api config::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-api/src/config.rs
git commit -m "feat(api): ApiConfig with sane defaults"
```

---

## Task 3: Wire request types (`wire.rs`)

**Files:**
- Modify: `crates/bridge-api/src/wire.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/bridge-api/src/wire.rs`:
```rust
#[cfg(test)]
mod request_tests {
    use super::*;
    #[test]
    fn chat_request_serializes_expected_shape() {
        let req = ChatRequest {
            model: Some("qwen3.5:9b".into()),
            messages: vec![Message::user("hi")],
            tools: vec![crate::tool::tool_def()],
            stream: true,
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["model"], "qwen3.5:9b");
        assert_eq!(v["stream"], true);
        assert_eq!(v["messages"][0]["role"], "user");
        assert_eq!(v["messages"][0]["content"], "hi");
        assert_eq!(v["tools"][0]["function"]["name"], "get_current_time");
    }
    #[test]
    fn assistant_tool_call_and_tool_result_messages_serialize() {
        let tc = ToolCall { id: "call_1".into(), kind: "function".into(),
            function: FunctionCall { name: "get_current_time".into(), arguments: "{}".into() } };
        let asst = Message::assistant_tool_calls(vec![tc.clone()]);
        let result = Message::tool_result("call_1", "2026-01-01T00:00:00Z");
        let va = serde_json::to_value(&asst).unwrap();
        let vr = serde_json::to_value(&result).unwrap();
        assert_eq!(va["role"], "assistant");
        assert_eq!(va["tool_calls"][0]["id"], "call_1");
        assert!(va.get("content").is_none() || va["content"].is_null());
        assert_eq!(vr["role"], "tool");
        assert_eq!(vr["tool_call_id"], "call_1");
        assert_eq!(vr["content"], "2026-01-01T00:00:00Z");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-api wire::request_tests`
Expected: FAIL — types not found.

- [ ] **Step 3: Implement**

Replace the placeholder in `crates/bridge-api/src/wire.rs` (above any test modules):
```rust
//! OpenAI-compatible wire types + a TOLERANT streamed-response parser.
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Request ──────────────────────────────────────────────────────────────────
#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Value>,
    pub stream: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self { role: "user".into(), content: Some(text.into()), tool_calls: None, tool_call_id: None }
    }
    pub fn assistant_tool_calls(calls: Vec<ToolCall>) -> Self {
        Self { role: "assistant".into(), content: None, tool_calls: Some(calls), tool_call_id: None }
    }
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self { role: "tool".into(), content: Some(content.into()), tool_calls: None,
            tool_call_id: Some(tool_call_id.into()) }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "default_function")]
    pub kind: String,
    pub function: FunctionCall,
}
fn default_function() -> String { "function".into() }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p bridge-api wire::request_tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-api/src/wire.rs
git commit -m "feat(api): OpenAI-compatible request wire types"
```

---

## Task 4: Tolerant streamed-response parser (`SseAccumulator`)

**Files:**
- Modify: `crates/bridge-api/src/wire.rs`

This is the load-bearing parser. It must tolerate Ollama variance: `tool_calls` fragments may **lack `index`** (use a positional counter), and a turn may finish with `finish_reason:"stop"` **even when tool calls were emitted** (ollama/ollama#7881).

- [ ] **Step 1: Write the failing test**

Append to `crates/bridge-api/src/wire.rs`:
```rust
#[cfg(test)]
mod stream_tests {
    use super::*;

    fn feed(acc: &mut SseAccumulator, lines: &[&str]) {
        for l in lines { acc.push_sse_line(l); }
    }

    #[test]
    fn accumulates_text_deltas() {
        let mut acc = SseAccumulator::default();
        feed(&mut acc, &[
            r#"data: {"choices":[{"delta":{"content":"Hel"},"finish_reason":null}]}"#,
            r#"data: {"choices":[{"delta":{"content":"lo"},"finish_reason":"stop"}]}"#,
            "data: [DONE]",
        ]);
        assert!(acc.is_done());
        let out = acc.finish();
        assert_eq!(out.text, "Hello");
        assert!(out.tool_calls.is_empty());
    }

    #[test]
    fn assembles_indexed_tool_call_fragments() {
        let mut acc = SseAccumulator::default();
        feed(&mut acc, &[
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"get_current_time","arguments":""}}]},"finish_reason":null}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#,
        ]);
        let out = acc.finish();
        assert_eq!(out.tool_calls.len(), 1);
        assert_eq!(out.tool_calls[0].id, "call_1");
        assert_eq!(out.tool_calls[0].function.name, "get_current_time");
        assert_eq!(out.tool_calls[0].function.arguments, "{}");
    }

    #[test]
    fn tolerates_missing_index_and_stop_finish() {
        // ollama/ollama#7881: tool call with NO index, finishing "stop".
        let mut acc = SseAccumulator::default();
        feed(&mut acc, &[
            r#"data: {"choices":[{"delta":{"tool_calls":[{"id":"c9","function":{"name":"get_current_time","arguments":"{}"}}]},"finish_reason":"stop"}]}"#,
        ]);
        let out = acc.finish();
        assert_eq!(out.tool_calls.len(), 1, "tool call assembled despite no index + stop finish");
        assert_eq!(out.tool_calls[0].id, "c9");
    }

    #[test]
    fn ignores_blank_and_non_data_lines() {
        let mut acc = SseAccumulator::default();
        feed(&mut acc, &["", ": keep-alive", r#"data: {"choices":[{"delta":{"content":"x"}}]}"#]);
        assert_eq!(acc.finish().text, "x");
    }

    #[test]
    fn malformed_json_line_is_reported() {
        let mut acc = SseAccumulator::default();
        let err = acc.push_sse_line("data: {not json");
        assert!(err.is_err());
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-api wire::stream_tests`
Expected: FAIL — `SseAccumulator` not found.

- [ ] **Step 3: Implement**

Append to `crates/bridge-api/src/wire.rs` (below the request types, above the test modules):
```rust
use std::collections::BTreeMap;

// ── Streamed response chunk shapes ──────────────────────────────────────────
#[derive(Debug, Deserialize)]
struct StreamChunk { #[serde(default)] choices: Vec<StreamChoice> }
#[derive(Debug, Deserialize)]
struct StreamChoice {
    #[serde(default)] delta: Delta,
    #[serde(default)] finish_reason: Option<String>,
}
#[derive(Debug, Default, Deserialize)]
struct Delta {
    #[serde(default)] content: Option<String>,
    #[serde(default)] tool_calls: Option<Vec<ToolCallFragment>>,
}
#[derive(Debug, Deserialize)]
struct ToolCallFragment {
    #[serde(default)] index: Option<usize>,
    #[serde(default)] id: Option<String>,
    #[serde(default)] function: Option<FunctionFragment>,
}
#[derive(Debug, Default, Deserialize)]
struct FunctionFragment {
    #[serde(default)] name: Option<String>,
    #[serde(default)] arguments: Option<String>,
}

#[derive(Debug, Default, Clone)]
struct PartialToolCall { id: String, name: String, arguments: String }

/// The result of consuming a (streamed or non-streamed) response.
#[derive(Debug, Default)]
pub struct ParsedTurn {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
}

/// Tolerant streamed-SSE accumulator. Buffers tool_call fragments by `index`
/// when present, else by a running positional counter. Treats *any* accumulated
/// tool calls as a tool round regardless of the finish_reason string.
#[derive(Debug, Default)]
pub struct SseAccumulator {
    text: String,
    calls: BTreeMap<usize, PartialToolCall>,
    next_pos: usize,
    done: bool,
    /// Bytes deltas the caller should emit as Update::Text as they arrive.
    pending_text: Vec<String>,
}

impl SseAccumulator {
    /// Feed one raw SSE line (e.g. `data: {...}` or `data: [DONE]`). Returns the
    /// text delta (if any) to surface immediately, or Err on malformed JSON.
    pub fn push_sse_line(&mut self, line: &str) -> Result<Option<String>, ()> {
        let line = line.trim();
        let Some(payload) = line.strip_prefix("data:") else { return Ok(None) };
        let payload = payload.trim();
        if payload.is_empty() { return Ok(None) }
        if payload == "[DONE]" { self.done = true; return Ok(None) }
        let chunk: StreamChunk = serde_json::from_str(payload).map_err(|_| ())?;
        let mut emitted = None;
        for choice in chunk.choices {
            if let Some(c) = choice.delta.content {
                if !c.is_empty() { self.text.push_str(&c); emitted = Some(c); }
            }
            if let Some(frags) = choice.delta.tool_calls {
                for f in frags { self.absorb_fragment(f); }
            }
            if choice.finish_reason.is_some() { self.done = true; }
        }
        Ok(emitted)
    }

    fn absorb_fragment(&mut self, f: ToolCallFragment) {
        let key = match f.index {
            Some(i) => i,
            // No index: a new id starts a new slot, else append to the latest.
            None if f.id.is_some() => { let k = self.next_pos; self.next_pos += 1; k }
            None => self.next_pos.saturating_sub(1),
        };
        if f.index.is_some() { self.next_pos = self.next_pos.max(key + 1); }
        let slot = self.calls.entry(key).or_default();
        if let Some(id) = f.id { slot.id = id; }
        if let Some(func) = f.function {
            if let Some(n) = func.name { slot.name = n; }
            if let Some(a) = func.arguments { slot.arguments.push_str(&a); }
        }
    }

    pub fn is_done(&self) -> bool { self.done }

    pub fn finish(self) -> ParsedTurn {
        let tool_calls = self.calls.into_values()
            .filter(|p| !p.name.is_empty())
            .map(|p| ToolCall {
                id: if p.id.is_empty() { "call_0".into() } else { p.id },
                kind: "function".into(),
                function: FunctionCall { name: p.name, arguments: p.arguments },
            })
            .collect();
        ParsedTurn { text: self.text, tool_calls }
    }
}
```

(`pending_text` is declared for symmetry with the backend's streaming use; the backend surfaces deltas from `push_sse_line`'s return value, so remove `pending_text` if clippy flags it as unused — the field is not required by these tests.)

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p bridge-api wire::stream_tests`
Expected: PASS (5 tests). If clippy later flags `pending_text` as dead, delete that field + its initializer.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-api/src/wire.rs
git commit -m "feat(api): tolerant SSE tool_call accumulator"
```

---

## Task 5: Non-streamed (`stream:false`) parse path

**Files:**
- Modify: `crates/bridge-api/src/wire.rs`

A non-streamed response is a different shape (`choices[0].message.tool_calls`), NOT an SSE stream — a separate parse path (spec §5).

- [ ] **Step 1: Write the failing test**

Append to `crates/bridge-api/src/wire.rs`:
```rust
#[cfg(test)]
mod nonstream_tests {
    use super::*;
    #[test]
    fn parses_message_tool_calls_shape() {
        let body = r#"{"choices":[{"message":{"content":null,"tool_calls":[
            {"id":"call_1","type":"function","function":{"name":"get_current_time","arguments":"{}"}}]},
            "finish_reason":"tool_calls"}]}"#;
        let out = parse_nonstream(body).unwrap();
        assert_eq!(out.tool_calls.len(), 1);
        assert_eq!(out.tool_calls[0].id, "call_1");
        assert!(out.text.is_empty());
    }
    #[test]
    fn parses_plain_text_message() {
        let body = r#"{"choices":[{"message":{"content":"hello"},"finish_reason":"stop"}]}"#;
        let out = parse_nonstream(body).unwrap();
        assert_eq!(out.text, "hello");
        assert!(out.tool_calls.is_empty());
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-api wire::nonstream_tests`
Expected: FAIL — `parse_nonstream` not found.

- [ ] **Step 3: Implement**

Append to `crates/bridge-api/src/wire.rs` (above the test modules):
```rust
#[derive(Debug, Deserialize)]
struct NonStreamResponse { #[serde(default)] choices: Vec<NonStreamChoice> }
#[derive(Debug, Deserialize)]
struct NonStreamChoice { message: RespMessage }
#[derive(Debug, Deserialize)]
struct RespMessage {
    #[serde(default)] content: Option<String>,
    #[serde(default)] tool_calls: Option<Vec<ToolCall>>,
}

/// Parse a non-streamed (`stream:false`) chat completion body. Returns Err on
/// malformed JSON (mapped to FrameError by the backend).
pub fn parse_nonstream(body: &str) -> Result<ParsedTurn, ()> {
    let resp: NonStreamResponse = serde_json::from_str(body).map_err(|_| ())?;
    let mut out = ParsedTurn::default();
    if let Some(choice) = resp.choices.into_iter().next() {
        out.text = choice.message.content.unwrap_or_default();
        out.tool_calls = choice.message.tool_calls.unwrap_or_default();
    }
    Ok(out)
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p bridge-api wire::nonstream_tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-api/src/wire.rs
git commit -m "feat(api): non-streamed message.tool_calls parse path"
```

---

## Task 6: `ApiBackend` skeleton + trait wiring (no network yet)

**Files:**
- Modify: `crates/bridge-api/src/backend.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/bridge-api/src/backend.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::{EffectiveConfig, PermissionDecision, PermissionRequest, SessionContext};
    use bridge_core::ids::SessionId;
    use bridge_core::ports::{AgentBackend, PolicyEngine};
    use bridge_core::error::BridgeError;
    use std::sync::Arc;

    struct DenyAll;
    impl PolicyEngine for DenyAll {
        fn decide(&self, _: &PermissionRequest, _: &SessionContext) -> Result<PermissionDecision, BridgeError> {
            Err(BridgeError::PermissionDenied)
        }
    }

    #[tokio::test]
    async fn configure_session_stashes_model_and_object_safe() {
        let be = ApiBackend::new(crate::config::ApiConfig::new("http://127.0.0.1:1"));
        let s = SessionId::parse("s1").unwrap();
        be.configure_session(&s, &EffectiveConfig { model: Some("haiku".into()), ..Default::default() })
            .await.unwrap();
        assert_eq!(be.session_model(&s).as_deref(), Some("haiku"));
        be.forget_session(&s).await;
        assert!(be.session_model(&s).is_none());
        let _obj: Arc<dyn AgentBackend> = Arc::new(ApiBackend::new(crate::config::ApiConfig::new("http://127.0.0.1:1")));
    }

    #[tokio::test]
    async fn with_policy_swaps_engine() {
        let be = ApiBackend::new(crate::config::ApiConfig::new("http://127.0.0.1:1")).with_policy(Arc::new(DenyAll));
        // Exercised end-to-end in the wiremock deny test; here just assert it builds + is Send/Sync.
        fn assert_send_sync<T: Send + Sync>(_: &T) {}
        assert_send_sync(&be);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-api backend::`
Expected: FAIL — `ApiBackend` not found.

- [ ] **Step 3: Implement**

Replace the placeholder in `crates/bridge-api/src/backend.rs` (above the test module):
```rust
//! ApiBackend — the non-process OpenAI-compatible AgentBackend.
use crate::config::ApiConfig;
use bridge_core::domain::{EffectiveConfig, Part, PermissionDecision, PermissionRequest, SessionContext};
use bridge_core::error::BridgeError;
use bridge_core::ids::SessionId;
use bridge_core::ports::{AgentBackend, BackendStream, PolicyEngine};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

#[derive(Default)]
struct SessionState {
    model: Option<String>,
    cancel: Arc<AtomicBool>,
}

pub struct ApiBackend {
    cfg: ApiConfig,
    client: reqwest::Client,
    policy: Arc<StdMutex<Arc<dyn PolicyEngine>>>,
    sessions: Arc<StdMutex<HashMap<SessionId, SessionState>>>,
}

/// Default policy: approve everything (mirrors AcpBackend's default auto-approver).
struct AutoApprove;
impl PolicyEngine for AutoApprove {
    fn decide(&self, _: &PermissionRequest, _: &SessionContext) -> Result<PermissionDecision, BridgeError> {
        Ok(PermissionDecision::Approve)
    }
}

impl ApiBackend {
    pub fn new(cfg: ApiConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(cfg.request_timeout)
            .build()
            .expect("reqwest client builds");
        Self {
            cfg,
            client,
            policy: Arc::new(StdMutex::new(Arc::new(AutoApprove) as Arc<dyn PolicyEngine>)),
            sessions: Arc::new(StdMutex::new(HashMap::new())),
        }
    }

    #[must_use]
    pub fn with_policy(self, policy: Arc<dyn PolicyEngine>) -> Self {
        if let Ok(mut p) = self.policy.lock() { *p = policy; }
        self
    }

    /// Test/inspection helper: the stashed effective model for a session.
    pub fn session_model(&self, s: &SessionId) -> Option<String> {
        self.sessions.lock().ok()?.get(s).and_then(|st| st.model.clone())
    }

    fn cancel_latch(&self, s: &SessionId) -> Arc<AtomicBool> {
        let mut map = self.sessions.lock().expect("sessions lock");
        map.entry(s.clone()).or_default().cancel.clone()
    }
}

#[async_trait::async_trait]
impl AgentBackend for ApiBackend {
    async fn prompt(&self, _session: &SessionId, _parts: Vec<Part>) -> Result<BackendStream, BridgeError> {
        // Filled in Task 7.
        Err(BridgeError::AgentCrashed)
    }

    async fn cancel(&self, session: &SessionId) -> Result<(), BridgeError> {
        self.cancel_latch(session).store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn configure_session(&self, session: &SessionId, cfg: &EffectiveConfig) -> Result<(), BridgeError> {
        let mut map = self.sessions.lock().expect("sessions lock");
        map.entry(session.clone()).or_default().model = cfg.model.clone();
        Ok(())
    }

    async fn forget_session(&self, session: &SessionId) {
        if let Ok(mut map) = self.sessions.lock() { map.remove(session); }
    }
}
```

> `Update` and `STOP_REASON_CANCELLED` are deliberately NOT imported in this task — `prompt` is a stub here, so importing them would trip `-D warnings`. Task 7 adds them to the `use bridge_core::ports::{…}` line when it implements the turn loop.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p bridge-api backend::` then `cargo clippy -p bridge-api --all-targets -- -D warnings`
Expected: PASS + clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-api/src/backend.rs
git commit -m "feat(api): ApiBackend skeleton (configure_session/cancel/policy)"
```

---

## Task 7: Turn loop — text round-trip (DoD-1)

**Files:**
- Modify: `crates/bridge-api/src/backend.rs`
- Create: `crates/bridge-api/tests/wiremock_turns.rs`

Implement `prompt()` for the no-tool case: POST, stream-parse, emit `Text` deltas + `Done`. The stream polls the cancel latch each chunk.

- [ ] **Step 1: Write the failing test**

Create `crates/bridge-api/tests/wiremock_turns.rs`:
```rust
use bridge_api::{ApiBackend, ApiConfig};
use bridge_core::domain::Part;
use bridge_core::ids::SessionId;
use bridge_core::ports::{AgentBackend, Update};
use futures::StreamExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn sse(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).insert_header("content-type", "text/event-stream").set_body_string(body)
}

async fn drain(be: &ApiBackend, s: &SessionId) -> Vec<Update> {
    let mut st = be.prompt(s, vec![Part { text: "hi".into() }]).await.unwrap();
    let mut out = Vec::new();
    while let Some(item) = st.next().await { out.push(item.unwrap()); }
    out
}

#[tokio::test]
async fn text_round_trip_yields_text_then_done_no_permission() {
    let server = MockServer::start().await;
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n\
                data: {\"choices\":[{\"delta\":{\"content\":\" world\"},\"finish_reason\":\"stop\"}]}\n\n\
                data: [DONE]\n\n";
    Mock::given(method("POST")).and(path("/v1/chat/completions"))
        .respond_with(sse(body)).mount(&server).await;

    let be = ApiBackend::new(ApiConfig::new(format!("{}/v1", server.uri())));
    let updates = drain(&be, &SessionId::parse("s1").unwrap()).await;

    let text: String = updates.iter().filter_map(|u| if let Update::Text(t) = u { Some(t.clone()) } else { None }).collect();
    assert_eq!(text, "Hello world");
    assert!(matches!(updates.last(), Some(Update::Done { stop_reason }) if stop_reason == "stop"));
    assert!(!updates.iter().any(|u| matches!(u, Update::Permission(_))), "API backend NEVER yields Permission");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-api --test wiremock_turns text_round_trip`
Expected: FAIL — `prompt` returns `AgentCrashed`.

- [ ] **Step 3: Implement**

In `crates/bridge-api/src/backend.rs`: (1) extend the ports import to `use bridge_core::ports::{AgentBackend, BackendStream, PolicyEngine, Update, STOP_REASON_CANCELLED};`; (2) add the `use` lines below; (3) replace the entire Task-6 `impl AgentBackend for ApiBackend` block (the one whose `prompt` returns `Err(AgentCrashed)`) with the full one below. Full `prompt` + helpers:
```rust
use crate::wire::{ChatRequest, Message, SseAccumulator, ToolCall};
use futures::StreamExt;

impl ApiBackend {
    fn resolve_api_key(&self) -> Option<String> {
        self.cfg.api_key_env.as_ref().and_then(|var| std::env::var(var).ok())
    }
    fn resolve_model(&self, s: &SessionId) -> Option<String> {
        self.session_model(s).or_else(|| self.cfg.model.clone())
    }
}

#[async_trait::async_trait]
impl AgentBackend for ApiBackend {
    async fn prompt(&self, session: &SessionId, parts: Vec<Part>) -> Result<BackendStream, BridgeError> {
        let url = format!("{}/chat/completions", self.cfg.base_url.trim_end_matches('/'));
        let model = self.resolve_model(session);
        let api_key = self.resolve_api_key();
        let cancel = self.cancel_latch(session);
        cancel.store(false, Ordering::SeqCst); // fresh turn
        let client = self.client.clone();
        let policy = self.policy.clone();
        let max_rounds = self.cfg.max_tool_rounds;

        let mut messages: Vec<Message> = vec![Message::user(
            parts.iter().map(|p| p.text.as_str()).collect::<Vec<_>>().join("\n"),
        )];

        let stream = async_stream::try_stream! {
            for _round in 0..max_rounds {
                if cancel.load(Ordering::SeqCst) {
                    yield Update::Done { stop_reason: STOP_REASON_CANCELLED.into() };
                    return;
                }
                let req = ChatRequest { model: model.clone(), messages: messages.clone(),
                    tools: vec![crate::tool::tool_def()], stream: true };
                let mut builder = client.post(&url).json(&req);
                if let Some(k) = &api_key { builder = builder.bearer_auth(k); }
                let resp = builder.send().await.map_err(|_| BridgeError::AgentCrashed)?;
                if !resp.status().is_success() { Err(BridgeError::AgentCrashed)?; }

                let mut acc = SseAccumulator::default();
                let mut bytes = resp.bytes_stream();
                let mut buf = String::new();
                'read: while let Some(chunk) = bytes.next().await {
                    if cancel.load(Ordering::SeqCst) {
                        yield Update::Done { stop_reason: STOP_REASON_CANCELLED.into() };
                        return;
                    }
                    let chunk = chunk.map_err(|_| BridgeError::AgentCrashed)?;
                    buf.push_str(&String::from_utf8_lossy(&chunk));
                    while let Some(nl) = buf.find('\n') {
                        let line: String = buf.drain(..=nl).collect();
                        match acc.push_sse_line(&line) {
                            Ok(Some(text)) => { yield Update::Text(text); }
                            Ok(None) => {}
                            Err(()) => { Err(BridgeError::FrameError)?; }
                        }
                        if acc.is_done() { break 'read; }
                    }
                }
                let parsed = acc.finish();
                if parsed.tool_calls.is_empty() {
                    yield Update::Done { stop_reason: "stop".into() };
                    return;
                }
                // Tool round — Task 8 fills this. For now (text-only milestone) end.
                let _ = (&policy, &messages); // silence until Task 8
                yield Update::Done { stop_reason: "stop".into() };
                return;
            }
            yield Update::Done { stop_reason: "max_tool_rounds".into() };
        };
        Ok(Box::pin(stream))
    }

    async fn cancel(&self, session: &SessionId) -> Result<(), BridgeError> {
        self.cancel_latch(session).store(true, Ordering::SeqCst);
        Ok(())
    }
    async fn configure_session(&self, session: &SessionId, cfg: &EffectiveConfig) -> Result<(), BridgeError> {
        let mut map = self.sessions.lock().expect("sessions lock");
        map.entry(session.clone()).or_default().model = cfg.model.clone();
        Ok(())
    }
    async fn forget_session(&self, session: &SessionId) {
        if let Ok(mut map) = self.sessions.lock() { map.remove(session); }
    }
}
```

(Replace the whole Task-6 `impl AgentBackend` block with this one — there is exactly one `impl AgentBackend for ApiBackend`.)

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p bridge-api --test wiremock_turns text_round_trip`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-api/src/backend.rs crates/bridge-api/tests/wiremock_turns.rs
git commit -m "feat(api): prompt() text round-trip over SSE (DoD-1)"
```

---

## Task 8: Tool loop — silent approve path (DoD-2)

**Files:**
- Modify: `crates/bridge-api/src/backend.rs`
- Modify: `crates/bridge-api/tests/wiremock_turns.rs`

Replace the placeholder tool branch with the **silent** policy decision + tool execution + follow-up POST. The backend NEVER yields `Update::Permission`.

- [ ] **Step 1: Write the failing test**

Append to `crates/bridge-api/tests/wiremock_turns.rs`:
```rust
use wiremock::matchers::body_string_contains;

#[tokio::test]
async fn tool_approve_path_executes_and_feeds_result() {
    let server = MockServer::start().await;
    // Call 1: a tool_call. Call 2 (the follow-up that carries the tool result): final text.
    let call1 = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"get_current_time\",\"arguments\":\"{}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n\
                 data: [DONE]\n\n";
    let call2 = "data: {\"choices\":[{\"delta\":{\"content\":\"It is 2026.\"},\"finish_reason\":\"stop\"}]}\n\n\
                 data: [DONE]\n\n";
    // The follow-up request is the only one whose body contains the tool result string.
    Mock::given(method("POST")).and(path("/v1/chat/completions"))
        .and(body_string_contains("2026-01-01T00:00:00Z"))
        .respond_with(sse(call2)).up_to_n_times(1).mount(&server).await;
    Mock::given(method("POST")).and(path("/v1/chat/completions"))
        .respond_with(sse(call1)).mount(&server).await;

    let be = ApiBackend::new(ApiConfig::new(format!("{}/v1", server.uri()))); // default = auto-approve
    let updates = drain(&be, &SessionId::parse("s2").unwrap()).await;

    let text: String = updates.iter().filter_map(|u| if let Update::Text(t) = u { Some(t.clone()) } else { None }).collect();
    assert_eq!(text, "It is 2026.");
    assert!(matches!(updates.last(), Some(Update::Done { stop_reason }) if stop_reason == "stop"));
    assert!(!updates.iter().any(|u| matches!(u, Update::Permission(_))));

    // Falsifiable: the follow-up request carried the EXACT tool-result message.
    let reqs = server.received_requests().await.unwrap();
    let second = String::from_utf8_lossy(&reqs[1].body);
    assert!(second.contains("\"role\":\"tool\""));
    assert!(second.contains("\"tool_call_id\":\"call_1\""));
    assert!(second.contains("2026-01-01T00:00:00Z"));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-api --test wiremock_turns tool_approve_path`
Expected: FAIL — the placeholder ends after one round, no follow-up / no tool result.

- [ ] **Step 3: Implement**

In `crates/bridge-api/src/backend.rs`, replace the placeholder tool branch (the `// Tool round — Task 8 fills this` block) with:
```rust
                // Tool round: decide each call SILENTLY via the injected policy.
                // We yield NO Update::Permission — the backend is the sole authority
                // (mirrors AcpBackend::decide_permission; see spec §4.3).
                messages.push(Message::assistant_tool_calls(parsed.tool_calls.clone()));
                for tc in &parsed.tool_calls {
                    let result = decide_tool(&policy, tc);
                    messages.push(Message::tool_result(tc.id.clone(), result));
                }
                // loop: re-POST with the appended tool results.
```

Add the free function `decide_tool` at module level in `backend.rs` (below the `impl AgentBackend`):
```rust
/// Silent permission decision for one tool call → the `content` of its tool-result
/// message. Approve runs the stub tool; Deny/abstain feed a refusal string.
fn decide_tool(policy: &Arc<StdMutex<Arc<dyn PolicyEngine>>>, tc: &ToolCall) -> String {
    let req = PermissionRequest::with_id(tc.id.clone(), /*interactive=*/ false);
    let decision = policy.lock().ok().map(|p| p.decide(&req, &SessionContext));
    match decision {
        Some(Ok(PermissionDecision::Approve)) => {
            if tc.function.name == crate::tool::TOOL_NAME { crate::tool::run_tool() }
            else { format!("unknown tool: {}", tc.function.name) }
        }
        Some(Err(BridgeError::PermissionDenied)) => "permission denied: tool not executed".into(),
        _ /* abstain / poisoned */ => "permission unavailable: tool not executed".into(),
    }
}
```

Remove the now-unused `let _ = (&policy, &messages);` line.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p bridge-api --test wiremock_turns`
Expected: PASS (both turn tests).

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-api/src/backend.rs crates/bridge-api/tests/wiremock_turns.rs
git commit -m "feat(api): silent tool-call policy decision + approve path (DoD-2)"
```

---

## Task 9: Deny + abstain arms, direct-prompt (DoD-3 direct variant, DoD-5b abstain)

**Files:**
- Modify: `crates/bridge-api/tests/wiremock_turns.rs`

The logic exists (Task 8 `decide_tool`); this task proves the deny/abstain arms at the backend level. (The through-translator deny test is Task 12.)

- [ ] **Step 1: Write the failing test**

Append to `crates/bridge-api/tests/wiremock_turns.rs`:
```rust
use bridge_core::domain::{PermissionDecision, PermissionRequest, SessionContext};
use bridge_core::error::BridgeError;
use bridge_core::ports::PolicyEngine;
use std::sync::Arc;

struct Deny;
impl PolicyEngine for Deny {
    fn decide(&self, _: &PermissionRequest, _: &SessionContext) -> Result<PermissionDecision, BridgeError> {
        Err(BridgeError::PermissionDenied)
    }
}
struct Abstain;
impl PolicyEngine for Abstain {
    fn decide(&self, _: &PermissionRequest, _: &SessionContext) -> Result<PermissionDecision, BridgeError> {
        Err(BridgeError::FrameError) // any non-PermissionDenied Err = abstain
    }
}

async fn tool_then_text(server: &MockServer) {
    let call1 = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"get_current_time\",\"arguments\":\"{}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n";
    let call2 = "data: {\"choices\":[{\"delta\":{\"content\":\"done\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    Mock::given(method("POST")).and(path("/v1/chat/completions"))
        .and(body_string_contains("\"role\":\"tool\""))
        .respond_with(sse(call2)).up_to_n_times(1).mount(server).await;
    Mock::given(method("POST")).and(path("/v1/chat/completions")).respond_with(sse(call1)).mount(server).await;
}

#[tokio::test]
async fn deny_arm_feeds_denial_and_does_not_run_tool() {
    let server = MockServer::start().await;
    tool_then_text(&server).await;
    let be = ApiBackend::new(ApiConfig::new(format!("{}/v1", server.uri()))).with_policy(Arc::new(Deny));
    let _ = drain(&be, &SessionId::parse("s3").unwrap()).await;
    let reqs = server.received_requests().await.unwrap();
    let second = String::from_utf8_lossy(&reqs[1].body);
    assert!(second.contains("permission denied: tool not executed"));
    assert!(!second.contains("2026-01-01T00:00:00Z"), "stub tool MUST NOT have run");
}

#[tokio::test]
async fn abstain_arm_feeds_refusal() {
    let server = MockServer::start().await;
    tool_then_text(&server).await;
    let be = ApiBackend::new(ApiConfig::new(format!("{}/v1", server.uri()))).with_policy(Arc::new(Abstain));
    let _ = drain(&be, &SessionId::parse("s4").unwrap()).await;
    let reqs = server.received_requests().await.unwrap();
    let second = String::from_utf8_lossy(&reqs[1].body);
    assert!(second.contains("permission unavailable: tool not executed"));
}
```

- [ ] **Step 2: Run to verify it fails, then passes**

Run: `cargo test -p bridge-api --test wiremock_turns deny_arm` then `abstain_arm`
Expected: Both PASS immediately (logic already implemented in Task 8). If they fail, fix `decide_tool`.

- [ ] **Step 3: Commit**

```bash
git add crates/bridge-api/tests/wiremock_turns.rs
git commit -m "test(api): deny + abstain tool arms at backend level (DoD-5b)"
```

---

## Task 10: Cancel mid-stream + errors (DoD-4, DoD-5, DoD-5b malformed)

**Files:**
- Modify: `crates/bridge-api/tests/wiremock_turns.rs`

- [ ] **Step 1: Write the failing tests**

Append to `crates/bridge-api/tests/wiremock_turns.rs`:
```rust
use std::time::Duration;

#[tokio::test]
async fn mid_stream_cancel_ends_with_cancelled() {
    let server = MockServer::start().await;
    // Slow body: a chunk, then a long delay before the terminal — cancel mid-flight.
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"},\"finish_reason\":null}]}\n\n";
    Mock::given(method("POST")).and(path("/v1/chat/completions"))
        .respond_with(sse(body).set_delay(Duration::from_millis(50))).mount(&server).await;

    let be = Arc::new(ApiBackend::new(ApiConfig::new(format!("{}/v1", server.uri()))));
    let s = SessionId::parse("s5").unwrap();
    let be2 = be.clone(); let s2 = s.clone();
    let mut st = be.prompt(&s, vec![Part { text: "hi".into() }]).await.unwrap();
    tokio::spawn(async move { tokio::time::sleep(Duration::from_millis(10)).await; be2.cancel(&s2).await.unwrap(); });
    let mut last = None;
    while let Some(item) = st.next().await { last = Some(item.unwrap()); }
    assert!(matches!(last, Some(Update::Done { stop_reason }) if stop_reason == "cancelled"));
}

#[tokio::test]
async fn http_500_is_agent_crashed() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(500)).mount(&server).await;
    let be = ApiBackend::new(ApiConfig::new(format!("{}/v1", server.uri())));
    let mut st = be.prompt(&SessionId::parse("s6").unwrap(), vec![Part { text: "hi".into() }]).await.unwrap();
    let mut err = None;
    while let Some(item) = st.next().await { if let Err(e) = item { err = Some(e); } }
    assert!(matches!(err, Some(bridge_core::error::BridgeError::AgentCrashed)));
}

#[tokio::test]
async fn malformed_sse_is_frame_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/v1/chat/completions"))
        .respond_with(sse("data: {not valid json\n\n")).mount(&server).await;
    let be = ApiBackend::new(ApiConfig::new(format!("{}/v1", server.uri())));
    let mut st = be.prompt(&SessionId::parse("s7").unwrap(), vec![Part { text: "hi".into() }]).await.unwrap();
    let mut err = None;
    while let Some(item) = st.next().await { if let Err(e) = item { err = Some(e); } }
    assert!(matches!(err, Some(bridge_core::error::BridgeError::FrameError)));
}
```

- [ ] **Step 2: Run to verify**

Run: `cargo test -p bridge-api --test wiremock_turns`
Expected: PASS. (The cancel-poll-inside-loop, `AgentCrashed`, and `FrameError` paths were implemented in Task 7. If `connection-refused` coverage is wanted too, add a test pointing at `ApiConfig::new("http://127.0.0.1:1/v1")` asserting `AgentCrashed`.)

- [ ] **Step 3: Add the connection-refused case + commit**

Append:
```rust
#[tokio::test]
async fn connection_refused_is_agent_crashed() {
    let be = ApiBackend::new(ApiConfig::new("http://127.0.0.1:1/v1"));
    let mut st = be.prompt(&SessionId::parse("s8").unwrap(), vec![Part { text: "hi".into() }]).await.unwrap();
    let mut err = None;
    while let Some(item) = st.next().await { if let Err(e) = item { err = Some(e); } }
    assert!(matches!(err, Some(bridge_core::error::BridgeError::AgentCrashed)));
}
```
Run: `cargo test -p bridge-api --test wiremock_turns` → PASS.
```bash
git add crates/bridge-api/tests/wiremock_turns.rs
git commit -m "test(api): mid-stream cancel + HTTP/frame errors (DoD-4/5/5b)"
```

---

## Task 11: `stream:false` path used by the backend

**Files:**
- Modify: `crates/bridge-api/src/backend.rs`
- Modify: `crates/bridge-api/tests/wiremock_turns.rs`

Wire `cfg.stream == false` to a single non-streamed POST using `parse_nonstream` (Task 5).

- [ ] **Step 1: Write the failing test**

Append to `crates/bridge-api/tests/wiremock_turns.rs`:
```rust
#[tokio::test]
async fn nonstream_mode_text_round_trip() {
    let server = MockServer::start().await;
    let body = r#"{"choices":[{"message":{"content":"plain text"},"finish_reason":"stop"}]}"#;
    Mock::given(method("POST")).and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).insert_header("content-type","application/json").set_body_string(body))
        .mount(&server).await;
    let mut cfg = ApiConfig::new(format!("{}/v1", server.uri())); cfg.stream = false;
    let be = ApiBackend::new(cfg);
    let updates = drain(&be, &SessionId::parse("s9").unwrap()).await;
    let text: String = updates.iter().filter_map(|u| if let Update::Text(t)=u {Some(t.clone())} else {None}).collect();
    assert_eq!(text, "plain text");
    assert!(matches!(updates.last(), Some(Update::Done{stop_reason}) if stop_reason=="stop"));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-api --test wiremock_turns nonstream_mode`
Expected: FAIL — backend always streams (`stream:true`), the JSON body isn't SSE.

- [ ] **Step 3: Implement**

In `prompt()`'s loop, branch at the POST on `self.cfg.stream`. Capture `let do_stream = self.cfg.stream;` before the `async_stream!` block, then inside the loop replace the response-reading section with:
```rust
                let req = ChatRequest { model: model.clone(), messages: messages.clone(),
                    tools: vec![crate::tool::tool_def()], stream: do_stream };
                let mut builder = client.post(&url).json(&req);
                if let Some(k) = &api_key { builder = builder.bearer_auth(k); }
                let resp = builder.send().await.map_err(|_| BridgeError::AgentCrashed)?;
                if !resp.status().is_success() { Err(BridgeError::AgentCrashed)?; }

                let parsed = if do_stream {
                    let mut acc = SseAccumulator::default();
                    let mut bytes = resp.bytes_stream();
                    let mut buf = String::new();
                    'read: while let Some(chunk) = bytes.next().await {
                        if cancel.load(Ordering::SeqCst) {
                            yield Update::Done { stop_reason: STOP_REASON_CANCELLED.into() }; return;
                        }
                        let chunk = chunk.map_err(|_| BridgeError::AgentCrashed)?;
                        buf.push_str(&String::from_utf8_lossy(&chunk));
                        while let Some(nl) = buf.find('\n') {
                            let line: String = buf.drain(..=nl).collect();
                            match acc.push_sse_line(&line) {
                                Ok(Some(text)) => { yield Update::Text(text); }
                                Ok(None) => {}
                                Err(()) => { Err(BridgeError::FrameError)?; }
                            }
                            if acc.is_done() { break 'read; }
                        }
                    }
                    acc.finish()
                } else {
                    let body = resp.text().await.map_err(|_| BridgeError::AgentCrashed)?;
                    let parsed = crate::wire::parse_nonstream(&body).map_err(|_| BridgeError::FrameError)?;
                    if !parsed.text.is_empty() { yield Update::Text(parsed.text.clone()); }
                    parsed
                };
```
(Remove the old streaming-only block this replaces.)

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p bridge-api` (whole crate)
Expected: PASS (all backend + wire tests).

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-api/src/backend.rs crates/bridge-api/tests/wiremock_turns.rs
git commit -m "feat(api): stream:false non-streamed fallback path"
```

---

## Task 12: Deny through `Translator::run` — the B1-catching test (DoD-3)

**Files:**
- Create: `crates/bridge-api/tests/deny_through_translator.rs`

This is the test rev1 lacked: it drives the api turn through the real translator with a deny policy and asserts **no pending is persisted and the run completes** (the silent decision does NOT trip the translator's `Err`→suspend path).

- [ ] **Step 1: Write the failing test**

Create `crates/bridge-api/tests/deny_through_translator.rs`:
```rust
use bridge_api::{ApiBackend, ApiConfig};
use bridge_core::domain::{Part, PendingRequest, PeerTaskId, PermissionDecision, PermissionRequest, SessionContext};
use bridge_core::error::BridgeError;
use bridge_core::ids::{SessionId, TaskId};
use bridge_core::ports::{PolicyEngine, SessionStore};
use bridge_core::translator::Translator;
use futures::StreamExt;
use std::sync::{Arc, Mutex};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// Minimal in-test store (mirrors the bridge-core ports.rs FakeStore).
#[derive(Default)]
struct FakeStore { pending: Mutex<std::collections::HashMap<String, PendingRequest>> }
#[async_trait::async_trait]
impl SessionStore for FakeStore {
    async fn put(&self, _: &TaskId, _: &SessionId) -> Result<(), BridgeError> { Ok(()) }
    async fn session_for(&self, _: &TaskId) -> Result<Option<SessionId>, BridgeError> { Ok(None) }
    async fn put_pending(&self, t: &TaskId, r: &PendingRequest) -> Result<(), BridgeError> {
        self.pending.lock().unwrap().insert(t.as_str().into(), r.clone()); Ok(())
    }
    async fn take_pending(&self, t: &TaskId) -> Result<Option<PendingRequest>, BridgeError> {
        Ok(self.pending.lock().unwrap().remove(t.as_str()))
    }
    async fn set_peer_task(&self, _: &TaskId, _: &PeerTaskId) -> Result<(), BridgeError> { Ok(()) }
    async fn peer_task_for(&self, _: &TaskId) -> Result<Option<PeerTaskId>, BridgeError> { Ok(None) }
    async fn request_cancel(&self, _: &TaskId) -> Result<(), BridgeError> { Ok(()) }
    async fn cancel_requested(&self, _: &TaskId) -> Result<bool, BridgeError> { Ok(false) }
    async fn set_fanout(&self, _: &TaskId) -> Result<(), BridgeError> { Ok(()) }
    async fn is_fanout(&self, _: &TaskId) -> Result<bool, BridgeError> { Ok(false) }
}

struct Deny;
impl PolicyEngine for Deny {
    fn decide(&self, _: &PermissionRequest, _: &SessionContext) -> Result<PermissionDecision, BridgeError> {
        Err(BridgeError::PermissionDenied)
    }
}

#[tokio::test]
async fn deny_through_translator_does_not_suspend() {
    let server = MockServer::start().await;
    let sse = |b: &str| ResponseTemplate::new(200).insert_header("content-type","text/event-stream").set_body_string(b);
    let call1 = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"get_current_time\",\"arguments\":\"{}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n";
    let call2 = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    use wiremock::matchers::body_string_contains;
    Mock::given(method("POST")).and(path("/v1/chat/completions")).and(body_string_contains("\"role\":\"tool\""))
        .respond_with(sse(call2)).up_to_n_times(1).mount(&server).await;
    Mock::given(method("POST")).and(path("/v1/chat/completions")).respond_with(sse(call1)).mount(&server).await;

    // The SAME deny policy is threaded into both the backend AND the translator (as main.rs does).
    let backend = ApiBackend::new(ApiConfig::new(format!("{}/v1", server.uri()))).with_policy(Arc::new(Deny));
    let store = FakeStore::default();
    let policy = Deny;
    let task = TaskId::parse("t1").unwrap();
    let session = SessionId::parse("s1").unwrap();

    let events: Vec<_> = Translator::new()
        .run(&backend, &store, &policy, &task, &session, vec![Part { text: "what time is it".into() }])
        .collect().await;

    // 1) The run COMPLETED — it did NOT yield PermissionRequired (no suspend).
    assert!(events.iter().all(|e| e.is_ok()), "translator must not error/suspend: {events:?}");
    // 2) No pending permission was persisted.
    assert!(store.take_pending(&task).await.unwrap().is_none(), "no pending — backend decided silently");
    // 3) The deny reached the model as a tool result; the stub tool did not run.
    let reqs = server.received_requests().await.unwrap();
    let second = String::from_utf8_lossy(&reqs[1].body);
    assert!(second.contains("permission denied: tool not executed"));
    assert!(!second.contains("2026-01-01T00:00:00Z"));
}
```

> Cross-check the `SessionStore` trait method list against `crates/bridge-core/src/ports.rs` (the `FakeStore` there is the canonical shape) and adjust if the trait has changed.

- [ ] **Step 2: Run to verify it fails, then passes**

Run: `cargo test -p bridge-api --test deny_through_translator`
Expected: PASS (with the silent-decision design). If it yields `PermissionRequired` or persists pending, the backend is wrongly emitting `Update::Permission` — fix `prompt()` to never yield it.

- [ ] **Step 3: Commit**

```bash
git add crates/bridge-api/tests/deny_through_translator.rs
git commit -m "test(api): deny through Translator::run never suspends (DoD-3)"
```

---

## Task 13: REAL-CAPTURE fixture + replay-through-parser (DoD-6)

**Files:**
- Create: `crates/bridge-api/tests/fixtures/ollama-openai-compat.json`
- Create: `crates/bridge-api/tests/corpus_replay.rs`

The captured frames are the SINGLE SOURCE of the wiremock stub bodies AND are replayed through the real parser (not a bare presence check).

- [ ] **Step 1: Capture real frames (manual, documented)**

Run a real Ollama once and save raw frames. Documented command (engineer runs locally; if Ollama is unavailable, hand-author the fixture from the §5 shapes and mark `_provenance` accordingly, but prefer real):
```bash
# requires: ollama serve + ollama pull qwen3.5:9b
curl -sN http://localhost:11434/v1/chat/completions -H 'content-type: application/json' \
  -d '{"model":"qwen3.5:9b","stream":true,"messages":[{"role":"user","content":"say PONG"}]}' \
  | tee /tmp/api-text.sse
```
Create `crates/bridge-api/tests/fixtures/ollama-openai-compat.json`:
```json
{
  "_provenance": "REAL-CAPTURE",
  "agent": "ollama-openai-compat",
  "model": "qwen3.5:9b",
  "captured": "2026-06-01",
  "captured_by": "api-backend capture (curl /v1/chat/completions)",
  "text_turn_sse": "data: {\"choices\":[{\"delta\":{\"content\":\"PONG\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
  "tool_turn_sse": "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"get_current_time\",\"arguments\":\"{}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n"
}
```
(Replace `text_turn_sse`/`tool_turn_sse` with the actual captured bytes.)

- [ ] **Step 2: Write the failing test**

Create `crates/bridge-api/tests/corpus_replay.rs`:
```rust
use bridge_api::wire::SseAccumulator;
use serde_json::Value;

fn fixture() -> Value {
    let raw = include_str!("fixtures/ollama-openai-compat.json");
    serde_json::from_str(raw).expect("fixture is valid JSON")
}

#[test]
fn fixture_is_real_capture() {
    assert_eq!(fixture()["_provenance"], "REAL-CAPTURE");
    assert_eq!(fixture()["model"], "qwen3.5:9b");
}

#[test]
fn captured_text_turn_replays_through_parser() {
    let sse = fixture()["text_turn_sse"].as_str().unwrap().to_string();
    let mut acc = SseAccumulator::default();
    for line in sse.split('\n') { let _ = acc.push_sse_line(line); }
    let out = acc.finish();
    assert!(!out.text.is_empty(), "captured text turn must parse to non-empty text");
}

#[test]
fn captured_tool_turn_replays_to_a_tool_call() {
    let sse = fixture()["tool_turn_sse"].as_str().unwrap().to_string();
    let mut acc = SseAccumulator::default();
    for line in sse.split('\n') { let _ = acc.push_sse_line(line); }
    let out = acc.finish();
    assert_eq!(out.tool_calls.len(), 1);
    assert_eq!(out.tool_calls[0].function.name, "get_current_time");
}
```

This requires `SseAccumulator` to be reachable as `bridge_api::wire::SseAccumulator` — ensure `pub mod wire;` (Task 0) and `pub struct SseAccumulator` (Task 4) make it public.

- [ ] **Step 3: Run to verify**

Run: `cargo test -p bridge-api --test corpus_replay`
Expected: PASS (3 tests).

- [ ] **Step 4: Refactor the wiremock tests to source bodies from the fixture (DoD-6 single-source)**

In `tests/wiremock_turns.rs`, replace the inline `text_round_trip` SSE body literal with the fixture's `text_turn_sse`, and the tool-call body with `tool_turn_sse` (load via a small `fixture()` helper duplicated or shared through a `mod common`). Run `cargo test -p bridge-api` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-api/tests/fixtures crates/bridge-api/tests/corpus_replay.rs crates/bridge-api/tests/wiremock_turns.rs
git commit -m "test(api): REAL-CAPTURE fixture as stub source + replay-through-parser (DoD-6)"
```

---

## Task 14: Gated live Ollama test (DoD-7)

**Files:**
- Create: `crates/bridge-api/tests/live_ollama.rs`

- [ ] **Step 1: Write the gated test**

Create `crates/bridge-api/tests/live_ollama.rs`:
```rust
//! Gated live test against a real local Ollama. Run manually:
//!   brew install ollama && ollama serve && ollama pull qwen3.5:9b
//!   cargo test -p bridge-api --test live_ollama -- --ignored api_live_two_turns
use bridge_api::{ApiBackend, ApiConfig};
use bridge_core::domain::Part;
use bridge_core::ids::SessionId;
use bridge_core::ports::{AgentBackend, Update};
use futures::StreamExt;

fn base_url() -> String {
    std::env::var("OLLAMA_BASE_URL").unwrap_or_else(|_| "http://localhost:11434/v1".into())
}

async fn run(be: &ApiBackend, s: &SessionId, text: &str) -> Vec<Update> {
    let mut st = be.prompt(s, vec![Part { text: text.into() }]).await.unwrap();
    let mut out = Vec::new();
    while let Some(i) = st.next().await { out.push(i.unwrap()); }
    out
}

#[tokio::test]
#[ignore = "requires a local Ollama with qwen3.5:9b"]
async fn api_live_two_turns() {
    let mut cfg = ApiConfig::new(base_url());
    cfg.model = Some("qwen3.5:9b".into());
    let be = ApiBackend::new(cfg);
    let s = SessionId::parse("live").unwrap();

    // Turn 1: plain text.
    let t1 = run(&be, &s, "Reply with a short greeting.").await;
    let text1: String = t1.iter().filter_map(|u| if let Update::Text(t)=u {Some(t.clone())} else {None}).collect();
    assert!(!text1.trim().is_empty(), "turn 1 produced text");
    assert!(matches!(t1.last(), Some(Update::Done { .. })));

    // Turn 2: force a tool call.
    let t2 = run(&be, &s, "What time is it? You MUST call the get_current_time tool.").await;
    assert!(matches!(t2.last(), Some(Update::Done { .. })));
    // No Permission is ever emitted (silent decision).
    assert!(!t2.iter().any(|u| matches!(u, Update::Permission(_))));
}
```

- [ ] **Step 2: Verify it compiles + is skipped by default**

Run: `cargo test -p bridge-api --test live_ollama`
Expected: compiles; `api_live_two_turns` reported as ignored (0 run).

- [ ] **Step 3: (Optional, manual) run it live**

If Ollama is available: `cargo test -p bridge-api --test live_ollama -- --ignored api_live_two_turns` → PASS. Record the result in the task notes.

- [ ] **Step 4: Commit**

```bash
git add crates/bridge-api/tests/live_ollama.rs
git commit -m "test(api): gated live Ollama 2-turn smoke (DoD-7)"
```

---

## Task 15: Domain ripple — `cmd: Option`, `base_url`, `api_key_env` (Phase B begins)

**Files:**
- Modify: `crates/bridge-core/src/domain.rs`
- Modify: `bin/a2a-bridge/src/route.rs:95`, `crates/bridge-a2a-inbound/src/server.rs:1683`

- [ ] **Step 1: Write the failing test**

In `crates/bridge-core/src/domain.rs` add to the `tests` module:
```rust
#[test]
fn agent_entry_cmd_is_optional_and_has_url_fields() {
    let e = AgentEntry {
        id: AgentId::parse("ollama").unwrap(),
        cmd: None,
        args: vec![],
        kind: AgentKind::Api,
        base_url: Some("http://localhost:11434/v1".into()),
        api_key_env: None,
        model_provider: None, model: None, effort: None, mode: None, cwd: None,
        auth_method: None, name: None, description: None, tags: vec![], version: None,
        extensions: Default::default(),
    };
    assert!(e.cmd.is_none());
    assert_eq!(e.base_url.as_deref(), Some("http://localhost:11434/v1"));
    assert_eq!(e.kind, AgentKind::Api);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-core domain::`
Expected: FAIL to compile — `cmd` expects `String`, `base_url`/`api_key_env`/`AgentKind::Api` don't exist.

- [ ] **Step 3: Implement**

In `domain.rs`: add the enum variant and change the struct:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentKind {
    #[default]
    Acp,
    /// Non-process OpenAI-compatible HTTP backend (bridge-api).
    Api,
}
```
In `AgentEntry`:
```rust
    pub cmd: Option<String>,
    pub args: Vec<String>,
    pub kind: AgentKind,
    /// Non-process backends (kind="api"): the OpenAI-compatible base URL.
    pub base_url: Option<String>,
    /// kind="api": NAME of an env var holding a bearer token (never the secret).
    pub api_key_env: Option<String>,
```
Fix the two existing `AgentEntry` literals in `domain.rs` tests (`agent_entry_carries_kind`, `effective_config_layers_override_over_entry`): set `cmd: Some("…".into())`, add `base_url: None, api_key_env: None`.

In `bin/a2a-bridge/src/route.rs:95` and `crates/bridge-a2a-inbound/src/server.rs:1683`, update those `AgentEntry { … }` literals: `cmd: Some("…".into()), base_url: None, api_key_env: None`.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo build --workspace` (find every literal the compiler flags) then `cargo test -p bridge-core domain::`
Expected: build surfaces remaining literals (fix each: `cmd: Some(..)`, add the two `None` fields); test PASSES. Note: `crates/bridge-registry`, `bin/a2a-bridge/src/config.rs`, `main.rs`, e2e/common will still fail to build — fixed in Tasks 16-20. To keep THIS task green, scope the test run to bridge-core: `cargo test -p bridge-core`.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-core/src/domain.rs bin/a2a-bridge/src/route.rs crates/bridge-a2a-inbound/src/server.rs
git commit -m "feat(core): AgentKind::Api + AgentEntry cmd:Option + base_url/api_key_env"
```

---

## Task 16: Config parsing — `parse_kind`, TOML struct, `into_snapshot`, validation

**Files:**
- Modify: `bin/a2a-bridge/src/config.rs`

- [ ] **Step 1: Write the failing tests**

Add to `config.rs` tests:
```rust
#[test]
fn parse_kind_accepts_api() {
    assert_eq!(parse_kind("api").unwrap(), bridge_core::domain::AgentKind::Api);
    assert!(parse_kind("bogus").is_err());
}

#[test]
fn api_entry_parses_without_cmd() {
    let toml = r#"
default = "ollama"
[[agents]]
id = "ollama"
kind = "api"
base_url = "http://localhost:11434/v1"
model = "qwen3.5:9b"
"#;
    let snap = RegistryConfig::parse(toml).unwrap().into_snapshot().unwrap();
    let e = snap.entries.iter().find(|e| e.id.as_str() == "ollama").unwrap();
    assert!(e.cmd.is_none());
    assert_eq!(e.base_url.as_deref(), Some("http://localhost:11434/v1"));
    // allowed_cmds union skips the None cmd:
    assert!(snap.allowed_cmds.is_empty() || !snap.allowed_cmds.iter().any(|c| c.is_empty()));
}

#[test]
fn api_entry_with_cmd_is_rejected() {
    let toml = r#"
default = "x"
[[agents]]
id = "x"
kind = "api"
base_url = "http://h/v1"
cmd = "should-not-be-here"
"#;
    assert!(RegistryConfig::parse(toml).unwrap().into_snapshot().is_err());
}

#[test]
fn acp_entry_without_cmd_is_rejected() {
    let toml = r#"
default = "x"
[[agents]]
id = "x"
kind = "acp"
"#;
    assert!(RegistryConfig::parse(toml).unwrap().into_snapshot().is_err());
}
```
(Match `RegistryConfig::parse` to the actual constructor name in `config.rs`.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge --lib config::`
Expected: FAIL — `parse_kind("api")` errors; TOML `cmd`/`base_url` types mismatch.

- [ ] **Step 3: Implement**

In `config.rs`:
- The raw TOML agent struct: `cmd: Option<String>` (was `String`); add `pub base_url: Option<String>`, `pub api_key_env: Option<String>`.
- `parse_kind`:
```rust
fn parse_kind(s: &str) -> Result<AgentKind, ConfigError> {
    Ok(match s {
        "acp" => AgentKind::Acp,
        "api" => AgentKind::Api,
        other => return Err(ConfigError::Registry(format!("invalid kind: {other:?} (expected acp|api)"))),
    })
}
```
- `into_snapshot`: the `allowed_cmds` default union → `self.agents.iter().filter_map(|a| a.cmd.clone())`; assign `base_url: a.base_url`, `api_key_env: a.api_key_env`, `cmd: a.cmd` onto each `AgentEntry`. After building each entry, the parse-shape check:
```rust
match kind {
    AgentKind::Acp if a_cmd.is_none() =>
        return Err(ConfigError::Registry(format!("acp agent {:?} requires cmd", id.as_str()))),
    AgentKind::Api if a_base_url.is_none() =>
        return Err(ConfigError::Registry(format!("api agent {:?} requires base_url", id.as_str()))),
    AgentKind::Api if a_cmd.is_some() =>
        return Err(ConfigError::Registry(format!("api agent {:?} must not set cmd", id.as_str()))),
    _ => {}
}
```
(bind `a_cmd`/`a_base_url` before the entry is moved.)

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p a2a-bridge --lib config::`
Expected: PASS (4 new tests + existing).

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/config.rs
git commit -m "feat(config): parse kind=api, optional cmd, base_url validation"
```

---

## Task 17: `registry::validate` fix + reuse-identity + tests

**Files:**
- Modify: `crates/bridge-registry/src/registry.rs`

- [ ] **Step 1: Write the failing test**

Add to `registry.rs` tests (mirror the existing `cmd_change_replaces_slot` test for the new one — locate it first):
```rust
#[test]
fn validate_allows_api_entry_without_cmd() {
    let snap = RegistrySnapshot {
        default: AgentId::parse("ollama").unwrap(),
        entries: vec![AgentEntry {
            id: AgentId::parse("ollama").unwrap(), cmd: None, args: vec![], kind: AgentKind::Api,
            base_url: Some("http://h/v1".into()), api_key_env: None,
            model_provider: None, model: None, effort: None, mode: None, cwd: None,
            auth_method: None, name: None, description: None, tags: vec![], version: None,
            extensions: Default::default(),
        }],
        allowed_cmds: vec![], // no cmds — must NOT reject the api entry
    };
    assert!(validate(&snap).is_ok());
}

#[test]
fn validate_rejects_api_entry_missing_base_url() {
    let mut snap = api_snap();          // helper building the above
    snap.entries[0].base_url = None;
    assert!(validate(&snap).is_err());
}
```
(Add an `api_snap()` helper in the test module returning the valid snapshot above. Also add a `base_url_change_replaces_slot` test cloned from `cmd_change_replaces_slot`, mutating `base_url` instead of `cmd` and asserting the slot Arc changes.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-registry`
Expected: FAIL to compile — `e.cmd` is now `Option`, `format!("{}", e.cmd)` has no `Display`; the api entry would be rejected.

- [ ] **Step 3: Implement**

In `validate` (`registry.rs:~94`):
```rust
    for e in &snap.entries {
        if !seen.insert(e.id.clone()) {
            return Err(BridgeError::ConfigInvalid { reason: format!("duplicate agent id: {}", e.id.as_str()) });
        }
        match e.kind {
            AgentKind::Acp => {
                let Some(cmd) = e.cmd.as_deref() else {
                    return Err(BridgeError::ConfigInvalid { reason: format!("acp agent {} requires cmd", e.id.as_str()) });
                };
                if !snap.allowed_cmds.iter().any(|c| c == cmd) {
                    return Err(BridgeError::ConfigInvalid { reason: format!("cmd not allowed: {cmd}") });
                }
            }
            AgentKind::Api => {
                if e.base_url.is_none() {
                    return Err(BridgeError::ConfigInvalid { reason: format!("api agent {} requires base_url", e.id.as_str()) });
                }
                if e.cmd.is_some() {
                    return Err(BridgeError::ConfigInvalid { reason: format!("api agent {} must not set cmd", e.id.as_str()) });
                }
                // No allowed_cmds gate — a non-process backend has no command to allow.
            }
        }
    }
```
Reuse-identity (`:~245`): add `&& c.base_url == e.base_url` to the boolean tuple. Fix the test-only `.cmd = "…".into()` mutations at `:559`/`:646` → `cmd = Some("…".into())`, and the literal at `:349` (+ `base_url: None, api_key_env: None`).

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p bridge-registry`
Expected: PASS (incl. `base_url_change_replaces_slot`).

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-registry/src/registry.rs
git commit -m "feat(registry): validate kind-invariant + skip allowed-cmds for api; base_url in reuse identity"
```

---

## Task 18: Factory `Api` arm + bin dependency

**Files:**
- Modify: `bin/a2a-bridge/Cargo.toml`
- Modify: `bin/a2a-bridge/src/main.rs`

- [ ] **Step 1: Add the dep**

`bin/a2a-bridge/Cargo.toml` `[dependencies]`: add
```toml
bridge-api = { path = "../../crates/bridge-api" }
```

- [ ] **Step 2: Implement the factory arms**

In `main.rs` factory `match entry.kind` (`:107`), update the Acp arm to require `cmd` and add the Api arm:
```rust
                AgentKind::Acp => {
                    let cmd = entry.cmd.as_deref().ok_or(BridgeError::ConfigInvalid {
                        reason: format!("acp agent {} missing cmd", entry.id.as_str()),
                    })?;
                    let acp = AcpConfig {
                        cwd,
                        model: entry.model.clone(),
                        mode: entry.mode.clone(),
                        auth_method: entry.auth_method.clone(),
                        ..AcpConfig::default()
                    };
                    let be = AcpBackend::spawn(cmd, &args_ref, acp).await?.with_policy(policy);
                    Ok(Arc::new(be) as Arc<dyn AgentBackend>)
                }
                AgentKind::Api => {
                    let base_url = entry.base_url.clone().ok_or(BridgeError::ConfigInvalid {
                        reason: format!("api agent {} missing base_url", entry.id.as_str()),
                    })?;
                    let mut cfg = bridge_api::ApiConfig::new(base_url);
                    cfg.model = entry.model.clone();
                    cfg.api_key_env = entry.api_key_env.clone();
                    let be = bridge_api::ApiBackend::new(cfg).with_policy(policy);
                    Ok(Arc::new(be) as Arc<dyn AgentBackend>)
                }
```
(`AcpBackend::spawn(cmd, …)` now takes `cmd: &str` from the `as_deref()` — verify the existing signature accepts `&str`; the prior call passed `&entry.cmd` where `entry.cmd: String` derefs to `&str`, so this is unchanged.)

- [ ] **Step 3: Run to verify**

Run: `cargo build -p a2a-bridge` then `cargo test -p a2a-bridge`
Expected: compiles; existing tests pass (the `cwd` var is still used by the Acp arm — no unused warning).

- [ ] **Step 4: Commit**

```bash
git add bin/a2a-bridge/Cargo.toml bin/a2a-bridge/src/main.rs Cargo.lock
git commit -m "feat(bin): factory Api arm builds ApiBackend; Acp arm requires cmd"
```

---

## Task 19: Kind-aware e2e factory + `kind="api"` e2e (DoD-8)

**Files:**
- Modify: `bin/a2a-bridge/tests/e2e_registry.rs`, `bin/a2a-bridge/tests/common/mod.rs`

- [ ] **Step 1: Make the test spawn factory kind-aware**

In `tests/e2e_registry.rs:~114` the spawn closure calls `AcpBackend::spawn(&entry.cmd, …)`. Update it:
```rust
        match entry.kind {
            bridge_core::domain::AgentKind::Acp => {
                let cmd = entry.cmd.clone().expect("acp entry has cmd");
                // ...existing AcpBackend::spawn(&cmd, ...) path...
            }
            bridge_core::domain::AgentKind::Api => {
                let mut cfg = bridge_api::ApiConfig::new(entry.base_url.clone().expect("api entry has base_url"));
                cfg.model = entry.model.clone();
                Ok(std::sync::Arc::new(bridge_api::ApiBackend::new(cfg)) as std::sync::Arc<dyn AgentBackend>)
            }
        }
```
Add `bridge-api` to `bin/a2a-bridge/Cargo.toml` `[dev-dependencies]`. Fix the `AgentEntry` literals at `tests/e2e_registry.rs:211` and `tests/common/mod.rs:23` (`cmd: Some(..)`, `base_url`/`api_key_env`).

- [ ] **Step 2: Write the failing test**

Add to `tests/e2e_registry.rs` (use a `wiremock` server as the api endpoint; add `wiremock` to bin dev-deps):
```rust
#[tokio::test]
async fn api_entry_resolves_and_serves_through_registry() {
    let server = wiremock::MockServer::start().await;
    let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(200).insert_header("content-type","text/event-stream").set_body_string(sse))
        .mount(&server).await;
    // Build a snapshot with one kind="api" entry pointed at the mock, resolve via Registry,
    // run one prompt turn through the resolved backend, assert "hi" + Done.
    // (Mirror the existing multi-agent snapshot helper; append the api entry.)
}
```
(Fill the body using the file's existing snapshot+registry helpers — `four_agent_snapshot()`/resolve pattern — appending the api entry and pointing `base_url` at `format!("{}/v1", server.uri())`.)

Also add validation-rejection assertions: a snapshot with an api entry that sets `cmd` → `Registry::new` errors; an api entry missing `base_url` → errors.

- [ ] **Step 3: Run to verify it fails, then passes**

Run: `cargo test -p a2a-bridge --test e2e_registry api_entry`
Expected: PASS after wiring.

- [ ] **Step 4: Commit**

```bash
git add bin/a2a-bridge/tests/e2e_registry.rs bin/a2a-bridge/tests/common/mod.rs bin/a2a-bridge/Cargo.toml Cargo.lock
git commit -m "test(e2e): kind-aware spawn factory + api entry through Registry (DoD-8)"
```

---

## Task 20: CI coverage floor + full green sweep

**Files:**
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Add the bridge-api gate**

After the `bridge-acp` coverage step (`ci.yml:57`):
```yaml
      - name: Coverage — bridge-api (≥90% line coverage)
        run: cargo llvm-cov --package bridge-api --fail-under-lines 90
```

- [ ] **Step 2: Verify coverage locally (after clean)**

Run:
```bash
cargo llvm-cov clean --workspace
cargo llvm-cov --package bridge-api --fail-under-lines 90
cargo llvm-cov --package bridge-core --fail-under-lines 90
cargo llvm-cov --workspace --fail-under-lines 85
```
Expected: all three pass. If `bridge-api` < 90, add offline tests for the uncovered arms (the live test is `#[ignore]` and adds no coverage). If `bridge-core` dropped below 90 from the domain change, add a domain test.

- [ ] **Step 3: Full quality sweep**

Run:
```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
Expected: fmt clean, clippy clean, all tests green.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: bridge-api 90% line-coverage floor"
```

---

## Task 21: ADR-0007

**Files:**
- Create: `docs/adr/0007-api-backend.md`

- [ ] **Step 1: Write the ADR**

Create `docs/adr/0007-api-backend.md` recording: the non-process OpenAI-compatible API backend (`kind="api"`); the surface-A ripple (`cmd→Option`, `registry::validate`, factory) and surface-B evidence (the `PolicyEngine` port absorbs a client-side function-calling permission model unchanged, decided silently); the **refined conductor finding** (the `Update::Permission`/translator suspend path is NOT reusable for non-interactive client-side denials — it keys on policy `Err`, not `interactive`, and the backend has no resume — so per-tool/non-interactive permission needs port enrichment) + tool-blindness; that this is the cheap/free B1 replacement and the conductor re-eval stays parked, now with two backend kinds (ACP process + API non-process) as evidence.

- [ ] **Step 2: Commit (controller doc — trailer REQUIRED)**

```bash
git add docs/adr/0007-api-backend.md
git commit -m "$(cat <<'EOF'
docs(adr): 0007 — vendor-neutral OpenAI-compatible API backend (kind=api)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 22: Final verification

- [ ] **Step 1: Clean coverage + full suite**

```bash
cargo llvm-cov clean --workspace
cargo llvm-cov --workspace --fail-under-lines 85
cargo llvm-cov --package bridge-core --fail-under-lines 90
cargo llvm-cov --package bridge-acp --fail-under-lines 90
cargo llvm-cov --package bridge-api --fail-under-lines 90
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
cargo test --workspace
```
Expected: all green.

- [ ] **Step 2: Confirm DoD checklist** (spec §6): DoD-1..8 each map to a passing test; DoD-7 is `#[ignore]` (run manually if Ollama available). The backend yields only `Text`/`Done` (grep `Update::Permission` in `bridge-api/src` → zero hits).

- [ ] **Step 3: Hand back to the controller** for the holistic review + merge (finishing-a-development-branch).

---

## Self-Review notes (controller)

- **Spec coverage:** §2 → Tasks 15-16; §3 → Tasks 15-19; §4.1 → Tasks 2/6/7/11; §4.2 → Tasks 7/8/11; §4.3 (silent) → Tasks 8/9/12; §4.5 tool → Task 1; §4.6 cancel/error → Tasks 7/10; §5 → Tasks 4/5; §6 DoD-1..8 → Tasks 7/8/12/10/10/13/14/19; coverage → Task 20; ADR → Task 21. No gap.
- **Ordering:** the crate (Phase A, Tasks 0-14) builds against the *current* bridge-core (it never references `AgentKind`), so the domain ripple (Phase B, Tasks 15-19) can't break it. Phase B keeps each task's touched crate green by scoping test runs; the workspace is green again by Task 18.
- **TDD throughout; one logical change per commit; no `Co-Authored-By` on subagent commits (Task 21 controller doc excepted).**
