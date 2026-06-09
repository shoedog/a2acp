# Per-agent MCP servers (`[[agents.mcp]]`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the bridge offer each ACP agent a config-driven list of stdio MCP servers via `[[agents.mcp]]`, flowing through the existing `session/new` `mcpServers` seam with `{cwd}` templated — `prism-mcp` as the first instance, wired to whichever of claude/codex/kiro a front-loaded probe confirms honor the ACP param.

**Architecture:** An SDK-free `McpServerSpec` on `AgentEntry` (bridge-core), mapped to the ACP `McpServer::Stdio` at session-mint in bridge-acp (the sole `new_session_request` chokepoint), with `{cwd}` substituted into args+env. Stdio only → no egress change. The keystone unknown (does each agent honor the ACP param?) is retired by a manual probe (Task 0) BEFORE the mechanism is built.

**Tech Stack:** Rust workspace; `agent-client-protocol` SDK (`McpServer`/`McpServerStdio`/`EnvVariable`, in bridge-acp only); `serde`/`toml`; Docker (containerized agents). No new deps.

**Spec:** `docs/superpowers/specs/2026-06-08-per-agent-mcp-design.md` (v3, contracts pinned).

**Ground-truth references (read first):**
- `crates/bridge-core/src/domain.rs`: `AgentEntry` (89-117, add `mcp`), `SandboxConfig` (44).
- `bin/a2a-bridge/src/config.rs`: `AgentEntryToml` (156-200, add `mcp`), the `AgentEntry { … }` build in `into_snapshot` (637-657).
- `crates/bridge-acp/src/acp_backend.rs`: `AcpConfig` (68-92) + `Default` (114), `new_session_request(cwd)` (382-384, `.mcp_servers(vec![])`), `ensure_session` caller (966, `Self::new_session_request(PathBuf::from(&cwd_for_mint))`).
- `bin/a2a-bridge/src/main.rs`: the `:ro` reader `AcpConfig` literal ending in `..AcpConfig::default()` (183-189), the `ContainerRwConfig` build (~357).
- `crates/bridge-container/src/lib.rs`: `ContainerRwConfig` (41-61, add `mcp`), the per-turn `AcpConfig` literal (~228).
- `crates/bridge-acp/tests/golden_frames.rs`: the wire-golden locking `mcpServers: []` (~59-96).
- ACP SDK (`~/.cargo/.../agent-client-protocol-schema-0.13.2/src/v1/agent.rs`): `McpServer::Stdio`, `McpServerStdio::new(name, command).args(..).env(..)` (2968+), `EnvVariable::new(name, value)` (3029+).
- prism: `~/code/slicing`, `src/bin/prism-mcp.rs` (`--repo`/`--cache-dir`/`--no-cache`); tools `nodes_at`/`callers`/`callees`/`ego-graph` (`src/mcp/`).

**Conventions:** TDD per task. `cargo test -p <crate> <name>` to scope. Commit after each task (`feat(mcp):`). Gate `cargo fmt --all -- --check` + `cargo clippy --workspace --all-targets -- -D warnings` before every commit. Subagent task commits carry NO `Co-Authored-By` trailer; the ADR (Task 7) does.

**Mechanical-literal rule (spec MINOR 6):** adding a field to `AgentEntry`/`AcpConfig`/`ContainerRwConfig` breaks every existing struct literal + test. In the SAME commit that adds the field, `rg 'AgentEntry \{'` (resp. `AcpConfig \{`, `ContainerRwConfig \{`) and add `mcp: vec![]` to each, so the workspace compiles. This is part of the task.

---

### Task 0: Front-loaded probe (manual spike — de-risks the keystone, NOT committed)

**Goal:** answer "does each agent honor the ACP-passed `mcpServers`?" before building anything. This is a throwaway hardcode + live mint, reverted at the end.

- [ ] **Step 1: Build prism-mcp for the container arch**

```bash
cd ~/code/slicing
# Apple-Silicon Docker Desktop = linux/arm64. Build a linux binary (cross or in a builder container):
docker run --rm -v "$PWD":/src -w /src rust:1-slim bash -c \
  'cargo build --release --bin prism-mcp --features mcp && cp target/release/prism-mcp /src/prism-mcp-linux'
mkdir -p ~/.local/share/a2a && cp ~/code/slicing/prism-mcp-linux ~/.local/share/a2a/prism-mcp-linux
```
Expected: a `linux` ELF at `~/.local/share/a2a/prism-mcp-linux`. Confirm `nodes_at`'s exact param names: `rg 'nodes_at' ~/code/slicing/src/mcp` (file + line args).

