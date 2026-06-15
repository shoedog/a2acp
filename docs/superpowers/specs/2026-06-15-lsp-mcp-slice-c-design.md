# LSP-over-MCP Slice C — Multi-language nav (Python / basedpyright) — Design

**Status:** approved direction (brainstormed 2026-06-15); **revised after a codex gpt-5.5 xhigh spec review** (1 BLOCKER + 4 HIGH + 3 MEDIUM folded — basedpyright config channel, exact non-mutating uv warm, `--lang auto` ambiguity, pyright readiness, edit-reflection fallback). Pending final user review.
**Predecessors:** Slice A (`docs/.../2026-06-13-lsp-mcp-nav-design.md`, host reviewers, Rust), Slice B (`docs/.../2026-06-14-lsp-mcp-slice-b-design.md`, in-container `:rw` implementor, Rust). Slice C is the **multi-language** generalization, first new language = **Python**.

## Goal

Let the bridge's host-side reviewers AND its containerized `:rw` implementor navigate **Python** repos with type-resolved semantic nav, using the SAME 7-tool MCP surface as Rust. Generalize `lsp-mcp` from a rust-analyzer-only shim into a `--lang`→language-server registry, with Python driven by **basedpyright**.

## Architecture (3 sentences)

`lsp-mcp` gains a `LangServer` abstraction selected by `--lang`: it parameterizes the server command, the project-root marker, the LSP `initialize` params, the readiness signal, and the dependency-warming strategy — everything else (the 7 nav tools, name→position resolution, `NavHit` shaping, the MCP server, idle-evict) stays shared. Python adds a `basedpyright` `LangServer` whose dependency model is a **uv-warmed, read-only venv** (no build-target cache — pyright analysis is in-memory, so Python is *simpler* than Rust). Host reviewers run host basedpyright against each repo's existing venv; the in-container implementor gets a Python toolchain image (basedpyright + uv baked) with a warmed read-only venv mounted and the Slice B idle-evict reused.

## Non-goals (YAGNI)

- **TypeScript/JS, Go** — later slices; the registry is designed to extend, but this slice ships ONE new language to de-risk the abstraction.
- **Type-checking / diagnostics tools** — Slice C stays a *navigation* shim (the same 7 read-only nav tools). No "lint/typecheck" tool.
- **In-repo venv mutation** — the warmed venv is a separate managed cache, mounted read-only at nav time; we never write into the repo's tree (mirrors Slice B's read-only `/cargo`).
- **Multi-venv / monorepo-with-many-projects** — one server rooted at one project root per session (same as Rust).
- **Editor-grade features** (rename, code actions, completions) — out of scope.

## §1. The `--lang`→server registry (the core refactor)

**`--lang auto` (new default) detects the language from the repo's root markers** — but `auto` means **single-unambiguous-root ONLY** (codex #3). `Cargo.toml`→rust, `pyproject.toml`/`setup.py`/`setup.cfg`/`requirements.txt`→python. When markers for BOTH appear at the root (pyo3/maturin, docs-tooling, a polyglot monorepo), `auto` is **ambiguous → the shim refuses to start for that repo** (logged, not a guess). Such repos require an **explicit `--lang rust|python`** (and, for a project rooted under a subdir, a new `--project-root <dir>` / for Python a `--python-config-path <pyrightconfig.json>`). So the host-reviewer MCP entry uses `--lang auto` and serves any *unambiguous* repo with one entry; **hybrid repos need explicit config (or a second MCP entry)** — the earlier "one auto entry serves ALL repos" claim is dropped. The container implementor always passes an explicit `--lang` (its language is known from config), so it never hits the ambiguity.

