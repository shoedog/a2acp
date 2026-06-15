# Slice C1 — basedpyright host-path proof (Task 1)

**Date:** 2026-06-14 · **Verdict: host path is viable; Task 6 ships the WRAPPED settings envelope; readiness must NOT depend on a progress cycle.**

Empirically proves the host basedpyright behavior the later C1 tasks depend on: the config-channel envelope (Gate 1a/1b/1c), the readiness/no-progress settle (Gate 2), and the `--lang auto` detection predicates (Gate 3). Every gate quotes a REAL stdio request/response transcript, not an assertion.

## Environment

- **basedpyright version:** `basedpyright 1.39.8` (based on pyright `1.1.410`), installed isolated via `uv tool install basedpyright` → `basedpyright==1.39.8` + bundled `nodejs-wheel-binaries==24.16.0` (Node v24.16.0). Binary on PATH at `~/.local/bin/basedpyright-langserver`; the langserver's own `--version` flag errors (it expects `--stdio`), so the version is from the `basedpyright` CLI.
  - Note: `basedpyright-langserver --version` prints `Error: Connection input stream is not set. Use … '--stdio'` — this is expected; it is a langserver, not a one-shot. Version comes from `basedpyright --version`.
- **Driver:** throwaway stdio JSON-RPC client at `/tmp/lsp_spike/driver.py` (LSP `Content-Length` framing). It answers server→client requests (`workspace/configuration`, `client/registerCapability`, `window/workDoneProgress/create`).
- **Target repo (read-only):** `~/code/agent-eval` — `.venv/bin/python` is CPython **3.14.4**; site-packages has real third-party `click` 8.3.2 (also pydantic/httpx/anthropic/rich). Probe symbol = `click.echo` at `scripts/run_eval.py:183`. The repo's venv was used READ-ONLY (pointed at, never written/pip-installed).
- **System python** (the no-venv fallback): `/usr/bin/python3` = CPython **3.9.6**, which has NO `click` — this version split (3.14 venv vs 3.9 system) is what makes the gates observable.

A key protocol fact discovered up front: **basedpyright uses BOTH config channels** — it also *pulls* config via `workspace/configuration` (sections `python`, `python.analysis`, `basedpyright`, `basedpyright.analysis`, `pyright`). All gates below answer the pull with `null` so the result is attributable solely to the *pushed* `workspace/didChangeConfiguration` envelope (or its absence).

---

## Gate 1a — config-channel resolution from an existing venv (the WRAPPED envelope) — PASS

Drove `initialize` (advertising NO `window/workDoneProgress`: `"window": {}`) → `initialized` → the **wrapped** push:

```json
{"jsonrpc":"2.0","method":"workspace/didChangeConfiguration",
 "params":{"settings":{"python":{"pythonPath":"/Users/wesleyjinks/code/agent-eval/.venv/bin/python"}}}}
```

(`workspace/configuration` pulls were answered `null` — this is push-only.) Then `textDocument/didOpen` of `scripts/run_eval.py` and `textDocument/definition` / `textDocument/hover` on `click.echo` (line 182 0-based, char 11).

**Observed — resolves into the venv's site-packages, NOT "unknown":**

```json
// definition response (id=2):
{"id":2,"result":[{"uri":"file:///Users/wesleyjinks/code/agent-eval/.venv/lib/python3.14/site-packages/click/utils.py",
                   "range":{"start":{"line":221,"character":4},"end":{"line":221,"character":8}}}]}
// hover response (id=3):
{"id":3,"result":{"contents":{"kind":"markdown","value":
  "```python\n(function) def echo(\n    message: Any | None = None,\n    file: IO[Any] | None = None,\n    nl: bool = True,\n    err: bool = False,\n    color: bool | None = None\n) -> None\n```\n---\nPrint a message and newline…"}}}
```

Server log confirms it adopted the venv interpreter: `Assuming Python version 3.14.4.final.0` (the 3.14 venv, NOT system 3.9.6).

**Task 6 implication:** the wrapped `{ "settings": { "python": { "pythonPath": … } } }` push envelope alone resolves third-party defs/hover into the venv's site-packages. **Task 6's `post_init_config` ships exactly this wrapped form** (NOT a bare `{ "python": { "pythonPath": … } }`).

---

## Gate 1b — repo override behavior — repo config WINS (shim must not fight it)

Throwaway repo `/tmp/lsp_spike/gate1b_repo/` with a VALID `pyrightconfig.json` pinning a DIFFERENT interpreter (an empty `.emptyvenv` with NO `click`), while the spike PUSHED the agent-eval venv (which HAS `click`):

```json
// /tmp/lsp_spike/gate1b_repo/pyrightconfig.json
{ "venvPath": ".", "venv": ".emptyvenv" }
// pushed (same wrapped envelope as 1a): settings.python.pythonPath = agent-eval/.venv/bin/python (has click)
```

