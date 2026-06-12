# Advertise Available Models / Effort / Modes — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Codebase note:** This is a multi-crate Rust workspace. Tasks marked **[anchored]** modify large existing files; each cites the exact `file:line` to read and mirror — read that code before editing. Tasks marked **[pure]** are self-contained and the code blocks are complete. Spec: `docs/superpowers/specs/2026-06-12-advertise-models-design.md`.

**Goal:** Advertise each configured agent's available models (+ effort levels + modes) in the A2A Agent Card and via a new `a2a-bridge models` CLI subcommand, so consumers can discover what they may pass to the already-shipped `a2a-bridge.{model,effort,mode}` per-request override.

**Architecture:** A `ModelCatalog` (probe-and-cache) maps `agent_id → AgentCaps`. A kind/adapter-aware probe fills it: ACP agents (claude/codex) via a clean `session/new` that reads advertised `configOptions`; kiro via native `kiro-cli chat --list-models`; `api` agents via OpenAI `GET /v1/models`. The catalog is built at `serve` startup (and on `SIGHUP`), rendered as one `AgentExtension` on the card, and printed by the CLI. Every probe is timeout-bounded and degrades per-agent.

**Tech Stack:** Rust, tokio, `async_trait`, `reqwest` (bridge-api), `agent_client_protocol` 0.12.1 (ACP schema), `a2a-lf` 0.3.0 (`AgentCard`/`AgentExtension`), `arc-swap`.

**Crate placement (locked from spec Open Q #1):**
- `bridge-core`: `AgentCaps`, `ModelCatalog` types + pure parsers (`parse_kiro_list_models`, `parse_ollama_models`).
- `bridge-acp`: `mode_values` reader + `caps_from_config_options` mapper + `AcpBackend::describe_options`.
- `bridge-a2a-inbound`: card renders the extension; `InboundServer` holds the catalog.
- `bin/a2a-bridge`: probe orchestration, startup/SIGHUP wiring, `models` subcommand.

---

### Task 1: `AgentCaps` + `ModelCatalog` types  **[pure]**

**Files:**
- Create: `crates/bridge-core/src/catalog.rs`
- Modify: `crates/bridge-core/src/lib.rs` (add `pub mod catalog;`)
- Test: in `catalog.rs` `#[cfg(test)]`

- [ ] **Step 1: Create `catalog.rs` with the types**

```rust
//! Per-agent capability catalog: advertised models/effort/modes, probed live.
use std::collections::BTreeMap;

/// What one agent advertises. Empty vecs mean "the backend advertises none"
/// (e.g. kiro/api have no effort/modes) — renderers omit empty keys.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentCaps {
    pub current_model: Option<String>,
    pub models: Vec<String>,
    pub effort_levels: Vec<String>,
    pub modes: Vec<String>,
    pub current_mode: Option<String>,
}

/// agent_id -> caps. An agent that failed to probe is ABSENT (not a stub).
pub type ModelCatalog = BTreeMap<String, AgentCaps>;
```

- [ ] **Step 2: Add the module export**

In `crates/bridge-core/src/lib.rs`, add `pub mod catalog;` beside the other `pub mod` lines (mirror the existing module declarations).

- [ ] **Step 3: Add a smoke test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn caps_default_is_empty() {
        let c = AgentCaps::default();
        assert!(c.models.is_empty() && c.effort_levels.is_empty() && c.modes.is_empty());
        assert!(c.current_model.is_none());
    }
}
```

- [ ] **Step 4: Run**

Run: `cargo test -p bridge-core catalog::tests::caps_default_is_empty`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-core/src/catalog.rs crates/bridge-core/src/lib.rs
git commit -m "feat(catalog): AgentCaps + ModelCatalog types"
```

---

### Task 2: kiro `--list-models` parser  **[pure]**

**Files:**
- Modify: `crates/bridge-core/src/catalog.rs`
- Test: same file

