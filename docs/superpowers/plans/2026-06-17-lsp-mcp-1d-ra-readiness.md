# #1d rust-analyzer readiness under per-turn serve/run-workflow — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make in-container rust/go nav work under the per-turn `run-workflow` path (warm the dep cache + inject offline lsp-env at registry-build time), and make every path (incl. `serve`) fail honestly when rust-analyzer isn't ready instead of returning misleading empty hits.

**Architecture:** Part A (lsp-mcp): configurable readiness budget + an honest "still indexing / couldn't index offline" tool response. Part B (bin/a2a-bridge): `run-workflow` pre-warms each `container_rw` entry up front (it stamps a fixed `--session-cwd` before registry build), reusing the exact `build_warm_impl` machinery via a shared `apply_warm_lsp` helper. `serve` per-request warm is OUT of scope (deferred follow-up); it benefits from Part A only.

**Tech Stack:** Rust workspace. Crates touched: `lsp-mcp` (Part A), `bin/a2a-bridge` (Part B). No cross-crate seam (bridge-container untouched).

**Spec:** `docs/superpowers/specs/2026-06-17-lsp-mcp-1d-ra-readiness-per-turn.md`

---

## File Structure

- `crates/lsp-mcp/src/mcp/mod.rs` — Part A: `parse_ready_secs` + `ready_timeout()` + `not_ready_response()` + `dispatch` rewrite.
- `crates/lsp-mcp/src/lsp/mod.rs` — Part A: `wait_ready`/`ensure_ready` return `bool` (became-ready vs timed-out); latch `readied` only on true.
- `bin/a2a-bridge/src/implement.rs` — Part B Task 3: `compose_warm_fetch` gains `read_only: bool`.
- `bin/a2a-bridge/src/main.rs` — Part B: `warm_lsp_deps_step` gains `read_only`; new shared `apply_warm_lsp`; `build_warm_impl` calls it; `run_workflow_cmd` pre-mutation.

---

## Task 1: Configurable readiness budget (lsp-mcp, pure helper)

