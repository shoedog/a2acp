# LSP-over-MCP Slice C — Multi-language nav (Python / basedpyright) — Design

**Status:** approved direction (brainstormed 2026-06-15), pending spec review.
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

**`--lang auto` (new default) detects the language from the repo's root markers** (`Cargo.toml`→rust, `pyproject.toml`/`setup.py`/`setup.cfg`/`requirements.txt`→python); explicit `--lang rust|python` overrides. This is what lets a SINGLE host-reviewer MCP entry serve repos of either language (the reviewer drives many codebases per session) — no per-repo reconfiguration. The container implementor targets one known-language repo, so it may pass an explicit `--lang` (from config) but `auto` works there too. Unknown/ambiguous markers → a clear error (and, for `auto`, the shim simply doesn't start for that repo rather than guessing wrong).

Replace the rust-only hardcoding in `crates/lsp-mcp/src/{lib.rs,lsp/mod.rs}` with a `LangServer` config resolved from `--lang` (or detected when `auto`):

```
trait/enum LangServer {
    fn program_argv(&self) -> (String, Vec<String>);   // rust-analyzer  |  basedpyright-langserver --stdio
    fn is_project_root(&self, repo: &Path) -> bool;     // Cargo.toml     |  pyproject.toml|setup.py|setup.cfg|requirements.txt
    fn initialize_params(&self, repo, dep_root) -> Value; // capabilities + per-server initializationOptions (pyright venvPath)
    fn spawn_env(&self, dep_cache) -> Vec<(String,String)>; // rust: CARGO_TARGET_DIR  |  python: VIRTUAL_ENV/none
    fn readiness(&self) -> Readiness;                   // rust: progress+serverStatus | python: (spike — workDoneProgress / settle)
}
```

| | Rust (unchanged) | Python (new) |
|---|---|---|
| server | `rust-analyzer` | `basedpyright-langserver --stdio` |
| root marker | `Cargo.toml` | `pyproject.toml` / `setup.py` / `setup.cfg` / `requirements.txt` |
| dep resolution | `CARGO_TARGET_DIR` warmed via `cargo fetch` | **uv-warmed venv** (site-packages) via `venvPath` |
| build-target cache | `/lsp-target` (writable) | **none** (pyright analysis is in-memory) |
| nav offline? | `CARGO_NET_OFFLINE=true` | inherent (pyright reads site-packages, never fetches) |
| readiness | `$/progress` + `experimental/serverStatus` (FU3) | spike (workDoneProgress / initialize-settle) |

**The 7 nav tools are LSP-standard** (`workspace/symbol`, `textDocument/{definition,references,hover,implementation,documentSymbol}`, `callHierarchy/*`) → **reused unchanged** across languages. Name-addressing (resolve a symbol *name* → position via `workspace/symbol`, then issue the position-addressed request) is language-agnostic and stays. `NavHit` shaping stays (the `signature`/`context` fields are already generic).

`is_ready` (FU3) generalizes: the readiness predicate becomes part of `LangServer::readiness` so each server's signal is honored (RA's `serverStatus`, pyright's progress/settle).

## §2. Python dependency model — uv-warmed, read-only venv

pyright/basedpyright resolves third-party imports from a **venv's site-packages** (configured via `venvPath`+`venv`, or `pythonPath`). There is **no compile step and no persistent heavy index** (unlike RA's `target`), so the ONLY warm need is the venv.