Replace the rust-only hardcoding in `crates/lsp-mcp/src/{lib.rs,lsp/mod.rs}` by **splitting the shared LSP client from the per-language config** (codex #8): a language-agnostic `LspClient` (process spawn, stdin/reader thread, request/response correlation, name→position resolution, the 7 tools, idle-evict) parameterized by a `LangServerConfig`. **The Rust path keeps byte-for-byte behavior** (a Rust regression test pins the current rust-analyzer spawn/init/readiness); Python is a new `LangServerConfig`. Response shaping is NOT assumed identical across servers — Python fixture tests cover **all 7 tools** (incl. `implementation` + call hierarchy) independently.

```
struct LangServerConfig {
    fn program_argv(&self) -> (String, Vec<String>);   // rust-analyzer  |  basedpyright-langserver --stdio
    fn is_project_root(&self, repo: &Path) -> bool;     // Cargo.toml     |  pyproject.toml|setup.py|setup.cfg|requirements.txt
    fn initialize_params(&self, repo) -> Value;         // capabilities (Rust advertises serverStatus; Python advertises NO window/workDoneProgress so RA-style + pyright/*Progress both work)
    fn post_init_config(&self, dep_root) -> Option<Value>; // Python: workspace/didChangeConfiguration { python.pythonPath } after `initialized` (NOT initializationOptions — codex #1)
    fn spawn_env(&self, dep_cache) -> Vec<(String,String)>; // rust: CARGO_TARGET_DIR  |  python: none
    fn readiness(&self) -> Readiness;                   // Rust: progress+serverStatus | Python: pyright/{begin,report,end}Progress (codex #4)
}
```

| | Rust (unchanged) | Python (new) |
|---|---|---|
| server | `rust-analyzer` | `basedpyright-langserver --stdio` |
| root marker | `Cargo.toml` | `pyproject.toml` / `setup.py` / `setup.cfg` / `requirements.txt` |
| dep resolution | `CARGO_TARGET_DIR` warmed via `cargo fetch` | **uv-warmed venv**; basedpyright pointed at it via `python.pythonPath=/venv/venv/bin/python` sent over `workspace/didChangeConfiguration` (spike-gated vs `venvPath+venv`) |
| build-target cache | `/lsp-target` (writable) | **none** (pyright analysis is in-memory) |
| nav offline? | `CARGO_NET_OFFLINE=true` | inherent (pyright reads site-packages, never fetches) |
| readiness | `$/progress` + `experimental/serverStatus` (FU3) | `pyright/{begin,report,end}Progress` (basedpyright's signal; spike-confirmed) |

**The 7 nav tools are LSP-standard** (`workspace/symbol`, `textDocument/{definition,references,hover,implementation,documentSymbol}`, `callHierarchy/*`) → **reused unchanged** across languages. Name-addressing (resolve a symbol *name* → position via `workspace/symbol`, then issue the position-addressed request) is language-agnostic and stays. `NavHit` shaping stays (the `signature`/`context` fields are already generic).

`is_ready` (FU3) generalizes: the readiness predicate becomes part of `LangServer::readiness` so each server's signal is honored (RA's `serverStatus`, pyright's progress/settle).

## §2. Python dependency model — uv-warmed, read-only venv

pyright/basedpyright resolves third-party imports from a **venv's site-packages** in a *configured Python environment*. There is **no compile step and no persistent heavy index** (unlike RA's `target`), so the ONLY warm need is the venv. Two things the codex review corrected:

**Config-delivery channel (codex #1, BLOCKER).** The setting is **`python.pythonPath`** (a path to the interpreter; basedpyright docs *recommend* `pythonPath` and *discourage* `venvPath`) — NOT a bare `venvPath`, and NOT `initializationOptions` (which is not basedpyright's general settings channel). The shim either (a) **does not advertise `workspace.configuration`** and sends one **`workspace/didChangeConfiguration`** with `{ "python": { "pythonPath": "/venv/venv/bin/python" } }` immediately after `initialized`, or (b) implements the `workspace/configuration` request handler returning the same. **(a) is the default**; the spike confirms it actually resolves third-party defs from a read-only venv (and whether a repo's own `pyrightconfig.json`/`pyproject` `[tool.*]` overrides it). Host reviewers point at the repo's **existing** interpreter the same way (`python.pythonPath` = the repo's `.venv/bin/python` if present).

**Exact, NON-MUTATING uv warm (codex #5/#6).** `uv sync` writes a lockfile/`.venv` unless run frozen with `UV_PROJECT_ENVIRONMENT` set, and `uv sync` is for uv's own `uv.lock` (not arbitrary Poetry locks). The warm therefore uses explicit branches that write ONLY into the managed cache venv (never the clone) and leave **no stale packages**:
- `uv.lock` present → `UV_PROJECT_ENVIRONMENT=/venv/venv uv sync --frozen` (exact, no lock write).
- else `requirements*.txt` → `uv venv /venv/venv && VIRTUAL_ENV=/venv/venv uv pip sync -r requirements.txt` (`pip sync` is exact — it REMOVES packages not in the file, so a dep dropped on this branch can't linger and resolve a stale import).
- else `pyproject.toml` with no lock → `uv venv` + `uv pip install .` (best-effort; a sdist that needs a compiler may fail → degrade).
- Poetry/PDM/Hatch lockfiles → unsupported this slice → **degrade to workspace-only** (documented; a proven `export`-to-requirements path is a follow-up).
All via the **verify registries-egress** (PyPI), **NO creds** — identical posture to Slice B's `cargo fetch`. Because each branch uses exact/`--frozen` semantics, the cache may stay **repo-keyed** (the Slice B reuse lesson) without a stale-package hazard.
- **Mount** the cache venv read-only at nav time; nav never fetches.
- **Key the cache on the SOURCE repo** (canonical path), reusing `verify::cache_volume_name` with a new base `a2a-impl-py-venv` — bounded + reused per repo.
- **Degrade** like Slice B: no resolvable deps / warm fails → `eprintln` a warning and serve **workspace-only** nav (intra-repo symbols still resolve; third-party imports don't).

## §3. Host reviewers (Slice A analogue)

The existing host reviewers' single `[[agents.mcp]] lsp` entry becomes `args = ["--repo", "{cwd}", "--lang", "auto", ...]` — `auto` serves any **unambiguous** rust-or-python repo with one entry. **Hybrid/ambiguous repos** (both `Cargo.toml` + `pyproject.toml`) are NOT auto-served — they need an explicit `--lang` entry or a second MCP server (documented; not claimed away). basedpyright installed on the host (`pip install basedpyright` — bundles its Node runtime). Resolves via the repo's existing interpreter (`python.pythonPath` = the repo's `.venv/bin/python` when present, via `didChangeConfiguration`). Read-only by prompt-contract (same as the Rust reviewers).

## §4. Container implementor (Slice B analogue)

- **`a2a-py-toolchain` image**: a Python base + `uv` + `pip install basedpyright` (bundles Node) baked; the Linux `lsp-mcp` baked (same repo-root build stage as Slice B). Separate from `a2a-toolchain` (Rust) — an impl run targets one language's image. **Image acceptance gates** (codex #9): `basedpyright-langserver --stdio` starts; `uv` warms a **wheel-only** fixture AND an **sdist** fixture (so missing build deps surface); NO creds mounted during warm; a private/unreachable dep **degrades with a clear warning** (not a hang).
- **Runtime mounts** (appended host-side in `build_warm_impl`, the Slice B pattern, repo-keyed): `-v <a2a-impl-py-venv-*>:/venv:ro`; basedpyright configured with `python.pythonPath=/venv/venv/bin/python`. No `/lsp-target` (none needed).
- **`warm_lsp_deps` generalized**: dispatch on the impl agent's language (config) → `cargo fetch` (rust) or the exact uv venv warm (python, §2), both via `[verify]` runtime+image+egress (the Slice B fixes: repo-keyed, runtime-honored).
- **Idle-evict reused** unchanged, but **data-driven**: basedpyright is lighter than RA (the memory spike sets whether/when Python evicts — the machinery stays language-agnostic, the timeout is per-language config). The nit that eviction "may be unnecessary" for Python is resolved by the §6 memory measurement, not assumed.
- The Python implementor's *editing* + verify (`ruff`/`pytest`) + review-the-diff reuse the existing implement loop; only the toolchain image + the lsp dep-warm differ.

## §5. Readiness (per-language)

RA's readiness (FU3: `$/progress` + `serverStatus.quiescent`) is RA-specific. The readiness predicate becomes a per-language `Readiness` (codex #4): `Readiness::RustRa` (progress + serverStatus) and `Readiness::Pyright`. basedpyright, when the client does **not** advertise `window.workDoneProgress`, emits its own **`pyright/beginProgress` / `pyright/reportProgress` / `pyright/endProgress`** — which the current shim does NOT parse, so a naive port would re-impose the ~30s first-call tax (the exact FU3 failure, repeated). So `Readiness::Pyright` parses `pyright/{begin,end}Progress` (settled = begun-and-ended), with post-`initialized`/settings-applied as a secondary ready signal. The fallback stays the bounded best-effort timeout (never a hang). **A test asserts the first Python `workspace_symbol` does NOT wait the full timeout when no progress arrives** (the FU3 regression guard, per-language).

## §6. Empirical spike FIRST (Task 1, per the arc)

Before building, measure basedpyright in the `a2a-py-toolchain` container against a representative Python repo (e.g. `code-review-backtest` or `agent-eval`). **These are GATES — implementation does not start until the config-channel matrix (1) and edit-reflection (5) are green** (codex verdict):

1. **Config channel × read-only venv resolution (GATE)** — a MATRIX: `{python.pythonPath, python.venvPath+venv}` × `{via didChangeConfiguration, via workspace/configuration}` × `{with, without a repo pyrightconfig/pyproject override}`. For each, does basedpyright resolve a third-party import (e.g. `import requests` → `definition`/`hover` into site-packages) from a venv mounted **read-only**? The production path is the first combination that resolves cleanly. (Subsumes the Slice B read-only-vs-writable decision.)
2. **Memory footprint** — RSS vs rust-analyzer → sets the Python idle-evict timeout (or disables it if trivially small).
3. **Readiness signal + timing** — confirm `pyright/{begin,end}Progress` fires; cold vs warm time; that the first call doesn't pay the full timeout.
4. **uv warm (each branch)** — `uv sync --frozen` (uv.lock) AND `uv pip sync` (requirements) through the verify egress, no creds; confirm each populates a mountable venv and writes nothing into the clone; time them.
5. **Edit reflection (GATE) + fallback** — does basedpyright reflect a mid-session on-disk edit WITHOUT the shim sending notifications? If NOT (likely — unlike RA, pyright may need client signals), the named fallback ships: the shim sends **`workspace/didChangeWatchedFiles`** for `*.py`/`pyproject.toml`/lockfiles/`requirements*` (or `didOpen`+read the target file before a positional request). This gate decides whether the fallback is in scope for Slice C.

Spike verdict written to `docs/superpowers/spikes/2026-06-15-slice-c-basedpyright-proof.md`.

## Risks / open questions

- **basedpyright config channel** (was a BLOCKER) — resolved to `python.pythonPath` via `workspace/didChangeConfiguration` after `initialized`, **spike-gated** (§6.1) against repo-config overrides.
- **Edit reflection** — basedpyright may not self-watch like RA; the §6.5 gate ships a `didChangeWatchedFiles` fallback if so.
- **uv lockfile coverage**: Poetry/PDM/Hatch + bare-script repos → workspace-only nav (documented degrade; an export-to-requirements path is a follow-up).
- **basedpyright Node bundling size** in the image (the `nodejs` wheel) — measured at image-build (acceptance gate §4).
- **Two toolchain images** (`a2a-toolchain` Rust + `a2a-py-toolchain`) — an impl/verify run is single-language; the config selects the image. No cross-language session.
- **File-URI builder** (nit) — the generalization fixes the current naive `file://{display}` construction (proper percent-encoding) so non-ASCII/space paths work across servers.

## Execution

Empirical spike → generalize the registry (Rust unchanged, regression-guarded) → Python `LangServer` → host wiring → Python toolchain image + uv warm + runtime mounts → live gate (host review of a Python repo + in-container Python implement DoD). Dogfooded via the bridge where clean; host-side for the registry refactor + integration. Full-branch review vs main before merge (the Slice B lesson).
