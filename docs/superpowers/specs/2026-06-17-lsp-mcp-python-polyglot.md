# Python in the containerized polyglot implementor — design spec

**Status:** design (approved approach: uv-first deps + mise-provisioned tooling + full parity)
**Date:** 2026-06-17
**Track:** LSP-MCP polyglot. Follows C2a (rust+go) + #1 a/c/b/d. Python is **slice 1 of 2**; JS/TS is the next slice. See [[lsp-mcp-c2-polyglot]].

## Goal

Bring **Python to rust/go parity** in the containerized polyglot implementor: in-container
semantic nav + a warm dep cache + a deterministic verify, selected per-session-cwd via the
existing `[[languages]]` machinery. **No new lsp-mcp language** — lsp-mcp already navigates Python
via basedpyright (the C1 work + the `Pyright` readiness machine). This slice is **toolchain image +
a python profile + the #1d env lesson + a live gate**, reusing `apply_warm_lsp` / `select_profile` /
`warm_lsp_deps_step` / `compose_warm_fetch` unchanged.

## Why Python is structurally different from rust/go (read first)

rust/go resolve dependency types from a **shared package cache** the LSP server reads via an **env
var** (`CARGO_HOME=/cargo`, `GOMODCACHE=/go/pkg/mod`). **basedpyright resolves types from a Python
environment's `site-packages`, discovered via the interpreter's `pythonPath`** — there is no
"cache-home" env. So Python's "dep cache" is a **populated virtualenv**, and the lever that points
basedpyright at it is the **interpreter path**, not a cache env. `lsp-mcp` already exposes that lever:
`resolve_python_path` (lang.rs:363) honors `LSP_MCP_PYTHON_PATH` as the **highest-precedence override**
(Hard-fail if missing/non-executable). The whole design hangs on this difference.

## Architecture (three layers, mirroring rust/go)

### Layer 1 — Toolchain image (mise-provisioned, real-binary exposure)

`deploy/containers/toolchain.Containerfile` gains a Python layer. **Provision with mise** (matches the
host, which already runs node via mise; uniform + version-pinned), installing into the standard
`~/.local/share/mise/installs/<tool>/<version>/bin` real-binary locations:

