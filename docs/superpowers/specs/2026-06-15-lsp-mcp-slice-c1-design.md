# LSP-over-MCP Slice C1 — Host-reviewer Python nav + the multi-language registry refactor — Design

**Status:** approved direction (brainstormed 2026-06-15); revised after **two rounds** of codex xhigh + opus xhigh deep reviews. Slice C was **SPLIT** (the original "host + container" scope had container-side structural holes); this is **Slice C1** (host nav + registry refactor), with **Slice C2** (in-container implementor) deferred. The C1-round reviews' findings are folded below: respawn-config-resend pulled INTO C1, an explicit interpreter/venv discovery contract, a broadened fake-LSP characterization harness, concrete `--lang auto` predicates, required hierarchical `document_symbols`, and a **live `{cwd}` spike gate** (the codex-reviewer `{cwd}` asymmetry was live-observed broken and is unreadable from source → the spike decides whether FU1 must be fixed in C1). Pending final user review.
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

**`--lang auto` (new default) detects the language from the repo's root markers, single-unambiguous-root ONLY** — with CONCRETE, testable predicates (both reviews: "a real dependency/project section" isn't implementable as prose):
- **rust** iff `Cargo.toml` at the root.
- **python** iff any of: `setup.py`, `setup.cfg`, a `requirements*.txt`, OR a `pyproject.toml` that has a real project/dep section (`[project]`, `[tool.poetry]`, `[tool.pdm]`, `[build-system]`, or a `[project.dependencies]`/`dynamic`) — NOT merely a tooling table (`[tool.black]`/`[tool.ruff]`), OR at least one `*.py` file found by a shallow scan that **excludes** `.venv`/`venv`/`.git`/`target`/`node_modules`/hidden + common build/vendor dirs.
- BOTH rust AND python markers (pyo3/maturin, polyglot monorepo) → **ambiguous → refuse to start** (logged); requires an explicit `--lang`.
- **At startup the shim LOGS the resolved root path + the detected language** (observability: a misrouted `{cwd}` that lands on a Rust repo + `auto`→rust becomes visible in the call log, not a silent wrong-language answer — opus).
So the host reviewers' single `--lang auto` entry serves any *unambiguous* rust-or-python repo; hybrid repos need an explicit `--lang` entry (documented).

**Split the shared LSP client from the per-language config.** The current `LspSession` (`crates/lsp-mcp/src/lsp/mod.rs`) hardcodes rust-analyzer pervasively — not in one spot: `spawn_ra` (the binary name + `CARGO_TARGET_DIR`), `handshake` (the `serverStatusNotification` capability), **the `ReadyState` struct + `parse_quiescent` + the reader thread's `$/progress`/`experimental/serverStatus` parsing**, and `is_ready`. The refactor produces:
- a language-agnostic `LspClient` (process spawn, the stdin/reader thread, request/response correlation, name→position resolution, the 7 tools, idle-evict), parameterized by
- a `LangServerConfig` { `program_argv`, `is_project_root`, `initialize_params` (capabilities — Rust advertises `serverStatus`; Python advertises NO `window/workDoneProgress` so `pyright/*Progress` fires), `post_init_config` (Python: the `didChangeConfiguration` settings, §2), `spawn_env` (rust: `CARGO_TARGET_DIR`), `readiness` }.
- **`Readiness` absorbs the reader-thread NOTIFICATION PARSING**, not just the predicate (opus H1): today the readiness state machine is woven through the reader loop + `ReadyState` + `is_ready` (the FU3/idle-race-fixed concurrency code). This is the **highest-risk part** of the refactor. **The reader loop's request-correlation (`id`-routing) is language-agnostic and STAYS in `LspClient`** — only the notification-parsing (`$/progress`, `serverStatus`, and Python's `pyright/*Progress`) moves behind `Readiness` (both reviews — a naive "move the reader thread into Readiness" would wrongly drag id-routing along).

