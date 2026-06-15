# LSP-over-MCP Go (gopls) nav — host reviewers — Design

**Status:** approved direction (brainstormed 2026-06-15). MUCH smaller than Slice C1: the `--lang`→language-server REGISTRY, the `LangServerConfig`/`Readiness` seam, the byte-for-byte Rust characterization harness, the MCP-framing/protocolVersion/PATH bugs, and the hierarchical `document_symbols` + percent-encoded `file://` + generic tool descriptions are ALL already shipped and merged in C1. This increment ("Go nav") ADDS one config (`go_config`) + one `Lang::Go` arm + a `resolve_lsp_server` search extension to that working registry — strictly mirroring the C1/basedpyright pattern. No client/refactor/harness work. Pending final user review.
**Predecessors:** Slice A (host reviewers, Rust), Slice C1 (the multi-language registry refactor + Python/basedpyright). This increment is the THIRD language onto the C1 registry.

## Goal

Let the bridge's **host-side reviewers** navigate **Go** repos with the same 7-tool type-resolved MCP nav surface they already get on Rust and Python, by adding **Go (gopls)** to the `--lang auto` registry. The host reviewers' `[[agents.mcp]] lsp` entries are ALREADY `--lang auto` (C1), so once `detect_lang` recognizes a `go.mod` root and `go_config()` exists, a Go repo is served with zero config change. This is the minimal third-language proof on the registry — Go is materially **simpler than Python** (gopls auto-configures from `go.mod`: no interpreter discovery, no `didChangeConfiguration`, no venv).

## Non-goals (this increment)

- **The in-container Go implementor / Go verify / a Go toolchain image** → overlaps the deferred **Slice C2** (in-container, per-language verify). This increment is **HOST reviewers ONLY**; the in-container `impl` agent stays `--lang rust`.
- **No client refactor.** `LspClient`, the `Readiness` seam, the reader-thread id-routing, `wait_ready`, idle-evict/respawn, the 7 tools, percent-encoded `file://`, recursive `document_symbols`, the generic tool descriptions — all land unchanged from C1. Go ADDS a `LangServerConfig`; it does not touch the client.
- **No characterization harness.** C1's `tests/characterization.rs` + the existing Rust integration tests already pin the Rust path byte-for-byte; adding a config does not perturb them (re-run as a regression gate, do not rewrite).
- **TypeScript/JS** and any further languages — later increments.
- **Diagnostics / type-check / editor-grade tools** — stays the 7 read-only nav tools.
- **Hybrid/polyglot repos & subdir-rooted projects** — `--lang auto` refuses ambiguous roots (now extended to rust+go / python+go); `--project-root` remains C2.

## §1. The registry addition (the whole increment, in one place)

C1 already produced the language-agnostic `LspClient` parameterized by a `LangServerConfig` `{ name, program_argv, spawn_env, is_project_root, initialize_params, post_init_config, new_readiness }`, with a `Readiness` enum owning ONLY the reader-thread notification parsing (id-routing stays in `LspClient`). Adding Go is three localized edits to `crates/lsp-mcp/src/lang.rs` (+ one `run()` arm in `lib.rs`):

1. **`Lang::Go`** added to the `Lang` enum (`as_str()` → `"go"`), and **`detect_lang`** extended: `go` iff `go.mod` at the root. The current `detect_lang` is a 2-way `(is_rust, is_python)` match; it becomes a 3-marker decision where **any two-of-three markers present → ambiguous → refuse to start** (rust+go, python+go, rust+python, and the all-three case all refuse). NEITHER → the existing "cannot detect" error. The startup log (`[lsp-mcp] root=… lang=…`, already in `run()`) prints `lang=go` once detected.
2. **`go_config()` → `LangServerConfig`** mirroring `pyright_config` but SIMPLER (see §2).
3. **`resolve_lsp_server` search EXTENSION** so a bare `gopls` resolves on hosts where the Go toolchain bin is not on `$PATH` (see §3). `run()` gains a `Lang::Go => go_config(...)` arm.

The `is_project_root` field (C1) is reused: `run()` validates an explicit `--lang go` against the repo via `(cfg.is_project_root)(repo)` before starting, exactly as it does for rust/python today.

## §2. Go `LangServerConfig` — gopls (host)