**Observed — the repo `pyrightconfig.json` wins over the pushed pythonPath:**

```
log: Loading configuration file at /tmp/lsp_spike/gate1b_repo/pyrightconfig.json
log: Assuming Python version 3.9.6.final.0          // the repo config's default, NOT the pushed 3.14 venv
definition (import click)  -> null
hover (click.echo)         -> "(function) echo: Unknown"
diagnostic: Import "click" could not be resolved (reportMissingImports)
diagnostic: Type of "echo" is unknown (reportUnknownMemberType)
```

(An earlier variant that put `pythonPath` directly in `pyrightconfig.json` produced `Config contains unrecognized setting "pythonPath"` — pyrightconfig does NOT accept `pythonPath`; the valid override keys are `venvPath`+`venv` / `pythonVersion`. Even so, the mere presence of a repo config caused the pushed pythonPath to be ignored — the override wins either way.)

**Task 6 / §2 implication:** when a repo carries its own `pyrightconfig.json` / `pyproject [tool.basedpyright]`, that repo config WINS over the shim's pushed `didChangeConfiguration` pythonPath. **The shim must NOT fight a repo override** — it should push its venv pythonPath as a best-effort default and let an in-repo config take precedence (the §2 behavior). It must not assume its push always determines the interpreter.

---

## Gate 1c — no-venv fallback — stdlib resolves, third-party DEGRADES (must LOG, not silently empty)

Throwaway dir `/tmp/lsp_spike/gate1c_repo/` with NO venv and NO `pyrightconfig.json`; the spike sent NO settings push (and answered pulls `null`) → basedpyright falls back to `python3`-on-PATH (system 3.9.6). File imports both stdlib (`json`) and third-party (`click`).

**Observed — split behavior:**

```
log: Assuming Python version 3.9.6.final.0   // python3-on-PATH fallback
// STDLIB resolves:
definition (import json) -> [{"uri":"file:///…/basedpyright/dist/typeshed-fallback/stdlib/json/__init__.pyi", …}]
hover (json.dumps)       -> "(function) def dumps(obj: Any, *, skipkeys: bool = False, …)"   // full typed sig
// THIRD-PARTY degrades:
definition (import click) -> null
hover (click.echo)        -> "(function) echo: Unknown"
diagnostic: Import "click" could not be resolved (reportMissingImports)
diagnostic: Type of "echo" is unknown (reportUnknownMemberType)
```

Stdlib resolves via basedpyright's bundled typeshed; third-party (`click`) cannot be found because the fallback interpreter has no site-packages with it.

**Task 6 implication:** with no venv and no `--python-path`, resolution silently degrades to **incomplete third-party** (stdlib still works). The result is NOT an error — `definition` returns `null` and `hover` returns `Unknown` with a `reportMissingImports` diagnostic. **This is exactly the case the shim must LOG a WARNING for** (e.g. "no venv/pythonPath resolved; third-party symbols may be unresolved"), so a reviewer sees a `null`/`Unknown` is a config gap, not a real "no definition." Do NOT return a silent empty result.

---

## Gate 2 — readiness + no-progress settle — progress NEVER fired; short post-settings settle is the readiness signal

Tested several configurations against the agent-eval repo, watching 8–15s tails:

| Config | progress notifications | early `workspace/symbol` |
|---|---|---|
| NO `window/workDoneProgress` advertised, no file open (full repo) | **none** | issued ~300ms after settings → returned **0 symbols** at t=1045ms |
| `window/workDoneProgress: true` advertised, one file open | **none** | issued ~600ms after settings → returned **9 symbols** at t=1505ms |
| `diagnosticMode: workspace` pushed, 15s tail | **none** | — |

In every run the received message kinds were only `window/logMessage`, `workspace/configuration`, and the request responses — **zero** `pyright/beginProgress`, `pyright/endProgress`, `pyright/reportProgress`, `$/progress`, or `window/workDoneProgress/create`. (Those notification names DO exist in the basedpyright 1.39.8 dist — `grep` of `pyright-langserver.js` finds `pyright/beginProgress`, `pyright/endProgress`, `pyright/reportProgress`, `$/progress`, `window/workDoneProgress/create` — but they did not fire for these small/medium analyses.)

