# JS/TS in the containerized polyglot implementor — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Add `Lang::TypeScript` (typescript-language-server, swappable) to lsp-mcp + a `typescript`
`[[languages]]` profile + toolchain image layer + npm egress, so the containerized implementor navigates/
verifies TS/JS. Keystone spiked (`docs/superpowers/spikes/2026-06-17-jsts-node-modules-resolution.md`):
fixed `/node_modules` resolves via the root-walk → reuse the fixed-path machinery UNCHANGED.

**Tech Stack:** Rust (lsp-mcp — the new code); Docker/mise + `a2a-toolchain`; typescript-language-server +
typescript + node (image base). **Spec:** `docs/superpowers/specs/2026-06-17-lsp-mcp-jsts-polyglot.md`.
**MUST-READ:** `docs/containerized-mcp-env-trap.md`.

---

## Task 1: lsp-mcp — `Lang::TypeScript` + `ts_config` + shared `SettleReady` (HIGH-RISK; codex-reviewed)

**Files:** `crates/lsp-mcp/src/lang.rs` (readiness extraction + Lang + detect + ts_config), `lib.rs`
(dispatch). Tests: `lang.rs`.

### 1a — Extract a shared `SettleReady` (refactor; behavior-preserving)

- [ ] **Write/keep tests for current settle behavior**, then refactor: `PyrightReady` + `GoplsReady` are
  byte-identical (`{began, active, settled_at}` + identical `settled_no_progress`). Replace BOTH with one:
```rust
#[derive(Debug, Default)]
pub struct SettleReady {
    pub began: bool,
    pub active: u32,
    pub settled_at: Option<Instant>,
}
impl SettleReady {
    pub fn settled_no_progress(&self, settle: Duration) -> bool {
        !self.began && self.settled_at.map(|t| t.elapsed() >= settle).unwrap_or(false)
    }
}
```
- [ ] `Readiness` enum → `RustRa(RustReady)`, `Pyright(SettleReady)`, `Gopls(SettleReady)`,
  `Ts(SettleReady)`. Update `on_notification`: keep the Pyright arm (`pyright/{begin,end}Progress`); make
  Gopls AND Ts parse `$/progress` begin/end (identical bodies — share via `|` or a helper). `is_ready`:
  Pyright/Gopls/Ts → `s.began && s.active == 0`.
- [ ] `lsp/mod.rs` `settled_no_progress`: match `Pyright(s)|Gopls(s)|Ts(s) => s.settled_no_progress(PYRIGHT_SETTLE)`,
  `RustRa(_) => false`. The handshake settle-stamp: Pyright stamps after `post_init_config`; **Gopls AND Ts**
  stamp after `initialized` (Ts has no post_init_config) — extend the existing `if let Gopls` to also match Ts
  (e.g. `Gopls(s) | Ts(s)`).
- [ ] Run `cargo test -p lsp-mcp` — existing rust/python/go readiness + nav tests stay GREEN (behavior-preserving).

### 1b — `Lang::TypeScript` + detection (tsconfig.json) + `ts_config`

- [ ] **Failing test:**
```rust
#[test]
fn detect_typescript_via_tsconfig_only() {
    let d = tempfile::tempdir().unwrap();
    // a tooling-only package.json must NOT trigger TS (no rust/python regression — Opus m2)
    std::fs::write(d.path().join("package.json"), "{}").unwrap();
    assert_eq!(detect(d.path()), Detection::None);
    std::fs::write(d.path().join("tsconfig.json"), "{}").unwrap();
    assert_eq!(detect(d.path()), Detection::Detected(Lang::TypeScript));
}
```
- [ ] `Lang::TypeScript` + `as_str` → `"typescript"`. In `detect()`: add `let is_ts = repo.join("tsconfig.json").is_file();`
  to the marker array + the `1 if is_ts => Detection::Detected(Lang::TypeScript)` arm; extend `detect_lang`'s
  ambiguity message. (NOT package.json — avoids regressing rust/python.)
- [ ] `ts_config`:
```rust
pub fn ts_config(repo: &Path) -> anyhow::Result<LangServerConfig> {
    let _ = repo;
    let server = std::env::var("LSP_MCP_TS_SERVER").unwrap_or_else(|_| "typescript-language-server".into());
    let server_bin = resolve_lsp_server(&server);
    eprintln!("[lsp-mcp] typescript server: {server_bin}");
    Ok(LangServerConfig {
        name: "typescript-language-server",
        program_argv: vec![server_bin, "--stdio".to_string()],
        spawn_env: vec![],
        is_project_root: Box::new(|root: &Path| root.join("tsconfig.json").exists() || root.join("package.json").exists()),
        initialize_params: Box::new(|root_uri: &str| json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "capabilities": { "workspace": { "symbol": {} },
                "textDocument": { "documentSymbol": { "hierarchicalDocumentSymbolSupport": true } } },
            "workspaceFolders": [{ "uri": root_uri, "name": "root" }],
            // If impl finds tsserver isn't auto-discovered under the stripped env, add:
            // "initializationOptions": { "tsserver": { "path": "<global typescript lib tsserver.js>" } }
        })),
        post_init_config: None,
        new_readiness: Box::new(|| Readiness::Ts(SettleReady::default())),
    })
}
```
- [ ] **lib.rs dispatch:** `--lang` match: `"typescript" | "ts" | "javascript" | "js" => Lang::TypeScript`;
  config match: `Lang::TypeScript => crate::lang::ts_config(&repo)?`; extend the `--lang must be …` error.
