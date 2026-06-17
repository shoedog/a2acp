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
  gopls/rust-analyzer fix, lines 62-64): `python3`, `uv`, `ruff`, **both `basedpyright` AND
  `basedpyright-langserver`** (the bare `basedpyright` is what answers `--version` for the stripped-env
  gate; `-langserver` is stdio-only — codex review MAJOR-5). Then they resolve under any PATH, no mise
  activation. (`/usr/local/bin` IS on codex's stripped MCP PATH — confirmed by the #1d env capture — so
  `lsp_env` needs **no `PATH` override**; Opus MINOR-3.)
- Leave the existing rust (rustup) + go (manual) + node (base) layers **untouched** — mise provisions
  only the *new* Python tooling. (The image base is already `node:24-slim`, so `basedpyright-langserver`,
  a node CLI, has its runtime.)

### Layer 2 — The `python` `[[languages]]` profile (both `containerized.toml` + `.podman.toml`)

```
[[languages]]
id = "python"
# uv-first. ALWAYS create the venv at /pyvenv (so the interpreter exists even with no installable deps —
# codex MAJOR-4), then best-effort install the project's deps INTO /pyvenv (UV_PROJECT_ENVIRONMENT pins the
# target so uv never touches /work/.venv on the :ro real repo — codex MAJOR-2). Deps failing must NOT fail
# venv creation (else the mount is absent and the hard LSP_MCP_PYTHON_PATH override crashes basedpyright).
fetch = "uv venv /pyvenv && { UV_PROJECT_ENVIRONMENT=/pyvenv uv sync --frozen || UV_PROJECT_ENVIRONMENT=/pyvenv uv pip install -r requirements.txt || true; }"
warm_cache = "a2a-impl-lsp-cache-py"     # the named volume backing /pyvenv
dep_cache_path = "/pyvenv"               # the venv mount (NOT a package cache) → basedpyright reads site-packages here
verify_cache_path = "/cache"
fetch_env = { UV_CACHE_DIR = "/uvcache" }                 # uv's module cache (separate from the venv)
verify_env = { UV_CACHE_DIR = "/cache/uv" }
# #1d KEYSTONE for python: point basedpyright at the WARMED venv interpreter (highest-precedence override).
# PATH is NOT needed (the binaries are symlinked into /usr/local/bin, already on codex's stripped PATH).
# PYTHONDONTWRITEBYTECODE guards against a __pycache__ write attempt under the :ro venv mount (Opus M5).
lsp_env = { LSP_MCP_PYTHON_PATH = "/pyvenv/bin/python", PYTHONDONTWRITEBYTECODE = "1" }
# NO `image =` line — inherit `[verify].image` like rust/go (profile.image only affects fetch/verify
# containers, NEVER the nav container which uses the impl agent's sandbox image; Opus M2).
[[languages.verify]]   # ruff is self-contained (no project deps); pytest NEEDS the deps installed →
name = "format"        # provision a verify venv under /cache and run pytest IN it (codex MAJOR-3).
cmd  = "ruff format --check ."
[[languages.verify]]
name = "lint"
cmd  = "ruff check ."
[[languages.verify]]
name = "test"
cmd  = "UV_PROJECT_ENVIRONMENT=/cache/venv uv sync --frozen && /cache/venv/bin/pytest -q"
```

Notes:
- **venv path-stability + `:ro`:** a uv venv embeds its creation path (`pyvenv.cfg`, shebangs). Create AND
  mount at the same `/pyvenv`. Mounted `:ro` for nav (basedpyright reads `site-packages`, doesn't write —
  `PYTHONDONTWRITEBYTECODE=1` is belt-and-suspenders; under `:ro` a stray write fails hard, the safe mode).
- **MAJOR-4 (degrade vs hard override) — resolved by always-create:** the `uv venv /pyvenv && { … || true; }`
  shape guarantees `/pyvenv/bin/python` exists whenever the image is sane, so `warm_lsp_deps_step` returns
  the vol → `apply_warm_lsp` mounts `/pyvenv` → the hard `LSP_MCP_PYTHON_PATH` always resolves. A repo with
  no lock still gets a bare venv (stdlib resolves; 3rd-party imports show as unresolved, NOT a crash). Only
  a BROKEN image (no python/uv) makes warm return `None` → `/pyvenv` absent → the hard override fails LOUD —
  acceptable (that's the override's intended fail-loud behavior). So `apply_warm_lsp`/`warm_lsp_deps_step`
  are reused UNCHANGED — the fix lives in the profile `fetch` string, not the shared helpers.
- **`apply_warm_lsp` reuse:** the Lsp-ctx `cache_binding` mounts `dep_cache_path` (`/pyvenv`) `:ro` + applies
  `lsp_env`, so `LSP_MCP_PYTHON_PATH` reaches basedpyright via the exact per-turn path #1d fixed
  (`apply_warm_lsp` → entry.mcp → codex `-c …env.*`). No new plumbing.

### Layer 3 — Detection + readiness (already done)

`detect_lang`/`detect` already recognize `setup.py`/`setup.cfg`/`requirements*.txt`/`pyproject`
project-section/`.py` (lang.rs). The `Pyright` settle-based readiness machine already exists. No change.

### Known limitation — repo pyright config can override the warmed interpreter (codex MAJOR-1)

