# JS/TS in the containerized polyglot implementor — design spec

**Status:** design (approved approach: typescript-language-server behind a swappable seam + full parity + mise + uv-style reuse)
**Date:** 2026-06-17
**Track:** LSP-MCP polyglot. Slice 2 of 2 (Python was slice 1). Follows C2a (rust+go) + #1 + python. See [[lsp-mcp-c2-polyglot]], [[containerized-mcp-env-trap]].

## Goal

JS/TS parity in the containerized polyglot implementor: in-container semantic nav + a warm dep cache +
verify, selected per-session-cwd. Unlike python (which already had a basedpyright nav layer), **this is
the greenfield slice — a NEW lsp-mcp language** (`Lang::TypeScript`, covering .ts/.tsx/.js/.jsx; one
server handles both). The LSP server (typescript-language-server) is kept behind a **swappable seam** —
the user flagged TS LSP tooling will churn in 6-12 months (vtsls, tsgo, …).

## Keystone finding (SPIKED + VALIDATED, 2026-06-17)

The hard question was: TS deps live in `node_modules`, which tsserver finds by **walking up from the
source file** — unlike rust/go/python's fixed-absolute caches. But the walk reaches **`/node_modules` at
the filesystem root**. Spike: `lodash`+`@types/lodash` installed ONLY at `/node_modules` (repo has no
local node_modules) → `tsc --noEmit` resolves the type (trace: *"Found 'package.json' at
'/node_modules/lodash/package.json'"*). tsserver shares the resolver. **⟹ `dep_cache_path="/node_modules"`
(fixed absolute) works, reusing the existing `cache_binding`/`apply_warm_lsp` machinery UNCHANGED** — no
cwd-relative mount, no S6 nesting issue, no new bridge capability. TS becomes a config+image+lang slice
like python, NOT a bridge-machinery change.

## Architecture (mirrors rust/go/python, + the one new language)

### Layer 1 — lsp-mcp: the new `Lang::TypeScript` (THE greenfield code)

`crates/lsp-mcp/src/lang.rs` + `lib.rs`:
- **`Lang::TypeScript`** (as_str `"typescript"` — the profile id must match; covers JS too since tsserver
  does). `detect()` adds a TS marker: **`package.json` OR `tsconfig.json` at the root** (NOT a `.ts/.js`
  scan — too noisy: tooling configs, `node_modules`). Two-or-more markers → Ambiguous (existing rule; a
  fullstack repo needs explicit `--lang`).
- **`ts_config(repo)` → `LangServerConfig`** (the swappable seam):
  - `program_argv` = `[server, "--stdio"]` where `server` = `resolve_lsp_server(env LSP_MCP_TS_SERVER else
    "typescript-language-server")`. **Swappability:** point `LSP_MCP_TS_SERVER` at `vtsls`/`tsgo` (no
    rebuild) for the common case; a server needing different args/readiness is a one-function edit. Doc this.
  - `is_project_root` = `package.json || tsconfig.json` exists.
  - `initialize_params` = standard (processId, rootUri, workspace/symbol + hierarchical documentSymbol,
    workspaceFolders) — same shape as go_config.
  - `new_readiness` = a **settle-based** machine (like gopls/pyright — typescript-language-server's
    `$/progress` is unreliable; the no-progress settle is the safe readiness signal). Add a `Ts` readiness
    variant mirroring `GoplsReady` (or reuse a shared settle struct).
  - `post_init_config`: likely None; if tsserver needs `initializationOptions` (e.g. `tsserver` path /
    `maxTsServerMemory`), set them in `initialize_params`. (Spike during impl.)
- **lib.rs dispatch:** add `"typescript" | "ts" | "javascript" | "js" => Lang::TypeScript` to the
  `--lang` match and `Lang::TypeScript => ts_config(&repo)?` to the config match; extend the
  `--lang must be …` error string.

### Layer 2 — Toolchain image (mise, real-binary symlinks — same as python)

`toolchain.Containerfile`: `mise use -g -y npm:typescript-language-server@<pin> npm:typescript@<pin>`
(node is already the image base). Symlink the REAL binaries into `/usr/local/bin`:
`typescript-language-server`, `tsc`, `tsserver` (NEVER mise shims — [[containerized-mcp-env-trap]]).
typescript-language-server needs the `typescript` package (tsserver) — installing `npm:typescript`
globally provides it; confirm the server finds tsserver (it bundles/auto-discovers, else point it via
`--tsserver-path` / `initializationOptions.tsserver.path`).

### Layer 3 — The `typescript` `[[languages]]` profile (both configs, mirrored)

```toml
[[languages]]
id = "typescript"
# Install the project's deps into the /node_modules VOLUME so tsserver resolves 3rd-party types via the
# root-walk (spiked). npm ci needs package-lock.json; --prefix / targets /node_modules (validate during impl;
# fallback `npm install`). Best-effort (|| true) so a no-lock repo still gets a usable (bare) server.
fetch = "cd /work && { npm ci --prefix / || npm install --prefix / || true; }"   # /work = the mounted repo; refine at impl
warm_cache = "a2a-impl-lsp-cache-ts"
dep_cache_path = "/node_modules"     # the root-walk target (spiked); mounted :ro for nav, rw for fetch
verify_cache_path = "/cache"
fetch_env = { npm_config_cache = "/npmcache" }
verify_env = { npm_config_cache = "/cache/npm" }
# Per the env trap: tsserver is node-based; node is on /usr/local/bin (codex's stripped PATH). No interpreter
# pointer needed (deps resolve via /node_modules root-walk). Add any tsserver env here if impl finds a gap.
lsp_env = {}
[[languages.verify]]
name = "typecheck"
cmd  = "tsc --noEmit"            # resolves via /node_modules root-walk (spiked)
[[languages.verify]]
name = "lint"
cmd  = "test -f .eslintrc* && eslint . || echo 'no eslint config; skip'"   # eslint is project-config-driven
[[languages.verify]]
name = "test"
cmd  = "npm test --if-present"   # honors the project's test script; no-op if absent
```
(Exact `fetch`/`verify` strings are impl-validated in-container like python's uv commands were — the
keystone `/node_modules` mount is what's load-bearing + spiked.)

### Detection + readiness

Detection above (package.json/tsconfig.json). Readiness = settle-based (new `Ts` variant). No other change.

## Definition of Done (live gate)

A **deps-bearing TS fixture** under `allowed_cwd_root` (e.g. imports a typed package like `lodash`, with
`package.json` + `package-lock.json` + `tsconfig.json`, NO committed `node_modules`):
1. **3rd-party nav via /node_modules:** `run-workflow c2b-nav --session-cwd <ts fixture>` → tsserver returns
   a **type-resolved** signature whose type involves the installed dep (proves the warmed `/node_modules`
   resolved via the root-walk under locked egress). lsp call log shows the call.
2. **`:ro` safety:** fixture unmutated (`git status --porcelain` empty).
3. **Cache:** one `-cache-ts-<hash>` node_modules volume per fixture, reused.
4. **Verify:** `tsc --noEmit` resolves deps (green) in-container; `npm test --if-present` runs.
5. **Swappable seam:** `LSP_MCP_TS_SERVER` overrides the server binary (smoke: set it to the same binary by
   absolute path and confirm it's honored — proves the seam without needing a second server installed).
6. **Degrade:** a TS repo with no resolvable lock → bare server; local + lib types nav works (no crash);
   3rd-party shows unresolved (mirrors python's bare-venv degrade).
7. **No regression:** rust/go/python nav still work; host-only workflows pay no warm cost.
8. **env-trap shim guard:** in a stripped env, `typescript-language-server --version` (or `--stdio` probe)
   + `tsc --version` resolve via `/usr/local/bin` (no mise activation).

## Out of scope (follow-ups)

- **Monorepos / workspaces** (multiple/nested node_modules, pnpm workspaces) — the single-`/node_modules`
  root-walk covers a single-package repo; workspace-local resolution is a fast-follow.
- **pnpm/yarn-PnP** — npm is the default installer; pnpm's symlink store + Yarn PnP resolve differently.
- **eslint/test depth** — defaults are project-config-driven (honor `.eslintrc`/`npm test`); not a curated
  ruleset.
- **`serve` per-request warm** — still deferred (#1d).

## Risks

- **Greenfield lsp-mcp language (highest):** `Lang::TypeScript` + `ts_config` + a readiness machine are new
  code. Readiness: typescript-language-server's progress is unreliable → settle-based (proven safe for
  gopls/pyright). Spike the server handshake + a nav (hover/definition) during impl before the full gate.
- **`/node_modules` root-walk (spiked, low):** validated with tsc; confirm with the REAL
  typescript-language-server in the gate (same resolver, expected pass). A repo with its OWN committed
  `node_modules` would shadow `/node_modules` — acceptable (then the repo's own deps are used).
- **fetch into /node_modules:** `npm ci --prefix /` targeting the mounted vol needs in-container validation
  (like python's `uv pip` VIRTUAL_ENV detail) — impl-time, before the gate.
- **env trap:** tsserver/tsc are node CLIs; node is at `/usr/local/bin` (on the stripped PATH). Real-binary
  symlinks, no shims. The env-dump wrapper diagnostic is ready.

## Constraints (carried)

sonnet implementor; codex for high-risk (the lsp-mcp language) + final review, Opus per-task/arch;
`max_attempts = 3`; reviewers judge **intent, not verbatim**. This spec gets a **codex xhigh** + Opus
spec-review before planning.
