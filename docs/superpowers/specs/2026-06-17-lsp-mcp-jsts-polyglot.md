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
  does). **Detection marker = `tsconfig.json` ONLY** (NOT bare `package.json`, NOT a `.ts/.js` scan).
  Rationale (Opus/codex MINOR): `package.json` is present in a huge fraction of repos that are *primarily*
  rust/python (tooling, husky, docs sites) — making it a TS marker would flip those to **Ambiguous** and
  REGRESS their auto-detect. `tsconfig.json` is a strong, low-noise TS signal. A tsconfig-less pure-JS
  project needs explicit `--lang typescript` (acceptable; documented). **Wiring:** `detect()` is a
  count-of-markers over a per-language boolean ARRAY (`lang.rs:~39`) — add `is_ts = tsconfig.json exists`
  to that array AND to `detect_lang`'s ambiguity message; `select_profile` (bin) dispatches on
  `lsp_mcp::lang::detect`, so this is the real selection path (not just the lib.rs match).
- **`ts_config(repo)` → `LangServerConfig`** (the swappable seam):
  - `program_argv` = `[server, "--stdio"]` where `server` = `resolve_lsp_server(env LSP_MCP_TS_SERVER else
    "typescript-language-server")`. **Swappability (M1):** the lever is the env, but in the CONTAINERIZED
    path the env reaches lsp-mcp ONLY via the profile `lsp_env` (the stripped-env rule) — so "swap with no
    rebuild" = set `lsp_env = { LSP_MCP_TS_SERVER = "vtsls" }` in the TS profile (a config edit). A server
    needing different args/readiness is a one-function edit in `ts_config`.
  - `is_project_root` = `tsconfig.json || package.json` exists (the explicit-`--lang` guard can be more
    lenient than auto-detect — a user who passes `--lang typescript` at a package.json-only repo is fine).
  - `initialize_params` = standard (processId, rootUri, workspace/symbol + hierarchical documentSymbol,
    workspaceFolders) — same shape as go_config. **If** typescript-language-server can't auto-find the
    `typescript` lib under the stripped env, pass `initializationOptions.tsserver.path` (→ the global
    typescript `lib`/`tsserver.js`, a real path) HERE — NOT in `lsp_env` (it must survive the env strip as
    a literal in the mint params). Impl-spike the auto-discovery first; add only if needed.
  - `new_readiness` = a **settle-based** machine (typescript-language-server's `$/progress` is unreliable;
    the no-progress settle is the safe signal — proven for gopls/pyright). **`PyrightReady` and `GoplsReady`
    are already byte-identical** (`{began, active, settled_at}` + identical `settled_no_progress`) — TS
    would be a 3rd copy, so **extract a shared `SettleReady` struct** and have pyright/gopls/ts use it
    (wire it into `Readiness`, `handshake`, and `settled_no_progress` — codex: it's real wiring, not docs).
  - `post_init_config`: None (tsserver config goes in `initialize_params` per above).
- **lib.rs dispatch:** add `"typescript" | "ts" | "javascript" | "js" => Lang::TypeScript` to the
  `--lang` match and `Lang::TypeScript => ts_config(&repo)?` to the config match; extend the
  `--lang must be …` error string.

### Layer 2 — Toolchain image (`npm install -g` for TS — a reasoned deviation from python's mise)

`toolchain.Containerfile`: **`npm install -g typescript-language-server@5.3.0 typescript@6.0.3`** — NOT
mise. **Why deviate (VALIDATED):** mise installs each npm package in an ISOLATED dir, so
typescript-language-server can't find `typescript` as a sibling → tsserver discovery fails. `npm install
-g` co-locates them in `/usr/local/lib/node_modules` (siblings → tsls auto-discovers tsserver, **no
`tsserver.path` needed**) AND puts REAL binaries on `/usr/local/bin` (env-trap compliant, no shims — node
is the image base). **Spiked:** under a fully stripped env (`PATH=/usr/local/bin:/usr/bin:/bin HOME=/root`),
`typescript-language-server --stdio` responds to `initialize` with capabilities + no error, and
`tsls/tsc/node/npm --version` all resolve. So `ts_config` needs no `initializationOptions.tsserver.path`
and `lsp_env={}` is honest. (This satisfies the user's env-trap requirement — real binaries on PATH — which
was the actual goal; mise's value was uniformity for python's *mixed* ecosystem, not pure-npm tooling.)

### Layer 3 — The `typescript` `[[languages]]` profile (both configs, mirrored)

```toml
[[languages]]
id = "typescript"
# SPIKED fetch (spike 2): `npm ci --prefix /` FAILS (EUSAGE — it looks for the lock at the prefix). The
# working shape copies the manifests to / then `npm ci` at / → populates the mounted /node_modules vol.
# --ignore-scripts: a warm fetch must NOT run arbitrary dep postinstall scripts (codex hardening). Best-
# effort (|| true) so a no-lock/no-deps repo still gets a usable bare server (degrades, never crashes).
fetch = "cp /work/package.json /work/package-lock.json / 2>/dev/null && cd / && { npm ci --ignore-scripts || npm install --ignore-scripts || true; }"
warm_cache = "a2a-impl-lsp-cache-ts"
dep_cache_path = "/node_modules"     # the root-walk target (spiked); mounted :ro for nav, rw for fetch
verify_cache_path = "/node_modules"  # B1 FIX: verify gets its OWN /node_modules vol (the verify container
                                     # does NOT mount the warm dep cache + the clone is :ro), so the verify
                                     # cmds install deps into it, then tsc resolves via the root-walk.
fetch_env = { npm_config_cache = "/npmcache" }
verify_env = { npm_config_cache = "/cache/npm" }
# Swappable seam (M1): ts_config reads LSP_MCP_TS_SERVER (else "typescript-language-server"). To swap the
# CONTAINERIZED server with NO rebuild, set it HERE (lsp_env reaches the in-container lsp-mcp via the
# stripped-env path; a host/image env would NOT — see containerized-mcp-env-trap). Default: unset → tsls.
# (tsserver's typescript-lib path, if needed, goes in initialize_params, NOT lsp_env — see Layer 1.)
lsp_env = {}   # e.g. { LSP_MCP_TS_SERVER = "/usr/local/bin/vtsls" } to swap
[[languages.verify]]
# B1: verify installs deps into the /node_modules verify vol (copy manifests + npm ci), THEN tsc resolves
# via the root-walk. Mirrors python's verify-venv-into-/cache.
name = "typecheck"
cmd  = "cp /work/package.json /work/package-lock.json / 2>/dev/null && (cd / && npm ci --ignore-scripts || npm install --ignore-scripts) && cd /work && tsc --noEmit"
[[languages.verify]]
name = "lint"
# eslint is project-local (in /node_modules/.bin) + config-driven; guard on a config + put the bin on PATH.
cmd  = "if ls .eslintrc* eslint.config.* >/dev/null 2>&1; then PATH=/node_modules/.bin:$PATH eslint .; else echo 'no eslint config; skip'; fi"
[[languages.verify]]
name = "test"
cmd  = "npm test --if-present"   # honors the project's test script; no-op if absent
```
Both spike-2-validated fetch/verify shapes are concrete (not hand-waved); the keystone `/node_modules`
mount is spiked. **Egress (codex MAJOR):** the warm/verify npm install needs the npm registry, which the
verify-egress allowlist does NOT currently have → add `(^|\.)npmjs\.org$` (+ any npm CDN redirect host) to
`deploy/containers/tinyproxy.verify.filter` and restart `a2a-verify-proxy` (same as the PyPI add for python).
**Note:** `apply_warm_lsp` also unconditionally appends a `/lsp-target` mount (used by rust's CARGO_TARGET_DIR);
TS doesn't use it — harmless, part of the unchanged-reuse story.

### Detection + readiness

Detection = **`tsconfig.json`** (Layer 1; avoids the package.json regression). Readiness = **settle-based
via the extracted shared `SettleReady`** (pyright+gopls+ts), wired into `Readiness`/`handshake`/
`settled_no_progress`.

## Definition of Done (live gate)

A **deps-bearing TS fixture** under `allowed_cwd_root` (e.g. imports a typed package like `lodash`, with
`package.json` + `package-lock.json` + `tsconfig.json`, NO committed `node_modules`):
1. **3rd-party nav via /node_modules:** `run-workflow c2b-nav --session-cwd <ts fixture>` → tsserver returns
   a **type-resolved** signature whose type involves the installed dep (proves the warmed `/node_modules`
   resolved via the root-walk under locked egress). lsp call log shows the call.
2. **`:ro` safety:** fixture unmutated (`git status --porcelain` empty).
3. **Cache:** one `-cache-ts-<hash>` node_modules volume per fixture, reused.
4. **Verify resolves deps:** the `typecheck` step (install-into-/node_modules + `tsc --noEmit`) is GREEN on
   the deps-bearing fixture (proves the verify container resolves 3rd-party types — B1); `npm test
   --if-present` runs.
5. **Swappable seam (via lsp_env):** set `LSP_MCP_TS_SERVER` **in the TS profile's `lsp_env`** to the same
   server by absolute path and confirm it's honored in-container (proves the seam reaches the stripped-env
   lsp-mcp — NOT a host env var; M1). No second server needed.
6. **Degrade (nav AND verify):** a TS repo with no resolvable lock → bare server; local + lib-type nav works
   (no crash, 3rd-party unresolved); the verify install best-efforts (no crash). Mirrors python's bare-venv.
7. **No regression:** rust/go/python nav still work; **a rust/python repo with a tooling-only `package.json`
   (no tsconfig) still auto-detects as rust/python** (the tsconfig-only marker — Opus m2); host-only
   workflows pay no warm cost.
8. **env-trap shim guard:** in a stripped env (`env -i PATH=/usr/local/bin:/usr/bin:/bin …`),
   `typescript-language-server --version`, `tsc --version`, **and `node`/`npm --version`** resolve (no mise
   activation).

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
- **`/node_modules` root-walk + fetch (BOTH spiked, low):** spike doc
  `docs/superpowers/spikes/2026-06-17-jsts-node-modules-resolution.md` — tsc resolves from `/node_modules`
  (resolution) AND `cp manifests / && npm ci` populates it (fetch). Confirm with the REAL
  typescript-language-server in the gate (same resolver). A repo with its OWN committed `node_modules`
  shadows `/node_modules` — acceptable.
- **npm egress:** the warm/verify install needs `registry.npmjs.org` — NOT in the verify-egress allowlist
  today (codex MAJOR). Add `(^|\.)npmjs\.org$` to `tinyproxy.verify.filter` + restart `a2a-verify-proxy`.
- **env trap:** tsserver/tsc/node/npm are node CLIs at `/usr/local/bin` (on the stripped PATH). Real-binary
  symlinks, no shims; tsserver-lib pointer (if needed) via `initialize_params`. env-dump diagnostic ready.

## Review status

Dual-reviewed before planning: **codex gpt-5.5 xhigh** (correctness) + **Opus 4.8** (architecture). Both
`fix-then-ship`, no redesign — the keystone (`/node_modules` fixed-path reuse, S6 sidestep, new-language
seam) confirmed sound against the real code. **All findings folded above:** B1 (verify can't see
`/node_modules` → `verify_cache_path=/node_modules` + install-then-tsc), the wrong fetch (`npm ci --prefix /`
→ copy-manifests + `npm ci --ignore-scripts`, spiked), the **npm egress allowlist gap** (codex — new), the
swappable seam reaching the stripped env only via `lsp_env` (M1), the eslint PATH/guard, the tsserver-lib
path via `initialize_params`, detection→`tsconfig.json` (avoid the rust/python regression), and extracting
a shared `SettleReady`. Both spikes committed. **Net new code: the lsp-mcp `Lang::TypeScript` + `ts_config`
+ `SettleReady` extraction; everything else is config (profile + egress) + image.**

## Constraints (carried)

sonnet implementor; codex for high-risk (the lsp-mcp language) + final review, Opus per-task/arch;
`max_attempts = 3`; reviewers judge **intent, not verbatim**.