basedpyright honors a repo-local `pyrightconfig.json` or `[tool.pyright]`/`[tool.basedpyright]` in
`pyproject.toml` **over** the `pythonPath` we push via `didChangeConfiguration` (the C1 spike recorded
this: `docs/superpowers/spikes/2026-06-15-slice-c1-basedpyright-proof.md`). So for a repo that pins its
own interpreter/venv path, the warmed `/pyvenv` may be ignored → third-party types unresolved. `lsp-mcp`
sends the envelope but does **not** detect such an override (lang.rs:~531). **Scope of our claim:**
`LSP_MCP_PYTHON_PATH` is highest-precedence *within lsp-mcp's discovery*, not within basedpyright when the
repo dictates its own config. **This slice:** (a) the gate fixture has NO `pyrightconfig.json` (the common
case — warmed `/pyvenv` wins); (b) document the limitation; (c) **small optional lsp-mcp warn** — if a
repo pyright config is present, log a one-line warning that the pushed `pythonPath` may be overridden
(cheap; the only candidate code change in this slice — otherwise config+image only). Full
honor-repo-config-then-locate-its-venv resolution is a fast-follow.

## Definition of Done (live gate)

A **deps-bearing python fixture** under `allowed_cwd_root` (e.g. a package that imports a 3rd-party lib
like `requests`, with a committed `uv.lock`/`requirements.txt`):
1. **Nav (3rd-party, via the warmed venv):** `run-workflow c2b-nav --session-cwd <py fixture> --config
   containerized.toml` → basedpyright returns a **type-resolved** signature for a function whose type
   involves the **installed 3rd-party dep** (not just stdlib — proving the warm venv resolved it). The
   lsp call log must show the resolved interpreter is `/pyvenv/bin/python` (NOT the `python3` fallback —
   catches a silent fallback regression; Opus M6). Since the agent egress is locked, a 3rd-party type can
   ONLY resolve from the warmed venv, so this is unambiguous. Fixture has no `pyrightconfig.json` (MAJOR-1).
2. **`:ro` safety:** the fixture is unmutated after the run (`git status --porcelain` empty).
3. **Cache:** exactly one `-cache-py-<hash>` venv volume per fixture, reused across runs (no per-run leak).
4. **Verify:** `ruff format`/`ruff check` + `pytest` (in the synced `/cache/venv`) run green in-container
   via the bridge's deterministic verify — proving the verify venv sees project deps (codex MAJOR-3).
5. **No regression:** rust + go nav still work; host-only workflows pay no warm cost (the #1d
   workflow-scoped guard); the `implement` warm path unaffected.
6. **stripped-env shim guard (codex MAJOR-5):** in a stripped env, `python3`, `uv`, `ruff` answer
   `env -i PATH=/usr/local/bin:/usr/bin:/bin <bin> --version`; for the LSP server use **`basedpyright
   --version`** (the bare CLI — `basedpyright-langserver` is stdio-only and won't answer `--version`) +
   `command -v basedpyright-langserver`. Proves the symlink exposure dodges the shim/activation trap.

## Out of scope (follow-ups)

- **JS/TS** — the next slice (new lsp-mcp language + typescript-language-server, kept behind a swappable
  abstraction since TS LSP tooling will churn in 6-12 months).
- **Non-uv project flows** (poetry/pdm/pipenv-native) — uv consumes `pyproject`/`requirements`; native
  poetry/pdm resolution is a fast-follow. conda/system-python projects degrade (Part-A honest message).
- **`serve` per-request warm** — still deferred (#1d): per-request cwd arrives after spawn.

## Risks

- **#1d redux (highest):** basedpyright is node+python; its interpreter must survive codex's stripped
  MCP-subprocess env. Mitigations: `LSP_MCP_PYTHON_PATH` in `lsp_env` (explicit, Hard-fail loud) + real-
  binary symlinks into `/usr/local/bin` (no shims). PATH is NOT an issue (`/usr/local/bin` is on codex's
  stripped PATH per the #1d capture). The wrapper-diagnostic (`/bin/sh -c 'env > …; exec <real>'`, **no
  `{}` braces** — the mcp-arg validator rejects them) is ready if nav fails.
- **hard-override × warm-failure (codex MAJOR-4):** resolved by the always-create-venv `fetch` shape;
  only a broken image hard-fails (acceptable, loud). Verify at gate time that a no-lock repo degrades to a
  bare venv (no crash), not just the happy path.
- **verify needs its own dep'd venv (codex MAJOR-3):** the `test` step syncs `/cache/venv` and runs
  `pytest` from it; ruff steps are dep-free. Confirm the verify venv egress (uv→PyPI via the verify
  registries-egress) works and creds-XOR-registries holds.
- **repo pyright config (codex MAJOR-1):** documented limitation + optional warn; gate fixture avoids it.
- **venv path-sensitivity / `:ro`:** create + mount at the same `/pyvenv`; the gate tests a 3rd-party
  import AND asserts the interpreter is `/pyvenv/bin/python`.
- **mise in Docker build:** non-interactive, version-pinned; the build step uses the verify/registries
  egress (same trust model as the existing `sh.rustup.rs`/`go.dev` curls), not the locked agent egress.

## Constraints (carried)

sonnet implementor; codex for high-risk + final review, Opus per-task; `max_attempts = 3`; reviewers
judge **intent, not verbatim**.

## Review status

Dual-reviewed before planning: **codex gpt-5.5 xhigh** (correctness — found MAJOR 1-5, all folded above)
+ **Opus 4.8** (architecture — fix-then-ship, MINORs folded). Both `fix-then-ship`, no BLOCKER/redesign.
The architecture is a thin config+image slice over #1d-hardened machinery (reuse verified); the codex
MAJORs sharpened the concrete `fetch`/`verify`/gate mechanics. **Net code change this slice: optional
one-line pyright-config warn in lsp-mcp; everything else is config (two `[[languages]]` blocks) + the
toolchain image Python layer + the live gate.**
