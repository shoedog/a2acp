# LSP-over-MCP Slice C1 — Host-reviewer Python nav + the multi-language registry refactor — Design

**Status:** approved direction (brainstormed 2026-06-15); revised after **codex xhigh** + **opus xhigh** deep reviews. Slice C was **SPLIT** (both reviews: the original "host + container" scope wasn't plannable as one unit — the container half has structural holes). This doc is **Slice C1**; the in-container Python implementor is **Slice C2** (deferred, requirements captured at the end). Pending final user review.
**Predecessors:** Slice A (host reviewers, Rust), Slice B (in-container `:rw` implementor, Rust), + FU3 (readiness/quiescent).

## Goal

Let the bridge's **host-side reviewers** navigate **Python** repos with the same 7-tool type-resolved MCP surface as Rust, by generalizing `lsp-mcp` from a rust-analyzer-only shim into a `--lang`→language-server registry (Python = **basedpyright**). This **de-risks the multi-language abstraction with one new language** (the original arc intent) while staying low-risk to the working Rust + FU3 path. The heavy container machinery (warm venv, Python verify, the per-language config seam) is explicitly **out of C1** — see Slice C2.

## Non-goals (C1)

- **The in-container Python implementor** → **Slice C2** (needs the per-language config seam, a Python verify story, and the uv-in-container gates — see the deferred section).
- **uv venv warming / read-only venv mounts / a Python toolchain image** → C2 (C1 host reviewers use each repo's *existing* venv).
- **TypeScript/JS, Go** — later slices.
- **Diagnostics / type-check / editor-grade tools** — C1 stays the 7 read-only nav tools.
- **Hybrid/polyglot repos & subdir-rooted projects** — `auto` refuses ambiguous roots; `--project-root` is **C2** (the container always passes explicit `--lang`).

## §1. The `--lang`→server registry refactor (the core of C1)

**`--lang auto` (new default) detects the language from the repo's root markers, single-unambiguous-root ONLY.** `Cargo.toml`→rust; `pyproject.toml`/`setup.py`/`setup.cfg`/`requirements.txt`→python — **and** (codex re-review) python requires at least one `.py` file or a real dependency/project section, so a tooling-only `pyproject.toml` (e.g. a Rust repo's `[tool.black]`) is not a false python positive. BOTH-language root markers (pyo3/maturin, polyglot monorepo) → **ambiguous → refuse to start for that repo** (logged), requiring an explicit `--lang`. So the host reviewers' single `--lang auto` MCP entry serves any *unambiguous* rust-or-python repo; hybrid repos need an explicit entry (documented, not claimed away).

**Split the shared LSP client from the per-language config.** The current `LspSession` (`crates/lsp-mcp/src/lsp/mod.rs`) hardcodes rust-analyzer pervasively — not in one spot: `spawn_ra` (the binary name + `CARGO_TARGET_DIR`), `handshake` (the `serverStatusNotification` capability), **the `ReadyState` struct + `parse_quiescent` + the reader thread's `$/progress`/`experimental/serverStatus` parsing**, and `is_ready`. The refactor produces:
- a language-agnostic `LspClient` (process spawn, the stdin/reader thread, request/response correlation, name→position resolution, the 7 tools, idle-evict), parameterized by
- a `LangServerConfig` { `program_argv`, `is_project_root`, `initialize_params` (capabilities — Rust advertises `serverStatus`; Python advertises NO `window/workDoneProgress` so `pyright/*Progress` fires), `post_init_config` (Python: the `didChangeConfiguration` settings, §2), `spawn_env` (rust: `CARGO_TARGET_DIR`), `readiness` }.
- **`Readiness` absorbs the reader-thread NOTIFICATION PARSING**, not just the predicate (opus H1): today the readiness state machine is woven through the reader loop + `ReadyState` + `is_ready` (the FU3/idle-race-fixed concurrency code). This is the **highest-risk part** of the refactor.

**Regression discipline (opus H1):** write a **Rust characterization test FIRST** (pin the exact `initialize` JSON + the readiness-settle behavior) BEFORE the refactor, so the rust path stays byte-for-byte. The 7 nav tools are LSP-standard and reused, **but response shaping is NOT assumed identical** (opus H2) — Python fixture tests cover **all 7 tools** (incl. `implementation` + call hierarchy), and these shared spots get explicit per-server tolerance:
- `resolve_pos` "first `workspace/symbol` hit" — basedpyright's ranking/shape differ; the fixture test pins Python behavior.
- `document_symbols` currently parses a **flat** `DocumentSymbol[]`; basedpyright returns **hierarchical** (classes→methods `children`) — generalize the extraction (or document the flat-drop) and test it.
- the **`file://` URI build/decode round-trip** (`file_uri` + `shape::file_path_from_uri`) — fix the naive `format!("file://{}", display)` to proper percent-encoding so site-packages/venv paths round-trip symmetrically.

## §2. Python `LangServerConfig` — basedpyright (host)

- **Server:** `basedpyright-langserver --stdio` (pip-installed; bundles its Node runtime).
- **Root markers:** `pyproject.toml`/`setup.py`/`setup.cfg`/`requirements.txt` (+ the `.py`/dep-section guard above).
- **Dependency resolution = the repo's EXISTING venv** (C1 host case — the developer already has one; no warming). basedpyright is pointed at it via **`python.pythonPath`** (docs *recommend* `pythonPath`, *discourage* `venvPath`) = the repo's `.venv/bin/python` (auto-detected) or a configured path. **Delivery channel (the prior BLOCKER, now codex-proven):** the shim does NOT advertise `workspace.configuration` and sends one **`workspace/didChangeConfiguration`** with `{ "python": { "pythonPath": "…/.venv/bin/python" } }` immediately after `initialized`. (Codex's re-review *locally proved* basedpyright resolves a third-party definition from a venv via this channel.) Repo `pyrightconfig.json`/`pyproject [tool.*]` may override — the spike confirms behavior with/without.
- **Readiness:** `Readiness::Pyright` parses `pyright/{begin,end}Progress` (basedpyright's signal when the client doesn't advertise `window.workDoneProgress` — upstream-confirmed). **No-progress case** (codex re-review): if no progress arrives within a short settle after `initialized`+settings-applied, treat as ready (don't pay the full timeout) — a test asserts the first Python `workspace_symbol` doesn't wait the full bound (the FU3 guard, per-language).

## §3. Host wiring (Slice A analogue)

- The host reviewers' single `[[agents.mcp]] lsp` entry → `args = ["--repo", "{cwd}", "--lang", "auto", ...]` (serves any unambiguous rust-or-python repo). Hybrid repos: explicit `--lang` entry (documented).
- **basedpyright on the host:** `pip install basedpyright`; **the DoD includes a host presence/version check** (`basedpyright-langserver --version`), mirroring the Rust path's `rust-analyzer --version` (opus M4).
- **Inherited precondition (opus M4):** host-reviewer nav depends on the bridge's host-reviewer `{cwd}` resolving to the repo under review — the **deferred Slice A `{cwd}` asymmetry** (codex host-reviewer `{cwd}`=base). C1 inherits it: if unresolved, Python nav would point basedpyright at the wrong repo/venv. **State it as a precondition**; if the live gate hits it, fix the asymmetry (already a known follow-up) as part of C1.
- Read-only by prompt-contract (same as the Rust reviewers).

## §4. Spike FIRST (C1-scoped, light)

The heavy uv/container/symlink gates are C2. C1's spike validates the host path:
1. **Config-channel resolution from an EXISTING venv** — `python.pythonPath` via `didChangeConfiguration` resolves a third-party `definition`/`hover` (mostly proven by codex; confirm against a repo that ALSO has a `pyrightconfig.json`/`pyproject [tool.basedpyright]` to check override behavior).
2. **Readiness** — confirm `pyright/{begin,end}Progress` fires + the no-progress settle doesn't pay the full timeout.
3. **`--lang auto` detection** — rust/python/ambiguous/`.py`-guard cases.

(Edit-reflection, read-only-venv resolution, the uv-symlink interpreter, and uv warm timing are **C2 gates** — host reviewers don't edit the repo and use the existing venv.)

Spike verdict → `docs/superpowers/spikes/2026-06-15-slice-c1-basedpyright-proof.md`.

## Tests / regression

- **Rust characterization test BEFORE the refactor** (pin `initialize` + readiness).
- **Python fixture tests for all 7 tools** against a small Python fixture with a third-party import (incl. `implementation`, call hierarchy, hierarchical `document_symbols`).
- `--lang auto` detection unit tests (incl. the `.py`/dep-section guard + ambiguous-refusal).
- The existing Rust lsp-mcp unit + integration tests stay green (FU3 `is_ready`/`parse_quiescent` move behind `Readiness::RustRa` unchanged).

## Risks (C1)

- **The reader-thread/`ReadyState` restructure** is the high-risk item (it carries the FU3 + idle-race fixes) — mitigated by the characterization test + keeping Rust byte-for-byte.
- **The host `{cwd}` asymmetry** (§3) is an inherited precondition.
- **`document_symbols` hierarchy / `resolve_pos` first-hit** shared-code tolerance — covered by Python fixture tests.

## Execution

Spike (host path) → Rust characterization test → registry refactor (Rust byte-for-byte) → Python `LangServerConfig` → host wiring + DoD (host review of one of the 3 Python repos with semantic nav) → full-branch review vs main before merge. Dogfooded where clean; host-side for the refactor.

---

## Deferred to Slice C2 — in-container Python implementor (its own design pass)

The two deep reviews showed the container half needs design the original spec didn't have. C2 gets its own brainstorm→spec→spike→reviews, and MUST design:

- **The per-language config seam (opus B1):** there is **no `language` field** on any agent/sandbox today; `warm_lsp_deps_step` hardcodes `cargo fetch`; the warm/toolchain image is the single global `[verify].image`. C2 must introduce where `language` lives, a warm/toolchain image field **decoupled from `[verify].image`**, and the `warm_lsp_deps_step` dispatch signature.
- **The Python verify story (opus B2):** the implement loop runs `[verify]` (Rust `cargo …`) **unconditionally** — a Python run would fail every gate and oscillate to abort. C2 must design per-language verify (`ruff`/`pytest` image + commands) selected by the agent's language.
- **uv-in-container warm gates (codex + opus H3/H4):** mount `/work:ro`; `UV_PROJECT_ENVIRONMENT=/venv/venv uv sync --frozen --no-install-project` (uv.lock) / `uv pip sync` (requirements); **`UV_LINK_MODE=copy` + a copied (not symlinked) interpreter** so `pythonPath` isn't a dangling symlink under the `:ro` mount; a **writable scratch** if basedpyright needs one; thread **`no_proxy`** into the warm path (`WarmEgress` drops it today); analyze the **sdist `setup.py` arbitrary-code-execution** posture during warm (NOT "identical to cargo fetch" — download vs build-and-run). A new Python warm compose function (not a reuse of `compose_warm_fetch`).
- **Respawn re-sends config (codex):** an evicted basedpyright must get `didChangeConfiguration` re-sent on respawn (make `post_init_config` part of every handshake) or it returns without its venv — with a post-eviction resolution test.
- **`--project-root`/subdir threading** through `{cwd}` substitution (opus M1), monorepo degrade (opus M2), Python-version selection vs `requires-python` (opus M3), per-language idle-evict timeout mechanism (opus M6), and **edit-reflection** as a gate with a `didChangeWatchedFiles` fallback (both reviews; reviewers don't edit, but the implementor does).