- [ ] **Step 2: Hardcode ONE stdio server at the seam (throwaway)**

In `crates/bridge-acp/src/acp_backend.rs:383`, temporarily replace:
```rust
NewSessionRequest::new(cwd).mcp_servers(vec![])
```
with (hardcoded, reverted in Step 5):
```rust
use agent_client_protocol_schema::v1::agent::{McpServer, McpServerStdio};
NewSessionRequest::new(cwd).mcp_servers(vec![McpServer::Stdio(
    McpServerStdio::new("prism", "/opt/prism/prism-mcp")
        .args(vec!["--repo".into(), "/Users/wesleyjinks/code/a2a-bridge".into(), "--no-cache".into()]),
)])
```
Add the binary `:ro` mount to claude/codex/kiro `[agents.sandbox].volumes` in `examples/a2a-bridge.containerized.toml`: `"/Users/wesleyjinks/.local/share/a2a/prism-mcp-linux:/opt/prism/prism-mcp:ro"`. For codex, append `["-c","sandbox_mode=\"danger-full-access\"","-c","approval_policy=\"never\""]` to its `args`.

- [ ] **Step 3: Rebuild + mint against each agent**

```bash
cargo build --release --bin a2a-bridge
# For each agent (claude, codex, kiro): run a single-agent workflow whose prompt forces a nodes_at call:
#   "Call ONLY the nodes_at MCP tool with file crates/bridge-core/src/domain.rs line 89. Reply with the raw
#    tool result and nothing else. Do not read files or use any other tool."
# (Use a throwaway single-node workflow per agent, or run-workflow design with one agent; capture docker logs.)
```

- [ ] **Step 4: Record the matrix (the acceptance artifact)**

For each agent, classify EXACTLY one (spec failure taxonomy): **success** / **param-ignored** / **spawn-failed** / **call-failed** / **not-called**. Success = a `tool_call` (server `prism`, tool `nodes_at`) + a non-error `tool_call_update` in the session stream (`docker logs <container>`). Write the result into the spec's matrix and decide which agents the reference config wires.

- [ ] **Step 5: Revert the hardcode**

```bash
git checkout crates/bridge-acp/src/acp_backend.rs examples/a2a-bridge.containerized.toml
```
Expected: clean tree. **Do not commit anything from Task 0.** If NO agent succeeded, STOP and reconsider (the native-config path is a different increment).

---

### Task 1: `McpServerSpec` domain type + `{cwd}` template helpers + `AgentEntry.mcp`