**Regression discipline (both reviews):** write a **fake-LSP characterization HARNESS FIRST**, BEFORE the refactor. It pins more than the `initialize` JSON (which is value-comparable): it drives an **ordered synthetic notification stream** (`$/progress` begin/end, `serverStatus{quiescent}`, out-of-order, warm-no-progress) through the Rust readiness machine and asserts the same **ready/not-ready transition table** + the request-touch + respawn ordering (**failure leaves `evicted=true`**) + that Rust sends **no** post-init config — identical pre/post refactor. The existing pure tests (`is_ready_via_quiescent_or_progress`, `parse_quiescent_…`) are the seed. The 7 nav tools are LSP-standard and reused, **but response shaping is NOT assumed identical** — Python fixture tests cover **all 7 tools** (incl. `implementation` + call hierarchy), and these shared spots get explicit per-server handling:
- `resolve_pos` "first `workspace/symbol` hit" — basedpyright's ranking/shape differ; a **duplicate-name fixture** pins Python behavior + documents the degradation (keep the name-only API; don't expand the MCP schema unless proven unusable).
- `document_symbols` currently parses a **flat** top-level array; basedpyright returns **hierarchical** `DocumentSymbol` with `children` — **recursive `children` extraction is REQUIRED, not optional** (both reviews: flat-dropping Python methods is a real functional regression vs the "same 7 tools" goal). Confirm which shape rust-analyzer sends (it reads `range`, suggesting `DocumentSymbol[]` already) so the change is additive; test class→method extraction.
- the **`file://` URI build/decode round-trip** (`file_uri` + `shape::file_path_from_uri`) — the builder is naive `format!("file://{}", display)` (no percent-encoding) while the decoder already percent-*decodes* → **asymmetric today**; fix the builder + test spaces/`%`/`#`/non-ASCII paths.
- **Two MCP tool *descriptions* are Rust-flavored** (`references` "resolves generics/traits", `implementations` "trait impls") — advertised to the agent in `tools/list`; genericize them (or make them language-aware) so a Python reviewer isn't misled. **`hover`** must handle basedpyright's `MarkupContent` (covered) AND not silently return `None` on a `MarkedString[]` array form — the hover fixture asserts non-empty content.

## §2. Python `LangServerConfig` — basedpyright (host)