- **Warm** (the `warm_lsp_deps` analogue): `uv venv <cache>/venv` then `uv pip install` from the repo's deps — `uv pip install -r requirements.txt`, or `uv pip install .` (pyproject/setup), or `uv sync` when a `uv.lock`/`poetry.lock` is present. uv is chosen because it handles the fragmented Python ecosystem (requirements / pyproject / lockfiles) fast and deterministically. Runs through the **verify registries-egress** (PyPI), **NO creds** — identical posture to Slice B's `cargo fetch`.
- **Mount** read-only at nav time; point basedpyright at it (`venvPath=<cache>`, `venv=venv`). Nav never fetches.
- **Key the cache on the SOURCE repo** (canonical path), reusing `verify::cache_volume_name` with a new base `a2a-impl-py-venv` — bounded + reused per repo (the Slice B review's keying lesson, baked in from the start).
- **Degrade** like Slice B: no resolvable deps / warm fails → `eprintln` a warning and serve **workspace-only** nav (intra-repo symbols still resolve; third-party imports don't).
- **Host reviewers**: simplest path — point basedpyright at the repo's **existing** venv (`.venv`/`venv` auto-detected, or a configured `venvPath`); no warming needed host-side (the developer already has one).

## §3. Host reviewers (Slice A analogue)

The existing host reviewers' single `[[agents.mcp]] lsp` entry becomes `args = ["--repo", "{cwd}", "--lang", "auto", ...]` — `auto` detects rust vs python per repo, so ONE entry serves both (no second MCP server, no per-repo config). basedpyright installed on the host (`pip install basedpyright` — bundles its Node runtime). Resolves via the repo's existing venv (auto-detected `.venv`/`venv`, or a configured `venvPath`). Read-only by prompt-contract (same as the Rust reviewers).

## §4. Container implementor (Slice B analogue)

- **`a2a-py-toolchain` image**: a Python base + `uv` + `pip install basedpyright` (bundles Node) baked; the Linux `lsp-mcp` baked (same repo-root build stage as Slice B). Separate from `a2a-toolchain` (Rust) — an impl run targets one language's image.
- **Runtime mounts** (appended host-side in `build_warm_impl`, the Slice B pattern, repo-keyed): `-v <a2a-impl-py-venv-*>:/venv:ro`; basedpyright `venvPath=/venv`. No `/lsp-target` (none needed).
- **`warm_lsp_deps` generalized**: dispatch on the impl agent's language (config) → `cargo fetch` (rust) or `uv` venv warm (python), both via `[verify]` runtime+image+egress (the Slice B fixes: repo-keyed, runtime-honored).
- **Idle-evict reused** unchanged (basedpyright is lighter than RA — a few hundred MB vs ~3 GB — but evicting it while idle during review is still free; the machinery is language-agnostic).
- The Python implementor's *editing* + verify (`ruff`/`pytest`) + review-the-diff reuse the existing implement loop; only the toolchain image + the lsp dep-warm differ.

## §5. Readiness (per-language)

RA's readiness (FU3: `$/progress` + `serverStatus.quiescent`) is RA-specific. basedpyright signals initial-analysis completion differently (likely `$/progress` work-done for the background analysis, or simply being answerable shortly after `initialize`). The spike (§6) measures basedpyright's actual signal; `LangServer::readiness` encodes it. Fallback stays the existing best-effort timeout, so a missing signal degrades to a bounded wait (never a hang).

## §6. Empirical spike FIRST (Task 1, per the arc)

Before building, measure basedpyright in the `a2a-py-toolchain` container against a representative Python repo (e.g. `code-review-backtest` or `agent-eval`):

1. **Read-only venv resolution** — does basedpyright resolve a third-party import (e.g. `import numpy`/`requests` → `definition`/`hover` into site-packages) from a venv mounted **read-only**? (The Slice B offline-proof analogue — decides read-only vs writable venv.)
2. **Memory footprint** — RSS vs rust-analyzer (informs whether idle-evict matters as much).
3. **Readiness signal + timing** — what notification marks initial analysis done; cold vs warm (re-open) time.
4. **uv warm** — `uv venv` + install from requirements/pyproject through the verify egress, no creds; time + that it populates a mountable venv.
5. **Edit reflection** — does basedpyright reflect a mid-session on-disk edit (the impl-agent use case; Slice B item 2 analogue)?

Spike verdict written to `docs/superpowers/spikes/2026-06-15-slice-c-basedpyright-proof.md`.

## Risks / open questions

- **basedpyright venv config in headless MCP**: `venvPath` may need to arrive via `initializationOptions` vs `workspace/didChangeConfiguration`; the spike pins the working channel.
- **uv lockfile coverage**: repos without requirements/pyproject (bare scripts) → workspace-only nav (acceptable degrade).
- **basedpyright Node bundling size** in the image (the `nodejs` wheel) — acceptable; measured at image-build.
- **Two toolchain images** (`a2a-toolchain` Rust + `a2a-py-toolchain`) — an impl/verify run is single-language; the config selects the image. No cross-language session.

## Execution

Empirical spike → generalize the registry (Rust unchanged, regression-guarded) → Python `LangServer` → host wiring → Python toolchain image + uv warm + runtime mounts → live gate (host review of a Python repo + in-container Python implement DoD). Dogfooded via the bridge where clean; host-side for the registry refactor + integration. Full-branch review vs main before merge (the Slice B lesson).