**Files:**
- Modify: `crates/bridge-core/src/domain.rs` (add `McpServerSpec`, `AgentEntry.mcp`)
- Create: `crates/bridge-core/src/mcp.rs` (the pure template validate/substitute helpers)
- Modify: `crates/bridge-core/src/lib.rs` (add `pub mod mcp;` if domain doesn't host it)
- Test: `crates/bridge-core/src/mcp.rs` (`#[cfg(test)]`)

- [ ] **Step 1: Write the failing template-helper tests**

Create `crates/bridge-core/src/mcp.rs`:
```rust
//! Pure `{cwd}` template helpers for MCP server specs (the ONLY supported placeholder is `{cwd}`).

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validate_accepts_only_cwd_token() {
        for ok in ["--repo={cwd}", "/c/{cwd}", "{cwd}", "nodes_at", "--flag"] {
            assert!(validate_cwd_template(ok).is_ok(), "{ok:?} should be OK");
        }
        for bad in ["{repo}", "{{cwd}}", "{cwd", "{\"k\":\"v\"}", "a{b}c"] {
            assert!(validate_cwd_template(bad).is_err(), "{bad:?} should ERROR");
        }
    }
    #[test]
    fn substitute_replaces_all_cwd() {
        assert_eq!(substitute_cwd("--repo={cwd}", "/r/x"), "--repo=/r/x");
        assert_eq!(substitute_cwd("/c/{cwd}/{cwd}", "/r"), "/c/r/r");
        assert_eq!(substitute_cwd("nobrace", "/r"), "nobrace");
    }
}
```

- [ ] **Step 2: Run it (fails)**

Run: `cargo test -p bridge-core mcp:: 2>&1 | tail -5`
Expected: FAIL — `validate_cwd_template`/`substitute_cwd` undefined.

- [ ] **Step 3: Implement the helpers**

In `crates/bridge-core/src/mcp.rs` (above the tests):
```rust
/// Validate that the ONLY `{…}` token in `s` is exactly `{cwd}`. Scans left→right: at each `{`, the substring
/// through the next `}` must equal `{cwd}`; an unterminated `{` or any other `{…}` is an error. (Literal
/// braces / JSON args are unsupported in v1.)
pub fn validate_cwd_template(s: &str) -> Result<(), String> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'{' {
            let close = s[i..].find('}').map(|o| i + o)
                .ok_or_else(|| format!("unterminated '{{' in {s:?}"))?;
            if &s[i..=close] != "{cwd}" {
                return Err(format!("unsupported placeholder {:?} in {s:?} (only {{cwd}} is allowed)", &s[i..=close]));
            }
            i = close + 1;
        } else if b[i] == b'}' {
            return Err(format!("stray '}}' in {s:?}"));
        } else {
            i += 1;
        }
    }
    Ok(())
}

/// Replace every `{cwd}` with `cwd`. Call ONLY on strings that passed `validate_cwd_template`.
#[must_use]
pub fn substitute_cwd(s: &str, cwd: &str) -> String {
    s.replace("{cwd}", cwd)
}
```
Add `pub mod mcp;` to `crates/bridge-core/src/lib.rs`.

- [ ] **Step 4: Run it (passes)**

Run: `cargo test -p bridge-core mcp:: 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 5: Add `McpServerSpec` + `AgentEntry.mcp` + fix all literals**

In `domain.rs`, above `AgentEntry`:
```rust
/// A configured MCP server an ACP agent should spawn at session mint. SDK-free; bridge-acp maps it to the ACP
/// `McpServer::Stdio`. `args`/`env`-values may contain the `{cwd}` placeholder (validated at config load).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerSpec {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}
```
Add to `AgentEntry` (after `sandbox`): `pub mcp: Vec<McpServerSpec>,`. Then `rg 'AgentEntry \{'` across the workspace and add `mcp: vec![]` to every literal (e.g. `domain.rs` tests, `config.rs:637`, route/registry tests) — the build won't compile until all are fixed.

- [ ] **Step 6: Build + test + fmt/clippy + commit**

Run: `cargo build --workspace 2>&1 | tail -5 && cargo test -p bridge-core 2>&1 | tail -5`
Expected: build OK; tests PASS.
```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
git add crates/bridge-core
git commit -m "feat(mcp): McpServerSpec domain type + {cwd} template helpers + AgentEntry.mcp"
```

---

### Task 2: `[[agents.mcp]]` config — `McpToml`/`EnvToml`, validation, conversion

**Files:**
- Modify: `bin/a2a-bridge/src/config.rs` (`McpToml`/`EnvToml`, `AgentEntryToml.mcp`, validation in `into_snapshot`, the `AgentEntry { … }` build)
- Test: `bin/a2a-bridge/src/config.rs` (`#[cfg(test)]`)

- [ ] **Step 1: Write the failing config tests**

```rust
#[test]
fn mcp_config_parse_and_validation() {
    let ok = r#"
default = "a"
allowed_cwd_root = "/x"
[server]
addr = "127.0.0.1:8080"
[[agents]]
id = "a"
cmd = "claude-agent-acp"
[[agents.mcp]]
name = "prism"
command = "/opt/prism/prism-mcp"
args = ["--repo", "{cwd}"]
env = [{ name = "RUST_LOG", value = "warn" }]
"#;
    let snap = RegistryConfig::parse(ok).unwrap().into_snapshot().unwrap();
    let e = snap.entries.iter().find(|e| e.id.as_str() == "a").unwrap();
    assert_eq!(e.mcp.len(), 1);
    assert_eq!(e.mcp[0].command, "/opt/prism/prism-mcp");
    assert_eq!(e.mcp[0].args, vec!["--repo".to_string(), "{cwd}".to_string()]);
    assert_eq!(e.mcp[0].env, vec![("RUST_LOG".to_string(), "warn".to_string())]);

    let bad = |mcp: &str| format!("default=\"a\"\nallowed_cwd_root=\"/x\"\n[server]\naddr=\"127.0.0.1:8080\"\n[[agents]]\nid=\"a\"\ncmd=\"c\"\n{mcp}");
    // empty name, brace in command, bad placeholder, dup name, dup env key
    for m in [
        "[[agents.mcp]]\nname=\"\"\ncommand=\"/x\"",
        "[[agents.mcp]]\nname=\"p\"\ncommand=\"{cwd}/x\"",
        "[[agents.mcp]]\nname=\"p\"\ncommand=\"/x\"\nargs=[\"{repo}\"]",
        "[[agents.mcp]]\nname=\"p\"\ncommand=\"/x\"\n[[agents.mcp]]\nname=\"p\"\ncommand=\"/y\"",
        "[[agents.mcp]]\nname=\"p\"\ncommand=\"/x\"\nenv=[{name=\"K\",value=\"1\"},{name=\"K\",value=\"2\"}]",
    ] {
        assert!(RegistryConfig::parse(&bad(m)).unwrap().into_snapshot().is_err(), "should reject: {m}");
    }
}
```

- [ ] **Step 2: Run it (fails)**

Run: `cargo test -p a2a-bridge mcp_config_parse_and_validation -- --exact 2>&1 | tail -5`
Expected: FAIL — `no field mcp` on `AgentEntryToml`.

- [ ] **Step 3: Add the toml structs + the field**

In `config.rs`:
```rust
#[derive(Debug, Clone, serde::Deserialize)]
pub struct EnvToml { pub name: String, pub value: String }

#[derive(Debug, Clone, serde::Deserialize)]
pub struct McpToml {
    pub name: String,
    pub command: String,
    #[serde(default)] pub args: Vec<String>,
    #[serde(default)] pub env: Vec<EnvToml>,
}
```
Add `#[serde(default)] pub mcp: Vec<McpToml>,` to `AgentEntryToml`.

- [ ] **Step 4: Validate + convert in `into_snapshot`**

Just before the `entries.push(AgentEntry { … })` at `config.rs:637`, build the validated mcp vec:
```rust
let mut mcp = Vec::new();
let mut seen = std::collections::HashSet::new();
for m in a.mcp {
    if m.name.trim().is_empty() { return Err(ConfigError::Registry(format!("agent {id}: [[agents.mcp]] name must be non-empty"))); }
    if !seen.insert(m.name.clone()) { return Err(ConfigError::Registry(format!("agent {id}: duplicate mcp server name {:?}", m.name))); }
    if m.command.trim().is_empty() { return Err(ConfigError::Registry(format!("agent {id}: mcp {:?} command must be non-empty", m.name))); }
    if m.command.contains('{') { return Err(ConfigError::Registry(format!("agent {id}: mcp {:?} command must not contain a placeholder", m.name))); }
    for arg in &m.args {
        bridge_core::mcp::validate_cwd_template(arg).map_err(|e| ConfigError::Registry(format!("agent {id}: mcp {:?} arg: {e}", m.name)))?;
    }
    let mut envseen = std::collections::HashSet::new();
    let mut env = Vec::new();
    for e in m.env {
        if e.name.trim().is_empty() { return Err(ConfigError::Registry(format!("agent {id}: mcp {:?} env name must be non-empty", m.name))); }
        if !envseen.insert(e.name.clone()) { return Err(ConfigError::Registry(format!("agent {id}: mcp {:?} duplicate env key {:?}", m.name, e.name))); }
        bridge_core::mcp::validate_cwd_template(&e.value).map_err(|err| ConfigError::Registry(format!("agent {id}: mcp {:?} env {:?}: {err}", m.name, e.name)))?;
        env.push((e.name, e.value));
    }
    mcp.push(bridge_core::domain::McpServerSpec { name: m.name, command: m.command, args: m.args, env });
}
```
Add `mcp,` to the `AgentEntry { … }` literal.

- [ ] **Step 5: Run it (passes), fmt/clippy, commit**

Run: `cargo test -p a2a-bridge mcp_config_parse_and_validation -- --exact 2>&1 | tail -5` (Expected: PASS)
```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
git add bin/a2a-bridge/src/config.rs
git commit -m "feat(mcp): [[agents.mcp]] config — McpToml/EnvToml + validation + conversion"
```

---

### Task 3: bridge-acp mapping — `AcpConfig.mcp` + `new_session_request` `{cwd}` mapping + wire-golden

**Files:**
- Modify: `crates/bridge-acp/src/acp_backend.rs` (`AcpConfig.mcp`, `Default`, `new_session_request`, the `ensure_session` caller at 966)
- Modify: `crates/bridge-acp/tests/golden_frames.rs` (the `mcpServers` golden)
- Test: `crates/bridge-acp/src/acp_backend.rs` + `golden_frames.rs`

- [ ] **Step 1: Write the failing mapping test**

In `acp_backend.rs` tests:
```rust
#[test]
fn new_session_request_maps_specs_and_substitutes_cwd() {
    use bridge_core::domain::McpServerSpec;
    let specs = vec![McpServerSpec {
        name: "prism".into(), command: "/opt/prism/prism-mcp".into(),
        args: vec!["--repo".into(), "{cwd}".into()],
        env: vec![("CACHE".into(), "/c/{cwd}".into())],
    }];
    let req = AcpBackend::new_session_request("/repo/x", &specs);
    // serialize and assert the populated mcpServers shape with {cwd} substituted
    let v = serde_json::to_value(&req).unwrap();
    let s0 = &v["mcpServers"][0];
    assert_eq!(s0["name"], "prism");
    assert_eq!(s0["args"][1], "/repo/x");                 // {cwd} substituted in args
    assert_eq!(s0["env"][0]["value"], "/c//repo/x");      // {cwd} substituted in env value
    // empty specs -> empty array (unchanged behavior)
    let empty = AcpBackend::new_session_request("/repo/x", &[]);
    assert_eq!(serde_json::to_value(&empty).unwrap()["mcpServers"], serde_json::json!([]));
}
```

- [ ] **Step 2: Run it (fails)**

Run: `cargo test -p bridge-acp new_session_request_maps_specs -- --exact 2>&1 | tail -5`
Expected: FAIL — `new_session_request` takes 1 arg.

- [ ] **Step 3: Add `AcpConfig.mcp` + map in `new_session_request`**

Add `pub mcp: Vec<bridge_core::domain::McpServerSpec>,` to `AcpConfig` (68); add `mcp: vec![]` to its `Default` impl (114). Rewrite `new_session_request` (382):
```rust
pub fn new_session_request(
    cwd: impl Into<PathBuf>,
    mcp: &[bridge_core::domain::McpServerSpec],
) -> NewSessionRequest {
    let cwd = cwd.into();
    let cwd_s = cwd.to_string_lossy().to_string();
    let servers: Vec<McpServer> = mcp.iter().map(|s| {
        let args = s.args.iter().map(|a| bridge_core::mcp::substitute_cwd(a, &cwd_s)).collect::<Vec<_>>();
        let env = s.env.iter()
            .map(|(k, v)| EnvVariable::new(k.clone(), bridge_core::mcp::substitute_cwd(v, &cwd_s)))
            .collect::<Vec<_>>();
        McpServer::Stdio(McpServerStdio::new(s.name.clone(), s.command.clone()).args(args).env(env))
    }).collect();
    NewSessionRequest::new(cwd).mcp_servers(servers)
}
```
Add the SDK imports (`McpServer`, `McpServerStdio`, `EnvVariable`) at the top of `acp_backend.rs`.

- [ ] **Step 4: Update the `ensure_session` caller (966)**

Change `Self::new_session_request(PathBuf::from(&cwd_for_mint))` → `Self::new_session_request(PathBuf::from(&cwd_for_mint), &self.config.mcp)`. (`self.config` is the `AcpConfig`.)

- [ ] **Step 5: Update the wire-golden**

In `tests/golden_frames.rs` (~59-96), the empty-`mcpServers` golden uses `new_session_request(cwd)` — update the call to `new_session_request(cwd, &[])` (still `mcpServers: []`), and ADD a second assertion with two specs incl. `env`, asserting the serialized array shape (name/command/args/env) with `{cwd}` substituted.

- [ ] **Step 6: Run, fmt/clippy, commit**

Run: `cargo test -p bridge-acp new_session_request_maps_specs golden 2>&1 | tail -8` (Expected: PASS)
```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
git add crates/bridge-acp
git commit -m "feat(mcp): map McpServerSpec -> ACP McpServer::Stdio at mint with {cwd} substitution"
```

---

### Task 4: Threading — `:ro` exhaustive literal + `:rw` `ContainerRwConfig.mcp`

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs` (the `:ro` `AcpConfig` literal at 183; the `ContainerRwConfig` build at ~357)
- Modify: `crates/bridge-container/src/lib.rs` (`ContainerRwConfig.mcp` at 41; the per-turn `AcpConfig` literal at ~228)
- Test: `bin/a2a-bridge/src/main.rs` (in-file unit test — `acp_spawn_inputs` is private, keep the test in-module)

- [ ] **Step 1: Thread into the `:ro` builder (drop `..default()`)**

At `main.rs:183`, add `mcp: entry.mcp.clone(),` AND replace `..bridge_acp::acp_backend::AcpConfig::default()` with the two remaining fields spelled out (so the compiler enforces `mcp` here — spec MAJOR 4):
```rust
    let acp = bridge_acp::acp_backend::AcpConfig {
        cwd,
        model: entry.model.clone(),
        mode: entry.mode.clone(),
        auth_method: entry.auth_method.clone(),
        container,
        mcp: entry.mcp.clone(),
        handshake_timeout: bridge_acp::acp_backend::DEFAULT_HANDSHAKE_TIMEOUT,
        cancel_grace: bridge_acp::acp_backend::DEFAULT_CANCEL_GRACE,
    };
```
(Confirm the `DEFAULT_*` const names/visibility in `acp_backend.rs`; if private, `pub` them or keep `..default()` AND add `mcp:` explicitly above it — but prefer the exhaustive literal.)

- [ ] **Step 2: Thread into `ContainerRwConfig` + the per-turn `AcpConfig`**

Add `pub mcp: Vec<bridge_core::domain::McpServerSpec>,` to `ContainerRwConfig` (`lib.rs:41`). At its per-turn `AcpConfig` literal (`lib.rs:228`), add `mcp: self.cfg.mcp.clone(),` (match the field name the struct uses). At `main.rs:357` where `ContainerRwConfig { … }` is built, add `mcp: entry.mcp.clone(),`. Then `rg 'ContainerRwConfig \{'` and add `mcp: vec![]` to any test literals.

- [ ] **Step 3: Write + run the threading test (drives the REAL main.rs builder)**

In `main.rs` in-file tests, build an `AgentEntry` with one `McpServerSpec` and assert the `:ro` builder (`acp_spawn_inputs` / the fn holding the `main.rs:183` literal) yields an `AcpConfig` whose `mcp` carries it:
```rust
#[test]
fn ro_builder_threads_mcp() {
    let entry = /* minimal AgentEntry with mcp: vec![McpServerSpec{ name:"prism".into(), command:"/x".into(), args:vec![], env:vec![] }] */;
    let (_p, _argv, acp) = acp_spawn_inputs(&entry /*, …*/).unwrap();
    assert_eq!(acp.mcp.len(), 1);
    assert_eq!(acp.mcp[0].name, "prism");
}
```
(Use the real builder fn name + args from `main.rs:160-191`; construct the minimal `AgentEntry` the same way other `main.rs` tests do.)

Run: `cargo test -p a2a-bridge ro_builder_threads_mcp -- --exact 2>&1 | tail -5` → PASS.

- [ ] **Step 4: Build workspace, fmt/clippy, commit**

Run: `cargo build --workspace 2>&1 | tail -5 && cargo test --workspace 2>&1 | tail -8`
Expected: build OK; tests PASS (the exhaustive literal proves the `:ro` site is compiler-guarded).
```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
git add bin/a2a-bridge/src/main.rs crates/bridge-container/src/lib.rs
git commit -m "feat(mcp): thread entry.mcp into the :ro (exhaustive literal) + :rw AcpConfig sites"
```

---

### Task 5: Reference config + docs

**Files:**
- Modify: `examples/a2a-bridge.containerized.toml` (wire prism to the probe-confirmed agents)
- Modify: `docs/containerized-agents.md`, `AGENTS.md`
- Test: a config-parse test that the updated reference config loads

- [ ] **Step 1: Wire prism into the reference config**

For each agent Task 0 confirmed honors the param, add under its `[[agents]]`:
```toml
[[agents.mcp]]
name    = "prism"
command = "/opt/prism/prism-mcp"
args    = ["--repo", "{cwd}", "--cache-dir", "/tmp/prism/{cwd}"]   # per-repo cache (spec MINOR 8)
```
and to that agent's `[agents.sandbox].volumes`: `"/Users/wesleyjinks/.local/share/a2a/prism-mcp-linux:/opt/prism/prism-mcp:ro"`. For the `:ro` codex entries, append `"-c", "sandbox_mode=\"danger-full-access\"", "-c", "approval_policy=\"never\""` to `args`.

- [ ] **Step 2: Add a parse test**

In `config.rs` tests (or an integration test), load `examples/a2a-bridge.containerized.toml` and assert `into_snapshot()` succeeds and at least one agent has a non-empty `mcp`.

Run: `cargo test -p a2a-bridge containerized_config 2>&1 | tail -5` → PASS.

- [ ] **Step 3: Docs**

In `docs/containerized-agents.md` + `AGENTS.md`: the `[[agents.mcp]]` schema; the **command == volumes-mount-RHS** invariant + the symptom→cause note (a typo shows as the **spawn-failed** class, distinct from **param-ignored** — spec MINOR 9); build-prism-for-linux/arm64; egress-unchanged; the per-agent support matrix from Task 0; the deferred native-config fallback.

- [ ] **Step 4: fmt + commit**

```bash
cargo fmt --all
git add examples/a2a-bridge.containerized.toml docs/containerized-agents.md AGENTS.md bin/a2a-bridge/src/config.rs
git commit -m "feat(mcp): wire prism into the containerized reference config + docs"
```

---

### Task 6: Live probe + dogfood (operator gate)

Not a unit test — operator-run against the REAL config (peers idle).

- [ ] **Step 1: Re-run the per-agent matrix** — for each wired agent, the Task-0 tool-forcing prompt; classify success/param-ignored/spawn-failed/call-failed/not-called from `docker logs`. Confirm the support matrix in the spec/docs matches.
- [ ] **Step 2: Dogfood** — run `design` or `code-review` on a real diff; confirm an agent calls a prism tool (`nodes_at`/`callers`) and the review/design references slicing evidence. Capture the transcript.
- [ ] **Step 3: Confirm egress unchanged** — `docker logs a2a-egress-proxy` shows no new outbound from the prism subprocess (stdio, in-container).

---

### Task 7: ADR-0028 + §codex-sandbox

**Files:**
- Create: `docs/adr/0028-per-agent-mcp.md`

- [ ] **Step 1: Write ADR-0028**

Sections: **Context** (agents need MCP tools; the `new_session_request` seam already owns `mcpServers`). **Decision** (config-driven `[[agents.mcp]]` → SDK-free `McpServerSpec` → `McpServer::Stdio` at mint, `{cwd}` templated, stdio-only). **§codex-sandbox** (verbatim from the spec: premise verified via the merge plan-review's `bubblewrap unavailable`; bake-bwrap considered-and-REJECTED — needs userns or `CAP_SYS_ADMIN`+seccomp; `:ro`-only scope; future cross-namespace agents named). **Alternatives** (per-agent native config files — deferred; HTTP/SSE — deferred). **Consequences** (no egress change; the support matrix; the command==mount operator contract).

- [ ] **Step 2: Commit (ADR carries the trailer)**

```bash
git add docs/adr/0028-per-agent-mcp.md
git commit -m "$(printf 'docs: ADR-0028 per-agent MCP servers\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Self-Review (checklist)

**Spec coverage:** mechanism→T1/T2/T3/T4; `{cwd}` invariant + templating→T1/T3; validation/scanner→T1/T2; probe harness→T0/T6; threading + `..default()` guard→T4; reference config + per-repo cache + codex flags→T5; docs + symptom→cause→T5; ADR §codex-sandbox→T7; deferred follow-ups→ADR (T7). All spec sections mapped.

**Placeholder scan:** every code step shows complete code; commands have expected output; the only "fill in" is Task-0/Task-6 operator observations (inherently manual) and the Task-4 minimal-`AgentEntry` construction (deferred to "same as other main.rs tests" — acceptable for an in-file test mirroring existing ones).

**Type consistency:** `McpServerSpec { name, command, args, env: Vec<(String,String)> }` (T1) is the one type threaded through `AgentEntry.mcp` (T1) → `AcpConfig.mcp`/`ContainerRwConfig.mcp` (T3/T4) → `new_session_request(cwd, &[McpServerSpec])` (T3); `validate_cwd_template`/`substitute_cwd` (T1) used by config validation (T2) and the mint mapping (T3); `McpToml`/`EnvToml` (T2) convert to `McpServerSpec`. Names consistent across tasks.