The **no-progress request** test (the core of the gate): `workspace/symbol` issued ~0.3–0.6s after `initialized`+settings (before any progress cycle) **RETURNED** — with content (9 symbols) once a file was open; with 0 only because no document was open yet (basedpyright's `workspace/symbol` draws from open/indexed docs). It did NOT block for ~30s waiting on a progress cycle.

**Task 6 implication (REVISES the plan's progress assumption):**
1. Readiness must **NOT** require a `beginProgress`→`endProgress` cycle — for small analyses that cycle **never fires**, so waiting for `endProgress` would hang to the full timeout for the common case. Treat a **short post-settings settle** (a few hundred ms after `initialized`+`didChangeConfiguration`) as ready; a request issued then returns.
2. The **begin-without-end stall** is therefore the dominant case, not an edge case: since `beginProgress` frequently never arrives (and when it does, `endProgress` is not guaranteed within the window), the readiness logic must NOT key off "saw begin, now wait for end" — that would force the full timeout. Use the settle + treat the first successful response as the liveness proof, and cap any progress wait so a begin-without-end cannot pin the full 30s.

---

## Gate 3 — `--lang auto` detection predicates — all cases validated

Pure host-dir predicate checks (markers: Rust = `Cargo.toml`; Python = `setup.py` / `setup.cfg` / `requirements*.txt` / pyproject with a REAL section `[project]`/`[tool.poetry]`/`[tool.setuptools…]`/`[tool.hatch…]`/`[tool.flit…]`/`[tool.pdm…]`; else recursive `.py`-scan excluding `.venv venv .git target node_modules build vendor` + hidden dirs; both markers → ambiguous→refuse). Validated against real host dirs AND synthetic edge cases:

| Dir | Markers | Predicate result |
|---|---|---|
| `~/code/a2a-bridge` (real) | `Cargo.toml` only | **rust** |
| `~/code/agent-eval` (real) | `pyproject.toml` with `[project]` | **python** (marker) |
| `~/code/a2a-local-bridge` (real) | `pyproject.toml` with `[project]` | **python** (marker) |
| `gate3/tooling_only` (`[tool.black]` only, no .py) | tooling-only pyproject | **UNKNOWN→refuse** (not python by that marker; .py-scan empty) |
| `gate3/tooling_only_withpy` (`[tool.ruff]` only + `main.py`) | tooling-only pyproject + .py | **python** via .py-scan (`main.py`) |
| `gate3/scan_excluded` (.py only under `.venv/node_modules/target/.git`) | none | **UNKNOWN→refuse** (all .py excluded) |
| `gate3/scan_real` (`src/app.py`, `.venv` ignored) | none | **python** via .py-scan (`src/app.py`) |
| `gate3/ambiguous` (`Cargo.toml` + `[project]` pyproject) | rust + python | **AMBIGUOUS→refuse** |
| `gate3/scan_hidden` (.py only under `build/vendor/.hidden`) | none | **UNKNOWN→refuse** (build/vendor/hidden excluded) |

A tooling-only `[tool.black]` pyproject is correctly NOT treated as a python marker (it falls through to the .py-scan); the .py-scan correctly excludes `.venv venv .git target node_modules build vendor` and hidden dirs.

**Task 4 implication:** the predicates Task 4 will implement are sound against the actual repos on this host. The two refuse paths are real and reachable (ambiguous rust+python; and no-marker-no-scannable-.py). The "real section" check must distinguish `[project]`/build-backend sections from tooling-only sections (`[tool.black]`/`[tool.ruff]` alone do NOT mark python).

---

## Decisions for the plan (summary)

1. **Task 6 ships the WRAPPED settings envelope** `{ "settings": { "python": { "pythonPath": "<venv>/bin/python" } } }` via `workspace/didChangeConfiguration` — proven in Gate 1a. basedpyright also PULLS (`workspace/configuration`); the shim's host wiring should be prepared to answer the pull too (harmless to also return the python section), but the push alone is sufficient for resolution.
2. **§2 — do not fight a repo override.** A repo `pyrightconfig.json` / `pyproject [tool.basedpyright]` wins over the pushed pythonPath (Gate 1b). Push best-effort; let in-repo config take precedence.
3. **No-venv → LOG a warning** (Gate 1c). `null`/`Unknown` + `reportMissingImports` with no venv is a config gap, not "no definition"; the shim must surface it, not return a silent empty result. Stdlib still resolves via bundled typeshed.
4. **Readiness = short post-settings settle, NOT a progress cycle** (Gate 2). For small/medium analyses basedpyright emits NO progress notifications; a request issued shortly after `initialized`+settings returns. Cap any progress wait so a begin-without-end stall cannot force the full 30s.
5. **`--lang auto` predicates validated** (Gate 3) including the tooling-only-pyproject, excluded-dir-scan, and ambiguous-refuse cases.

## Reproduction / artifacts

All throwaway scripts and temp dirs live under `/tmp/lsp_spike/` (driver `driver.py`, scenario generators `gen_1a/1b/1c/2.py`, captured transcripts `out_1a/1b/1c/2*.json`, predicate ref `detect.py`, throwaway repos `gate1b_repo/ gate1c_repo/ gate3/`). No production code landed; nothing was written into `~/code/agent-eval` or its venv. basedpyright stays installed as an isolated `uv` tool.