**Files:**
- Modify: `crates/lsp-mcp/src/mcp/mod.rs`
- Test: same file (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/lsp-mcp/src/mcp/mod.rs`:

```rust
#[test]
fn parse_ready_secs_defaults_and_overrides() {
    assert_eq!(parse_ready_secs(None), 90);
    assert_eq!(parse_ready_secs(Some(String::new())), 90);
    assert_eq!(parse_ready_secs(Some("notanum".into())), 90);
    assert_eq!(parse_ready_secs(Some("0".into())), 90); // 0 is meaningless → default
    assert_eq!(parse_ready_secs(Some("120".into())), 120);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lsp-mcp parse_ready_secs`
Expected: FAIL — `parse_ready_secs` not found.

- [ ] **Step 3: Write minimal implementation**

Add near the top of `crates/lsp-mcp/src/mcp/mod.rs` (after the `use` lines):

```rust
/// Readiness budget for `ensure_ready`, in seconds. `LSP_MCP_READY_SECS` overrides the default; a
/// cold (even warm-cached) rust-analyzer cold index can exceed the old 30s. Pure for testing.
fn parse_ready_secs(var: Option<String>) -> u64 {
    var.and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(90)
}

/// The configured readiness budget as a Duration.
fn ready_timeout() -> std::time::Duration {
    std::time::Duration::from_secs(parse_ready_secs(std::env::var("LSP_MCP_READY_SECS").ok()))
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p lsp-mcp parse_ready_secs`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/lsp-mcp/src/mcp/mod.rs
git commit -m "feat(lsp-mcp): configurable readiness budget (LSP_MCP_READY_SECS, default 90)"
```

---

## Task 2: Honest not-ready tool response (lsp-mcp)

**Files:**
- Modify: `crates/lsp-mcp/src/lsp/mod.rs` (`wait_ready`, `ensure_ready` return `bool`)
- Modify: `crates/lsp-mcp/src/mcp/mod.rs` (`not_ready_response` + `dispatch` rewrite)
- Test: `crates/lsp-mcp/src/mcp/mod.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/lsp-mcp/src/mcp/mod.rs`:

```rust
#[test]
fn not_ready_response_is_iserror_with_retry_hint() {
    let r = not_ready_response(&json!(7));
    assert_eq!(r["id"], json!(7));
    assert_eq!(r["result"]["isError"], json!(true));
    let txt = r["result"]["content"][0]["text"].as_str().unwrap();
    assert!(txt.contains("indexing"), "{txt}");
    assert!(txt.contains("retry"), "{txt}");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lsp-mcp not_ready_response`
Expected: FAIL — `not_ready_response` not found.

- [ ] **Step 3a: Change `wait_ready`/`ensure_ready` to report readiness**

In `crates/lsp-mcp/src/lsp/mod.rs`, change `wait_ready` to return `bool` (true = became ready, false = timed out):

```rust
    pub fn wait_ready(&mut self, timeout: Duration) -> anyhow::Result<bool> {
        let t0 = Instant::now();
        loop {
            // An in-progress index wait is active use — touch so the watcher can't evict the server
            // mid-index (a slow in-container cold/re-index can exceed the idle timeout otherwise).
            self.touch();
            {
                let g = self.ready.lock().unwrap();
                if g.is_ready() || settled_no_progress(&g) {
                    return Ok(true);
                }
            }
            if t0.elapsed() >= timeout {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Lazily ensure the index is ready — waits only until ready (idempotent once ready). Returns whether
    /// the server is ready. A timed-out wait does NOT latch `readied`, so the agent's retry waits again
    /// (giving a slow cold index more time) instead of permanently short-circuiting.
    pub fn ensure_ready(&mut self, timeout: std::time::Duration) -> anyhow::Result<bool> {
        if self.evicted.load(Ordering::SeqCst) {
            self.respawn()?;
        }
        if self.readied {
            return Ok(true);
        }
        let ready = self.wait_ready(timeout)?;
        if ready {
            self.readied = true;
        }
        Ok(ready)
    }
```

- [ ] **Step 3b: Add `not_ready_response` + rewrite `dispatch`**

In `crates/lsp-mcp/src/mcp/mod.rs`, add the helper (near `ok`):

```rust
/// The honest "RA not ready" tool reply: an `isError` content the agent can read and retry on, instead of
/// an empty hit list it misreads as "no lsp tool".
fn not_ready_response(id: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "isError": true,
            "content": [{
                "type": "text",
                "text": "rust-analyzer is still indexing (or could not index offline); retry shortly"
            }]
        }
    })
}
```

Then rewrite the body of `dispatch` (replace the `let result = ...; match result { ... }` block):

```rust
    s.touch();
    match s.ensure_ready(ready_timeout()) {
        Err(e) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "isError": true,
                "content": [{ "type": "text", "text": format!("lsp-mcp error: {e}") }] }
        }),
        Ok(false) => not_ready_response(id),
        Ok(true) => match dispatch_body(tool, &a, s) {
            Ok(body) => ok(id, body),
            Err(e) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "isError": true,
                    "content": [{ "type": "text", "text": format!("lsp-mcp error: {e}") }] }
            }),
        },
    }
```

(Remove the old hardcoded `from_secs(30)` call — `ready_timeout()` replaces it.)

- [ ] **Step 4: Run tests + build**

Run: `cargo test -p lsp-mcp not_ready_response && cargo build -p lsp-mcp`
Expected: PASS + clean build (the `bool` return is consumed only inside `dispatch`; no other callers — verified).

- [ ] **Step 5: Commit**

```bash
git add crates/lsp-mcp/src/lsp/mod.rs crates/lsp-mcp/src/mcp/mod.rs
git commit -m "feat(lsp-mcp): honest not-ready tool response (no misleading empty hits)"
```

---

## Task 3: `compose_warm_fetch` read-only repo mount (implement.rs)

**Why:** on the per-turn path the "clone" is the user's REAL repo — the warm fetch must mount it `:ro` (cargo/go fetch reads the lock, writes only to the cache vol). `implement`'s disposable quarantine clone keeps `:rw` (no behavior change).

**Files:**
- Modify: `bin/a2a-bridge/src/implement.rs` (`compose_warm_fetch` signature + the `/work` mount)
- Modify: `bin/a2a-bridge/src/main.rs` (`warm_lsp_deps_step` passes `read_only`; the 2 implement call sites pass `false`)
- Test: `bin/a2a-bridge/src/implement.rs` tests

- [ ] **Step 1: Write the failing test**

In `bin/a2a-bridge/src/implement.rs` tests, add (and update the existing byte-for-byte test to pass `false`):

```rust
#[test]
fn compose_warm_fetch_read_only_mounts_work_ro() {
    let p = bridge_core::profile::rust_profile();
    let e = WarmEgress { network: "n".into(), proxy: "http://p:8888".into() };
    let binding = p.cache_binding(bridge_core::profile::CacheCtx::Fetch, "vol", "");
    let (_prog, argv) = compose_warm_fetch("docker", "img:latest", "/clone", &binding, &p.fetch_cmd, &e, true);
    assert!(argv.iter().any(|a| a == "/clone:/work:ro"), "expected :ro work mount, got {argv:?}");
    assert!(!argv.iter().any(|a| a == "/clone:/work"), "must not also mount rw");
}
```

Update the existing `compose_warm_fetch_via_binding_is_byte_for_byte` (and any other caller in tests) to pass `false` as the new last arg; its expected output is unchanged.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p a2a-bridge compose_warm_fetch`
Expected: FAIL — arity mismatch / `:ro` mount absent.

- [ ] **Step 3: Implement**

In `compose_warm_fetch` add `read_only: bool` as the final param and change the `/work` mount:

```rust
    argv.push("-v".into());
    argv.push(if read_only {
        format!("{clone}:/work:ro")
    } else {
        format!("{clone}:/work")
    });
```

In `main.rs`, `warm_lsp_deps_step` gains `read_only: bool` (final param) and forwards it into the `compose_warm_fetch(...)` call (add `, read_only` as the last arg). Update the two `warm_lsp_deps_step(...)` call sites in the `implement` paths (`implement_cmd` ~line 1803, `implement_resume_cmd` ~line 2100) to pass `false`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p a2a-bridge compose_warm_fetch && cargo build -p a2a-bridge`
Expected: PASS + clean build.

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/implement.rs bin/a2a-bridge/src/main.rs
git commit -m "feat(implement): compose_warm_fetch read_only repo mount (:ro) for the per-turn path"
```

---

## Task 4: Shared `apply_warm_lsp` helper (extract from `build_warm_impl`)

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs` (extract helper; `build_warm_impl` calls it)
- Test: `bin/a2a-bridge/src/main.rs` tests

- [ ] **Step 1: Write the failing test**

In `main.rs` tests:

```rust
#[test]
fn apply_warm_lsp_injects_env_mounts_and_target_vol() {
    let p = bridge_core::profile::rust_profile();
    let mut mcp = vec![bridge_core::mcp::McpServerSpec {
        name: "lsp".into(),
        command: "/usr/local/bin/lsp-mcp".into(),
        args: vec!["--repo".into(), "{cwd}".into()],
        env: vec![],
    }];
    let mut vols: Vec<String> = vec![];
    apply_warm_lsp(&mut mcp, &mut vols, Some(&p), Some("warmvol"), std::path::Path::new("/tmp/repo"));
    // env injected onto the lsp spec
    assert!(mcp[0].env.iter().any(|e| e.name == "CARGO_HOME"), "lsp env missing CARGO_HOME: {:?}", mcp[0].env);
    // warm dep cache mounted (because warm vol is Some) + the per-repo target vol mounted
    assert!(vols.iter().any(|v| v.contains("warmvol")), "missing /cargo mount: {vols:?}");
    assert!(vols.iter().any(|v| v.ends_with(":/lsp-target")), "missing /lsp-target mount: {vols:?}");
}
```

(Confirm the exact `McpServerSpec` field names by reading `crates/bridge-core/src/mcp.rs`; adjust the literal if `env` uses a different shape — `apply_lsp_env`'s existing usage is the source of truth.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p a2a-bridge apply_warm_lsp`
Expected: FAIL — `apply_warm_lsp` not found.

- [ ] **Step 3: Extract the helper**

Add to `main.rs` (near `apply_lsp_env`):

```rust
/// Inject the selected language profile's in-container LSP nav into a `container_rw` agent's `mcp` + sandbox
/// `volumes`: the profile's Lsp env (e.g. `CARGO_HOME=/cargo`, `CARGO_NET_OFFLINE=true`), the warmed dep
/// cache mount at `/cargo` (RO; only when `warm_cache_vol` is `Some` — i.e. the warm fetch succeeded), and a
/// writable per-repo target cache at `/lsp-target` (keyed on the SOURCE repo so it is reused, not leaked per
/// run). `profile == None` (`--lang none`) drops the lsp server. Shared by `build_warm_impl` (implement) and
/// the `run-workflow` pre-mutation so the two per-turn flows can't drift.
fn apply_warm_lsp(
    mcp: &mut Vec<bridge_core::mcp::McpServerSpec>,
    volumes: &mut Vec<String>,
    profile: Option<&bridge_core::profile::LanguageProfile>,
    warm_cache_vol: Option<&str>,
    repo: &std::path::Path,
) {
    let target_canon = std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
    let target_vol = verify::cache_volume_name("a2a-impl-lsp-target", &target_canon.to_string_lossy());
    match profile {
        None => drop_lsp(mcp),
        Some(p) => {
            let lsp = p.cache_binding(
                bridge_core::profile::CacheCtx::Lsp,
                warm_cache_vol.unwrap_or(""),
                "",
            );
            apply_lsp_env(mcp, &lsp.env);
            if warm_cache_vol.is_some() {
                volumes.extend(lsp.mounts);
            }
        }
    }
    volumes.push(format!("{target_vol}:/lsp-target"));
}
```

- [ ] **Step 4: Make `build_warm_impl` call it (byte-for-byte)**

Replace `build_warm_impl`'s inline block (`main.rs` ~1434–1458: the `target_canon`/`impl_lsp_target_vol` computation, the `match profile { ... }`, and the unconditional `ccfg.sandbox.volumes.push(...)`) with a single call:

```rust
    apply_warm_lsp(
        &mut ccfg.mcp,
        &mut ccfg.sandbox.volumes,
        profile,
        impl_lsp_cache_vol,
        repo,
    );
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p a2a-bridge apply_warm_lsp drop_lsp && cargo build -p a2a-bridge`
Expected: PASS (existing `implement` tests unaffected — the extraction is byte-for-byte).

- [ ] **Step 6: Commit**

```bash
git add bin/a2a-bridge/src/main.rs
git commit -m "refactor(implement): extract shared apply_warm_lsp helper (no behavior change)"
```

---

## Task 5: `run-workflow` warm-on-spawn pre-mutation (HIGH-RISK — codex review)

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs` (`run_workflow_cmd`, between snapshot/stamp and `make_spawn_fn`)

- [ ] **Step 1: Implement the pre-mutation**

In `run_workflow_cmd`, **before** `let cfg = ... .into_snapshot()` (line ~2242), capture verify + select the profile (best-effort; `select_profile(Auto)` ERRORS on undetected/ambiguous, so swallow the error):

```rust
    // #1d: warm the in-container LSP dep cache up front for container_rw agents. run-workflow targets ONE
    // repo (the stamped --session-cwd), so the profile + warm are resolved once and applied to every
    // container_rw entry. Best-effort: any failure degrades to no in-container nav (Part A reports it).
    let verify_cfg_raw = cfg.verify.as_ref().map(|t| t.to_config());
    let warm_repo: Option<std::path::PathBuf> = session_cwd.as_ref().map(std::path::PathBuf::from);
    let warm_profile = match warm_repo.as_ref() {
        Some(repo) => match select_profile(&cfg, &LangArg::Auto, repo) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[run-workflow] lsp warm: language detect skipped: {e:?}");
                None
            }
        },
        None => None,
    };
```

Then, **after** `into_snapshot()` produces `snapshot` and **after** the `session_cwd` stamping loop (line ~2255), gate the verify runtime and warm/mutate (guarded so pure host-side workflows pay nothing):

```rust
    let has_container_rw = snapshot
        .entries
        .iter()
        .any(|e| e.kind == bridge_core::domain::AgentKind::ContainerRw);
    if has_container_rw {
        if let (Some(repo), Some(p)) = (warm_repo.as_ref(), warm_profile.as_ref()) {
            let verify_cfg = config::gate_verify_runtime(verify_cfg_raw, &snapshot.allowed_cmds);
            // read_only=true: the per-turn "clone" is the user's REAL repo — never mutate it.
            let warm_vol = warm_lsp_deps_step(&verify_cfg, Some(p), repo, repo, true);
            for e in &mut snapshot.entries {
                if e.kind == bridge_core::domain::AgentKind::ContainerRw {
                    if let Some(sb) = e.sandbox.as_mut() {
                        apply_warm_lsp(&mut e.mcp, &mut sb.volumes, Some(p), warm_vol.as_deref(), repo);
                    }
                }
            }
        }
    }
```

(If `verify_cfg_raw` is moved/borrowed elsewhere, capture a clone before this block. Confirm `gate_verify_runtime`'s signature at `config.rs` — it takes the owned `Option<Result<VerifyConfig, _>>` + `&[String]` allowed_cmds.)

- [ ] **Step 2: Build + clippy + fmt + existing tests**

Run:
```bash
cargo build -p a2a-bridge && cargo clippy -p a2a-bridge --all-targets -- -D warnings && cargo fmt -p a2a-bridge -- --check && cargo test -p a2a-bridge
```
Expected: clean. (No unit test for the wiring itself — covered by the live gate, Task 6. A wiring unit test would require a full RegistryConfig + docker; out of proportion.)

- [ ] **Step 3: Commit**

```bash
git add bin/a2a-bridge/src/main.rs
git commit -m "feat(run-workflow): warm-on-spawn LSP dep cache for container_rw agents (#1d)"
```

---

## Task 6: Live gate (DoD)

**Build the release binary first** (the dogfood/gate runs the release binary — a stale binary silently omits the new wiring):
```bash
cargo build --release --bin a2a-bridge --bin lsp-mcp
```

- [ ] **Gate 1 — rust nav works under run-workflow.** Create a tiny deps-bearing rust repo under `/Users/wesleyjinks/code` (with a committed `Cargo.lock`, e.g. one `serde` dep), then:
  ```bash
  ./target/release/a2a-bridge run-workflow c2b-nav --input /dev/null \
    --session-cwd /Users/wesleyjinks/code/<rust-fixture> \
    --config examples/a2a-bridge.containerized.toml --out /tmp/1d-rust.md
  ```
  Expected: a nav tool (`hover`/`definition`) returns a **type-resolved** result (NOT empty, NOT "no lsp tool"). Confirm the warm fetch ran via the registries-egress (`[implement] lsp warm-deps: ok ...` on stderr) and that RA reached quiescent.

- [ ] **Gate 2 — repo not mutated.** `git -C /Users/wesleyjinks/code/<rust-fixture> status --porcelain` is empty after the run (the `:ro` mount left the repo untouched — no rewritten `Cargo.lock`).

- [ ] **Gate 3 — cache reuse, no leak.** `docker volume ls | grep a2a-impl-lsp` shows ONE dep-cache vol + ONE target vol for the fixture; a second run reuses them (no per-run proliferation).

- [ ] **Gate 4 — honest degrade.** Point the same workflow at a no-language dir (or a repo whose language has no `[[languages]]` profile): the nav tool returns the "still indexing / could not index offline; retry" message (Part A), not empty hits. (`LSP_MCP_READY_SECS=5` makes the timeout fast to observe.)

- [ ] **Gate 5 — no regression.** A host-side workflow with no container_rw agents (e.g. `code-review`) pays no warm cost (no `lsp warm-deps` log); the `implement` warm path still works (`apply_warm_lsp` extraction is byte-for-byte).

---

## Self-Review notes (author)

- **Spec coverage:** Part A = Tasks 1–2; Part B = Tasks 3–5; DoD = Task 6. serve deferred (spec §Out of scope) — not a task.
- **Type consistency:** `apply_warm_lsp` signature is identical at both call sites (`build_warm_impl`, `run_workflow_cmd`); `ensure_ready`/`wait_ready` now return `bool` (only caller is `dispatch`); `warm_lsp_deps_step`/`compose_warm_fetch` gain a trailing `read_only: bool` (implement callers pass `false`, run-workflow passes `true`).
- **Risk:** Task 5 is the integration (config capture ordering around `into_snapshot`, entry mutation reaching the spawn). Codex reviews Task 5 + the final branch; Opus reviews Tasks 1–4 per-task.
- **Open confirmations for the implementer:** (1) `McpServerSpec` field names in the Task-4 test (read `bridge-core/src/mcp.rs`); (2) `gate_verify_runtime` ownership/signature; (3) `bridge_core::profile::rust_profile()` is `pub` for the tests. Adjust literals to match real signatures — do not invent.