`go_config(repo: &Path) -> anyhow::Result<LangServerConfig>` (fallible only because `resolve_lsp_server`'s "not found" warning path mirrors pyright; the function itself does no interpreter discovery). Field-by-field, contrasted with `pyright_config`:

- **`name`** = `"gopls"`.
- **`program_argv`** = `vec![resolve_lsp_server("gopls"), "serve".to_string()]` — **the spike confirms the exact stdio invocation** (`gopls` bare vs `gopls serve`; see §4 Gate 1). gopls serves LSP over stdin/stdout; the argv is whichever bare/`serve` form the spike proves reads LSP on stdio. `resolve_lsp_server` returns an absolute path (or the bare name as the documented degrade).
- **`spawn_env`** = `vec![]` by default. gopls inherits the process environment (`GOPATH`/`GOROOT`/`GOFLAGS` from the host shell). The spike (Gate 2) confirms gopls does NOT need these injected for the host case; **only if** the spike shows a host where gopls fails to find the toolchain do we add the relevant `GOPATH`/`GOROOT` pair to `spawn_env` — and even then the values come from `go env`, not hard-coded. Default is empty (contrast: Rust injects `CARGO_TARGET_DIR`; Python injects nothing).
- **`is_project_root`** = `Box::new(|root: &Path| root.join("go.mod").exists())` — a single-file check (contrast: Python's predicate ORs four marker families). Reused by `run()` for explicit `--lang go` validation.
- **`initialize_params`** = the standard params rooted at `root_uri`, advertising hierarchical `documentSymbol` support (so the C1 recursive `collect_doc_symbols` surfaces methods on a struct/interface) and `workspace.symbol`. **`window/workDoneProgress`** advertised-or-not is decided by the spike (Gate 3): if gopls only emits progress when the client advertises `window.workDoneProgress`, the cleanest path is to REUSE the no-progress settle (do not advertise it, settle like basedpyright). See readiness below.
- **`post_init_config`** = **`None`** — the load-bearing simplification. gopls auto-configures from `go.mod`; there is no per-repo SDK/venv selection, so there is NO `workspace/didChangeConfiguration`, NO `pythonPath`-equivalent, and NO interpreter discovery. (Contrast: Python sends the wrapped `{settings:{python:{pythonPath}}}` envelope and runs `resolve_python_path`.) Because `post_init_config` is `None`, the reader thread's `python_path_from_cfg` returns `None` for Go, so a Go config never answers a `workspace/configuration` server-request with a python path — it falls to the existing `-32601` method-not-found reply, which the spike confirms is acceptable (Gate 4).
- **`new_readiness`** = a fresh readiness machine per spawn — `Readiness::Gopls(GoplsReady::default())` OR a reuse of the Pyright settle, decided by the spike (see below).

**Readiness — REUSE the settle if at all possible.** The spike (Gate 3) determines gopls's load-time signal:
- If gopls emits **no** progress for a typical analysis (the basedpyright shape), add a **`Readiness::Gopls(GoplsReady)`** variant that mirrors `PyrightReady` exactly: `began`/`active`/`settled_at`, the same `settled_no_progress(settle)` predicate, and (because there is no `post_init_config` to stamp `settled_at` from) **`settled_at` is stamped at the end of `handshake` for the Gopls variant whenever `post_init_config` is `None`** — i.e. settle is timed from `initialized` rather than from settings-applied. The C1 `handshake` already stamps `settled_at` only inside the `if let Some(post_init_config)` block; this increment widens that to also stamp the Gopls variant after `initialized` (a one-line addition, kept behind the variant match so the Rust path is untouched).
- If gopls emits `$/progress` (`workDoneProgress` begin/end) like rust-analyzer, `GoplsReady` parses those (`$/progress` begin/end → `began`/`active`, ready on begun-and-ended) PLUS the same no-progress settle backstop.
- **Preference:** a `Gopls` variant that carries the SAME `settled_no_progress` machinery as `PyrightReady` (the settle is the load-bearing branch; begin/end parsing is harmless belt-and-suspenders), wired into `wait_ready`'s existing `pyright_settled`-style branch (generalize that helper to `settled(&Readiness)` covering both Pyright and Gopls, or add a parallel `gopls_settled`). The settle window reuses the existing `PYRIGHT_SETTLE` constant (rename to `LSP_SETTLE` if it now serves two languages — a pure rename, no behavior change).

**Third-party-by-name resolution caveat (same as Python).** gopls `workspace/symbol` indexes the workspace (and its build-list), so a third-party symbol may be reachable **only positionally** (go-to-definition AT the usage site jumps into the module cache / `$GOPATH/pkg/mod`), not by the bridge's name-only `definition("SomeThirdPartyType")`. The spike (Gate 4) confirms whether `workspace/symbol` returns third-party names or only workspace names; either way the name-only MCP API is kept (matching the documented Python degradation), and the fixture's third-party assertions use the positional path where needed (mirroring `python_nav.rs`'s `basemodel_def_targets` helper).

## §3. `resolve_lsp_server` — find gopls

C1's `resolve_lsp_server(name)` searches, in order: a name already containing a path separator (returned as-is) → each `$PATH` dir → `$HOME/.local/bin` → `$HOME/.cargo/bin`, returning the first `is_usable_interpreter`-passing absolute path, else the bare name (with a logged "not found" hint). **gopls is not a `cargo`/`uv` tool**: it lives at the active Go toolchain's bin (`$(go env GOROOT)/bin/gopls` or `$(dirname $(which go))/gopls`) or at **`$GOPATH/bin/gopls`** (`go install golang.org/x/tools/gopls@latest`).

**Host recon (2026-06-15):** on this host gopls is at `$GOROOT/bin/gopls` (mise-managed; `GOBIN` points there) AND it is on `$PATH` — so the *current* `resolve_lsp_server` already finds it via the PATH search. But that is host-specific (mise puts the toolchain bin on PATH); a non-mise host, or a container, may have gopls in `$GOPATH/bin` with neither on PATH. So the extension is for robustness, not this host:

- Extend `resolve_lsp_server`'s fallback search (after the existing PATH / `~/.local/bin` / `~/.cargo/bin` probes) to ALSO probe Go toolchain locations: **`$GOPATH/bin/<name>`** and the **active Go bin** (`$(go env GOROOT)/bin/<name>`, or the directory of `go` on PATH). Keep the function GENERIC and TOTAL (the spike confirms which of these actually holds gopls on a representative host): the cleanest approach is to add a couple of Go-aware candidate dirs derived by shelling `go env GOPATH GOROOT` (best-effort — if `go` is absent the probe is skipped, no panic), preserving the "return the bare name on no match" degrade. The `resolve_lsp_server_with_env(name, path_var, home_dir)` hermetic-test seam from C1 is preserved/extended (add an optional `go_env` parameter so the Go candidates are unit-testable without a real `go` binary).

## §4. Spike FIRST (Go-scoped, light — mirrors C1 Task 1)

A throwaway/measurement task settling the Go-specific unknowns into a verdict file BEFORE any config lands. Run on the host with real gopls (`gopls v0.22.0`) + a throwaway Go module (a `go.mod` with one third-party dep, `go mod download`ed). Gates:

1. **Exact stdio invocation.** Confirm which of `gopls` (bare) vs `gopls serve` reads LSP on stdin/stdout. Drive `initialize`/`initialized`/`textDocument/definition` by hand over stdio and capture which argv produces a working LSP session. Record the proven `program_argv`.
2. **Env needs.** Confirm gopls resolves the standard library + the module cache with the INHERITED environment (no `spawn_env` injection). Note `GOPATH`/`GOROOT`/`GOFLAGS` values; record whether anything must be injected (expected: nothing — `spawn_env = []`).
3. **Readiness signal.** Observe whether gopls emits `$/progress` / `window/workDoneProgress` during workspace load, or nothing. Decide REUSE-settle (no progress, basedpyright-shape) vs parse-progress (rust-analyzer-shape). Confirm a `workspace/symbol` issued shortly after `initialized` returns without paying a full timeout (the FU3 guard, per-language).
4. **`workspace/configuration` + third-party-by-name.** Confirm (a) whether gopls pulls `workspace/configuration` (if so, the existing reader-thread `-32601` reply for a non-python section is benign — the live session still works); (b) whether `workspace/symbol` returns third-party module symbols by name or only workspace symbols → confirm the positional caveat (§2).
5. **`--lang auto` Go detection.** Confirm `go.mod` at root → go; and the new ambiguous refusals (a dir with `go.mod` + `Cargo.toml`, or `go.mod` + `pyproject [project]`) → ambiguous (pure host-dir reasoning, no binary needed).
6. **gopls binary resolution.** Confirm where gopls resolves on a representative host (`$GOROOT/bin`, `$(dirname go)`, and/or `$GOPATH/bin`) → validates the §3 search extension. (Host recon already shows `$GOROOT/bin` + on-PATH here; the spike confirms the `$GOPATH/bin` case for the extension's value.)

Spike verdict → `docs/superpowers/spikes/2026-06-15-go-gopls-proof.md`.

## §5. Host wiring + DoD

- **No config change for host wiring.** The host reviewers' `[[agents.mcp]] lsp` entries in `examples/a2a-bridge.containerized.toml` AND `examples/a2a-bridge.containerized.podman.toml` are ALREADY `--lang auto` (C1) → a Go repo is served the moment `detect_lang` + `go_config` exist. The adjacent comments say "rust-analyzer (rust) or basedpyright (python)"; update them to add "or gopls (go)". The in-container `impl` lsp entry stays `--lang rust` (Go-in-container is C2).
- **Host gopls presence/version check in the DoD** (`gopls version` → `golang.org/x/tools/gopls v0.22.0`), mirroring the Rust path's `rust-analyzer --version` and the Python path's `basedpyright-langserver --version`.
- **The MCP path is already fixed (C1).** The newline-framing bug, the basedpyright-PATH fix, the protocolVersion handshake — all merged in C1. The Go increment inherits a WORKING MCP path, so the live DoD should work end-to-end on the first try (no new MCP plumbing to debug).
- **DoD = a host review of a Go repo** (a throwaway module, or a real Go repo cloned for the gate) through the now-working MCP path, with semantic nav working: the startup log shows `lang=go` + the correct root, and at least one lsp tool call (`definition`/`hover`/`references`) returns semantic results. Covers the host reviewers (claude + codex) — and because C1 already settled the codex `{cwd}` threading, no `{cwd}` gate is needed here.

## §6. Tests / regression

- **`tests/fixtures/gosample/`** — a small Go module mirroring the `pysample` shape: `go.mod`, a package with a func `Add`, an **interface + an impl** (so `implementations` / `document_symbols`-on-methods have real targets), and a **third-party import** (so `definition`/`hover`/`references` reach a real external target via the positional path). The module's deps need fetching (`go mod download`) for third-party resolution — a documented one-time setup step, and the Go build cache is gitignored (a `.gitignore` in the fixture dir, mirroring `pysample/.gitignore` ignoring `.venv/`).
- **`tests/go_nav.rs`** — live tests covering all 7 tools (`workspace_symbol`, `document_symbols` incl. a method-on-a-type via children recursion, `definition`, `references`, `hover` non-empty, `implementations` of the interface, `call_hierarchy`), GUARDED on `gopls version` succeeding + the fixture present, with an **opt-in required gate `LSP_MCP_REQUIRE_GO=1`** that turns a missing prerequisite into a hard failure (mirroring `python_nav.rs`'s `PyGate`/`LSP_MCP_REQUIRE_PYTHON`), so a CI job enforces real Go coverage instead of silently skipping.
- **`tests/lang_detect.rs`** — add Go detection unit tests: `go.mod → Lang::Go`; `go.mod + Cargo.toml → ambiguous`; `go.mod + pyproject[project] → ambiguous`; `go.mod present alongside an excluded dir's stray markers stays go`. Plus the existing rust/python cases stay green.
- **`lang.rs` pure unit tests** — `go_config()` builds the proven `program_argv` and `post_init_config.is_none()`; the readiness variant's settle behaves like the Pyright settle (a pure `settled_no_progress` test if a `GoplsReady` variant is added); `resolve_lsp_server`'s Go-aware candidates resolve a fake `gopls` from a fake `$GOPATH/bin` via the hermetic `_with_env` seam.
- **Regression discipline:** the existing Rust + Python suites (`cargo test -p lsp-mcp`) + `cargo clippy -p lsp-mcp -- -D warnings` + `cargo fmt --check` stay green at every task. The C1 characterization harness is re-run unchanged — adding a config does not perturb the Rust readiness/respawn/byte-for-byte locks.

## §7. Risks

- **gopls invocation / readiness shape is the only real unknown** — settled by the spike (Gates 1–3) before any config lands. The fallback (no-progress settle, `spawn_env = []`, `gopls serve`) is the conservative default if the spike is inconclusive.
- **gopls binary resolution off-PATH** (containers / non-mise hosts) — mitigated by the §3 search extension (hermetically unit-tested); on THIS host gopls is already on PATH so the live gate is unaffected.
- **Third-party-by-name** may be workspace-only (same caveat as Python) — covered by the fixture's positional third-party assertion + a kept name-only API.
- **No new client risk:** the high-risk C1 items (the reader-thread/`Readiness` restructure, the byte-for-byte Rust lock) are DONE; Go only adds a config, so the blast radius is `lang.rs` + one `run()` arm + tests + the two example comments.

## §8. Execution order

Spike (Go path) → `detect_lang` + `Lang::Go` (+ ambiguous-refusal + unit tests) → `resolve_lsp_server` Go-aware extension (hermetic tests) → `go_config` + the readiness variant + `run()` wiring (pure tests) → `gosample` fixture + `go_nav.rs` 7-tool live tests + the `LSP_MCP_REQUIRE_GO` gate → host wiring (comment-only) + the live DoD (host review of a Go repo) → full-crate test/clippy/fmt as a regression gate each task. ~5–6 tasks; no characterization harness, no client refactor. Dogfooded where clean; the live gate runs against a throwaway module.

---

## Deferred (its own design pass when triggered)

- **In-container Go implementor** — overlaps **Slice C2** (the per-language config seam, a Go verify story `go build`/`go test`, a Go toolchain image decoupled from `[verify].image`, the warm-`go mod download` analogue of `warm_lsp_deps_step`). The in-container `impl` agent stays `--lang rust` until C2.
- **TypeScript/JS** — a later increment onto the same registry, following this exact pattern (one `LangServerConfig` + one `Lang` arm + a resolver tweak if the LS lives somewhere new).
- **Go `workspace/symbol` third-party indexing** beyond positional — only if a reviewer proves the positional path is insufficient in practice (kept name-only otherwise, matching Python).