- **Server:** `basedpyright-langserver --stdio` (pip-installed; bundles its Node runtime).
- **Root markers:** `pyproject.toml`/`setup.py`/`setup.cfg`/`requirements.txt` (+ the `.py`/dep-section guard above).
- **Dependency resolution = the repo's EXISTING venv** (C1 host case — the developer already has one; no warming). basedpyright is pointed at it via **`python.pythonPath`** (docs *recommend* `pythonPath`, *discourage* `venvPath`). **Interpreter discovery is an EXPLICIT, tested contract** (both reviews — "auto-detected" hid an algorithm): ordered precedence (1) `--python-path <p>` CLI flag / `LSP_MCP_PYTHON_PATH` env, (2) `$VIRTUAL_ENV/bin/python`, (3) `<repo>/.venv/bin/python`, (4) `<repo>/venv/bin/python`, (5) **none → fall back to `python3` on PATH with a LOGGED WARNING** that third-party resolution will be incomplete (NOT a silent empty result). Poetry's out-of-tree venv is **not** auto-discovered in C1 — it needs the explicit `--python-path` override (documented scope cut). The chosen path is validated (exists + executable) before use.
- **Delivery channel (the prior BLOCKER, now codex-proven):** the shim does NOT advertise `workspace.configuration` and sends one **`workspace/didChangeConfiguration`** with `{ "python": { "pythonPath": "<resolved>" } }` immediately after `initialized`. (Codex *locally proved* basedpyright resolves a third-party definition from a venv via this channel.) Repo `pyrightconfig.json`/`pyproject [tool.basedpyright]` may override — the spike confirms behavior with/without.
- **Respawn re-sends the config (pulled into C1 — codex BLOCKER):** C1 host sessions use the shared idle-evict/respawn path, so `post_init_config` (the `didChangeConfiguration`) must be part of **every** spawn AND respawn handshake — an evicted basedpyright that respawns without it loses its venv → third-party resolution silently breaks mid-session. A **post-eviction Python resolution test** guards this (evict → next call respawns + still resolves a third-party def). (This requirement was wrongly in the C2-deferred list; reviewers corrected it — C1 owns it because C1 uses respawn.)
- **Readiness:** `Readiness::Pyright` parses `pyright/{begin,end}Progress` (basedpyright's signal when the client doesn't advertise `window.workDoneProgress` — upstream-confirmed). **No-progress case:** if no progress arrives within a short settle after `initialized`+settings-applied, treat as ready (don't pay the full timeout) — a test asserts the first Python `workspace_symbol` doesn't wait the full bound (the FU3 guard, per-language).

## §3. Host wiring (Slice A analogue)

- The host reviewers' single `[[agents.mcp]] lsp` entry → `args = ["--repo", "{cwd}", "--lang", "auto", ...]` (serves any unambiguous rust-or-python repo). Hybrid repos: explicit `--lang` entry (documented).
- **basedpyright on the host:** `pip install basedpyright`; **the DoD includes a host presence/version check** (`basedpyright-langserver --version`), mirroring the Rust path's `rust-analyzer --version` (opus M4).
- **The `{cwd}` asymmetry is a §4.4 LIVE SPIKE GATE, not a soft precondition** (opus BLOCKER): for the **codex** reviewer, `{cwd}` is baked at spawn from static config and was live-observed resolving to the bridge launch dir (→ `auto`→rust, a *silent wrong-language* answer). The reviewers disagreed reading the source (the memory says the disconnect isn't readable), so the spike settles it live; if broken, FU1 is fixed in C1 or the Python gate ships claude-only (whose Acp `{cwd}` is verified-correct) — see §4.4. The startup root+language log (§1) makes any misroute loud.
- Read-only by prompt-contract (same as the Rust reviewers).

## §4. Spike FIRST (C1-scoped, light)

The heavy uv/container/symlink gates are C2. C1's spike validates the host path:
1. **Config-channel resolution from an EXISTING venv** — `python.pythonPath` via `didChangeConfiguration` resolves a third-party `definition`/`hover` (mostly proven by codex; confirm against a repo that ALSO has a `pyrightconfig.json`/`pyproject [tool.basedpyright]` to check override behavior). Include the **no-venv repo** case → confirm the `python3` fallback degrades with a logged warning (not a silent empty result).
2. **Readiness** — confirm `pyright/{begin,end}Progress` fires + the no-progress settle doesn't pay the full timeout.
3. **`--lang auto` detection** — rust/python/ambiguous/tooling-only-`pyproject`/`.py`-guard cases.
4. **Live `{cwd}` GATE (the codex asymmetry — spike decides, per the chosen approach):** run the bridge's host **codex** reviewer against a Python repo via the per-request session-cwd and confirm codex's lsp-mcp **`{cwd}` resolves to that repo** (the startup log shows the target root + detected language = python), NOT the bridge launch dir (which would `auto`→rust silently). The memory records this was live-observed broken (codex `=base`) and it's unreadable from source — so this is a live go/no-go. **If broken:** fix FU1 (thread the per-request session_cwd into codex's `render_codex_mcp_args` at the SpawnFn boundary) as in-scope C1 work, OR ship the Python live-gate claude-only and fast-follow FU1 — decided by what the spike finds.

(Edit-reflection, read-only-venv resolution, the uv-symlink interpreter, and uv warm timing are **C2 gates** — host reviewers don't edit the repo and use the existing venv.)

Spike verdict → `docs/superpowers/spikes/2026-06-15-slice-c1-basedpyright-proof.md`.

## Tests / regression

- **Fake-LSP characterization HARNESS BEFORE the refactor** — `initialize` bytes + the readiness transition table (ordered/out-of-order `$/progress` + `serverStatus`, warm-no-progress) + request-touch + respawn ordering (failure→`evicted=true`) + Rust sends no post-init config. Identical pre/post refactor.
- **Python fixture tests for all 7 tools** against a small Python fixture with a third-party import (incl. `implementation`, call hierarchy, **recursive `document_symbols` class→method**, duplicate-name `resolve_pos`, non-empty `hover`).
- **Interpreter discovery tests** — the precedence order; missing/non-executable path; no-venv→`python3`+warning; `--python-path`/`LSP_MCP_PYTHON_PATH` override.
- **Post-eviction Python resolution test** — evict → respawn re-sends `didChangeConfiguration` → still resolves a third-party def.
- `--lang auto` detection unit tests (rust / python / tooling-only-`pyproject` / `.py`-guard with excluded dirs / ambiguous-refusal) + the startup root+language log.
- **URI round-trip tests** (spaces/`%`/`#`/non-ASCII).
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
- **`--project-root`/subdir threading** through `{cwd}` substitution (opus M1), monorepo degrade (opus M2), Python-version selection vs `requires-python` (opus M3), per-language idle-evict timeout mechanism (opus M6), and **edit-reflection** as a gate with a `didChangeWatchedFiles` fallback (both reviews; reviewers don't edit, but the implementor does).

*(Respawn re-sending `didChangeConfiguration` was moved OUT of this list INTO C1 — C1 host sessions also use respawn.)*
