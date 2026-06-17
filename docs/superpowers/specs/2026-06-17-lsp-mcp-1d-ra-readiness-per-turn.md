# #1d — rust-analyzer readiness under per-turn serve/run-workflow

**Status:** design (approach approved: "warm-on-spawn + degrade")
**Date:** 2026-06-17
**Track:** LSP-MCP nav-hardening (follows #1 a+c+b). See [[lsp-mcp-c2-polyglot]].

## Problem

Under the per-turn `container_rw` path (`run-workflow` / `serve`), in-container rust nav
fails: the codex/sonnet agent reports "no lsp tool". gopls "worked" in the C2b gate only
because that fixture had zero external deps.

### Root cause (code-grounded, deterministic — not "just slow")

1. **"No lsp tool" is a misnomer.** `tools/list` is static (`mcp/mod.rs:17 tool_schemas()`)
   and always advertises the 7 nav tools. The phrase means *the tool **call** returned
   empty/errored*. On a cold index, `ensure_ready(30s)` (`mcp/mod.rs:128`) times out;
   `wait_ready` returns `Ok` best-effort (`lsp/mod.rs:448`) and queries a not-quiescent RA
   → empty hits.

2. **The per-turn path never makes RA ready.** The warm `implement` path
   (`build_warm_impl`, `main.rs:1404+`) does three things the per-turn path
   (`container_rw_cfg_from_entry`, `main.rs:429`) does **not**:
   - `warm_lsp_deps_step` — fetch deps into a `/cargo` cache via the **registries-only**
     verify-egress.
   - mount `/cargo` (RO) + a `/lsp-target` cache at runtime.
   - `apply_lsp_env` — inject `CARGO_HOME=/cargo` + `CARGO_NET_OFFLINE=true` into the lsp
     server's `McpServerSpec`.

   The per-turn path passes `entry.mcp`/`entry.sandbox.volumes` straight through.

3. **The agent egress is locked.** The `impl` agent runs on `a2a-egress-internal` behind
   `a2a-egress-proxy` (anthropic-only allowlist). With no offline env + no warm cache, RA
   runs `cargo metadata` → tries crates.io → **proxy blocks it** → metadata fails → never
   quiescent → empty nav. *Deterministic* for any deps-bearing rust (and any deps-bearing Go).

## Approach: warm-on-spawn (run-workflow) + universal degrade

Two parts. Part A is universal and low-risk; Part B makes nav actually work on the path
that is exercised for nav today (`run-workflow`, the C2b/dogfood gate).

### Part A — Degrade (lsp-mcp, benefits ALL paths incl. serve)

- **Configurable readiness budget.** `ensure_ready` timeout reads `LSP_MCP_READY_SECS`
  (default raised 30 → 90; a cold warm-cached RA index can exceed 30s). Pure parse helper,
  unit-tested.
- **Honest not-ready signal.** `wait_ready` already loops to `is_ready() || settled`; add a
  return that distinguishes "became ready" from "timed out". When a tool call runs against a
  *not-ready* RA, return `isError:true` with a clear message —
  `"rust-analyzer is still indexing (or could not index offline); retry shortly"` — instead
  of an empty hit list the agent misreads as "no tool". Genuine empty results (RA ready, no
  match) stay as today.
- **Scope of the degrade.** This covers the **detected-but-not-ready** case (a language WAS
  detected; RA is slow/offline). A *truly undetected* repo is NOT covered: `lsp-mcp --lang auto`
  calls `detect_lang(&repo)?` and **exits before serving** (`crates/lsp-mcp/src/lib.rs:37`), so
  there is no MCP server to return a message — that is pre-existing, acceptable behavior (no
  language → no nav server). The degrade is for *detected* repos whose RA can't index in time.
- No change to the RustRa/Pyright/Gopls readiness machines.

### Part B — Warm-on-spawn for run-workflow (entry pre-mutation, NO cross-crate change)

`run-workflow` stamps a fixed `--session-cwd` into every entry's `session_cwd` *before*
registry build. So we warm + mutate each `container_rw` entry up front, and the existing
per-turn `container_rw_cfg_from_entry` picks up the mutated entry unchanged.

In `run_workflow_cmd`, after config parse + snapshot, for each `container_rw` agent entry:
1. `select_profile(&cfg, &LangArg::Auto, session_cwd)` — auto-detect language from the
   stamped cwd. `None` (→ `--lang none` / undetected) → skip warming (degrade covers it).
2. `warm_lsp_deps_step(verify_cfg, profile, repo=session_cwd, clone=session_cwd)` — but with
   a **read-only repo mount** (see constraint below). Returns the cache vol on success.
3. A new shared helper `apply_warm_lsp(mcp, volumes, profile, warm_cache_vol, repo)` —
   extracted byte-for-byte from `build_warm_impl` (`main.rs:1434–1458`): `apply_lsp_env` +
   `/cargo` RO mount (only when warm succeeded) + `/lsp-target` cache mount keyed on the
   source repo. Called by BOTH `build_warm_impl` and the new run-workflow pre-mutation so the
   two paths can't drift.

**Read-only fetch constraint.** `compose_warm_fetch` mounts the repo `{clone}:/work` **`:rw`**
(`implement.rs:96`). Fine for `implement`'s disposable quarantine clone; on the run-workflow
path the "clone" is the user's *real* repo, so add a `read_only: bool` param → `:ro` for
per-turn. `cargo fetch --locked` / `go mod download` read the lock and write only to the
cache vol; if a lock is absent the fetch degrades (Part A covers nav).

### Out of scope (documented follow-up: "#1d-serve")

`serve`'s per-request cwd arrives at session-mint (inside bridge-core/bridge-container),
*after* spawn — so warm-on-spawn for serve needs a cross-crate hook into
`ContainerRwBackend::open_inner` (a `RwWarmHook` injected via `ContainerRwConfig`). That is a
bigger, higher-risk change to the crate just touched for #1b, and serve-with-rust-nav is not
the exercised path. Deferred. **serve still benefits from Part A (degrade)** — it returns the
honest "still indexing / not available offline" message instead of misleading empty hits.

## Decomposition & DoD

- **Task 1 (lsp-mcp degrade):** configurable `LSP_MCP_READY_SECS` + not-ready→clear message.
  Unit tests for the parse helper + the not-ready branch.
- **Task 2 (shared `apply_warm_lsp` helper):** extract from `build_warm_impl`; prove
  byte-for-byte equivalence (the implement path is unchanged). Unit test the mount/env shape.
- **Task 3 (`compose_warm_fetch` `:ro` param):** add `read_only`; implement path passes
  `false`, per-turn passes `true`. Byte-for-byte test for the existing (`false`) shape +
  a new test asserting `:ro` when `true`.
- **Task 4 (run-workflow pre-mutation):** wire profile-select + warm + `apply_warm_lsp` into
  `run_workflow_cmd` for `container_rw` entries. Skip cleanly on `None` profile.

**Acceptance (live gate):** `run-workflow <nav workflow> --session-cwd <deps-bearing rust
repo> --lang auto` → in-container RA reaches quiescent and a nav tool (`hover`/`definition`)
returns a **type-resolved** result (not empty, not "no lsp tool"); the warm fetch goes
through the **registries-egress** (agent stays locked) and does **not** mutate the user's
repo (`:ro`); the cache is keyed on the source repo (one volume, reused, no per-run leak); a
**detected-but-not-ready** RA (warm cold + `LSP_MCP_READY_SECS=2`) returns the honest degrade
message instead of empty hits; the `implement` warm path and codex in-container nav are unchanged.

## Constraints carried from the session

- Implementor = **sonnet**; reviewer = **codex gpt-5.5 (high-risk + final only)** + **Opus
  4.8** (per-task + plan). `max_attempts = 3`. Reviewer prompts judge **intent, not verbatim**.
- Dogfood: implement via the bridge's own warm `implement` loop where possible; controller-side
  for anything that touches the very path being changed (chicken-and-egg).