Reference fixture (verified live output of `kiro-cli chat --list-models`):
```
Available models (* = default):

* auto                 1.00x credits      Models chosen by task for optimal usage and consistent quality
  claude-sonnet-4.5    1.30x credits      The Claude Sonnet 4.5 model
  claude-haiku-4.5     0.40x credits      The latest Claude Haiku model
```

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn parses_kiro_list_models() {
    let out = "Available models (* = default):\n\n* auto                 1.00x credits      Models chosen by task\n  claude-sonnet-4.5    1.30x credits      The Claude Sonnet 4.5 model\n  claude-haiku-4.5     0.40x credits      The latest Claude Haiku model\n";
    let caps = parse_kiro_list_models(out);
    assert_eq!(caps.models, vec!["auto", "claude-sonnet-4.5", "claude-haiku-4.5"]);
    assert_eq!(caps.current_model.as_deref(), Some("auto"));
    assert!(caps.effort_levels.is_empty() && caps.modes.is_empty());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-core catalog::tests::parses_kiro_list_models`
Expected: FAIL (`parse_kiro_list_models` not found)

- [ ] **Step 3: Implement**

```rust
/// Parse `kiro-cli chat --list-models` text. Each model line is
/// `[*] <id> <multiplier>x credits  <description>`; the `*` marks the default.
/// Header lines (no model id) are skipped.
pub fn parse_kiro_list_models(stdout: &str) -> AgentCaps {
    let mut caps = AgentCaps::default();
    for line in stdout.lines() {
        let trimmed = line.trim_start();
        let (is_default, rest) = match trimmed.strip_prefix('*') {
            Some(r) => (true, r.trim_start()),
            None => (false, trimmed),
        };
        // A model line's first token is the id and the line carries "credits".
        let Some(id) = rest.split_whitespace().next() else { continue };
        if !rest.contains("credits") || id.is_empty() {
            continue; // header / blank / non-model line
        }
        caps.models.push(id.to_string());
        if is_default {
            caps.current_model = Some(id.to_string());
        }
    }
    caps
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p bridge-core catalog::tests::parses_kiro_list_models`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-core/src/catalog.rs
git commit -m "feat(catalog): parse kiro-cli --list-models"
```

---

### Task 3: ollama `/v1/models` parser  **[pure]**

**Files:**
- Modify: `crates/bridge-core/src/catalog.rs`
- Test: same file

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn parses_ollama_models_list() {
    let body = r#"{"object":"list","data":[{"id":"qwen2.5-coder:7b","object":"model"},{"id":"llama3.1:8b","object":"model"}]}"#;
    let caps = parse_ollama_models(body).expect("valid list");
    assert_eq!(caps.models, vec!["qwen2.5-coder:7b", "llama3.1:8b"]);
    assert!(caps.current_model.is_none() && caps.effort_levels.is_empty());
}

#[test]
fn ollama_models_rejects_garbage() {
    assert!(parse_ollama_models("not json").is_err());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-core catalog::tests::parses_ollama_models_list`
Expected: FAIL (`parse_ollama_models` not found)

- [ ] **Step 3: Implement** (uses `serde_json`, already a `bridge-core` dep)

```rust
/// Parse an OpenAI-compatible `GET /v1/models` body → model ids (in `data[].id` order).
pub fn parse_ollama_models(body: &str) -> Result<AgentCaps, serde_json::Error> {
    #[derive(serde::Deserialize)]
    struct Entry { id: String }
    #[derive(serde::Deserialize)]
    struct List { data: Vec<Entry> }
    let list: List = serde_json::from_str(body)?;
    Ok(AgentCaps { models: list.data.into_iter().map(|e| e.id).collect(), ..Default::default() })
}
```

(If `bridge-core/Cargo.toml` lacks `serde` with `derive`, add `serde = { workspace = true, features = ["derive"] }` — check the workspace `Cargo.toml` first; `serde_json` is already used in `bridge-core` per `mcp.rs`.)

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p bridge-core catalog::tests`
Expected: PASS (both new tests)

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-core/src/catalog.rs crates/bridge-core/Cargo.toml
git commit -m "feat(catalog): parse OpenAI /v1/models list (ollama)"
```

---

### Task 4: `mode_values` reader for ACP configOptions  **[pure]**

**Files:**
- Modify: `crates/bridge-acp/src/model_effort.rs`
- Test: same file (`#[cfg(test)]` mod already exists there)

Read first: `crates/bridge-acp/src/model_effort.rs:78-109` — `matches_category_or_id`, `find_select`, `model_values`. You will mirror `model_values` exactly, swapping the category to `Cat::Mode` and the id list to `["mode"]`.

- [ ] **Step 1: Write the failing test** (mirror `model_values_reads_ungrouped_model_option` at `model_effort.rs:393`; build a `SessionConfigOption` with `category: Some(Cat::Mode)`, a `Select` with `current_value` + two options)

```rust
#[test]
fn mode_values_reads_mode_select() {
    // Build a Mode select with current "default" + options ["default","plan"].
    // (Construct the SessionConfigOption exactly as model_values_reads_ungrouped_model_option does,
    //  but with category = Cat::Mode and id "mode".)
    let opts = vec![/* see the model-option test fixture; swap category→Mode, id→"mode" */];
    let (id, current, values) = mode_values(&opts).expect("mode select");
    assert_eq!(id, "mode");
    assert_eq!(current, "default");
    assert_eq!(values, vec!["default", "plan"]);
}
```

(Copy the fixture construction verbatim from `model_values_reads_ungrouped_model_option` — do not invent the `SessionConfigOption` shape; reuse that test's exact builder with the two field swaps.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-acp model_effort::tests::mode_values_reads_mode_select`
Expected: FAIL (`mode_values` not found)

- [ ] **Step 3: Implement** (beside `model_values` at `model_effort.rs:107`)

```rust
/// Returns `(config_id, current_value, values)` for the advertised mode select, if any.
/// Mirrors `model_values` against the `Mode` category (id `"mode"`).
pub fn mode_values(opts: &[SessionConfigOption]) -> Option<(String, String, Vec<String>)> {
    find_select(opts, Cat::Mode, &["mode"])
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p bridge-acp model_effort::tests::mode_values_reads_mode_select`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-acp/src/model_effort.rs
git commit -m "feat(model_effort): mode_values reader (Mode-category select)"
```

---

### Task 5: `caps_from_config_options` mapper (ACP configOptions → AgentCaps)  **[pure]**

**Files:**
- Modify: `crates/bridge-acp/src/model_effort.rs`
- Test: same file

This bridges the ACP schema (`bridge-acp`) to the core type (`bridge-core::catalog::AgentCaps`), so it lives in `bridge-acp`. Reuses `model_values`, `effort_opt`, `mode_values`.

- [ ] **Step 1: Write the failing test** (build a `Vec<SessionConfigOption>` with a model select [current "sonnet", values default/sonnet/haiku], an effort `thought_level` select [low/medium/high], and a mode select [default/plan]; reuse the existing fixture builders)

```rust
#[test]
fn caps_from_config_options_maps_all_three() {
    let opts = vec![/* model + thought_level + mode selects, reusing the existing fixture builders */];
    let caps = caps_from_config_options(&opts);
    assert_eq!(caps.current_model.as_deref(), Some("sonnet"));
    assert_eq!(caps.models, vec!["default","sonnet","haiku"]);
    assert_eq!(caps.effort_levels, vec!["low","medium","high"]);
    assert_eq!(caps.modes, vec!["default","plan"]);
    assert_eq!(caps.current_mode.as_deref(), Some("default"));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-acp model_effort::tests::caps_from_config_options_maps_all_three`
Expected: FAIL (`caps_from_config_options` not found)

- [ ] **Step 3: Implement**

```rust
use bridge_core::catalog::AgentCaps;

/// Map advertised ACP `configOptions` (claude/codex) → AgentCaps. effort_opt already
/// filters out the "default" pseudo-level (see effort_opt at model_effort.rs:128).
pub fn caps_from_config_options(opts: &[SessionConfigOption]) -> AgentCaps {
    let (current_model, models) = match model_values(opts) {
        Some((_, current, values)) => (Some(current), values),
        None => (None, Vec::new()),
    };
    let effort_levels = effort_opt(opts).map(|e| e.levels).unwrap_or_default();
    let (current_mode, modes) = match mode_values(opts) {
        Some((_, current, values)) => (Some(current), values),
        None => (None, Vec::new()),
    };
    AgentCaps { current_model, models, effort_levels, modes, current_mode }
}
```

(Confirm `bridge-acp/Cargo.toml` depends on `bridge-core` — it does, `model_effort.rs:7` already imports `bridge_core::domain::Effort`.)

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p bridge-acp model_effort::tests::caps_from_config_options_maps_all_three`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-acp/src/model_effort.rs
git commit -m "feat(model_effort): caps_from_config_options mapper"
```

---

### Task 6: `AcpBackend::describe_options` (mint → read configOptions → reap)  **[anchored]**

**Files:**
- Modify: `crates/bridge-acp/src/acp_backend.rs`

**Read first:** the mint path `acp_backend.rs:1140-1240` (where `configure_model_option` is called during `prompt`'s lazy mint) and `:1745` (the SessionSpec stash). The advertised `configOptions` (`opts0`) are already in hand at `session/new` *before* model resolution. `describe_options` does the same `initialize` + `session/new`, captures `opts0` (and the kiro `models0`/`SessionModelState`), maps via `caps_from_config_options` (or `model_state_values` for kiro's surface), then tears the session down — **without sending a prompt**.

- [ ] **Step 1: Add the inherent method on `AcpBackend`** (NOT on the `AgentBackend` trait — discovery is host-side and separate from runtime serving). Signature:

```rust
/// Mint a throwaway session, read the advertised model/effort/mode options, and reap.
/// No prompt is sent. `cwd` is any readable dir (the session needs one; reads nothing).
pub async fn describe_options(
    &self,
    cwd: &std::path::Path,
) -> Result<bridge_core::catalog::AgentCaps, BridgeError> {
    // 1. Reuse the existing connect + session/new path used by the lazy mint
    //    (acp_backend.rs:1140-1240). Capture `opts0` (Vec<SessionConfigOption>) and
    //    `models0` (Option<SessionModelState>) from session/new.
    // 2. If `opts0` has a model select → caps = caps_from_config_options(&opts0).
    //    Else if `models0` is Some (kiro's surface) → caps = AgentCaps {
    //        current_model: Some(state.current_model_id.0.to_string()),
    //        models: model_state_values(&state), ..Default::default() }.
    //    Else → AgentCaps::default().
    // 3. forget_session / drop the connection so the child process is reaped
    //    (reuse the teardown in `forget_session`/the Supervised drop path).
    // 4. Return caps.
}
```

Implement the body by factoring the connect+`session/new` half out of the existing mint (or calling a shared helper) — do **not** duplicate the spawn logic. Keep the bogus-model error trick OUT; read `opts0` directly.

- [ ] **Step 2: Bound it** — callers wrap with `tokio::time::timeout` (Task 7), so no timeout inside.

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p bridge-acp`
Expected: builds clean.

- [ ] **Step 4: Live check** (no pure unit test — needs a real adapter; the DoD live gate in Task 11 covers assertion). Manual smoke:

Run: a tiny harness or the Task 9 CLI once it lands. Defer assertion to Task 11.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-acp/src/acp_backend.rs
git commit -m "feat(acp): AcpBackend::describe_options (read advertised options, no prompt)"
```

---

### Task 7: probe orchestration — `probe_agent` + `probe_all`  **[anchored]**

**Files:**
- Create: `bin/a2a-bridge/src/catalog_probe.rs`
- Modify: `bin/a2a-bridge/src/main.rs` (add `mod catalog_probe;`)
- Test: in `catalog_probe.rs`

**Read first:** `main.rs:455` `make_spawn_fn` and the host (no-sandbox) `AcpBackend` construction; `bridge-core/src/domain.rs:115` `AgentEntry` (fields `cmd`, `args`, `kind`, `sandbox`); `bridge-api` `ApiBackend::new` + its `base_url`.

- [ ] **Step 1: Write `probe_agent` (kind/adapter dispatch)**

```rust
use std::time::Duration;
use bridge_core::catalog::{AgentCaps, ModelCatalog, parse_kiro_list_models, parse_ollama_models};
use bridge_core::domain::{AgentEntry, AgentKind};

const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// Probe ONE agent host-side (sandbox ignored — the advertised list is sandbox-independent; see spec §2).
pub async fn probe_agent(entry: &AgentEntry, cwd: &std::path::Path) -> Result<AgentCaps, String> {
    let cmd = entry.cmd.clone().unwrap_or_default();
    let basename = std::path::Path::new(&cmd)
        .file_name().and_then(|s| s.to_str()).unwrap_or(&cmd);
    let fut = async {
        match entry.kind {
            AgentKind::Api => probe_api(entry).await,
            _ if basename == "kiro-cli" => probe_kiro(entry).await,
            _ => probe_acp_host(entry, cwd).await,    // claude / codex
        }
    };
    match tokio::time::timeout(PROBE_TIMEOUT, fut).await {
        Ok(r) => r,
        Err(_) => Err("probe timed out".into()),
    }
}
```

(Confirm the `AgentKind` variant names against `domain.rs` — `grep -n "enum AgentKind" crates/bridge-core/src/domain.rs`; use the real variant for the `api` kind.)

- [ ] **Step 2: kiro probe (native command)**

```rust
async fn probe_kiro(entry: &AgentEntry) -> Result<AgentCaps, String> {
    let cmd = entry.cmd.clone().unwrap_or_else(|| "kiro-cli".into());
    let out = tokio::process::Command::new(&cmd)
        .args(["chat", "--list-models"])
        .output().await.map_err(|e| format!("spawn {cmd}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{cmd} exited {:?}", out.status.code()));
    }
    Ok(parse_kiro_list_models(&String::from_utf8_lossy(&out.stdout)))
}
```

- [ ] **Step 3: ollama/api probe (`GET {base_url}/v1/models`)**

```rust
async fn probe_api(entry: &AgentEntry) -> Result<AgentCaps, String> {
    // base_url lives on the api config; read it off the entry (grep domain.rs for where
    // api base_url is stored on AgentEntry — it parallels `model`). Compose `{base_url}/models`
    // (base_url already ends in /v1 per the example configs).
    let base = api_base_url(entry).ok_or("api agent missing base_url")?;
    let url = format!("{}/models", base.trim_end_matches('/'));
    let body = reqwest::Client::new().get(&url).send().await
        .map_err(|e| format!("GET {url}: {e}"))?
        .text().await.map_err(|e| format!("read {url}: {e}"))?;
    parse_ollama_models(&body).map_err(|e| format!("parse {url}: {e}"))
}
```

(Add `reqwest` to `bin/a2a-bridge/Cargo.toml` if absent — it's already a workspace dep via bridge-api. Implement `api_base_url(entry)` by reading the field `domain.rs` stores the api `base_url` in.)

- [ ] **Step 4: ACP host probe** (constructs a host AcpBackend — sandbox stripped — and calls `describe_options`)

```rust
async fn probe_acp_host(entry: &AgentEntry, cwd: &std::path::Path) -> Result<AgentCaps, String> {
    // Build a HOST AcpBackend for (entry.cmd, entry.args) with NO sandbox — mirror the
    // host branch of make_spawn_fn (main.rs:455) / the non-containerized AcpBackend ctor.
    // Then: backend.describe_options(cwd).await.map_err(|e| e.to_string())
    todo!("construct host AcpBackend from entry.cmd/args (see make_spawn_fn host branch), call describe_options")
}
```

> NOTE: this is the one body that depends on the host-`AcpBackend` construction. Read `make_spawn_fn` (main.rs:455) + the `AcpBackend` ctor; build it with the agent's `cmd`/`args` and **no** sandbox, exactly as the host (non-`[agents.sandbox]`) path does. The probe must reap the child (describe_options handles teardown).

- [ ] **Step 5: `probe_all` (concurrent, degrade-per-agent)**

```rust
/// Probe every entry concurrently; failures are logged + omitted (the catalog only holds successes).
pub async fn probe_all(entries: &[(String, AgentEntry)], cwd: &std::path::Path) -> ModelCatalog {
    let futs = entries.iter().map(|(id, e)| async move {
        match probe_agent(e, cwd).await {
            Ok(caps) => Some((id.clone(), caps)),
            Err(reason) => { tracing::warn!(agent = %id, %reason, "model probe failed; omitting from catalog"); None }
        }
    });
    futures::future::join_all(futs).await.into_iter().flatten().collect()
}
```

- [ ] **Step 6: Test `probe_all` degradation with a fake-able split** — extract the per-agent step behind a closure so the test injects one Ok + one Err and asserts the catalog has only the Ok agent. (If injecting is awkward, test the pure dispatch decision: a small `fn probe_strategy(entry) -> Strategy` enum returning `Api|Kiro|Acp`, unit-tested by kind/cmd, and assert `probe_all` flattens `None`s.)

```rust
#[tokio::test]
async fn probe_all_omits_failures() {
    // Given a fake prober: agent "ok" -> Ok(caps{models:["m"]}), agent "bad" -> Err("x")
    // assert the resulting catalog contains "ok" and NOT "bad".
}
```

- [ ] **Step 7: Run**

Run: `cargo test -p a2a-bridge catalog_probe`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add bin/a2a-bridge/src/catalog_probe.rs bin/a2a-bridge/src/main.rs bin/a2a-bridge/Cargo.toml
git commit -m "feat(probe): kind-aware probe_agent + concurrent probe_all (degrade per-agent)"
```

---

### Task 8: Render the `agent-models` Agent Card extension  **[anchored, pure render]**

**Files:**
- Modify: `crates/bridge-a2a-inbound/src/card.rs`
- Test: same file (mirror `card_advertises_mcp_servers_as_extension` at `card.rs:252`)

**Read first:** `card.rs:97-119` (the existing MCP `AgentExtension` build) and `card.rs:121-144` (the `AgentCard` literal). You add a second extension built from a `&ModelCatalog`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn card_advertises_agent_models_extension() {
    use bridge_core::catalog::{AgentCaps, ModelCatalog};
    let mut cat = ModelCatalog::new();
    cat.insert("claude".into(), AgentCaps {
        current_model: Some("sonnet".into()),
        models: vec!["default".into(), "sonnet".into(), "haiku".into()],
        effort_levels: vec!["low".into(), "high".into()],
        modes: vec![], current_mode: None });
    let c = agent_card("http://x", &[], &[], &cat);
    let exts = c.capabilities.extensions.expect("extensions");
    let ext = exts.iter().find(|e| e.uri.contains("agent-models")).expect("agent-models ext");
    let agents = ext.params.as_ref().and_then(|p| p.get("agents")).expect("params.agents");
    assert_eq!(agents["claude"]["current"], serde_json::json!("sonnet"));
    assert_eq!(agents["claude"]["models"], serde_json::json!(["default","sonnet","haiku"]));
    assert_eq!(agents["claude"]["effort"], serde_json::json!(["low","high"]));
    assert!(agents["claude"].get("modes").is_none(), "empty modes omitted");
}

#[test]
fn card_has_no_agent_models_ext_when_catalog_empty() {
    let c = agent_card("http://x", &[], &[], &bridge_core::catalog::ModelCatalog::new());
    let has = c.capabilities.extensions.unwrap_or_default()
        .iter().any(|e| e.uri.contains("agent-models"));
    assert!(!has);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-a2a-inbound card::tests::card_advertises_agent_models_extension`
Expected: FAIL (arity: `agent_card` takes 3 args / type error)

- [ ] **Step 3: Add the `catalog` param + extension builder.** Change `agent_card`'s signature to `pub fn agent_card(base_url: &str, workflow_ids: &[&str], mcp_servers: &[(String, Vec<String>)], catalog: &bridge_core::catalog::ModelCatalog) -> AgentCard`. After the MCP `extensions` block, build the models extension and push it into the (now `mut`) `extensions` vec:

```rust
// agent-models extension: per-agent override matrix from the live catalog (omit empty keys).
let mut ext_vec = extensions.unwrap_or_default();
if !catalog.is_empty() {
    let agents: serde_json::Map<String, serde_json::Value> = catalog.iter().map(|(id, c)| {
        let mut o = serde_json::Map::new();
        if let Some(m) = &c.current_model { o.insert("current".into(), serde_json::json!(m)); }
        o.insert("models".into(), serde_json::json!(c.models));
        if !c.effort_levels.is_empty() { o.insert("effort".into(), serde_json::json!(c.effort_levels)); }
        if !c.modes.is_empty() { o.insert("modes".into(), serde_json::json!(c.modes)); }
        if let Some(m) = &c.current_mode { o.insert("current_mode".into(), serde_json::json!(m)); }
        (id.clone(), serde_json::Value::Object(o))
    }).collect();
    let mut params = std::collections::HashMap::new();
    params.insert("agents".to_string(), serde_json::Value::Object(agents));
    ext_vec.push(AgentExtension {
        uri: "https://github.com/shoedog/a2acp/ext/agent-models/v1".to_string(),
        description: Some("Per-agent model/effort/mode override matrix. To override a default, send \
            message.metadata `a2a-bridge.model` / `a2a-bridge.effort` / `a2a-bridge.mode` targeting \
            that agent.".to_string()),
        required: Some(false),
        params: Some(params),
    });
}
let extensions = if ext_vec.is_empty() { None } else { Some(ext_vec) };
```

(The existing `extensions` binding becomes intermediate; ensure the final `AgentCard { capabilities: AgentCapabilities { extensions, .. } }` uses this combined value.)

- [ ] **Step 4: Fix the existing callers + tests for the new arity.** `serve_card` (`server.rs:554`) and every `agent_card(...)` test in `card.rs` now need a 4th arg. For tests, pass `&bridge_core::catalog::ModelCatalog::new()`. For `serve_card`, see Task 9.

- [ ] **Step 5: Run**

Run: `cargo test -p bridge-a2a-inbound card::tests`
Expected: PASS (new + existing card tests)

- [ ] **Step 6: Commit**

```bash
git add crates/bridge-a2a-inbound/src/card.rs
git commit -m "feat(card): agent-models AgentExtension from the live catalog"
```

---

### Task 9: serve wiring — build catalog at startup, hold it, SIGHUP refresh  **[anchored]**

**Files:**
- Modify: `crates/bridge-a2a-inbound/src/server.rs` (`InboundServer` struct + `serve_card`)
- Modify: `bin/a2a-bridge/src/main.rs` (serve setup: probe + store; SIGHUP handler)
- Modify: `bridge-a2a-inbound/Cargo.toml` (add `arc-swap` if absent)

**Read first:** the `InboundServer` struct def (`grep -n "struct InboundServer" crates/bridge-a2a-inbound/src/server.rs`) and its constructor; `serve_card` at `server.rs:551-555`; the serve bootstrap in `main.rs` (where `InboundServer` is built and axum is started, near `main.rs:2689+`).

- [ ] **Step 1: Add the catalog field to `InboundServer`**

```rust
// in struct InboundServer:
pub model_catalog: std::sync::Arc<arc_swap::ArcSwap<bridge_core::catalog::ModelCatalog>>,
```

Default-initialise it to an empty catalog in the constructor/builder (mirror how `allowed_cwd_root` is set, `server.rs:258`), plus a `with_model_catalog(...)` setter.

- [ ] **Step 2: `serve_card` reads the catalog**

```rust
async fn serve_card(State(srv): State<Arc<InboundServer>>) -> Response {
    let workflow_ids: Vec<&str> = srv.workflows.keys().map(|k| k.as_str()).collect();
    let mcp = srv.registry.mcp_advertisement();
    let catalog = srv.model_catalog.load();
    Json(agent_card(&srv.base_url, &workflow_ids, &mcp, &catalog)).into_response()
}
```

- [ ] **Step 3: Probe at startup (main.rs serve bootstrap).** After the registry is built and before/just after `InboundServer` starts serving:

```rust
let entries: Vec<(String, AgentEntry)> = /* registry snapshot → (id, entry) pairs */;
let probe_cwd = std::env::current_dir().unwrap_or_else(|_| "/tmp".into());
let catalog = catalog_probe::probe_all(&entries, &probe_cwd).await;
let catalog_handle = std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(catalog));
// hand catalog_handle to InboundServer (via with_model_catalog); keep a clone for SIGHUP.
```

(Get the `(id, entry)` list from the registry snapshot — `grep -n "fn snapshot\|fn entries\|fn agent_ids\|mcp_advertisement" crates/bridge-registry/src/registry.rs` to find the accessor; if none exposes entries, add a small `pub fn entries(&self) -> Vec<(String, AgentEntry)>` mirroring `mcp_advertisement`.)

- [ ] **Step 4: SIGHUP re-probe + atomic swap**

```rust
let sighup_catalog = catalog_handle.clone();
let sighup_entries = entries.clone();
let sighup_cwd = probe_cwd.clone();
tokio::spawn(async move {
    let mut hup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
        .expect("install SIGHUP handler");
    while hup.recv().await.is_some() {
        tracing::info!("SIGHUP: re-probing model catalog");
        let fresh = catalog_probe::probe_all(&sighup_entries, &sighup_cwd).await;
        sighup_catalog.store(std::sync::Arc::new(fresh));
    }
});
```

- [ ] **Step 5: Build + the existing serve test still passes**

Run: `cargo build -p a2a-bridge && cargo test -p bridge-a2a-inbound server::tests::serves_agent_card`
Expected: builds; `serves_agent_card` passes (it now constructs `InboundServer` with an empty catalog → no agent-models ext, card still served).

- [ ] **Step 6: Commit**

```bash
git add crates/bridge-a2a-inbound/src/server.rs bin/a2a-bridge/src/main.rs crates/bridge-a2a-inbound/Cargo.toml
git commit -m "feat(serve): probe model catalog at startup; SIGHUP re-probe; card reads it"
```

---

### Task 10: `a2a-bridge models` subcommand  **[anchored]**

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs` (dispatch at `:2656`, a `models_cmd` handler, `parse_models_args`, help text)
- Test: `parse_models_args` unit test (mirror `parse_run_workflow_args_*` at `main.rs:3691`)

**Read first:** `main.rs:2656` dispatch block; `run_workflow_cmd` (`:1768`) + `parse_run_workflow_args` (`:538`) for the handler/arg-parse + config-loading pattern (it loads the registry from `--config`).

- [ ] **Step 1: Dispatch arm** — add to the match at `main.rs:2656`:

```rust
Some("models") => return models_cmd(&raw_args[2..]).await,
```

- [ ] **Step 2: `parse_models_args` + test** (flags `--config <f>`, `--agent <id>`, `--json`; mirror `parse_run_workflow_args`)

```rust
struct ModelsArgs { config: Option<String>, agent: Option<String>, json: bool }
fn parse_models_args(args: &[String]) -> Result<ModelsArgs, BoxError> { /* mirror parse_run_workflow_args flag loop */ }

#[test]
fn parse_models_args_flags() {
    let a = parse_models_args(&["--agent".into(),"codex".into(),"--json".into()]).unwrap();
    assert_eq!(a.agent.as_deref(), Some("codex")); assert!(a.json && a.config.is_none());
}
```

- [ ] **Step 3: `models_cmd` handler** (load registry like `run_workflow_cmd`, probe, print)

```rust
async fn models_cmd(args: &[String]) -> Result<(), BoxError> {
    let a = parse_models_args(args)?;
    // Load the registry from a.config (mirror run_workflow_cmd's config load → entries list).
    let entries: Vec<(String, AgentEntry)> = /* registry → (id, entry) */;
    let filtered: Vec<_> = entries.into_iter()
        .filter(|(id, _)| a.agent.as_ref().is_none_or(|want| want == id)).collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| "/tmp".into());
    let catalog = catalog_probe::probe_all(&filtered, &cwd).await;
    if a.json {
        println!("{}", serde_json::to_string_pretty(&catalog_to_json(&catalog))?);
    } else {
        for (id, _) in &filtered {
            match catalog.get(id) {
                Some(c) => {
                    let cur = c.current_model.as_deref().unwrap_or("?");
                    println!("{id}: {}  (current: {cur})", c.models.join(", "));
                    if !c.effort_levels.is_empty() { println!("    effort: {}", c.effort_levels.join(", ")); }
                    if !c.modes.is_empty() { println!("    modes:  {}", c.modes.join(", ")); }
                }
                None => println!("{id}: unavailable (probe failed — see logs)"),
            }
        }
    }
    Ok(())
}
```

(`catalog_to_json` = the same per-agent object the card builds in Task 8 — factor that map-building into a shared `bridge_core::catalog::caps_to_json(&AgentCaps) -> serde_json::Value` used by BOTH Task 8 and here, to stay DRY.)

- [ ] **Step 4: Update help text** — add `models` to the SUBCOMMANDS block printed by the help handler (`grep -n "SUBCOMMANDS" bin/a2a-bridge/src/main.rs`): `  models              List each agent's advertised models/effort/modes.  [--config <f>] [--agent <id>] [--json]`.

- [ ] **Step 5: Run**

Run: `cargo test -p a2a-bridge parse_models_args_flags && cargo build -p a2a-bridge`
Expected: PASS + builds.

- [ ] **Step 6: Commit**

```bash
git add bin/a2a-bridge/src/main.rs crates/bridge-core/src/catalog.rs
git commit -m "feat(cli): a2a-bridge models subcommand (probe + table/json)"
```

---

### Task 11: Refactor shared `caps_to_json`, docs, and the live DoD gate

**Files:**
- Modify: `crates/bridge-core/src/catalog.rs` (add `caps_to_json`), `card.rs` + `models_cmd` (use it)
- Modify: `docs/onboarding.md` (model/override row), `AGENTS.md` (mention `a2a-bridge models`)

- [ ] **Step 1: DRY — `caps_to_json`** in `catalog.rs` returning the per-agent object (`current`/`models`/`effort`/`modes`/`current_mode`, empty keys omitted); rewrite Task 8's inline map and Task 10's `catalog_to_json` to call it. Add a unit test asserting empty effort/modes are omitted.

Run: `cargo test -p bridge-core catalog::tests`
Expected: PASS

- [ ] **Step 2: Docs** — in `docs/onboarding.md`, extend the `model` row to note `a2a-bridge models` lists advertised values and the card's `agent-models` extension carries them. In `AGENTS.md`, add `a2a-bridge models` to the command list.

- [ ] **Step 3: Mode-override decision (spec Open Q #3).** Live-verify a mode override actually applies: pick an agent that advertises modes, run `a2a-bridge run-workflow` with `message.metadata["a2a-bridge.mode"]` (or a host probe) set to a non-default mode and confirm the agent honors it (transcript/`docker logs`). **If it does NOT apply, drop `modes` from `caps_to_json` (and the card) — advertise models + effort only.** Record the outcome in the spec/ADR.

- [ ] **Step 4: Live DoD gate** (real host adapters; **run with container peers idle** per the dogfood OOM note):

```bash
cargo build -p a2a-bridge --release
# CLI: claude + codex enumerate; kiro via native list; degrade if any unauthed
./target/release/a2a-bridge models --config examples/a2a-bridge.slicing-plan-review.toml
# expect: codex → gpt-5.5, gpt-5.4, gpt-5.4-mini, gpt-5.3-codex-spark ; claude → default,sonnet,sonnet[1m],haiku,claude-fable-5[1m]
./target/release/a2a-bridge models --json --agent codex --config examples/a2a-bridge.slicing-plan-review.toml
# Card: serve with a host config, GET the card, assert the agent-models extension carries those agents
# SIGHUP: `kill -HUP <serve-pid>`; re-fetch the card; assert it still serves (swapped, no dropped requests)
```

- [ ] **Step 5: Pre-merge gate**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: clean. Confirm ci.yml coverage floors hold (ws + per-crate gates).

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(catalog): DRY caps_to_json; docs; mode-override decision; live DoD gate"
```

---

## Self-review notes (resolved)

- **Spec coverage:** ModelCatalog (T1), discovery — kiro/ollama parsers (T2,T3) + ACP describe (T6) + orchestration/degrade/timeout (T7); mode_values+mapper (T4,T5); card extension single-card (T8); serve startup+SIGHUP (T9); CLI subcommand (T10); modes-conditional + docs + live gate (T11). All spec sections mapped.
- **Type consistency:** `AgentCaps`/`ModelCatalog` (bridge-core) used identically across T1/T5/T7/T8/T10; `caps_from_config_options` (T5) consumed by `describe_options` (T6); `probe_all`/`probe_agent` (T7) consumed by T9/T10; `agent_card(.., catalog)` 4-arg signature consistent T8/T9; `caps_to_json` (T11) shared by T8/T10.
- **Anchored bodies (read-then-mirror, not placeholders):** T6 `describe_options` (mint internals, `acp_backend.rs:1140-1240`), T7 `probe_acp_host` (host `AcpBackend` ctor, `make_spawn_fn` `main.rs:455`), T9 serve wiring (`InboundServer` struct + bootstrap). These touch large files; each cites the exact anchor to mirror — implement against the read code, do not invent signatures.