- Install mise non-interactively (`curl https://mise.run | sh` → `/root/.local/bin/mise`; that dir is
  already on codex's subprocess PATH per the #1d env capture).
- Pin + install: `python` (core), `uv` (core), `basedpyright` (npm backend), `ruff` (core/aqua).
  Pin versions in the Dockerfile (reproducibility), e.g. `mise use -g -y python@3.12 uv@<x>
  npm:basedpyright@<x> ruff@<x>`.
- **CRITICAL — DO NOT use mise shims at runtime.** Shims resolve the version from mise's env/config;
  codex hands MCP subprocesses a **stripped env** (the #1d finding), so a shim would fail exactly like
  the rustup proxy did. Instead **symlink the real binaries into `/usr/local/bin`** (mirrors the
  gopls/rust-analyzer fix, lines 62-64): `python3`, `uv`, `ruff`, `basedpyright-langserver`. Then they
  resolve under any PATH, with no mise activation.
- Leave the existing rust (rustup) + go (manual) + node (base) layers **untouched** — mise provisions
  only the *new* Python tooling. (The image base is already `node:24-slim`, so `basedpyright-langserver`,
  a node CLI, has its runtime.)

### Layer 2 — The `python` `[[languages]]` profile (both `containerized.toml` + `.podman.toml`)

```
[[languages]]
id = "python"
# uv-first: build a cached venv AND install the project's deps into it so basedpyright can RESOLVE
# imported types (the venv == python's "dep cache", analogous to /cargo). --locked/frozen for determinism.
fetch = "uv venv /pyvenv && VIRTUAL_ENV=/pyvenv uv pip install --requirement <project deps>"
warm_cache = "a2a-impl-lsp-cache-py"     # the named volume backing /pyvenv
dep_cache_path = "/pyvenv"               # the venv mount (NOT a package cache) → basedpyright reads site-packages here
verify_cache_path = "/cache"
fetch_env = { UV_CACHE_DIR = "/uvcache" }                 # uv's module cache (separate from the venv)
verify_env = { UV_CACHE_DIR = "/cache/uv" }
# #1d KEYSTONE for python: point basedpyright at the WARMED venv interpreter (highest-precedence override),
# + PATH for the node-based basedpyright-langserver + uv. codex strips the image ENV → these MUST be here.
lsp_env = { LSP_MCP_PYTHON_PATH = "/pyvenv/bin/python", PATH = "<...>", UV_CACHE_DIR = "/uvcache" }
image = "a2a-toolchain:latest"
[[languages.verify]]   # configurable defaults; mirror rust's fmt/clippy/build/test shape
name = "format"; cmd = "ruff format --check ."
[[languages.verify]]
name = "lint";   cmd = "ruff check ."
[[languages.verify]]
name = "test";   cmd = "pytest -q"
```

Notes:
- **venv path-stability:** a uv/virtualenv venv embeds its creation path (`pyvenv.cfg`, script shebangs).
  Create it at `/pyvenv` in the warm fetch and mount it at the **same** `/pyvenv` for nav → consistent.
  Mounted `:ro` for nav (basedpyright only reads `site-packages`).
- **`apply_warm_lsp` reuse:** the Lsp-ctx `cache_binding` mounts `dep_cache_path` (`/pyvenv`) `:ro` and
  applies `lsp_env`. So `LSP_MCP_PYTHON_PATH=/pyvenv/bin/python` reaches basedpyright via the *exact*
  per-turn delivery path #1d fixed (`apply_warm_lsp` → entry.mcp → codex `-c …env.*`). No new plumbing.
- **`fetch`/deps source:** prefer `uv.lock`/`pyproject` (`uv sync --frozen`) when present, else
  `requirements*.txt` (`uv pip install -r --no-deps?`). A repo with no resolvable lock → warm degrades
  (no in-container type resolution; Part-A degrade reports it), exactly like rust without `Cargo.lock`.

### Layer 3 — Detection + readiness (already done)

`detect_lang`/`detect` already recognize `setup.py`/`setup.cfg`/`requirements*.txt`/`pyproject`
project-section/`.py` (lang.rs). The `Pyright` settle-based readiness machine already exists. No change.

## Definition of Done (live gate)

A **deps-bearing python fixture** under `allowed_cwd_root` (e.g. a package that imports a 3rd-party lib
like `requests`, with a committed `uv.lock`/`requirements.txt`):
1. **Nav:** `run-workflow c2b-nav --session-cwd <py fixture> --config containerized.toml` → basedpyright
   returns a **type-resolved** signature for a function whose type involves the installed dep (proving
   the warm venv resolved third-party types, not just stdlib). codex's lsp call log shows the call.
2. **`:ro` safety:** the fixture is unmutated after the run (`git status --porcelain` empty).
3. **Cache:** exactly one `-cache-py-<hash>` venv volume per fixture, reused across runs (no per-run leak).
4. **Verify:** `ruff`/`pytest` run green in-container via the bridge's deterministic verify.
5. **No regression:** rust + go nav still work; host-only workflows pay no warm cost (the #1d
   workflow-scoped guard); the `implement` warm path unaffected.
6. **mise-shim guard:** confirm `basedpyright-langserver`, `python3`, `uv`, `ruff` resolve in a
   **stripped-env** container (`env -i PATH=/usr/local/bin:/usr/bin:/bin <bin> --version`) — proving the
   symlink exposure dodges the shim/activation trap.

## Out of scope (follow-ups)

- **JS/TS** — the next slice (new lsp-mcp language + typescript-language-server, kept behind a swappable
  abstraction since TS LSP tooling will churn in 6-12 months).
- **Non-uv project flows** (poetry/pdm/pipenv-native) — uv consumes `pyproject`/`requirements`; native
  poetry/pdm resolution is a fast-follow. conda/system-python projects degrade (Part-A honest message).
- **`serve` per-request warm** — still deferred (#1d): per-request cwd arrives after spawn.

## Risks

- **#1d redux (highest):** basedpyright is node+python; its interpreter + PATH must survive codex's
  stripped MCP-subprocess env. Mitigations: `LSP_MCP_PYTHON_PATH` in `lsp_env` (explicit, Hard-fail loud)
  + real-binary symlinks (no shims). The wrapper-diagnostic (`/bin/sh -c 'env > …; exec <real>'`, **no
  `{}` braces** — the mcp-arg validator rejects them) is ready if nav fails.
- **venv path-sensitivity / `:ro`:** create + mount at the same `/pyvenv`; verify basedpyright resolves
  third-party (not just stdlib) types — the gate explicitly tests a 3rd-party import.
- **mise in Docker build:** non-interactive, version-pinned, offline-safe egress for the install step
  (the build runs with the verify/registries egress, not the locked agent egress).

## Constraints (carried)

sonnet implementor; **codex available again** — codex for high-risk + final review, Opus per-task;
`max_attempts = 3`; reviewers judge **intent, not verbatim**. This spec gets a **codex gpt-5.5 xhigh**
review before planning.