- [ ] Run all-four: `cargo test -p lsp-mcp && cargo build -p lsp-mcp && cargo clippy -p lsp-mcp --all-targets -- -D warnings && cargo fmt -p lsp-mcp -- --check`.
- [ ] **Commit** (1a + 1b, or split): `feat(lsp-mcp): TypeScript language (typescript-language-server, swappable) + shared SettleReady`.

---

## Task 2: Toolchain image — TS layer (mise, real-binary symlinks)

**Files:** `deploy/containers/toolchain.Containerfile` (append after the python layer).

- [ ] Append (pin versions via `mise latest` at impl time):
```dockerfile
# JS/TS (LSP-MCP polyglot): mise-provisioned typescript-language-server + typescript; REAL binaries
# symlinked into /usr/local/bin (NEVER shims — env trap). node/npm already present (image base).
RUN /root/.local/bin/mise use -g -y "npm:typescript-language-server@<pin>" "npm:typescript@<pin>"
RUN set -eux; for t in typescript-language-server tsc tsserver; do \
      ln -sf "$(/root/.local/bin/mise which "$t")" "/usr/local/bin/$t"; done
```
- [ ] Build: `docker build -t a2a-toolchain:latest -f deploy/containers/toolchain.Containerfile .`
- [ ] **Stripped-env shim guard:** `env -i PATH=/usr/local/bin:/usr/bin:/bin` → `typescript-language-server
  --version`, `tsc --version`, `node --version`, `npm --version` all resolve. **Spike tsserver discovery:**
  confirm `typescript-language-server` finds the global `typescript` lib under that stripped env (else
  capture the lib path for `initializationOptions.tsserver.path` and add it to `ts_config`).
- [ ] **Commit:** `feat(toolchain): mise typescript-language-server + typescript, real binaries on /usr/local/bin`.

---

## Task 3: npm egress + the `typescript` `[[languages]]` profile (both configs)

**Files:** `deploy/containers/tinyproxy.verify.filter`; `examples/a2a-bridge.containerized.toml` + `.podman.toml`.

- [ ] **npm egress:** add `(^|\.)npmjs\.org$` to `tinyproxy.verify.filter` (+ a comment); `docker restart a2a-verify-proxy`.
- [ ] **Profile** (both configs, byte-identical) — use the spec's Layer-3 block verbatim: `id="typescript"`,
  the copy-manifests + `npm ci --ignore-scripts` `fetch`, `dep_cache_path=/node_modules`,
  `verify_cache_path=/node_modules`, `lsp_env={}` (swap lever documented), the 3 verify steps (typecheck
  install-then-tsc, eslint PATH-guard, `npm test --if-present`).
- [ ] **Value-pin test** (mirror `example_containerized_python_language_profile`): assert the `typescript`
  profile parses in BOTH configs + pin the fetch/verify command strings + dep_cache_path semantics.
- [ ] Run `cargo test -p a2a-bridge --bin a2a-bridge` (incl. `podman_example…mirrors_docker` + the new pin test) + clippy + fmt.
- [ ] **Commit:** `feat(containerized): typescript [[languages]] profile + npm verify-egress allowlist`.

---

## Task 4: Live gate (DoD)

Build release binaries (`cargo build --release --bin a2a-bridge --bin lsp-mcp`) + rebuild the image (bakes
Task-1 lsp-mcp). Create a **deps-bearing TS fixture** under `/Users/wesleyjinks/code` (e.g. `a2a-1d-ts`):
`package.json` + `package-lock.json` (a typed dep like `lodash`+`@types/lodash`) + `tsconfig.json` + a `.ts`
with a function whose type involves the dep; NO committed `node_modules`; `git init && commit`.

- [ ] **Gate 1 (3rd-party nav):** `run-workflow c2b-nav --input <fn-name> --session-cwd <fixture>` → tsserver
  returns a type-resolved signature involving the dep (proves the warmed `/node_modules` root-walk under
  locked egress); call log shows the call. (If "no lsp tool" → env-dump wrapper diagnostic.)
- [ ] **Gate 2 (:ro):** fixture unmutated. **Gate 3 (cache):** one `-cache-ts-<hash>` vol, reused.
- [ ] **Gate 4 (verify resolves deps):** the `typecheck` step is GREEN on the fixture (install-into-/node_modules + tsc).
- [ ] **Gate 5 (swappable seam):** set `LSP_MCP_TS_SERVER=/usr/local/bin/typescript-language-server` in the
  TS profile `lsp_env`, re-run, confirm honored (in-container, via lsp_env — not host).
- [ ] **Gate 6 (degrade):** a TS repo with no lock → bare server, local/lib nav works (no crash); verify best-efforts.
- [ ] **Gate 7 (no regression):** rust/go/python nav still work; **a rust/python repo with a tooling-only
  package.json (no tsconfig) still auto-detects** as rust/python.
- [ ] **Gate 8 (shim guard):** the Task-2 stripped-env probes.

---

## Execution notes

- **Task 1 is high-risk** (greenfield lsp-mcp language + a readiness refactor) → sonnet implements (TDD),
  **codex reviews Task 1 + the final branch**; Opus per-task on 2/3 + final arch. (codex is available.)
- Reuse is UNCHANGED for `apply_warm_lsp`/`cache_binding`/`warm_lsp_deps_step`/`compose_warm_fetch`/
  `compose_verify` — only lsp-mcp gets new code + config/image. If any bin/bridge-core change becomes
  necessary, STOP and re-review (it changes the risk profile).
- **Open confirmations:** current `npm:typescript-language-server`/`npm:typescript` versions (`mise latest`);
  tsserver auto-discovery under the stripped env (else the `initializationOptions.tsserver.path`); the npm
  registry CDN host (if `registry.npmjs.org` redirects to a separate download host, add it too).
