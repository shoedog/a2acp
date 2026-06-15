# LSP-MCP Slice C1 Implementation Plan

> **Status:** Revised after a THIRD plan-review round (opus + codex/gpt-5.5 staff review — "not ready to execute as-is"). All must-fixes folded: Task 1 keeps only host-probing gates and the live `{cwd}` gate moved to Task 9 (it needs the Task-4 binary); the `didChangeConfiguration` wrapped `settings` envelope is reconciled across Task 1↔Task 6; the `testkit` compile bug fixed (genuinely-`pub` + `#[doc(hidden)] pub mod testkit`); a GENUINE respawn-FAILURE-leaves-`evicted=true` test + a request-touch test added; `LangServerConfig.is_project_root` added AND wired in `run()` (not a dead field); `has_real_pyproject` `dynamic` predicate anchored; interpreter exec-validation + explicit-invalid-path HARD error + settle-clock-origin fix (`PyrightReady.settled_at`) + an automated pure no-progress test; the dup-name fixture false-green fixed + a guarded Rust `document_symbols` nested-children lock added; Task 9 makes the live `{cwd}` gate first, the Python 7-tool coverage a REQUIRED DoD gate, and — per **user decision D2 — FU1 MANDATORY in C1 if the gate finds codex `{cwd}` broken (no claude-only fallback)**. Harness = **TARGETED fixes** (user decision D1 — NOT a full fake-rust-analyzer binary).
>
> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Generalize `lsp-mcp` from a rust-analyzer-only shim into a `--lang`→language-server registry (rust + Python/basedpyright) so the bridge's host-side reviewers get the same 7-tool type-resolved nav surface on Python repos, keeping the working Rust/FU3 path byte-for-byte.

**Architecture:** A language-agnostic `LspClient` (process spawn, reader-thread `id`-routing, request/response correlation, the 7 tools, idle-evict/respawn) is parameterized by a `LangServerConfig` (`program_argv`, `spawn_env`, `is_project_root`, `initialize_params`, `post_init_config`, `new_readiness`). **`is_project_root` is a USED field** — `run()` calls `(cfg.is_project_root)(repo)` to validate an EXPLICIT `--lang rust|python` against the repo before starting (rust: `Cargo.toml` exists; python: the §2 root markers); `--lang auto` is already validated by `detect_lang`. A `Readiness` enum absorbs ONLY the reader-thread notification parsing (`Readiness::RustRa` = the current `$/progress`+`serverStatus` machine unchanged; `Readiness::Pyright` = `pyright/{begin,end}Progress` + a no-progress settle timed from `PyrightReady.settled_at` = settings-applied, NOT from `wait_ready` entry). `--lang auto` detects the language from root markers. The host reviewers' single `--lang auto` MCP entry serves any unambiguous rust-or-python repo.

**Tech Stack:** Rust (crates/lsp-mcp), rust-analyzer, basedpyright, MCP-over-stdio

---

## File structure

| File | Create/Modify | Responsibility |
| --- | --- | --- |
| `docs/superpowers/spikes/2026-06-15-slice-c1-basedpyright-proof.md` | Create | Spike verdict (Task 1, host-probing only): config-channel resolution (w/ + w/o repo override + no-venv fallback, driving the WRAPPED `{ "settings": { "python": { "pythonPath": … } } }` envelope), readiness + no-progress settle, `--lang auto` predicate cases. (The LIVE `{cwd}` codex gate moved to Task 9 — it needs the Task-4 binary.) |
| `crates/lsp-mcp/Cargo.toml` | Modify | Add `[dev-dependencies] tempfile = "3"` (matching the sibling crates' direct pin — `tempfile` is NOT a workspace dep) for the characterization harness + detection tests (no runtime deps added — percent-encoding is hand-rolled like the existing decoder). |
| `crates/lsp-mcp/src/lib.rs` | Modify | `Cli` gains `--python-path`; `--lang auto` default; `run()` dispatches `detect_lang` → builds the `LangServerConfig` → validates an EXPLICIT `--lang` via `(cfg.is_project_root)(repo)` → starts `LspClient`; startup root+language log. Adds `#[doc(hidden)] pub mod testkit` (genuinely-`pub` re-exports for the EXTERNAL characterization harness — `pub(crate)` is unreachable from a `tests/` crate). |
| `crates/lsp-mcp/src/lang.rs` | Create | `LangServerConfig` struct (incl. the USED `is_project_root` field), `Readiness` enum (+ its reader-thread notification parsing), `RustReady`/`PyrightReady` (with `settled_at` for the no-progress settle), `rust_ra_config()`, `pyright_config()`, interpreter discovery (`resolve_python_path` → `PyResolve` {Resolved/Fallback/Hard}), `detect_lang` predicates. Pure tests: readiness transitions + the no-progress settle. |
| `crates/lsp-mcp/src/lsp/mod.rs` | Modify | Rename/refactor `LspSession`→`LspClient` parameterized by `LangServerConfig`; `spawn`/`respawn` send `post_init_config`; reader thread keeps `id`-routing, delegates notification parsing to `Readiness`; `wait_ready` settle timed from `settled_at`; `file_uri` percent-encodes; `resolve_pos`/`document_symbols` (recursive children)/`hover` (array form) per-server handling. Genuinely-`pub` `is_ready`/`parse_quiescent`/`ReadyState`→`Readiness`; doc-hidden test wrappers (`respawn_for_test`/`is_evicted_for_test`/`set_cfg_for_test`/`last_activity_for_test`). |
| `crates/lsp-mcp/src/mcp/mod.rs` | Modify | Genericize the `references`/`implementations` tool descriptions; the dispatch path is unchanged (it calls `LspClient` methods by the same names). |
| `crates/lsp-mcp/src/shape.rs` | Modify | (No logic change; `file_path_from_uri` is the round-trip partner asserted by the new URI tests.) |
| `crates/lsp-mcp/tests/characterization.rs` | Create | Fake-LSP harness: pins Rust `initialize` bytes + the readiness transition table + the happy respawn ordering (`respawn_success_clears_evicted`). Green on CURRENT code first. Post-refactor (Task 3): the `Readiness`-based transition rewrite + the GENUINE `respawn_failure_leaves_evicted_true` (bogus `program_argv`) + the request-touch idle-race test. |
| `crates/lsp-mcp/tests/integration.rs` | Modify | Re-point `LspSession`→`LspClient` (Task 3); ADD a guarded Rust `document_symbols` test locking the additive nested-children output against the `sample` fixture (Task 8). |
| `crates/lsp-mcp/tests/lang_detect.rs` | Create | `detect_lang` unit/fixture tests (rust/python/tooling-only-pyproject/`.py`-guard with excluded dirs/ambiguous-refusal). |
| `crates/lsp-mcp/tests/python_nav.rs` | Create | Python fixture tests for all 7 tools (guarded on `basedpyright-langserver --version`), interpreter-discovery, post-eviction resolution. |
| `crates/lsp-mcp/tests/fixtures/pysample/` | Create | Small Python fixture: a package with a class→method (hierarchical symbols), a duplicate-name symbol, a third-party import, a `.venv` with that dependency installed (created by the test harness, gitignored). |
| `examples/a2a-bridge.containerized.toml` | Modify | Host reviewers' (claude + codex) `lsp` MCP entry: `--lang rust` → `--lang auto`. |
| `examples/a2a-bridge.containerized.podman.toml` | Modify | Same two HOST reviewer `lsp` entries → `--lang auto` (in-container `impl` stays `rust`). |
| `bin/a2a-bridge/src/main.rs` | Modify (MANDATORY-if-broken, D2) | FU1: thread per-request `session_cwd` into codex's `render_codex_mcp_args` `{cwd}` — ONLY if the Task-9 live `{cwd}` gate finds codex broken (no claude-only fallback). |
| `crates/lsp-mcp/src/cache_key.rs` | (No change) | Rust-only `CARGO_TARGET_DIR` keying; Python has no target cache in C1. Left as-is. |

---

### Task 1: Spike (host path) — host-probing GATES (no bridge binary needed)

A throwaway/measurement task. No production code lands; the deliverable is the verdict file. Task 1 keeps ONLY the host-probing gates that need no bridge binary (config-channel resolution, repo-override behavior, readiness + no-progress settle, the `--lang auto` predicate LOGIC). **The LIVE `{cwd}` codex gate was RELOCATED to the first step of Task 9** — it cannot run here because the current `lsp-mcp` has no `--lang auto` and no startup root/lang log; those land in Task 4, so the gate runs after the Task-4 binary exists (it remains the gate that forks Task 9). Run Task 1 on the host with a real `basedpyright-langserver` + an existing Python repo + a real `rust-analyzer`.

**Files:**
- Create: `docs/superpowers/spikes/2026-06-15-slice-c1-basedpyright-proof.md`

**Steps:**

- [ ] Confirm the host has basedpyright: `basedpyright-langserver --version` (if absent, `pip install basedpyright`). Record the version in the verdict file.
- [ ] **Gate 1a — config-channel resolution from an EXISTING venv, driving the WRAPPED envelope Task 6 ships.** Pick a Python repo with a venv that imports a third-party package (e.g. `~/code/agent-eval` if it has `.venv` with `requests`/`pydantic`). By hand, drive basedpyright over stdio: send `initialize` advertising NO `window/workDoneProgress`, then `initialized`, then `workspace/didChangeConfiguration` with the **LSP-standard wrapped form** `{ "settings": { "python": { "pythonPath": "<repo>/.venv/bin/python" } } }` (NOT a bare `{ "python": { "pythonPath": … } }`) — this is the EXACT envelope Task 6's `post_init_config` ships, so the spike proves the form that ships. Then `textDocument/definition` / `textDocument/hover` on a third-party symbol. Confirm it resolves into the venv's site-packages (NOT "unknown"). A throwaway script is fine; capture the request/response transcript into the verdict file. **Note: the proven form is the wrapped `settings` envelope; Task 6 ships exactly this.**
- [ ] **Gate 1b — repo override behavior.** Repeat 1a against a repo that ALSO has a `pyrightconfig.json` or `pyproject [tool.basedpyright]`. Record whether the repo override wins over the wrapped `didChangeConfiguration` `pythonPath` (informs the §2 documented behavior — the shim does not fight a repo override).
- [ ] **Gate 1c — no-venv fallback.** Run against a Python repo/dir with NO venv and no `--python-path`. Confirm the `python3`-on-PATH fallback degrades to incomplete third-party resolution (stdlib still resolves) and that this is the case the shim must LOG a warning for (not a silent empty result). Record the observed degradation.
- [ ] **Gate 2 — readiness + no-progress settle.** From the 1a transcript, confirm `pyright/beginProgress` + `pyright/endProgress` notifications fire after `initialized`+settings. Then test the no-progress case: confirm that a `workspace/symbol` issued shortly after `initialized`+settings (before any progress) still returns — i.e. the shim must treat a short post-settings settle as ready, not wait a full 30s bound, AND must NOT pay the full timeout when a `beginProgress` arrives without a matching `endProgress` (the begin-without-end stall §2 forbids). Record both observations.
- [ ] **Gate 3 — `--lang auto` detection LOGIC (predicate cases, no binary).** Reason about the predicates against real repos (no bridge binary needed — these are pure host-dir checks): rust (`Cargo.toml`), python (`setup.py`/`setup.cfg`/`requirements*.txt`/`pyproject` with a real section), tooling-only `pyproject` (`[tool.black]` only → NOT python by that marker, falls to `.py`-scan), `.py`-scan excluding `.venv`/`venv`/`.git`/`target`/`node_modules`/hidden/build/vendor, and BOTH rust+python markers → ambiguous→refuse. This validates the predicates Task 4 will implement. (The LIVE `{cwd}` codex gate that USED to be Gate 4 here is now the first step of Task 9.)
- [ ] `git add docs/superpowers/spikes/2026-06-15-slice-c1-basedpyright-proof.md && git commit -m "spike(lsp-mcp): C1 basedpyright host-path proof (config-channel + readiness + detection logic)"`

---

### Task 2: Fake-LSP characterization harness (BEFORE the refactor)

Pin the CURRENT Rust behavior so the registry refactor (Task 3) is provably byte-for-byte. This harness must be **green on the current code** before Task 3 touches anything. Task 2 drives a synthetic notification stream through the readiness machine and asserts the transition table, plus the `initialize` bytes and the happy respawn ordering (`respawn_success_clears_evicted`). (The GENUINE respawn-FAILURE-leaves-`evicted=true` test, the request-touch idle-race test, and the "Rust sends no post-init config" assertion all need the Task-3 `LangServerConfig`/`start_with` seam — they land in Task 3.)

**Files:**
- Create: `crates/lsp-mcp/tests/characterization.rs`
- Modify: `crates/lsp-mcp/Cargo.toml` (add `tempfile` dev-dep)
- Modify: `crates/lsp-mcp/src/lsp/mod.rs` (expose the pure readiness helpers + `ReadyState` so the harness can drive them; today `is_ready`/`parse_quiescent`/`ReadyState` are private)

**Steps:**

- [ ] Add the dev-dependency. `tempfile` is NOT a workspace dependency in this repo — the sibling crates (`bridge-core`, `bridge-container`, `bin/a2a-bridge`) each pin `tempfile = "3"` directly. Match that. In `crates/lsp-mcp/Cargo.toml`, after the `[dependencies]` block add:
  ```toml
  [dev-dependencies]
  tempfile = "3"
  ```
- [ ] Run `cargo build -p lsp-mcp` and see it compile (no harness yet).
- [ ] Make the readiness internals reachable from an EXTERNAL `tests/` integration test. **A `tests/` crate is a SEPARATE crate — it cannot see `pub(crate)` items, and you cannot `pub use` a `pub(crate)` item.** So make the three items GENUINELY `pub` (not `pub(crate)`), then re-export them under a `#[doc(hidden)]` module so they stay out of the documented public API while resolving from `tests/`. In `crates/lsp-mcp/src/lsp/mod.rs`, change the three private items to genuinely public: `pub fn is_ready`, `pub fn parse_quiescent`, `pub struct ReadyState` with `pub` fields `began`/`active`/`quiescent` (the fields the harness reads must be `pub` too). Then add at the bottom of `src/lib.rs`:
  ```rust
  #[doc(hidden)] // out of the documented API, but resolvable from the external characterization harness.
  pub mod testkit {
      //! Internal helpers exposed ONLY for the characterization harness (tests/characterization.rs).
      //! Doc-hidden so they don't appear in the public docs; the items themselves are `pub` because an
      //! external `tests/` crate cannot reach `pub(crate)` items (and `pub use` of `pub(crate)` won't compile).
      pub use crate::lsp::{is_ready, parse_quiescent, ReadyState};
  }
  ```
- [ ] Write the FAILING `initialize`-bytes characterization. In `crates/lsp-mcp/tests/characterization.rs`:
  ```rust
  //! Characterization harness — pins the CURRENT Rust readiness behavior + `initialize` bytes + respawn
  //! ordering so the Slice C1 registry refactor is provably byte-for-byte for the Rust path. Must be GREEN
  //! on the pre-refactor code, then stay green after `LangServerConfig`/`Readiness` are split out.
  use lsp_mcp::testkit::{is_ready, parse_quiescent, ReadyState};
  use serde_json::json;

  /// The exact `initialize` params the Rust path sends today (lib `handshake()`), captured here so the
  /// `Readiness::RustRa` config in Task 3 reproduces them value-for-value.
  fn rust_initialize_params(root_uri: &str, pid: u32) -> serde_json::Value {
      json!({
          "processId": pid,
          "rootUri": root_uri,
          "capabilities": { "workspace": { "symbol": {} },
              "experimental": { "serverStatusNotification": true } },
          "workspaceFolders": [{ "uri": root_uri, "name": "root" }],
      })
  }

  #[test]
  fn rust_initialize_params_are_pinned() {
      let p = rust_initialize_params("file:///repo", 7);
      assert_eq!(p["capabilities"]["experimental"]["serverStatusNotification"], json!(true));
      assert_eq!(p["capabilities"]["workspace"]["symbol"], json!({}));
      assert_eq!(p["workspaceFolders"][0]["uri"], json!("file:///repo"));
      assert_eq!(p["processId"], json!(7));
  }
  ```
- [ ] Run `cargo test -p lsp-mcp --test characterization rust_initialize_params_are_pinned` and see it PASS (this pins the bytes; Task 3 must keep `rust_ra_config().initialize_params(root)` value-equal to this).
- [ ] Add the readiness transition-table characterization (drives the synthetic notification stream through the pure machine). Append to `characterization.rs`:
  ```rust
  /// Apply one synthetic notification to a ReadyState the way the reader thread does today (mod.rs:99-118).
  fn apply(s: &mut ReadyState, msg: &serde_json::Value) {
      if msg.get("method").and_then(|m| m.as_str()) == Some("$/progress") {
          match msg["params"]["value"]["kind"].as_str() {
              Some("begin") => { s.began = true; s.active += 1; }
              Some("end") => { s.active = s.active.saturating_sub(1); }
              _ => {}
          }
      } else if msg.get("method").and_then(|m| m.as_str()) == Some("experimental/serverStatus") {
          if let Some(q) = parse_quiescent(&msg["params"]) { s.quiescent = q; }
      }
  }

  #[test]
  fn rust_readiness_transition_table() {
      let begin = json!({"method":"$/progress","params":{"value":{"kind":"begin"}}});
      let end = json!({"method":"$/progress","params":{"value":{"kind":"end"}}});
      let quiescent = json!({"method":"experimental/serverStatus","params":{"quiescent":true}});

      // ordered begin→end → ready
      let mut s = ReadyState::default();
      assert!(!is_ready(&s), "nothing heard yet");
      apply(&mut s, &begin);
      assert!(!is_ready(&s), "begun, still active");
      apply(&mut s, &end);
      assert!(is_ready(&s), "begun-and-ended → ready");

      // serverStatus quiescent alone (warm-no-progress) → ready, no $/progress needed
      let mut s = ReadyState::default();
      apply(&mut s, &quiescent);
      assert!(is_ready(&s), "quiescent alone is enough");

      // out-of-order: a stray `end` before any `begin` must NOT mark ready (active saturates at 0, began stays false)
      let mut s = ReadyState::default();
      apply(&mut s, &end);
      assert!(!is_ready(&s), "lone end is not ready");
  }
  ```
- [ ] Run `cargo test -p lsp-mcp --test characterization rust_readiness_transition_table` and see it PASS.
- [ ] Add the HAPPY respawn-ordering characterization using a real (guarded) RA session, mirroring `tests/integration.rs`'s `ra_available()` guard. (The GENUINE respawn-FAILURE test, the request-touch test, and the "Rust sends no post-init config" assertion all need the Task-3 `LangServerConfig`/`start_with` seam, so they land in Task 3.) Append:
  ```rust
  use std::time::Duration;

  fn ra_available() -> bool {
      std::process::Command::new("rust-analyzer").arg("--version").output()
          .map(|o| o.status.success()).unwrap_or(false)
  }
  fn sample_repo() -> std::path::PathBuf {
      std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample")
  }

  #[test]
  fn respawn_success_clears_evicted() {
      // HAPPY PATH: a respawn whose handshake SUCCEEDS clears `evicted` and the session resolves again.
      // (The GENUINE failure-leaves-evicted=true test — the crown-jewel invariant — lands in Task 3, where
      // the LangServerConfig seam lets us inject a bogus program_argv to FORCE a respawn failure.)
      // We evict a healthy session and assert the next ensure_ready re-spawns against the SAME (valid) repo
      // and resolves. Guarded: needs a real RA to start the first time.
      if !ra_available() { eprintln!("skip: rust-analyzer not on PATH"); return; }
      let mut s = lsp_mcp::lsp::LspClient::start(&sample_repo(), None).unwrap();
      s.ensure_ready(Duration::from_secs(120)).unwrap();
      s.evict();
      // After evict, the next ensure_ready respawns against the SAME (valid) repo and succeeds — assert the
      // evicted flag clears on success and the session resolves again (the happy respawn ordering).
      s.ensure_ready(Duration::from_secs(120)).unwrap();
      assert!(!s.workspace_symbol("add").unwrap().is_empty(), "respawn re-indexed after evict");
      s.shutdown();
  }
  ```
  (Note: this test references `LspClient` — the post-Task-3 name. BEFORE Task 3, write it against `LspSession` and rename in Task 3 when the type is renamed. Add a comment marking the rename. The OLD name `respawn_failure_leaves_evicted_true` was a MISLABEL — it asserts the success path — so we name it `respawn_success_clears_evicted` from the start; the real failure invariant is Task 3's `respawn_failure_leaves_evicted_true` below.)
- [ ] Run `cargo test -p lsp-mcp --test characterization` and see the whole harness PASS on the current code (the guarded test skips if RA is absent; the two pure tests must pass everywhere).
- [ ] `git add crates/lsp-mcp/Cargo.toml crates/lsp-mcp/src/lib.rs crates/lsp-mcp/src/lsp/mod.rs crates/lsp-mcp/tests/characterization.rs && git commit -m "test(lsp-mcp): fake-LSP characterization harness pins Rust readiness + initialize bytes (pre-refactor)"`

---

### Task 3: Registry refactor — split `LspClient` from `LangServerConfig`

Split the language-agnostic client from the per-language config. `Readiness` absorbs ONLY the reader-thread notification parsing; `id`-routing STAYS in `LspClient`. `Readiness::RustRa` wraps the current `$/progress`+`serverStatus`/`is_ready`/`parse_quiescent` logic unchanged. The Rust path must stay byte-for-byte: the Task 2 harness + the existing `tests/integration.rs` + the `src/lsp/mod.rs` unit tests all stay green.

**Files:**
- Create: `crates/lsp-mcp/src/lang.rs`
- Modify: `crates/lsp-mcp/src/lsp/mod.rs`
- Modify: `crates/lsp-mcp/src/lib.rs` (declare `pub mod lang;`; `run()` builds a `rust_ra_config()`)
- Modify: `crates/lsp-mcp/tests/integration.rs` (`LspSession`→`LspClient`)
- Test: `crates/lsp-mcp/tests/characterization.rs` (`LspSession`→`LspClient`)

**Steps:**

- [ ] Create `crates/lsp-mcp/src/lang.rs` with the config struct + the `Readiness` enum that owns notification parsing. Write the failing-to-compile skeleton first:
  ```rust
  //! The per-language server registry: `LangServerConfig` parameterizes the language-agnostic `LspClient`.
  //! `Readiness` absorbs ONLY the reader-thread NOTIFICATION parsing (id-routing stays in LspClient).
  use serde_json::{json, Value};
  use std::path::{Path, PathBuf};
  use std::time::{Duration, Instant};

  /// Per-language readiness: the notification-parsing half of the reader thread + the ready predicate.
  /// `RustRa` reproduces the current `$/progress` + `experimental/serverStatus` machine byte-for-byte.
  #[derive(Debug)]
  pub enum Readiness {
      RustRa(RustReady),
      Pyright(PyrightReady),
  }

  /// Rust path readiness state — the old `ReadyState`, unchanged fields/semantics.
  #[derive(Debug, Default)]
  pub struct RustReady {
      pub began: bool,
      pub active: u32,
      pub quiescent: bool,
  }

  /// Python (basedpyright) readiness: `pyright/{begin,end}Progress` + a short no-progress settle.
  #[derive(Debug, Default)]
  pub struct PyrightReady {
      pub began: bool,
      pub active: u32,
      /// Set (`Some(Instant::now())`) the moment `initialized`+settings are applied in `handshake`. This is
      /// the CORRECT settle-clock origin: the no-progress settle is timed from when settings were applied,
      /// NOT from `wait_ready` entry (Opus H2). Timing from `wait_ready` entry made a begin-without-end
      /// server pay the FULL timeout — the exact FU3 stall §2 forbids. None until settings are applied.
      pub settled_at: Option<Instant>,
  }

  impl PyrightReady {
      /// PURE no-progress settle gate: settings applied, no progress begun, and the settle window has
      /// elapsed since `settled_at`. Timed from `settled_at` (settings-applied), not from any wait entry.
      pub fn settled_no_progress(&self, settle: Duration) -> bool {
          !self.began && self.settled_at.map(|t| t.elapsed() >= settle).unwrap_or(false)
      }
  }

  impl Readiness {
      /// Parse one inbound NOTIFICATION (never a response — id-routing is the caller's job). Mutates state.
      pub fn on_notification(&mut self, method: &str, params: &Value) {
          match self {
              Readiness::RustRa(s) => match method {
                  "$/progress" => match params["value"]["kind"].as_str() {
                      Some("begin") => { s.began = true; s.active += 1; }
                      Some("end") => { s.active = s.active.saturating_sub(1); }
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
                  "pyright/beginProgress" => { s.began = true; s.active += 1; }
                  "pyright/endProgress" => { s.active = s.active.saturating_sub(1); }
                  _ => {}
              },
          }
      }

      /// PURE ready predicate. RustRa: quiescent OR begun-and-ended. Pyright: begun-and-ended, OR
      /// settings applied with no progress seen (the no-progress settle is timed by LspClient::wait_ready).
      pub fn is_ready(&self) -> bool {
          match self {
              Readiness::RustRa(s) => s.quiescent || (s.began && s.active == 0),
              Readiness::Pyright(s) => s.began && s.active == 0,
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
                      "experimental": { "serverStatusNotification": true } },
                  "workspaceFolders": [{ "uri": root_uri, "name": "root" }],
              })
          }),
          post_init_config: None,
          new_readiness: Box::new(|| Readiness::RustRa(RustReady::default())),
      }
  }
  ```
- [ ] Declare the module + drop the old `ReadyState`/`is_ready`/`parse_quiescent` from `mod.rs`. In `crates/lsp-mcp/src/lib.rs` add `pub mod lang;` next to the other `pub mod` lines. (Keep the `testkit` re-export pointing at the new home — update it in a later step.)
- [ ] Rename `LspSession`→`LspClient` and parameterize it by `LangServerConfig`. In `crates/lsp-mcp/src/lsp/mod.rs`: replace the `LspSession` struct's `target_cache: Option<PathBuf>` + the `ready: SharedReady` + `ReadyState` machinery. The new struct:
  ```rust
  type SharedReady = Arc<Mutex<crate::lang::Readiness>>;

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
  ```
- [ ] Replace `spawn_ra` with a config-driven `spawn`. The reader thread keeps `id`-routing inline and delegates notification parsing to `Readiness::on_notification`:
  ```rust
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
                      ready.lock().unwrap().on_notification(method, &msg["params"]);
                  }
              }
          });
      }
      Ok((child, stdin, pending, ready))
  }
  ```
- [ ] Make `handshake` config-driven + send `post_init_config` (Rust's is `None`, so Rust is byte-for-byte: it still only sends `initialize`+`initialized`). Replace the body:
  ```rust
  fn handshake(&mut self) -> anyhow::Result<()> {
      let root = file_uri(&self.repo);
      let params = (self.cfg.initialize_params)(&root);
      self.request("initialize", params, Duration::from_secs(30))?;
      self.notify("initialized", json!({}));
      if let Some((method, params)) = self.cfg.post_init_config.clone() {
          self.notify(&method, params);
      }
      Ok(())
  }
  ```
- [ ] Update `start`, `respawn`, `wait_ready`, `ensure_ready` to use `self.cfg` + `Readiness::is_ready`. `start` signature becomes `pub fn start(repo: &Path, cfg: crate::lang::LangServerConfig)` — but the existing integration tests call `LspClient::start(&repo, None)`. Keep a compatibility constructor:
  ```rust
  /// Slice-A/test-compat: start with the Rust config (optional CARGO_TARGET_DIR), matching the old signature.
  pub fn start(repo: &Path, target_cache: Option<&Path>) -> anyhow::Result<Self> {
      Self::start_with(repo, crate::lang::rust_ra_config(target_cache))
  }

  /// Start any language server from its config.
  pub fn start_with(repo: &Path, cfg: crate::lang::LangServerConfig) -> anyhow::Result<Self> {
      let cfg = Arc::new(cfg);
      let (child, stdin, pending, ready) = Self::spawn(repo, &cfg)?;
      let mut s = LspClient {
          child: Arc::new(Mutex::new(Some(child))),
          repo: repo.to_path_buf(),
          cfg,
          last_activity: Arc::new(Mutex::new(Instant::now())),
          evicted: Arc::new(AtomicBool::new(false)),
          stdin, next_id: 0, pending, ready, readied: false,
      };
      s.handshake()?;
      s.start_idle_watcher();
      Ok(s)
  }
  ```
- [ ] Update `respawn` to re-spawn from `self.cfg` (this is also what makes Task 7's respawn-resends-config work, since `handshake` now sends `post_init_config`):
  ```rust
  fn respawn(&mut self) -> anyhow::Result<()> {
      let (child, stdin, pending, ready) = Self::spawn(&self.repo, &self.cfg)?;
      *self.child.lock().unwrap() = Some(child);
      self.stdin = stdin;
      self.pending = pending;
      self.ready = ready;
      self.next_id = 0;
      self.readied = false;
      self.handshake()?; // re-sends initialize + initialized + post_init_config (Python venv survives respawn)
      self.evicted.store(false, Ordering::SeqCst);
      Ok(())
  }
  ```
- [ ] Update `wait_ready` to read `Readiness::is_ready` through the lock:
  ```rust
  pub fn wait_ready(&mut self, timeout: Duration) -> anyhow::Result<()> {
      let t0 = Instant::now();
      loop {
          self.touch();
          if self.ready.lock().unwrap().is_ready() {
              return Ok(());
          }
          if t0.elapsed() >= timeout {
              return Ok(());
          }
          std::thread::sleep(Duration::from_millis(100));
      }
  }
  ```
- [ ] Move the `parse_quiescent`/`is_ready`/`ReadyState` unit tests out of `mod.rs` into `lang.rs` (rewritten against `Readiness`), and update the `#[cfg(test)] mod tests` in `mod.rs` to keep only `should_evict_after_idle_timeout`. In `lang.rs` add:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;

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
          assert!(!r.is_ready(), "non-bool quiescent is ignored (keeps prior false)");
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
      fn rust_initialize_params_match_pinned_handshake() {
          let cfg = rust_ra_config(None);
          let p = (cfg.initialize_params)("file:///repo");
          assert_eq!(p["capabilities"]["experimental"]["serverStatusNotification"], json!(true));
          assert_eq!(p["capabilities"]["workspace"]["symbol"], json!({}));
          assert!(cfg.post_init_config.is_none(), "Rust sends NO post-init config");
      }
  }
  ```
- [ ] Update the `#[doc(hidden)] pub mod testkit` re-export in `lib.rs` to point at the new `Readiness`-based API (the harness's `is_ready`/`parse_quiescent`/`ReadyState` imports change to the `Readiness`-based ones). The items re-exported MUST be genuinely `pub` in `lang.rs` (`Readiness`, `RustReady`, `PyrightReady`, with `pub` fields the harness reads) — same genuinely-pub doc-hidden scheme as Task 2 (an external `tests/` crate cannot reach `pub(crate)`). Re-export:
  ```rust
  #[doc(hidden)] // out of the documented API; resolvable from tests/characterization.rs.
  pub mod testkit {
      pub use crate::lang::{Readiness, RustReady, PyrightReady};
  }
  ```
- [ ] **Opus H1 — rewrite `characterization.rs`'s `apply`/transition test against the new `Readiness` API with LITERAL code** (the old version imported the removed `is_ready`/`parse_quiescent`/`ReadyState`). Replace the `apply` helper and `rust_readiness_transition_table` test in `crates/lsp-mcp/tests/characterization.rs` with:
  ```rust
  use lsp_mcp::testkit::{Readiness, RustReady};

  #[test]
  fn rust_readiness_transition_table() {
      let begin = json!({"value":{"kind":"begin"}});
      let end = json!({"value":{"kind":"end"}});
      let quiescent = json!({"quiescent": true});

      // ordered begin→end → ready
      let mut r = Readiness::RustRa(RustReady::default());
      assert!(!r.is_ready(), "nothing heard yet");
      r.on_notification("$/progress", &begin);
      assert!(!r.is_ready(), "begun, still active");
      r.on_notification("$/progress", &end);
      assert!(r.is_ready(), "begun-and-ended → ready");

      // experimental/serverStatus quiescent alone (warm-no-progress) → ready, no $/progress needed
      let mut r = Readiness::RustRa(RustReady::default());
      r.on_notification("experimental/serverStatus", &quiescent);
      assert!(r.is_ready(), "quiescent alone is enough");

      // out-of-order: a stray `end` before any `begin` must NOT mark ready (active saturates at 0, began stays false)
      let mut r = Readiness::RustRa(RustReady::default());
      r.on_notification("$/progress", &end);
      assert!(!r.is_ready(), "lone end is not ready");
  }
  ```
  (Note: `on_notification` takes the `params` value directly — `&msg["params"]` at the reader-thread call site — so the test passes the `params`-shaped values above, matching `mod.rs`'s `ready.lock().unwrap().on_notification(method, &msg["params"])`.)
- [ ] Run `cargo test -p lsp-mcp --test characterization rust_readiness_transition_table` and see it PASS against the new `Readiness` machine.
- [ ] **FIX (crown-jewel invariant) — add the GENUINE respawn-FAILURE test (failure leaves `evicted=true`).** This is the real invariant the old mislabeled test never checked. It needs the `LangServerConfig` seam (to inject a bogus `program_argv`) plus `respawn` + the `evicted` flag reachable from the harness. Expose them: in `src/lsp/mod.rs` add `#[doc(hidden)] pub fn respawn_for_test(&mut self) -> anyhow::Result<()> { self.respawn() }` and `#[doc(hidden)] pub fn is_evicted_for_test(&self) -> bool { self.evicted.load(std::sync::atomic::Ordering::SeqCst) }` (thin doc-hidden wrappers so `respawn`/`evicted` stay private but the harness can drive them). (`start_with` is already `pub` from this task's rename step.) Append to `crates/lsp-mcp/tests/characterization.rs`:
  ```rust
  use lsp_mcp::lang::{LangServerConfig, Readiness, RustReady};

  #[test]
  fn respawn_failure_leaves_evicted_true() {
      // CROWN-JEWEL INVARIANT: a respawn whose handshake CANNOT succeed leaves `evicted=true` so the NEXT
      // call retries respawn (mod.rs:respawn re-inits BEFORE clearing `evicted`). Force failure by building
      // an LspClient over a LangServerConfig whose program_argv[0] is a non-existent binary — spawn() fails.
      let bogus = LangServerConfig {
          name: "bogus-lsp",
          program_argv: vec!["definitely-not-a-real-lsp-xyz".to_string()],
          spawn_env: vec![],
          is_project_root: Box::new(|_| true),
          initialize_params: Box::new(|root| serde_json::json!({ "rootUri": root })),
          post_init_config: None,
          new_readiness: Box::new(|| Readiness::RustRa(RustReady::default())),
      };
      // start_with FAILS to spawn the bogus binary, so build via a real RA first then swap is awkward;
      // instead assert start_with itself errors AND, when we DO have a started client, a forced respawn
      // against a bogus cfg leaves evicted=true. We use the real-RA path to get a started client, evict it,
      // then point respawn at the bogus cfg.
      if !ra_available() { eprintln!("skip: rust-analyzer not on PATH"); return; }
      let mut s = lsp_mcp::lsp::LspClient::start_with(&sample_repo(), lsp_mcp::lang::rust_ra_config(None)).unwrap();
      s.ensure_ready(Duration::from_secs(120)).unwrap();
      s.evict();
      assert!(s.is_evicted_for_test(), "evict() set evicted=true");
      // Swap the cfg to the bogus one and force a respawn — spawn() of the missing binary must Err...
      s.set_cfg_for_test(bogus);
      let err = s.respawn_for_test();
      assert!(err.is_err(), "respawn of a non-existent binary must fail");
      // ...and the invariant: evicted is STILL true after the failed respawn (next call retries respawn).
      assert!(s.is_evicted_for_test(), "FAILED respawn must leave evicted=true (crown-jewel invariant)");
  }
  ```
  To make this compile, also add a doc-hidden cfg-swap helper in `src/lsp/mod.rs`: `#[doc(hidden)] pub fn set_cfg_for_test(&mut self, cfg: crate::lang::LangServerConfig) { self.cfg = std::sync::Arc::new(cfg); }`. (`respawn` re-spawns from `self.cfg`, so swapping it then calling `respawn_for_test` forces the bogus spawn while keeping the eviction-ordering code under test.)
- [ ] Run `cargo test -p lsp-mcp --test characterization respawn_failure_leaves_evicted_true` (RA-guarded) and see it PASS: the failed respawn leaves `evicted=true`.
- [ ] **FIX (request-touch) — add a request-touch test so the Task-3 refactor cannot silently drop the idle-race touch.** The idle-race fix is that `request()`/`wait_ready()` call `touch()` (advancing `last_activity`), and `respawn` clears `evicted` only AFTER the handshake. The `stdin: ChildStdin` field can't be constructed without a live process, so this is an RA-guarded behavioral test (matching the existing harness guard) that proves the REQUEST PATH advances `last_activity` — a pure `touch()`-only unit test would prove `touch` works but NOT that `wait_ready`/`request` call it. Expose the clock for the test: `#[doc(hidden)] pub fn last_activity_for_test(&self) -> std::time::Instant { *self.last_activity.lock().unwrap() }` in `src/lsp/mod.rs`. Append to `crates/lsp-mcp/tests/characterization.rs` (it already has the `ra_available()` + `sample_repo()` helpers):
  ```rust
  #[test]
  fn request_path_advances_last_activity_idle_race_guard() {
      // The idle-race fix: wait_ready()/request() touch() on the active path so the watcher can't evict
      // mid-use. Assert last_activity ADVANCES across a wait_ready() call. If the refactor drops the
      // request-path touch(), last_activity does not advance and this fails. RA-guarded (needs a started client).
      if !ra_available() { eprintln!("skip: rust-analyzer not on PATH"); return; }
      let mut s = lsp_mcp::lsp::LspClient::start_with(&sample_repo(), lsp_mcp::lang::rust_ra_config(None)).unwrap();
      let before = s.last_activity_for_test();
      std::thread::sleep(Duration::from_millis(20));
      s.wait_ready(Duration::from_millis(50)).unwrap(); // touches on every loop iteration
      assert!(s.last_activity_for_test() > before, "request/wait path must touch() (idle-race fix)");
      s.shutdown();
  }
  ```
  (D1 = targeted: this reuses the existing RA guard + `start_with` + the doc-hidden clock accessor — no fake-rust-analyzer binary. The pure `should_evict_after_idle_timeout` test already locks the `should_evict` arithmetic; THIS test locks that the request path actually calls `touch()`.)
- [ ] Rename `LspSession`→`LspClient` everywhere it's referenced: `tests/integration.rs` (`lsp_mcp::lsp::LspSession` → `LspClient`, 4 sites), `tests/characterization.rs` (the `respawn_failure_leaves_evicted_true` test), `src/mcp/mod.rs` (`use crate::lsp::LspSession` → `LspClient`; the `dispatch`/`serve`/`dispatch_body` param types `s: &mut LspSession` → `&mut LspClient`), and `src/lib.rs` `run()`. Use `cargo build -p lsp-mcp` to find every site.
- [ ] Run `cargo test -p lsp-mcp` and see ALL tests pass: the moved unit tests, the characterization harness (now via `Readiness`), and the guarded integration tests (Rust path byte-for-byte). Run `cargo clippy -p lsp-mcp -- -D warnings` and fix any lints from the refactor.
- [ ] `git add crates/lsp-mcp/src/lang.rs crates/lsp-mcp/src/lib.rs crates/lsp-mcp/src/lsp/mod.rs crates/lsp-mcp/src/mcp/mod.rs crates/lsp-mcp/tests/integration.rs crates/lsp-mcp/tests/characterization.rs && git commit -m "refactor(lsp-mcp): split LspClient from LangServerConfig + Readiness (Rust byte-for-byte)"`

---

### Task 4: `--lang auto` detection

Concrete, testable predicates. `rust` iff `Cargo.toml`; `python` iff `setup.py`/`setup.cfg`/`requirements*.txt`/`pyproject` with a real section OR a `.py` found by a shallow scan excluding `.venv`/`venv`/`.git`/`target`/`node_modules`/hidden/build/vendor; BOTH → ambiguous→refuse. Startup LOGS the resolved root + detected language.

**Files:**
- Modify: `crates/lsp-mcp/src/lang.rs` (add `Lang` enum + `detect_lang`)
- Modify: `crates/lsp-mcp/src/lib.rs` (`--lang` default `"auto"`; `run()` dispatches detection + the startup log)
- Test: `crates/lsp-mcp/tests/lang_detect.rs`

**Steps:**

- [ ] Write the FAILING detection unit tests first. Create `crates/lsp-mcp/tests/lang_detect.rs`:
  ```rust
  //! `--lang auto` detection predicates (spec §1). Uses tempdirs to build marker fixtures.
  use lsp_mcp::lang::{detect_lang, Lang};
  use std::fs;

  fn td() -> tempfile::TempDir { tempfile::tempdir().unwrap() }

  #[test]
  fn cargo_toml_is_rust() {
      let d = td();
      fs::write(d.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
      assert_eq!(detect_lang(d.path()).unwrap(), Lang::Rust);
  }

  #[test]
  fn setup_py_is_python() {
      let d = td();
      fs::write(d.path().join("setup.py"), "from setuptools import setup\n").unwrap();
      assert_eq!(detect_lang(d.path()).unwrap(), Lang::Python);
  }

  #[test]
  fn requirements_txt_is_python() {
      let d = td();
      fs::write(d.path().join("requirements-dev.txt"), "pytest\n").unwrap();
      assert_eq!(detect_lang(d.path()).unwrap(), Lang::Python);
  }

  #[test]
  fn pyproject_with_real_section_is_python() {
      let d = td();
      fs::write(d.path().join("pyproject.toml"), "[project]\nname=\"x\"\n").unwrap();
      assert_eq!(detect_lang(d.path()).unwrap(), Lang::Python);
  }

  #[test]
  fn tooling_only_pyproject_is_not_python_by_marker_but_py_scan_wins() {
      let d = td();
      // ONLY a tooling table — not a real project/dep section → not python by the pyproject marker...
      fs::write(d.path().join("pyproject.toml"), "[tool.black]\nline-length=100\n").unwrap();
      // ...but a real .py file at the root makes it python via the shallow scan.
      fs::write(d.path().join("app.py"), "x = 1\n").unwrap();
      assert_eq!(detect_lang(d.path()).unwrap(), Lang::Python);
  }

  #[test]
  fn tooling_only_pyproject_with_no_py_is_unknown() {
      let d = td();
      fs::write(d.path().join("pyproject.toml"), "[tool.ruff]\nline-length=100\n").unwrap();
      assert!(detect_lang(d.path()).is_err(), "tooling-only pyproject + no .py → cannot detect");
  }

  #[test]
  fn py_scan_excludes_venv_and_dotdirs() {
      let d = td();
      fs::create_dir_all(d.path().join(".venv/lib")).unwrap();
      fs::write(d.path().join(".venv/lib/dep.py"), "x=1\n").unwrap();
      fs::create_dir_all(d.path().join("node_modules/pkg")).unwrap();
      fs::write(d.path().join("node_modules/pkg/m.py"), "x=1\n").unwrap();
      // .py only inside excluded dirs → NOT python.
      assert!(detect_lang(d.path()).is_err(), "excluded-dir .py must not count");
  }

  #[test]
  fn both_rust_and_python_markers_are_ambiguous() {
      let d = td();
      fs::write(d.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
      fs::write(d.path().join("setup.py"), "from setuptools import setup\n").unwrap();
      let err = detect_lang(d.path()).unwrap_err().to_string();
      assert!(err.contains("ambiguous"), "both markers → ambiguous refusal, got {err}");
  }
  ```
- [ ] Run `cargo test -p lsp-mcp --test lang_detect` and see it FAIL (no `Lang`/`detect_lang` yet).
- [ ] Implement `Lang` + `detect_lang` in `crates/lsp-mcp/src/lang.rs`:
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum Lang { Rust, Python }

  impl Lang {
      pub fn as_str(&self) -> &'static str {
          match self { Lang::Rust => "rust", Lang::Python => "python" }
      }
  }

  /// Detect the language from `repo`'s root markers, single-unambiguous-root ONLY (spec §1).
  /// Errors on BOTH-markers (ambiguous) or NEITHER (cannot detect → require explicit --lang).
  pub fn detect_lang(repo: &Path) -> anyhow::Result<Lang> {
      let is_rust = repo.join("Cargo.toml").is_file();
      let is_python = python_markers(repo) || has_real_pyproject(repo) || shallow_py_scan(repo);
      match (is_rust, is_python) {
          (true, true) => anyhow::bail!(
              "ambiguous repo root (both Rust and Python markers) at {repo:?}; pass an explicit --lang"
          ),
          (true, false) => Ok(Lang::Rust),
          (false, true) => Ok(Lang::Python),
          (false, false) => anyhow::bail!(
              "could not detect language at {repo:?} (no Cargo.toml / setup.py / setup.cfg / requirements*.txt / \
               pyproject project section / .py files); pass an explicit --lang"
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
      let Ok(text) = std::fs::read_to_string(&p) else { return false; };
      const REAL: [&str; 5] = ["[project]", "[tool.poetry]", "[tool.pdm]", "[build-system]", "[project.dependencies]"];
      text.lines().any(|l| {
          let l = l.trim();
          // `starts_with("dynamic")` matches a `dynamic = [...]` key (PEP 621) but NOT an arbitrary bare
          // `dynamic` token mid-line; the old `l == "dynamic"` matched nothing useful and a bare-`dynamic`
          // arm would over-match — anchor to the line start.
          REAL.iter().any(|m| l.starts_with(m)) || l.starts_with("dynamic")
      })
  }

  const EXCLUDED_SCAN_DIRS: [&str; 8] =
      [".venv", "venv", ".git", "target", "node_modules", "build", "dist", "vendor"];

  /// Shallow recursive scan for any `*.py`, excluding venv/build/vendor/hidden dirs. Bounded depth (3) so a
  /// huge tree doesn't stall startup; a real Python project has a `.py` within a few levels of the root.
  fn shallow_py_scan(repo: &Path) -> bool {
      fn walk(dir: &Path, depth: u8) -> bool {
          if depth == 0 { return false; }
          let Ok(rd) = std::fs::read_dir(dir) else { return false; };
          for e in rd.flatten() {
              let name = e.file_name();
              let name = name.to_string_lossy();
              let ty = e.file_type();
              if ty.as_ref().map(|t| t.is_dir()).unwrap_or(false) {
                  if name.starts_with('.') || EXCLUDED_SCAN_DIRS.contains(&name.as_ref()) {
                      continue;
                  }
                  if walk(&e.path(), depth - 1) { return true; }
              } else if name.ends_with(".py") {
                  return true;
              }
          }
          false
      }
      walk(repo, 3)
  }
  ```
- [ ] Run `cargo test -p lsp-mcp --test lang_detect` and see ALL detection tests PASS.
- [ ] Wire `--lang auto` into `Cli` + `run()`. In `crates/lsp-mcp/src/lib.rs`, change the `lang` default and rewrite `run()`:
  ```rust
  /// Language server to drive: "auto" (detect from repo markers), "rust", or "python".
  #[arg(long, default_value = "auto")]
  pub lang: String,
  ```
  ```rust
  pub fn run(cli: Cli) -> anyhow::Result<()> {
      let repo = cli
          .repo
          .canonicalize()
          .map_err(|e| anyhow::anyhow!("repo {:?}: {e}", cli.repo))?;
      // Track whether the language was chosen EXPLICITLY (vs auto-detected): an explicit `--lang rust|python`
      // is validated against the repo via `is_project_root` BEFORE starting (today explicit `--lang python`
      // on a non-Python dir is unguarded). `auto` is already validated by `detect_lang`.
      let explicit = cli.lang.as_str() != "auto";
      let lang = match cli.lang.as_str() {
          "auto" => crate::lang::detect_lang(&repo)?,
          "rust" => crate::lang::Lang::Rust,
          "python" => crate::lang::Lang::Python,
          other => anyhow::bail!("--lang must be auto|rust|python (got {other:?})"),
      };
      // Observability (spec §1): a misrouted {cwd} landing on the wrong language is now LOUD in the log.
      eprintln!("[lsp-mcp] root={} lang={}", repo.display(), lang.as_str());
      let cfg = match lang {
          crate::lang::Lang::Rust => {
              let target = cli.target_cache.as_deref().map(|base| {
                  let origin = git_origin(&repo);
                  cache_key::cache_dir(base, &repo, origin.as_deref())
              });
              crate::lang::rust_ra_config(target.as_deref())
          }
          crate::lang::Lang::Python => crate::lang::pyright_config(&repo, cli.python_path.as_deref())?,
      };
      // USE `is_project_root`: validate an EXPLICIT --lang against the repo (auto already validated above).
      if explicit && !(cfg.is_project_root)(&repo) {
          anyhow::bail!(
              "explicit --lang {} but {:?} is not a {} project root (missing the {} root markers); \
               pass the right --lang or point --repo at a {} root",
              lang.as_str(), repo, lang.as_str(), lang.as_str(), lang.as_str()
          );
      }
      let session = lsp::LspClient::start_with(&repo, cfg)?;
      mcp::serve(session)
  }
  ```
  (Note: `pyright_config` + `--python-path` land in Task 6; until then, leave a `crate::lang::Lang::Python => anyhow::bail!("python not yet implemented")` arm and a `// TODO Task 6` so `run()` compiles. Replace it in Task 6. The `--python-path` Cli field is added in Task 6. The `is_project_root` validation block above compiles from Task 4 on, since `rust_ra_config` already populates the field; it becomes meaningful for Python once Task 6 lands `pyright_config`'s predicate.)
- [ ] Run `cargo test -p lsp-mcp` (whole crate) and `cargo clippy -p lsp-mcp -- -D warnings`; confirm green.
- [ ] `git add crates/lsp-mcp/src/lang.rs crates/lsp-mcp/src/lib.rs crates/lsp-mcp/tests/lang_detect.rs && git commit -m "feat(lsp-mcp): --lang auto detection (rust/python predicates + ambiguous refusal + startup log)"`

---

### Task 5: `file://` URI builder fix

The builder is a naive `format!("file://{}", display)` (no percent-encoding) while `shape::file_path_from_uri` percent-DECODES — asymmetric today. Fix the builder + round-trip test spaces/`%`/`#`/non-ASCII.

**Files:**
- Modify: `crates/lsp-mcp/src/lsp/mod.rs` (`file_uri` percent-encodes)
- Modify: `crates/lsp-mcp/src/shape.rs` (expose `file_path_from_uri` for the round-trip test; add a `pub(crate)` percent-encode helper or co-locate)

**Steps:**

- [ ] Write the FAILING round-trip test. The encoder and decoder are in different modules; co-locate a `pub(crate) fn file_uri` test by exposing the builder. First, MOVE `file_uri` from `src/lsp/mod.rs` to `src/shape.rs` as `pub(crate) fn file_uri(p: &Path) -> String` (next to its decode partner) so encode+decode live together and a unit test can assert the round-trip. Update the two call sites in `mod.rs` (`handshake` root + `document_symbols`) to `shape::file_uri`. Then add to `shape.rs`'s `#[cfg(test)] mod tests`:
  ```rust
  #[test]
  fn file_uri_round_trips_through_decode() {
      use std::path::Path;
      for raw in [
          "/repo/src/foo.rs",
          "/repo/my code/a b.rs",        // spaces
          "/repo/100%done/x.rs",         // percent
          "/repo/issue#42/x.rs",         // hash
          "/repo/café/déjà.rs",          // non-ASCII
      ] {
          let uri = file_uri(Path::new(raw));
          assert!(uri.starts_with("file://"), "uri must be file://: {uri}");
          // The encoded form must NOT contain raw spaces/# (they'd break URI parsing).
          assert!(!uri.contains(' '), "spaces must be encoded: {uri}");
          let decoded = decode_for_test(&uri);
          assert_eq!(decoded, raw, "round-trip failed for {raw} via {uri}");
      }
  }

  // file_path_from_uri takes an lsp_types::Uri; build one from the encoded string to exercise the real decoder.
  fn decode_for_test(uri: &str) -> String {
      use std::str::FromStr;
      let u = lsp_types::Uri::from_str(uri).expect("valid uri");
      file_path_from_uri(&u).expect("decodes")
  }
  ```
- [ ] Run `cargo test -p lsp-mcp file_uri_round_trips` and see it FAIL (spaces/# in the raw `format!` break the round-trip / `Uri::from_str`).
- [ ] Implement percent-encoding in `src/shape.rs`, mirroring the hand-rolled decoder style (no new deps):
  ```rust
  /// Build a `file://` request URI from an absolute path with proper percent-encoding (lsp-types 0.97 has
  /// no `Url::from_file_path`). The decoder partner is `file_path_from_uri`; the two MUST round-trip.
  pub(crate) fn file_uri(p: &std::path::Path) -> String {
      let mut out = String::from("file://");
      for b in p.to_string_lossy().as_bytes() {
          let b = *b;
          // Keep path-safe ASCII unescaped: unreserved + `/`. Everything else is %XX (UTF-8 byte-wise).
          let safe = b.is_ascii_alphanumeric()
              || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~');
          if safe {
              out.push(b as char);
          } else {
              out.push('%');
              out.push_str(&format!("{b:02X}"));
          }
      }
      out
  }
  ```
- [ ] Run `cargo test -p lsp-mcp file_uri_round_trips` and see it PASS. Run the whole crate's tests + the guarded `tests/integration.rs` (the Rust path still resolves `add` — the encoded `file://` for the ASCII fixture path is unchanged byte-wise).
- [ ] `git add crates/lsp-mcp/src/lsp/mod.rs crates/lsp-mcp/src/shape.rs && git commit -m "fix(lsp-mcp): percent-encode file:// URIs (round-trip with file_path_from_uri)"`

---

### Task 6: Python `LangServerConfig` (basedpyright) + interpreter discovery

`basedpyright-langserver --stdio`; python root markers (also the `is_project_root` predicate); `initialize_params` advertising NO `window/workDoneProgress`; `post_init_config` = `workspace/didChangeConfiguration` with the **WRAPPED `{ settings:{ python:{ pythonPath } } }` envelope** (the form Task 1 proves); `Readiness::Pyright` no-progress settle timed from `settled_at`; the ordered interpreter-discovery contract (`PyResolve`, exists+executable, explicit-invalid = HARD error); add `--python-path` to `Cli`.

**Files:**
- Modify: `crates/lsp-mcp/src/lib.rs` (`Cli.python_path`; replace the Task-4 `python not yet implemented` arm)
- Modify: `crates/lsp-mcp/src/lang.rs` (`pyright_config` incl. `is_project_root`, `resolve_python_path`→`PyResolve`, `is_usable_interpreter` exec-check; pure tests: discovery cases + `pyright_no_progress_is_ready_after_settle_not_full_bound`)
- Modify: `crates/lsp-mcp/src/lsp/mod.rs` (the no-progress settle in `wait_ready` for `Readiness::Pyright`, timed from `settled_at` set in `handshake`)
- Test: `crates/lsp-mcp/tests/lang_detect.rs` (interpreter-discovery unit tests — pure, fixture-driven, incl. hard-error + non-executable cases)

**Steps:**

- [ ] Add the CLI flag. In `crates/lsp-mcp/src/lib.rs` `Cli`:
  ```rust
  /// Python interpreter for basedpyright's `pythonPath` (highest-precedence override). Also LSP_MCP_PYTHON_PATH.
  #[arg(long)]
  pub python_path: Option<PathBuf>,
  ```
- [ ] Write the FAILING interpreter-discovery unit tests. Append to `crates/lsp-mcp/tests/lang_detect.rs`:
  ```rust
  use lsp_mcp::lang::{resolve_python_path, PyResolve};
  use std::os::unix::fs::PermissionsExt;

  fn make_exe(p: &std::path::Path) {
      std::fs::create_dir_all(p.parent().unwrap()).unwrap();
      std::fs::write(p, "#!/bin/sh\n").unwrap();
      let mut perm = std::fs::metadata(p).unwrap().permissions();
      perm.set_mode(0o755);
      std::fs::set_permissions(p, perm).unwrap();
  }

  /// A regular file with NO execute bits — exists but is not a usable interpreter.
  fn make_nonexec(p: &std::path::Path) {
      std::fs::create_dir_all(p.parent().unwrap()).unwrap();
      std::fs::write(p, "not executable\n").unwrap();
      let mut perm = std::fs::metadata(p).unwrap().permissions();
      perm.set_mode(0o644);
      std::fs::set_permissions(p, perm).unwrap();
  }

  #[test]
  fn explicit_flag_wins() {
      let d = td();
      let py = d.path().join("custom/python");
      make_exe(&py);
      match resolve_python_path(d.path(), Some(&py), None) {
          PyResolve::Resolved(p) => assert_eq!(p, py),
          _ => panic!("explicit valid path must Resolve"),
      }
  }

  #[test]
  fn explicit_invalid_path_is_hard_error() {
      let d = td();
      // (a) missing explicit path → Hard (caller bails; NO silent python3 fallback).
      let missing = d.path().join("nope/python");
      assert!(matches!(resolve_python_path(d.path(), Some(&missing), None), PyResolve::Hard(_)),
          "missing explicit path → Hard error, not Fallback");
      // (b) present-but-non-executable explicit path → Hard.
      let noexec = d.path().join("bin/python");
      make_nonexec(&noexec);
      assert!(matches!(resolve_python_path(d.path(), Some(&noexec), None), PyResolve::Hard(_)),
          "non-executable explicit path → Hard error, not Fallback");
  }

  #[test]
  fn nonexecutable_venv_python_is_not_usable() {
      let d = td();
      // A `.venv/bin/python` that exists but isn't executable must NOT be accepted as a venv interpreter.
      make_nonexec(&d.path().join(".venv/bin/python"));
      assert!(matches!(resolve_python_path(d.path(), None, None), PyResolve::Fallback),
          "non-executable venv python is skipped → Fallback");
  }

  #[test]
  fn virtual_env_beats_dot_venv() {
      let d = td();
      let ve = d.path().join("ve");
      make_exe(&ve.join("bin/python"));
      make_exe(&d.path().join(".venv/bin/python"));
      match resolve_python_path(d.path(), None, Some(ve.as_path())) {
          PyResolve::Resolved(p) => assert_eq!(p, ve.join("bin/python"), "$VIRTUAL_ENV precedes <repo>/.venv"),
          _ => panic!("must Resolve to the $VIRTUAL_ENV interpreter"),
      }
  }

  #[test]
  fn dot_venv_then_venv() {
      let d = td();
      make_exe(&d.path().join("venv/bin/python")); // only `venv`, no `.venv`
      match resolve_python_path(d.path(), None, None) {
          PyResolve::Resolved(p) => assert_eq!(p, d.path().join("venv/bin/python")),
          _ => panic!("must Resolve to <repo>/venv/bin/python"),
      }
  }

  #[test]
  fn no_venv_falls_back_to_python3_with_warning() {
      let d = td(); // empty repo, no venv, no $VIRTUAL_ENV, no explicit override
      assert!(matches!(resolve_python_path(d.path(), None, None), PyResolve::Fallback),
          "no venv + no explicit override → Fallback (caller uses python3 + LOGGED WARNING, not silent)");
  }
  ```
- [ ] Run `cargo test -p lsp-mcp --test lang_detect resolve_python_path` and see it FAIL (no `resolve_python_path`).
- [ ] Implement `resolve_python_path` + `pyright_config` in `crates/lsp-mcp/src/lang.rs`:
  ```rust
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
          return if is_usable_interpreter(p) {
              PyResolve::Resolved(p.to_path_buf())
          } else {
              PyResolve::Hard(p.to_path_buf()) // bad explicit override → HARD ERROR (no silent fallback)
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

  /// Python / basedpyright config. Resolves the interpreter, advertises NO `window/workDoneProgress` (so
  /// basedpyright emits `pyright/*Progress`), and sends `didChangeConfiguration { python.pythonPath }`.
  pub fn pyright_config(repo: &Path, explicit_python: Option<&Path>) -> anyhow::Result<LangServerConfig> {
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
               LSP_MCP_PYTHON_PATH (no silent fallback to python3 for an explicit override)", p
          ),
          // No explicit override AND no venv → degrade to `python3` on PATH with a LOGGED WARNING (stdlib
          // still resolves; third-party may be incomplete). This is the ONLY warned-fallback case.
          PyResolve::Fallback => {
              eprintln!(
                  "[lsp-mcp] WARNING: no venv interpreter found for {repo:?}; using `python3` on PATH — \
                   third-party (site-packages) resolution may be incomplete. Pass --python-path to fix."
              );
              "python3".to_string()
          }
      };
      let post = (
          "workspace/didChangeConfiguration".to_string(),
          json!({ "settings": { "python": { "pythonPath": python_path } } }),
      );
      Ok(LangServerConfig {
          name: "basedpyright",
          program_argv: vec!["basedpyright-langserver".to_string(), "--stdio".to_string()],
          spawn_env: vec![],
          // USE `is_project_root`: the Python root predicates (reuses the Task-4 detection helpers) — a real
          // pyproject section / setup.py / setup.cfg / requirements*.txt / a `.py` shallow-scan. `run()`
          // validates an explicit `--lang python` against this before starting basedpyright.
          is_project_root: Box::new(|root: &Path| {
              python_markers(root) || has_real_pyproject(root) || shallow_py_scan(root)
          }),
          initialize_params: Box::new(|root_uri: &str| {
              // Advertise NO window/workDoneProgress → basedpyright emits pyright/{begin,end}Progress instead.
              json!({
                  "processId": std::process::id(),
                  "rootUri": root_uri,
                  "capabilities": { "workspace": { "symbol": {}, "configuration": false } },
                  "workspaceFolders": [{ "uri": root_uri, "name": "root" }],
              })
          }),
          post_init_config: Some(post),
          new_readiness: Box::new(|| Readiness::Pyright(PyrightReady::default())),
      })
  }
  ```
  (Note on the `didChangeConfiguration` payload: this ships the **LSP-standard WRAPPED `settings` envelope** `{ "settings": { "python": { "pythonPath": … } } }` — which is exactly the form Task 1 Gate 1a drives and PROVES resolves a third-party def. The spec §2 wrote a bare `{ "python": {...} }`; the WRAPPED form is the proven one and is what this `post_init_config` ships. Do NOT regress to the bare form.)
- [ ] Run `cargo test -p lsp-mcp --test lang_detect resolve_python_path explicit_invalid_path_is_hard_error nonexecutable_venv_python_is_not_usable` and see ALL discovery tests PASS (incl. the hard-error + non-executable cases).
- [ ] **Add the AUTOMATED no-progress readiness test (D1 = targeted: a PURE unit test on the readiness state machine, NOT a live basedpyright).** It proves the FU3 guard per-language: settings applied, no progress notification arrives, and readiness becomes ready after the settle window WITHOUT progress — i.e. the first call does NOT wait the full bound. Add to `crates/lsp-mcp/src/lang.rs`'s `#[cfg(test)] mod tests`:
  ```rust
  #[test]
  fn pyright_no_progress_is_ready_after_settle_not_full_bound() {
      let settle = Duration::from_millis(50);
      // Simulate "settings applied, no progress notification arrives": settled_at = now, began = false.
      let mut p = PyrightReady { began: false, active: 0, settled_at: Some(Instant::now()) };
      // Immediately after settings applied (within the settle window): NOT yet ready, but also NOT a full
      // timeout — the settle is timed from settled_at, not from any wait entry.
      assert!(!p.settled_no_progress(settle), "not ready before the settle window elapses");
      assert!(!Readiness::Pyright(std::mem::take(&mut p)).is_ready(), "no progress + no settle ⇒ is_ready() false");
      // After the settle window with NO progress → ready (the first call returns fast, not at the full bound).
      let p = PyrightReady { began: false, active: 0, settled_at: Some(Instant::now() - Duration::from_millis(60)) };
      assert!(p.settled_no_progress(settle), "settle elapsed with no progress → ready (no full-bound wait)");
      // settled_at = None (settings not yet applied) is never settle-ready.
      let p = PyrightReady { began: false, active: 0, settled_at: None };
      assert!(!p.settled_no_progress(settle), "no settled_at (settings not applied) → not settle-ready");
      // A begin-without-end does NOT settle (began == true) — documented begin-without-end ceiling.
      let p = PyrightReady { began: true, active: 1, settled_at: Some(Instant::now() - Duration::from_secs(1)) };
      assert!(!p.settled_no_progress(settle), "begin seen ⇒ no no-progress settle (needs a matching end)");
  }
  ```
  (`PyrightReady` derives `Default`; the explicit-field construction needs the fields `pub` — they are, per Task 3. `std::mem::take` needs `Default`, satisfied.)
- [ ] Run `cargo test -p lsp-mcp pyright_no_progress_is_ready_after_settle_not_full_bound` and see it PASS.
- [ ] Replace the Task-4 placeholder arm in `run()`: change `crate::lang::Lang::Python => anyhow::bail!("python not yet implemented")` to `crate::lang::Lang::Python => crate::lang::pyright_config(&repo, cli.python_path.as_deref())?`. Run `cargo build -p lsp-mcp`.
- [ ] Implement the no-progress settle for `Readiness::Pyright` in `wait_ready` (so the first Python call doesn't pay the full bound), with the **CORRECT clock origin (Opus H2)**: the settle is timed from when settings were APPLIED (`PyrightReady.settled_at`), NOT from `wait_ready` entry. Timing from `wait_ready` entry made a begin-without-end server pay the FULL timeout. Set `settled_at = Some(Instant::now())` at the end of `handshake` for the Pyright variant, and **REPLACE the Task-3 `wait_ready` body** (do NOT add a second function) so it treats "settings applied + no progress within a short settle SINCE settings-applied" as ready via the pure `PyrightReady::settled_no_progress`:
  ```rust
  // in handshake(), AFTER sending post_init_config — stamp the settle-clock origin (settings applied now):
  if self.cfg.post_init_config.is_some() {
      if let crate::lang::Readiness::Pyright(s) = &mut *self.ready.lock().unwrap() {
          s.settled_at = Some(Instant::now());
      }
  }
  ```
  ```rust
  // in wait_ready(): a Pyright no-progress settle of ~1.5s AFTER settings-applied (settled_at) counts as
  // ready. The settle is timed from settled_at (via settled_no_progress), so a begin-without-end server is
  // ready ~settle after settings — it does NOT pay the full `timeout` (Opus H2 / the FU3 stall §2 forbids).
  // BEGIN-WITHOUT-END BEHAVIOR (documented): once any `beginProgress` is seen, `is_ready()` requires a
  // matching `endProgress` (begun-and-ended). If `end` never arrives, the no-progress settle does NOT
  // re-apply (s.began is true), so wait_ready returns at `timeout` (best-effort) — this is the begin-without-
  // end ceiling; the common path (no progress, or paired begin/end) is fast.
  pub fn wait_ready(&mut self, timeout: Duration) -> anyhow::Result<()> {
      let t0 = Instant::now();
      let settle = Duration::from_millis(1500);
      loop {
          self.touch();
          {
              let g = self.ready.lock().unwrap();
              if g.is_ready() {
                  return Ok(());
              }
              if let crate::lang::Readiness::Pyright(s) = &*g {
                  // No progress yet, settings applied, settle elapsed SINCE settled_at → ready.
                  if s.settled_no_progress(settle) {
                      return Ok(());
                  }
              }
          }
          if t0.elapsed() >= timeout {
              return Ok(());
          }
          std::thread::sleep(Duration::from_millis(100));
      }
  }
  ```
- [ ] Run `cargo test -p lsp-mcp` (whole crate) + `cargo clippy -p lsp-mcp -- -D warnings`; confirm green (the Python fixture tests come in Task 8; this task's tests are the pure discovery ones).
- [ ] `git add crates/lsp-mcp/src/lib.rs crates/lsp-mcp/src/lang.rs crates/lsp-mcp/src/lsp/mod.rs crates/lsp-mcp/tests/lang_detect.rs && git commit -m "feat(lsp-mcp): basedpyright LangServerConfig + interpreter discovery + --python-path + no-progress settle"`

---

### Task 7: Respawn re-sends config + post-eviction Python resolution test

`post_init_config` is already part of `handshake` (Task 3), and `respawn` calls `handshake` (Task 3) — so respawn structurally re-sends the `didChangeConfiguration`. This task GUARDS that with a live post-eviction Python resolution test (evict → next call respawns + still resolves a third-party def) and asserts the structural property at unit level.

**Files:**
- Test: `crates/lsp-mcp/tests/python_nav.rs` (the post-eviction live test — created here, extended in Task 8)
- (No production change expected — Task 3 already routes `post_init_config` through `handshake`, which `respawn` calls. If a unit assertion below fails, fix `respawn`/`handshake`.)

**Steps:**

- [ ] Add a unit-level structural guard in `crates/lsp-mcp/src/lang.rs` tests asserting the Python config carries a re-sendable `post_init_config` (so any future refactor that drops it from `handshake` is caught by the live test, and the config-level invariant is pinned here):
  ```rust
  #[test]
  fn pyright_config_carries_resendable_post_init() {
      let d = tempfile::tempdir().unwrap();
      let cfg = pyright_config(d.path(), None).unwrap();
      let (method, params) = cfg.post_init_config.expect("python MUST send post-init config");
      assert_eq!(method, "workspace/didChangeConfiguration");
      // The pythonPath key must be present (the venv that respawn re-applies).
      let s = serde_json::to_string(&params).unwrap();
      assert!(s.contains("pythonPath"), "post-init config must set pythonPath, got {s}");
  }
  ```
  (`tempfile = "3"` is already a dev-dep from Task 2; `lang.rs`'s `#[cfg(test)]` can use it.)
- [ ] Run `cargo test -p lsp-mcp pyright_config_carries_resendable_post_init` and see it PASS.
- [ ] Create the live post-eviction test file `crates/lsp-mcp/tests/python_nav.rs` with the basedpyright guard + a fixture helper (the fixture itself lands in Task 8; reference it here):
  ```rust
  //! Python (basedpyright) fixture tests. Guarded on a working `basedpyright-langserver` like the Rust
  //! integration tests guard on rust-analyzer. The fixture (tests/fixtures/pysample) is built in Task 8.
  use lsp_mcp::lang::pyright_config;
  use lsp_mcp::lsp::LspClient;
  use std::path::PathBuf;
  use std::time::Duration;

  fn pyright_available() -> bool {
      std::process::Command::new("basedpyright-langserver").arg("--version").output()
          .map(|o| o.status.success()).unwrap_or(false)
  }
  fn pysample() -> PathBuf {
      PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pysample")
  }
  fn pysample_venv_python() -> PathBuf {
      pysample().join(".venv/bin/python")
  }
  /// Skip unless basedpyright AND the fixture venv (with the third-party dep) both exist.
  fn ready() -> bool { pyright_available() && pysample_venv_python().is_file() }

  fn start() -> LspClient {
      let repo = pysample();
      let cfg = pyright_config(&repo, Some(&pysample_venv_python())).unwrap();
      LspClient::start_with(&repo, cfg).expect("start basedpyright")
  }

  #[test]
  fn post_eviction_still_resolves_third_party_def() {
      if !ready() { eprintln!("skip: basedpyright or fixture venv missing"); return; }
      let mut s = start();
      s.ensure_ready(Duration::from_secs(60)).unwrap();
      // First resolution of a third-party symbol works (the venv is applied via didChangeConfiguration).
      let before = s.definition("third_party_symbol").unwrap();
      assert!(!before.is_empty(), "third-party def must resolve before eviction");
      // Evict, then the NEXT call must respawn AND re-send didChangeConfiguration so the venv survives.
      s.evict();
      s.ensure_ready(Duration::from_secs(60)).unwrap();
      let after = s.definition("third_party_symbol").unwrap();
      assert!(!after.is_empty(), "third-party def must STILL resolve after respawn (config re-sent)");
      s.shutdown();
  }
  ```
  (`third_party_symbol` is a placeholder name for the real imported symbol the Task-8 fixture uses — replace it with the actual symbol once the fixture's third-party import is chosen in Task 8, e.g. `BaseModel` from pydantic.)
- [ ] Run `cargo test -p lsp-mcp --test python_nav post_eviction` — it will SKIP if basedpyright/the fixture venv aren't present yet (the fixture lands in Task 8). On a host with basedpyright + the Task-8 fixture, it must PASS. Run it for real after Task 8.
- [ ] `git add crates/lsp-mcp/src/lang.rs crates/lsp-mcp/tests/python_nav.rs && git commit -m "test(lsp-mcp): post-eviction Python resolution guard (respawn re-sends didChangeConfiguration)"`

---

### Task 8: Shared-code per-server handling + Python fixture tests for all 7 tools

Recursive `document_symbols.children` (REQUIRED — class→method); `resolve_pos` duplicate-name fixture; genericize the rust-flavored `references`/`implementations` tool descriptions; `hover` handles `MarkupContent` AND non-empty on `MarkedString[]`. Build a small Python fixture with a third-party import.

**Files:**
- Create: `crates/lsp-mcp/tests/fixtures/pysample/` (package + `.venv` w/ a third-party dep, built by a setup step)
- Modify: `crates/lsp-mcp/src/lsp/mod.rs` (`document_symbols` recursion; `hover` array form)
- Modify: `crates/lsp-mcp/src/mcp/mod.rs` (genericize 2 tool descriptions)
- Test: `crates/lsp-mcp/tests/python_nav.rs` (extend — 7-tool coverage + duplicate-name + recursive symbols + non-empty hover)
- Test: `crates/lsp-mcp/tests/integration.rs` (ADD a guarded Rust `document_symbols` test locking the additive nested-children output against the `sample` fixture)

**Steps:**

- [ ] Build the Python fixture. Create `crates/lsp-mcp/tests/fixtures/pysample/` mirroring the Rust `tests/fixtures/sample` shape:
  - `pyproject.toml`:
    ```toml
    [project]
    name = "pysample"
    version = "0.0.0"
    dependencies = ["pydantic>=2"]
    ```
  - `pysample/__init__.py`: empty
  - `pysample/models.py` — a class with a method (hierarchical symbols), a third-party import, and a DUPLICATE name across two scopes. **The recursive-`document_symbols` test must FAIL if `children` recursion is dropped, so the NESTED method name must NOT also exist at module scope** (else `contains("greet")` false-greens from the module function). The class method is `greet`; the module-level function is renamed `module_greet` and the duplicate-name pair for `resolve_pos` is `module_greet` (module) vs a method `module_greet`-free design — we keep a distinct duplicate via a method named `greet` + a module function `greet_text` (NOT `greet`), and assert `document_symbols` sees `greet` ONLY as a child of `Greeter`:
    ```python
    from pydantic import BaseModel

    GREETING = "hi"

    class Greeter(BaseModel):           # third-party base class (third-party def target)
        name: str

        def greet(self) -> str:         # class->method `greet` — exists ONLY as a CHILD of Greeter
            return f"{GREETING} {self.name}"

        def module_greet(self) -> str:  # DUPLICATE name `module_greet` (method side; resolve_pos first-hit)
            return GREETING

    def module_greet() -> str:          # DUPLICATE name `module_greet` at module scope (resolve_pos first-hit)
        return GREETING
    ```
    (`greet` is now nested-ONLY: it appears in `document_symbols` output solely via `Greeter.children` extraction — so the recursive test fails if recursion is dropped. `module_greet` is the duplicate-name pair for `resolve_pos`, present at BOTH module and method scope.)
  - A `.gitignore` in the fixture dir ignoring `.venv/`.
- [ ] Add a fixture-venv setup note + helper. Create `crates/lsp-mcp/tests/fixtures/pysample/README.md` (one line) documenting the setup command the test host runs once:
  ```
  python3 -m venv .venv && .venv/bin/pip install 'pydantic>=2'
  ```
  (The live tests guard on `.venv/bin/python` existing — see Task 7's `ready()`. Do NOT commit the `.venv`.)
- [ ] Update Task 7's placeholder symbol: in `crates/lsp-mcp/tests/python_nav.rs`, replace `"third_party_symbol"` with `"BaseModel"` (the pydantic base class imported in `models.py`) — `definition("BaseModel")` resolves into the venv's pydantic site-packages.
- [ ] Write the FAILING recursive-`document_symbols` test. Append to `python_nav.rs`:
  ```rust
  fn models_py() -> PathBuf { pysample().join("pysample/models.py") }

  #[test]
  fn document_symbols_extracts_class_and_method_recursively() {
      if !ready() { eprintln!("skip"); return; }
      let mut s = start();
      s.ensure_ready(Duration::from_secs(60)).unwrap();
      let syms = s.document_symbols(&models_py()).unwrap();
      let names: Vec<&str> = syms.iter().filter_map(|h| h.signature.as_deref()).collect();
      assert!(names.iter().any(|n| *n == "Greeter"), "class Greeter, got {names:?}");
      // `greet` exists ONLY as a child of `Greeter` (no module-level `greet`). Asserting EXACT `greet`
      // proves child extraction — this test FAILS if `children` recursion is dropped (the flat top-level
      // parse never sees `greet`). Use exact equality, not `contains`, so `module_greet` can't false-green.
      assert!(names.iter().any(|n| *n == "greet"),
          "method `greet` must appear via Greeter.children recursion, got {names:?}");
      s.shutdown();
  }
  ```
- [ ] Run `cargo test -p lsp-mcp --test python_nav document_symbols_extracts` on a host with the fixture — see it FAIL (the current `document_symbols` parses only the FLAT top-level array; basedpyright nests `greet` under `Greeter.children`).
- [ ] Implement recursive `children` extraction in `crates/lsp-mcp/src/lsp/mod.rs` `document_symbols`. Replace the flat loop with a recursive walk that handles BOTH the `DocumentSymbol{children}` shape and the flat `SymbolInformation` shape (rust-analyzer reads `range` → already `DocumentSymbol[]`, so this is additive):
  ```rust
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
  /// SymbolInformation form (no `children`). Required so Python class methods aren't dropped (spec §1).
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
  ```
- [ ] Run `cargo test -p lsp-mcp --test python_nav document_symbols_extracts` and see it PASS. **The recursion ADDITIVELY includes nested children in the Rust `document_symbols` output too — it is NOT "byte-compatible" with the old flat parse (there was no Rust `document_symbols` test, so "re-run to confirm byte-compatible" would verify nothing). Instead, ADD a guarded Rust `document_symbols` characterization test that LOCKS the new recursive output against the `sample` fixture** (which has a trait `Greet` with a nested method `hi`). In `crates/lsp-mcp/tests/integration.rs` (it already has `sample_repo()` + `ra_available()`), add:
  ```rust
  #[test]
  fn rust_document_symbols_includes_nested_trait_method() {
      if !ra_available() { eprintln!("skip: rust-analyzer not on PATH"); return; }
      let mut s = lsp_mcp::lsp::LspClient::start(&sample_repo(), None).unwrap();
      s.ensure_ready(std::time::Duration::from_secs(120)).unwrap();
      let syms = s.document_symbols(&sample_repo().join("lib.rs")).unwrap();
      let names: Vec<&str> = syms.iter().filter_map(|h| h.signature.as_deref()).collect();
      // Top-level items still present (additive, not a replacement).
      assert!(names.iter().any(|n| *n == "add"), "top-level fn add, got {names:?}");
      assert!(names.iter().any(|n| *n == "Greet"), "trait Greet, got {names:?}");
      // NEW: the trait method `hi` is now extracted via children recursion (it was DROPPED by the old flat
      // parse). This LOCKS the additive recursive output for Rust — the change is intended, not byte-for-byte.
      assert!(names.iter().any(|n| *n == "hi"), "nested trait method `hi` (recursion), got {names:?}");
      s.shutdown();
  }
  ```
  (`hi` appears as a child of the `Greet` trait and the `impl Greet for En` block — the recursive walk surfaces it; the old flat top-level parse never did. This guarded test is the lock; the plan's wording is corrected: **Rust `document_symbols` now additively includes nested children — intended, locked by this test — NOT byte-compatible.**) Then run the full crate test (`cargo test -p lsp-mcp`) to confirm the other Rust paths (`definition`/`references`/`call_hierarchy`/`workspace_symbol`) are unchanged.
- [ ] Write the duplicate-name `resolve_pos` + non-empty `hover` + remaining-tools tests. Append to `python_nav.rs`:
  ```rust
  #[test]
  fn resolve_pos_handles_duplicate_name() {
      if !ready() { eprintln!("skip"); return; }
      let mut s = start();
      s.ensure_ready(Duration::from_secs(60)).unwrap();
      // `module_greet` exists as BOTH a method (Greeter.module_greet) and a module function. First-hit must
      // resolve to a real location (degradation documented: we keep the name-only API; basedpyright ranks
      // the hits). (`greet` is the nested-only name used by the recursion test — distinct from this pair.)
      let def = s.definition("module_greet").unwrap();
      assert!(!def.is_empty(), "duplicate `module_greet` must resolve to some def, got {def:?}");
      s.shutdown();
  }

  #[test]
  fn hover_is_non_empty() {
      if !ready() { eprintln!("skip"); return; }
      let mut s = start();
      s.ensure_ready(Duration::from_secs(60)).unwrap();
      let h = s.hover("Greeter").unwrap();
      assert!(h.as_deref().map(|x| !x.is_empty()).unwrap_or(false),
          "hover must return non-empty content (MarkupContent or MarkedString[]), got {h:?}");
      s.shutdown();
  }

  #[test]
  fn workspace_symbol_finds_class() {
      if !ready() { eprintln!("skip"); return; }
      let mut s = start();
      s.ensure_ready(Duration::from_secs(60)).unwrap();
      assert!(!s.workspace_symbol("Greeter").unwrap().is_empty());
      s.shutdown();
  }

  #[test]
  fn references_and_callhierarchy_and_definition() {
      if !ready() { eprintln!("skip"); return; }
      let mut s = start();
      s.ensure_ready(Duration::from_secs(60)).unwrap();
      assert!(!s.definition("Greeter").unwrap().is_empty(), "definition");
      let _refs = s.references("greet", true).unwrap();      // must not error
      let _calls = s.call_hierarchy("greet", true).unwrap(); // incoming callers, must not error
      let _impls = s.implementations("Greeter").unwrap();    // basedpyright may return [] — must not error
      s.shutdown();
  }
  ```
- [ ] On a host with the fixture, run `cargo test -p lsp-mcp --test python_nav` and see `hover_is_non_empty` (and possibly others) FAIL where shaping differs. Fix `hover` to handle the `MarkedString[]` array form in `src/lsp/mod.rs`:
  ```rust
  pub fn hover(&mut self, name: &str) -> anyhow::Result<Option<String>> {
      let v = self.positional("textDocument/hover", name)?;
      // MarkupContent { value } | a bare MarkedString string | MarkedString[] (array of strings/objects).
      let s = v["contents"]["value"].as_str().map(str::to_string)
          .or_else(|| v["contents"].as_str().map(str::to_string))
          .or_else(|| {
              v["contents"].as_array().map(|arr| {
                  arr.iter()
                      .filter_map(|e| e.as_str().map(str::to_string)
                          .or_else(|| e["value"].as_str().map(str::to_string)))
                      .collect::<Vec<_>>()
                      .join("\n")
              }).filter(|s| !s.is_empty())
          });
      Ok(s)
  }
  ```
- [ ] Genericize the two Rust-flavored tool descriptions in `crates/lsp-mcp/src/mcp/mod.rs`. Replace:
  - `references`: `"All references to a symbol (blast radius); resolves generics/traits."` → `"All references to a symbol (blast radius); type-resolved across the language's generics/polymorphism."`
  - `implementations`: `"Trait impls / who implements a trait or type."` → `"Implementations of a symbol (Rust trait impls; Python subclasses / overrides)."`
- [ ] Update the `mcp/mod.rs` unit test if it asserts on description text (it asserts only on tool names + count — confirm `exposes_the_seven_tools` still passes). Run `cargo test -p lsp-mcp --lib`.
- [ ] Run `cargo test -p lsp-mcp --test python_nav` on the fixture host and see all 7-tool tests PASS. Run the FULL crate test (`cargo test -p lsp-mcp`) + `cargo clippy -p lsp-mcp -- -D warnings`; confirm the Rust path is still green.
- [ ] `git add crates/lsp-mcp/tests/fixtures/pysample crates/lsp-mcp/src/lsp/mod.rs crates/lsp-mcp/src/mcp/mod.rs crates/lsp-mcp/tests/python_nav.rs crates/lsp-mcp/tests/integration.rs && git commit -m "feat(lsp-mcp): Python 7-tool fixture coverage — recursive document_symbols (+ Rust nested-children lock), hover arrays, generic tool descs"`

---

### Task 9: Live `{cwd}` gate (relocated) + host wiring + DoD

First runs the relocated LIVE `{cwd}` codex gate (it needs the Task-4 binary's `--lang auto` + startup root/lang log, so it could not live in Task 1). Per **decision D2: if the gate finds codex `{cwd}` BROKEN, FU1 is MANDATORY C1 work — there is NO claude-only fallback.** Then wires the host reviewers' lsp entry to `--lang auto`; a host basedpyright presence/version check in the DoD; the live DoD = host review of one of the user's Python repos with semantic nav working, covering BOTH claude + codex; full-branch review vs main before merge.

**Files:**
- Modify: `examples/a2a-bridge.containerized.toml` (the two host reviewers' `lsp` entries — claude ~line 110, codex ~line 128)
- Modify (MANDATORY if the gate finds `{cwd}` broken for codex — D2): `bin/a2a-bridge/src/main.rs` (FU1 fix — thread per-request `session_cwd` into codex's `{cwd}`)

**Steps:**

- [ ] **RELOCATED LIVE `{cwd}` codex gate (the go/no-go; moved from Task 1, now runnable).** It runs HERE because `--lang auto` + the startup root/lang log only exist after Task 4. Build the release binary first (`cargo build --release -p lsp-mcp`; confirm `target/release/lsp-mcp --help` shows `--lang` default `auto` and `--python-path`). Then run the bridge's host **codex** reviewer against a Python repo via the per-request session-cwd:
  ```
  a2a-bridge run-workflow code-review --session-cwd ~/code/agent-eval \
    --config <a host config whose codex `lsp` entry uses --lang auto>
  ```
  Inspect the lsp-mcp startup log (the `[lsp-mcp] root=… lang=…` line + the call-log) and confirm codex's lsp-mcp resolved its `--repo {cwd}` to **that Python repo** (startup log shows the target root + `lang=python`), NOT the bridge launch dir (which would `auto`→rust silently). Record the observed root + detected language into the spike verdict file's Gate-4 section. **This is the fork point for the rest of Task 9.**
- [ ] **Apply the D2 fork from the gate result:**
  - If `{cwd}` resolved correctly for codex → no FU1 needed; the live DoD below covers BOTH claude + codex. Skip to the wiring step.
  - **If `{cwd}` was BROKEN for codex → FU1 is MANDATORY (D2 — NO claude-only fallback).** Fix it with a TDD step (failing test → fix → pass), threading the per-request `session_cwd` into codex's `render_codex_mcp_args` `{cwd}` at the SpawnFn boundary in `bin/a2a-bridge/src/main.rs`. The seam (verified against ground truth):
    - the spawn site computes `resolve_static_session_cwd(entry.session_cwd.as_deref(), entry.cwd.as_deref())` (~line 470-471) → `cwd`;
    - `acp_spawn_inputs` canonicalizes that `cwd` into `mcp_cwd` (~line 201) and feeds it to `acp_program_argv(entry, …, &mcp_cwd)` (~line 240, helper at ~line 121-162);
    - `acp_program_argv` passes `mcp_cwd` to `render_codex_mcp_args(&entry.mcp, mcp_cwd)` (~line 140) — so codex's native `-c mcp_servers.*` `{cwd}` is substituted from `mcp_cwd`;
    - the per-request `--session-cwd` is stamped into `entry.session_cwd` for run-workflow at ~line 2079-2083 (`for e in &mut snapshot.entries { e.session_cwd = Some(dir.clone()); }`) BEFORE the SpawnFn runs.
    So for `run-workflow` the chain already threads the stamped `session_cwd` → `mcp_cwd` → codex `{cwd}`. **FU1's job is to (a) prove the live break's actual cause (the gate showed it broken at runtime even though the source chain looks correct — the memory records this disconnect is "unreadable from source"), and (b) ensure EVERY host-reviewer-driving path stamps `session_cwd` BEFORE `mcp_cwd` is computed.** Concretely:
    1. **Failing test first:** next to the existing `resolve_static_session_cwd_chain`/`acp_program_argv_appends_codex_native_mcp_args` tests (~line 3704-3751), add `fn codex_mcp_cwd_uses_stamped_session_cwd()` that builds a codex `AgentEntry` with `mcp` set and `session_cwd = Some("/python/repo")`, runs the SAME resolution the spawn site does (`resolve_static_session_cwd(entry.session_cwd.as_deref(), entry.cwd.as_deref())` → canonicalize-or-passthrough → `acp_program_argv(&entry, None, &[], &mcp_cwd)`), and asserts the codex argv contains the `{cwd}`-substituted value `/python/repo` (NOT the launch dir). If the current code already passes this, the test still LOCKS the contract against regression; if the live break is in a path the test doesn't cover (e.g. the `serve`/`message/send` per-request stamp at the points where the host reviewer is actually driven), extend the test/fix to THAT path — add the missing `e.session_cwd = Some(dir)` stamp before the spawn computes `mcp_cwd` there.
    2. **Fix:** add the missing per-request stamp (or correct the substitution) so the failing test passes.
    3. **Re-run the live gate** from the previous step and confirm codex now logs `lang=python` + the correct root.
  - There is NO option-B (claude-only) path. Remove any expectation of shipping the Python gate claude-only — D2 makes FU1 in-scope C1 work when the gate breaks.
- [ ] Change the host reviewers' lsp entries to `--lang auto`. In `examples/a2a-bridge.containerized.toml`, for BOTH the `claude` agent's lsp entry (~line 110) and the `codex` agent's lsp entry (~line 128), change `"--lang", "rust"` → `"--lang", "auto"`. Leave the `impl` agent's in-container lsp entry (~line 176) as `"--lang", "rust"` (the container always passes explicit `--lang`; Python-in-container is Slice C2). Update the adjacent comment from "type-resolved semantic nav via rust-analyzer" to "type-resolved semantic nav via rust-analyzer (rust) or basedpyright (python), auto-detected per repo".
- [ ] Mirror the change into `examples/a2a-bridge.containerized.podman.toml` (it has the same three `name = "lsp"` entries at ~108/126/176 — change the two HOST reviewer entries to `--lang auto`, leave the in-container `impl` one as `rust`).
- [ ] DoD basedpyright presence check: run `basedpyright-langserver --version` on the host and record it (mirrors the Rust path's `rust-analyzer --version` precedent). If absent, `pip install basedpyright`. Keep this presence/version check in the DoD.
- [ ] **REQUIRED DoD gate (Codex#8): the Python 7-tool suite must actually RUN, not silently skip.** The `python_nav.rs` tests guard on basedpyright + the fixture venv via `ready()` (Task 7) — if either is absent, ALL Python tests skip and the suite false-greens. So the DoD MUST set up the fixture venv and RUN the suite, failing/flagging if absent. Explicit DoD commands:
  ```
  # 1. fixture venv (one-time; gitignored) — the explicit setup command:
  python3 -m venv crates/lsp-mcp/tests/fixtures/pysample/.venv \
    && crates/lsp-mcp/tests/fixtures/pysample/.venv/bin/pip install 'pydantic>=2'
  # 2. presence gate (FAIL the DoD if missing — do NOT let the suite report green by skipping):
  basedpyright-langserver --version || { echo "DoD FAIL: basedpyright absent"; exit 1; }
  test -x crates/lsp-mcp/tests/fixtures/pysample/.venv/bin/python \
    || { echo "DoD FAIL: fixture venv absent — python_nav would silently skip"; exit 1; }
  # 3. run the Python suite (must execute the 7-tool + post-eviction tests, not skip):
  cargo test -p lsp-mcp --test python_nav
  ```
  Confirm the run shows the Python tests EXECUTING (e.g. `document_symbols_extracts_class_and_method_recursively ... ok`, `post_eviction_still_resolves_third_party_def ... ok`) — not `0 passed; 0 failed` from blanket skips. Record the test output in the DoD notes.
- [ ] LIVE DoD — host review of a Python repo with semantic nav, covering BOTH claude + codex. Pick one of `~/code/code-review-backtest`, `~/code/agent-eval`, `~/code/a2a-local-bridge` (whichever is a clean unambiguous Python repo). Run a host code-review through the bridge against it (`a2a-bridge run-workflow code-review --session-cwd <python-repo> --config examples/a2a-bridge.containerized.toml`) and confirm in the lsp-mcp call log that: (a) the startup line shows `lang=python` + the correct root, (b) at least one lsp tool call (e.g. `definition`/`hover`/`references`) returned semantic results. **The live DoD covers BOTH claude AND codex** (codex's `{cwd}` is correct either because the gate passed or because FU1 fixed it). Capture the evidence into the DoD notes (paste the relevant call-log lines into the commit message or a scratch note).
- [ ] Run the full workspace test + lint floors to confirm no cross-crate regression: `cargo test -p lsp-mcp && cargo clippy -p lsp-mcp -- -D warnings && cargo fmt --check`. (lsp-mcp + possibly `bin/a2a-bridge` (FU1) are the only crates touched; the workspace floors live in `ci.yml`.)
- [ ] `git add examples/a2a-bridge.containerized.toml examples/a2a-bridge.containerized.podman.toml` (+ `bin/a2a-bridge/src/main.rs` if FU1 was taken) `&& git commit -m "feat(lsp-mcp): host reviewers --lang auto + Python live DoD (claude+codex) (Slice C1)"`
- [ ] Full-branch review vs main before merge: run the bridge's own review on the whole `feat/lsp-mcp-slice-c` diff (`git diff main...HEAD`) — host code-review + a clean-room design cross-check — and address any blockers. Then use superpowers:finishing-a-development-branch to decide merge/PR.

---

## Self-review notes

**Spec § coverage:**

| Spec § | Covered by |
| --- | --- |
| §1 `--lang auto` predicates (rust/python/tooling-only/`.py`-scan/ambiguous) | Task 4 (`detect_lang` + `tests/lang_detect.rs`); `has_real_pyproject` `dynamic` arm anchored to `starts_with("dynamic")` |
| §1 startup LOGS resolved root + language | Task 4 (`eprintln!("[lsp-mcp] root=… lang=…")` in `run()`) |
| §1 split `LspClient` from `LangServerConfig` | Task 3 |
| §1/§2 `is_project_root` field added AND wired (explicit `--lang` validated before start) | Task 3 (field on `LangServerConfig` + rust predicate) + Task 4 (`run()` calls `(cfg.is_project_root)(repo)` for explicit `--lang`) + Task 6 (pyright predicate) |
| §1 `Readiness` absorbs ONLY notification parsing; `id`-routing STAYS in `LspClient` | Task 3 (`Readiness::on_notification`; reader thread keeps the `if let Some(id)` route inline) |
| §1 `Readiness::RustRa` wraps current logic unchanged | Task 3 (`RustReady` + `rust_ra_config`) + Task 2 characterization proving byte-for-byte (readiness + resolution paths; EXCEPT the intentional `document_symbols` recursion change AND the one added `hierarchicalDocumentSymbolSupport` initialize capability that enables it — both intended + locked by tests; see Task 8) |
| §1 fake-LSP characterization HARNESS FIRST (transition table + happy respawn) | Task 2 (green on current code, then re-asserted post-refactor); + Task 3 adds the GENUINE respawn-FAILURE + request-touch tests |
| §1 `resolve_pos` duplicate-name fixture + documented degradation | Task 8 (`resolve_pos_handles_duplicate_name`, `models.py` duplicate `greet`) |
| §1 recursive `document_symbols.children` REQUIRED (class→method) | Task 8 (`collect_doc_symbols` recursion + test) |
| §1 `file://` URI round-trip fix | Task 5 |
| §1 genericize the 2 rust-flavored tool descriptions | Task 8 |
| §1 `hover` handles `MarkupContent` AND `MarkedString[]` non-empty | Task 8 (`hover` array arm + `hover_is_non_empty`) |
| §2 server `basedpyright-langserver --stdio` | Task 6 (`pyright_config.program_argv`) |
| §2 root markers | Task 4 (`python_markers`/`has_real_pyproject`) |
| §2 interpreter discovery contract (5-step precedence + validate exists+executable + warn) | Task 6 (`resolve_python_path`→`PyResolve`; `is_usable_interpreter` checks mode&0o111; tests incl. `explicit_invalid_path_is_hard_error`, `nonexecutable_venv_python_is_not_usable`) |
| §2 explicit invalid `--python-path`/env → HARD error (not silent `python3`) | Task 6 (`PyResolve::Hard` → `anyhow::bail!`; `python3`+warning is no-explicit-override only) |
| §2 `--python-path`/`LSP_MCP_PYTHON_PATH` | Task 6 (`Cli.python_path` + env lookup) |
| §2 `didChangeConfiguration` WRAPPED `{ settings:{ python:{ pythonPath } } }` delivery, no `workspace.configuration` | Task 6 (`post_init_config` wrapped envelope + `configuration: false`) — proven by Task 1 Gate 1a |
| §2 respawn re-sends config + post-eviction resolution test | Task 7 (structural via Task 3's `handshake`-in-`respawn`; live guard test) |
| §2 `Readiness::Pyright` `pyright/{begin,end}Progress` + no-progress settle (first call doesn't wait full bound; settle timed from `settled_at`, not wait entry) | Task 6 (`PyrightReady.settled_at` + `settled_no_progress` in `wait_ready`) + automated pure test `pyright_no_progress_is_ready_after_settle_not_full_bound` |
| §3 host reviewers' `--lang auto` entry | Task 9 |
| §3 host basedpyright presence/version check in DoD | Task 9 |
| §3 Python 7-tool coverage as a REQUIRED DoD gate (fail if basedpyright/fixture-venv absent — no silent skip) | Task 9 (explicit fixture-venv setup + presence gate + `cargo test -p lsp-mcp --test python_nav`) |
| §3 `{cwd}` asymmetry as a LIVE GATE (not soft); FU1 MANDATORY-if-broken (D2) | Task 9 first step (relocated live gate) + D2 fork (FU1 in-scope, no claude-only) |
| §4.1 config-channel resolution (w/ + w/o repo override + no-venv fallback, WRAPPED envelope) | Task 1 Gates 1a/1b/1c |
| §4.2 readiness + no-progress + begin-without-end spike | Task 1 Gate 2 |
| §4.3 `--lang auto` detection LOGIC spike | Task 1 Gate 3 |
| §4.4 live `{cwd}` codex gate + go/no-go fork | RELOCATED to Task 9 first step (needs the Task-4 binary) |
| §4 spike verdict file | Task 1 |
| Tests/regression: all 7 tools Python fixture | Task 8 |
| Tests/regression: interpreter discovery | Task 6 |
| Tests/regression: post-eviction Python resolution | Task 7 |
| Tests/regression: URI round-trip | Task 5 |
| Tests/regression: existing Rust tests stay green (FU3 behind `Readiness::RustRa`) | Task 2 + Task 3 (full-crate test gate each task) |
| Tests/regression: Rust `document_symbols` additive nested-children LOCK | Task 8 (`rust_document_symbols_includes_nested_trait_method` guarded against the `sample` fixture) |
| Tests/regression: automated no-progress settle (pure, not live) | Task 6 (`pyright_no_progress_is_ready_after_settle_not_full_bound`) |
| Execution order (spike → characterization → refactor → Python config → wiring → full-branch review) | Tasks 1→9 in order |

**Review must-fix coverage:**

| Review must-fix | Covered by |
| --- | --- |
| Respawn-config-resend pulled INTO C1 (was wrongly C2-deferred) | Task 7 (+ Task 3 making `respawn`→`handshake`→`post_init_config`) |
| Explicit interpreter/venv discovery contract (not "auto-detected" prose) | Task 6 |
| Broadened fake-LSP characterization harness (transition table + request-touch + respawn ordering + no-post-init) | Task 2 (happy path) + Task 3 (GENUINE respawn-FAILURE→`evicted=true` via bogus `program_argv`; request-touch test) |
| `testkit` compile bug (external `tests/` can't see `pub(crate)`; `pub use` of `pub(crate)` won't compile; no `testkit` module) | Task 2 + Task 3 (genuinely-`pub` items + `#[doc(hidden)] pub mod testkit`) |
| `respawn_failure_leaves_evicted_true` was MISLABELED (asserted success) | Renamed to `respawn_success_clears_evicted` (Task 2); genuine failure test added (Task 3) |
| Concrete `--lang auto` predicates (not prose); `dynamic` over-match | Task 4 (`detect_lang`; `dynamic` → `starts_with`) |
| `is_project_root` add-AND-use (not a dead field) | Task 3 (add) + Task 4/Task 6 (wire in `run()`) |
| Required hierarchical `document_symbols` (not optional); dup-name fixture false-green; Rust output NOT byte-compatible | Task 8 (recursion; fixture `greet` nested-ONLY + `module_greet` dup pair; guarded Rust nested-children lock + corrected wording) |
| `Readiness` must NOT drag `id`-routing along | Task 3 (id-route stays inline in reader thread; only notifications delegate) |
| Live `{cwd}` gate decides FU1; FU1 MANDATORY-if-broken (D2) | Task 9 first step (relocated gate) + D2 fork (FU1 in-scope, TDD; no claude-only) |
| URI builder asymmetry (encode vs decode) | Task 5 |
| `references`/`implementations` descriptions are Rust-flavored, advertised to the agent | Task 8 |
| `hover` must not silently return None on `MarkedString[]` | Task 8 |
| Interpreter exec-validation (exists AND executable) + explicit-invalid HARD error | Task 6 (`is_usable_interpreter` mode&0o111; `PyResolve::Hard`→bail) |
| Settle clock origin (was timed from `wait_ready` entry → begin-without-end paid full timeout) | Task 6 (`PyrightReady.settled_at` set at settings-applied; `settled_no_progress`) + pure test |
| No-venv (no explicit override) → `python3` + LOGGED WARNING (not silent empty) | Task 6 (`PyResolve::Fallback` → `eprintln!` WARNING) |
| Python 7-tool coverage must RUN in the DoD (no silent skip) | Task 9 (fixture-venv setup + presence gate + run `--test python_nav`) |

**Open follow-ups (not C1):** Per **decision D2**, FU1 (codex `{cwd}`) is **MANDATORY C1 work IF the Task-9 live gate finds codex `{cwd}` broken** — there is NO claude-only fallback. If the gate finds `{cwd}` already correct, no FU1 is needed and the live DoD covers both reviewers as-is. All of Slice C2 (in-container Python implementor, per-language verify, uv warm) is explicitly deferred per the spec's C2 section.

**Harness scope (decision D1):** the readiness/respawn coverage uses **TARGETED fixes** — pure unit tests on the `Readiness` state machine + the no-progress settle, doc-hidden test wrappers to drive `respawn`/`evicted`, and a bogus-`program_argv` `LangServerConfig` to force a respawn failure — **NOT a full fake-rust-analyzer binary**.

**Placeholder scan:** No "TBD"/"similar to above"/"add error handling" placeholders. Three intentional, explicitly-flagged forward references: (1) Task 4's `Lang::Python` arm is a temporary `bail!` replaced in Task 6 (flagged inline); (2) Task 7's `third_party_symbol` placeholder is replaced by `BaseModel` in Task 8 (flagged); (3) Task 9's FU1 fix is conditional on the live-gate result (D2 — mandatory if broken; the seam + TDD step are fully specified). The `didChangeConfiguration` payload is NO LONGER conditional — Task 1 proves and Task 6 ships the SAME wrapped `settings` envelope. Each forward reference is a real, named action — not a vague gap.
