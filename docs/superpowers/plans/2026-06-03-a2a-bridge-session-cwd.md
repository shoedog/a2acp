# Session cwd / Per-Request Repo Targeting — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Decouple the ACP `session/new` working directory from the host process-spawn directory, and let a `message/send` target a repo per request via `a2a-bridge.cwd` metadata — so one `serve` drives agents against many codebases (host or containerized) without the identical-path mount hack.

**Architecture:** `cwd` rides the **existing per-session config mechanism**. `EffectiveConfig` (the struct `configure_session` stashes and `ensure_session` reads at ACP mint, alongside model/mode/effort) gains an optional `cwd`; the request's `a2a-bridge.cwd` becomes an `AgentOverride.cwd` layered into that `EffectiveConfig`. A new static `session_cwd` agent-config field feeds the `AcpConfig` fallback. The resolution chain at mint is **request → session_cwd → cwd → "."**. The detached/workflow path persists the per-task cwd (additive `tasks.session_cwd` column, W3b migration pattern) and restores it on resume; the executor forwards it as an opaque param (purity preserved).

**Tech Stack:** Rust, the existing `AgentBackend`/`EffectiveConfig`/registry/`TaskStore`/`WorkflowExecutor`, rusqlite, ACP §11A session semantics, A2A message metadata.

**Spec:** `docs/superpowers/specs/2026-06-03-a2a-bridge-session-cwd-design.md`.
**Branch:** `feat/session-cwd` off `main`.

**Grounding facts (confirmed against the code):**
- `AgentBackend::configure_session(&self, session, cfg: &EffectiveConfig)` (bridge-core/ports.rs:41) **stashes** the per-session effective config; `AcpBackend::ensure_session` (bridge-acp/acp_backend.rs:851) reads the stash at lazy ACP mint and currently derives `(mode, model, effort)` from it, but takes **`cwd` only from `self.config.cwd`** (the static `AcpConfig`, acp_backend.rs:854-858) → `Self::new_session_request(cwd)` (acp_backend.rs:345, called acp_backend.rs:~895). `forget_session` drops the stash.
- `EffectiveConfig` is the domain config type (bridge-core/src/domain.rs) carrying model/effort/mode today.
- Per-request overrides parse from `a2a-bridge.{model,effort,mode}` into `AgentOverride` → `RoutedCall.overrides` (server.rs:321-325); the metadata parser reads `params.message.metadata` for `a2a-bridge.skill/agent/model/effort/mode` (server.rs:2277-2330). `effective_config` layers overrides onto the entry defaults before `configure_session`.
- Agent config: `AgentEntryToml` (bin/a2a-bridge/src/config.rs) has `cwd: Option<String>` (~169), `model`/`model_provider` (~160), mapped to the registry entry (~314-320); the spawn fn (bin/a2a-bridge/src/main.rs:153,430) builds the host child cwd AND the `AcpConfig`. `[registry] allowed_cmds` exists; there is no `allowed_cwd_root` yet.
- W3b additive-column pattern: `tasks` migration via `migrate_tasks_columns` (pragma table_info-guarded ALTER, bridge-store/src/sqlite.rs), `TaskRecord` additive fields ripple to every literal (server.rs detached arm, workflow_producer.rs tests, sqlite.rs tests, task_store.rs tests). `run_from(graph,input,run_id,cancel,seed)` (executor.rs); `spawn_detached_workflow(srv,task,input,graph,run_id,token,seed)` (server.rs); `resume_working_tasks(srv,cap)` (server.rs).
- Tests: workflow_producer.rs under `-p bridge-a2a-inbound`; `-p a2a-bridge` has no `--lib`. Coverage floors: workspace 85, bridge-core 90, bridge-workflow 90 (after `cargo llvm-cov clean --workspace`).

---

## File Structure
- **Modify** `crates/bridge-core/src/domain.rs` — `EffectiveConfig` gains `cwd: Option<String>`.
- **Modify** `crates/bridge-acp/src/acp_backend.rs` — `ensure_session` reads `cwd` from the stash (fallback static); `AcpConfig` static cwd sourced from `session_cwd`.
- **Modify** `bin/a2a-bridge/src/config.rs` — `AgentEntryToml.session_cwd` + global `allowed_cwd_root`.
- **Modify** `bin/a2a-bridge/src/main.rs` — spawn fn: host child cwd from `cwd`; `AcpConfig` cwd from `session_cwd`(→`cwd`→`"."`).
- **Modify** `crates/bridge-a2a-inbound/src/server.rs` — parse + validate `a2a-bridge.cwd` → `AgentOverride.cwd`; thread into single-agent dispatch + detached workflow (persist + pass to `run_from`); resume restores it.
- **Modify** `crates/bridge-core/src/domain.rs`/`AgentOverride` — `cwd: Option<String>`.
- **Modify** `crates/bridge-workflow/src/executor.rs` — `run_from`/`run_node` forward `session_cwd: Option<String>` to per-node `configure_session`.
- **Modify** `crates/bridge-core/src/task_store.rs` + `crates/bridge-store/src/sqlite.rs` — `TaskRecord.session_cwd` + additive column + migration.
- **Create** `docs/adr/0014-session-cwd.md`.

---

## Phase A — Decouple `session_cwd` (config, behavior-preserving core)

### Task 1: `session_cwd` config field + `AcpConfig` cwd sourced from it

**Files:** Modify `bin/a2a-bridge/src/config.rs`, `bin/a2a-bridge/src/main.rs`.

- [ ] **Step 1: Failing config test.** Append to the config tests in `config.rs`:
```rust
    #[test]
    fn agent_session_cwd_parses_and_is_optional() {
        let cfg: RegistryConfig = RegistryConfig::parse(
            "default=\"a\"\n[[agents]]\nid=\"a\"\ncmd=\"x\"\ncwd=\"/host\"\nsession_cwd=\"/work\"\n[server]\naddr=\"127.0.0.1:8080\"\n",
        ).unwrap();
        let a = cfg.agents.iter().find(|a| a.id == "a").unwrap();
        assert_eq!(a.cwd.as_deref(), Some("/host"));
        assert_eq!(a.session_cwd.as_deref(), Some("/work"));
        let cfg2: RegistryConfig = RegistryConfig::parse(
            "default=\"a\"\n[[agents]]\nid=\"a\"\ncmd=\"x\"\n[server]\naddr=\"127.0.0.1:8080\"\n",
        ).unwrap();
        assert_eq!(cfg2.agents.iter().find(|a| a.id=="a").unwrap().session_cwd, None);
    }
```
   (Match the real config type name `RegistryConfig`/`parse` — confirm from the existing `store_resume_attempt_cap_parses_and_defaults` test added in W3b.)
- [ ] **Step 2: Run → fails.** `cargo test -p a2a-bridge agent_session_cwd_parses_and_is_optional`
- [ ] **Step 3: Implement.** In `config.rs`, add to `AgentEntryToml` next to `cwd`:
```rust
    #[serde(default)]
    pub session_cwd: Option<String>,
```
   Map it into the registry `AgentEntry` where `cwd` is mapped (~314-320): add `session_cwd: a.session_cwd`. Add the field to the registry's `AgentEntry` struct (mirror `cwd`). In `main.rs`'s spawn fn (both the warm-spawn at ~153 and the second site at ~430 if it builds an `AcpConfig`), set the **`AcpConfig` cwd** from `entry.session_cwd` falling back to `entry.cwd` then `"."`, while the **host child** `current_dir` stays `entry.cwd`:
```rust
    let session_cwd = entry.session_cwd.clone().or_else(|| entry.cwd.clone()).unwrap_or_else(|| ".".into());
    // host child working dir: entry.cwd (unchanged); AcpConfig.cwd: session_cwd
```
   (Find where `AcpConfig { cwd, .. }` is constructed in the spawn closure and set `cwd: PathBuf::from(session_cwd)`; leave the `Supervised::spawn(..., cwd: entry.cwd ...)` host cwd as-is.)
- [ ] **Step 4: Run → green.** `cargo test -p a2a-bridge`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo check --workspace`.
- [ ] **Step 5: Commit** (NO trailer): `git commit -m "feat(config,bin): session_cwd agent field — AcpConfig session cwd distinct from host spawn cwd"`

---

## Phase B — `EffectiveConfig.cwd` + per-request `a2a-bridge.cwd` parse & validation

### Task 2: `EffectiveConfig`/`AgentOverride` gain `cwd`; `ensure_session` reads it at mint

**Files:** Modify `crates/bridge-core/src/domain.rs`, `crates/bridge-acp/src/acp_backend.rs`.

- [ ] **Step 1: Failing test (acp).** In `acp_backend.rs` tests, drive `configure_session` with an `EffectiveConfig` carrying `cwd: Some("/req")` then assert the next `ensure_session` mint issues `session/new` with cwd `/req` (the existing acp tests have a fake ACP peer recording `NewSessionRequest`; mirror the `new_session_calls`/recorded-request pattern at acp_backend.rs:1615+). Assert: stash cwd present → mint cwd == `/req`; no stash → mint cwd == static `AcpConfig.cwd`.
- [ ] **Step 2: Run → fails** (EffectiveConfig has no cwd; ensure_session ignores it).
- [ ] **Step 3: Implement.**
  - `domain.rs`: add `pub cwd: Option<String>` to `EffectiveConfig` AND to `AgentOverride` (update their constructors/`effective_config` layering so an override `cwd` wins over the entry default; if neither sets it, `None`). Update every `EffectiveConfig{..}`/`AgentOverride{..}` literal (grep both) to include `cwd` (`None` where not relevant).
  - `acp_backend.rs` `ensure_session`: derive `cwd` from the stash first, else the static config:
```rust
        let stashed = self.session_cfg.lock().ok().and_then(|m| m.get(key).cloned());
        let cwd = stashed.as_ref().and_then(|c| c.cwd.clone())
            .or_else(|| self.config.as_ref().map(|c| c.cwd.to_string_lossy().into_owned()))
            .ok_or(BridgeError::AgentCrashed)?;
        // (mode,model,effort) block is UNCHANGED from today (acp_backend.rs:870-878):
        //   Some(cfg) => (cfg.mode, cfg.model, cfg.effort),
        //   None => (self.config…mode.clone(), self.config…model.clone(), None)
        // Only `cwd` derivation (above) is new. Then: new_session_request(PathBuf::from(cwd)) as today.
```
   (Adapt to the real `session_cfg`/`SessionCfg` stash type — it currently stores mode/model/effort; add `cwd` to it. Keep the existing `(mode,model,effort)` fallback logic unchanged.)
- [ ] **Step 4: Run → green.** `cargo test -p bridge-acp -p bridge-core`, clippy, `cargo check --workspace`.
- [ ] **Step 5: Commit** `git commit -m "feat(core,acp): EffectiveConfig.cwd rides per-session stash; session/new mint honors per-request cwd"`

### Task 3: parse + validate `a2a-bridge.cwd` from metadata (+ `allowed_cwd_root`)

**Files:** Modify `bin/a2a-bridge/src/config.rs` (`allowed_cwd_root`), `crates/bridge-a2a-inbound/src/server.rs` (parse + validate).

- [ ] **Step 1: Failing tests (inbound).** In `server.rs` tests: (a) a `message/send` whose `message.metadata` has `a2a-bridge.cwd="/abs/repo"` → the parsed `AgentOverride.cwd == Some("/abs/repo")`; (b) a relative cwd `"rel"` → `unary_message` returns `InvalidRequest`; (c) with `allowed_cwd_root="/abs"` set, `cwd="/abs/repo"` accepted, `cwd="/other"` → `InvalidRequest`.
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement.**
  - `config.rs`: add a global (top-level) `pub allowed_cwd_root: Option<String>` (with `#[serde(default)]`), threaded to the inbound server (e.g. on `InboundServer` via a builder, mirroring how other config reaches it).
  - `server.rs` metadata parser (the `a2a-bridge.{model,effort,mode}` site ~2277-2330): read `a2a-bridge.cwd` into `AgentOverride.cwd`. Then validate in `unary_message`/the gate BEFORE minting the task/dispatch: `cwd` must be absolute (`std::path::Path::new(cwd).is_absolute()`); if `allowed_cwd_root` is `Some(root)`, the cwd must canonicalize/normalize to a path under `root` (use a lexical `starts_with` on normalized absolute paths — do NOT require the path to exist, since a containerized cwd may not exist on the host); on violation return `bridge_err_to_jsonrpc(id, &BridgeError::InvalidRequest{ field: "a2a-bridge.cwd".into() })` (match the real `InvalidRequest` shape).
- [ ] **Step 4: Run → green.** `cargo test -p bridge-a2a-inbound -p a2a-bridge`, clippy, check.
- [ ] **Step 5: Commit** `git commit -m "feat(inbound,config): parse + validate a2a-bridge.cwd (absolute; allowed_cwd_root guard)"`

---

## Phase C — Single-agent per-request cwd (end-to-end)

### Task 4: Local-route dispatch applies the per-request cwd

**Files:** Modify `crates/bridge-a2a-inbound/src/server.rs`.

- [ ] **Step 1: Failing test.** With a fake backend that records the `EffectiveConfig` passed to `configure_session`, send a sync `message/send` (Local route) with `a2a-bridge.cwd="/req"` and assert the backend's `configure_session` received `cfg.cwd == Some("/req")` (so the mint will use it). (Reuse the inbound fake-backend harness used by existing model/mode override tests.)
- [ ] **Step 2: Run → fails** (overrides carry cwd but it isn't layered into the dispatched `EffectiveConfig`).
- [ ] **Step 3: Implement.** Where `effective_config(entry_defaults, overrides)` builds the `EffectiveConfig` for `configure_session` on the Local dispatch path, layer `overrides.cwd` in (request cwd → else entry `session_cwd`/`cwd` default → else `None`, leaving the static `AcpConfig` fallback to handle `None`). No new call sites — the cwd now flows through the same `configure_session` the model/mode overrides already use.
- [ ] **Step 4: Run → green.** `cargo test -p bridge-a2a-inbound`, clippy, check.
- [ ] **Step 5: Commit** `git commit -m "feat(inbound): single-agent dispatch layers per-request cwd into EffectiveConfig"`

---

## Phase D — Workflow per-request cwd (forwarded, executor stays pure)

### Task 5: `run_from`/`run_node` forward `session_cwd` to per-node dispatch

**Files:** Modify `crates/bridge-workflow/src/executor.rs`, `crates/bridge-a2a-inbound/src/server.rs` (detached runner call site).

- [ ] **Step 1: Failing test (workflow).** In the executor tests, a multi-node graph with a fake registry recording the `EffectiveConfig` each node's `configure_session` got; call `run_from(graph, input, run_id, cancel, seed, Some("/req".into()))` and assert every node's dispatched `cfg.cwd == Some("/req")`. Add a second assertion that `session_cwd=None` leaves `cfg.cwd == None` (static fallback).
- [ ] **Step 2: Run → fails** (run_from has no session_cwd param).
- [ ] **Step 3: Implement.** Add a trailing `session_cwd: Option<String>` param to `run_from` (and `run()` passes `None`). Forward it into `run_node` (alongside `run_id`/`cancel`) and into the per-node `configure_session` `EffectiveConfig` (set `cwd: session_cwd.clone()` when the node builds its effective config for dispatch). The graph/seed/scheduling logic is untouched — `session_cwd` is an opaque forwarded value (executor purity preserved). Update the detached runner: `spawn_detached_workflow` gains a `session_cwd: Option<String>` param and passes it into `run_from`; the unary detached arm passes the request's cwd; the test seams pass `None`.
- [ ] **Step 4: Run → green.** `cargo test -p bridge-workflow -p bridge-a2a-inbound`, clippy, check.
- [ ] **Step 5: Commit** `git commit -m "feat(workflow,inbound): forward per-request session_cwd to every workflow node's session mint"`

---

## Phase E — Persist + resume (W3b interaction)

### Task 6: `TaskRecord.session_cwd` + additive column + persist at submit

**Files:** Modify `crates/bridge-core/src/task_store.rs`, `crates/bridge-store/src/sqlite.rs`, `crates/bridge-a2a-inbound/src/server.rs`.

- [ ] **Step 1: Failing tests.** (a) `task_store.rs`: a `MemoryTaskStore` round-trip asserting `TaskRecord.session_cwd` persists/reads. (b) `sqlite.rs`: `w3b_schema`-style test asserting `session_cwd` round-trips + a migration test (old DB, reopen) leaves `session_cwd` NULL/`None`. (c) `server.rs`: the detached submit with `a2a-bridge.cwd="/req"` persists `TaskRecord.session_cwd == Some("/req")`.
- [ ] **Step 2: Run → fails.**
- [ ] **Step 3: Implement.**
  - `task_store.rs`: add `pub session_cwd: Option<String>` to `TaskRecord`; update `MemoryTaskStore` (whole-record store already persists it). **Grep every `TaskRecord {` literal and add `session_cwd: None`** (or the real value): server.rs detached arm, workflow_producer.rs tests, sqlite.rs tests, task_store.rs tests — the W3b additive-field ripple.
  - `sqlite.rs`: add `session_cwd TEXT` to `migrate_tasks_columns` (the pragma table_info-guarded ALTER set); `create` INSERTs it; `row_to_task` SELECTs/maps it; `working_tasks` returns it (via `row_to_task`).
  - `server.rs` detached arm: set `session_cwd: <the request cwd>` in the persisted `TaskRecord` (the cwd already validated in Task 3).
- [ ] **Step 4: Run → green.** `cargo test -p bridge-core -p bridge-store -p bridge-a2a-inbound`, clippy, `cargo check --workspace`.
- [ ] **Step 5: Commit** `git commit -m "feat(core,store,inbound): persist TaskRecord.session_cwd (additive column + migration) at detached submit"`

### Task 7: resume restores `session_cwd`

**Files:** Modify `crates/bridge-a2a-inbound/src/server.rs` (`resume_working_tasks`).

- [ ] **Step 1: Failing test.** Seed a `Working` task with `session_cwd="/req"` + a snapshot + a `codex`-only checkpoint; call `resume_working_tasks(&srv, 3)`; assert the resumed runner's nodes dispatched with `cfg.cwd == Some("/req")` (the recording fake registry). Poll to terminal (the W3b determinism pattern).
- [ ] **Step 2: Run → fails** (resume passes no cwd).
- [ ] **Step 3: Implement.** In `resume_working_tasks`, read `wt.session_cwd` and pass it as the `session_cwd` arg to `spawn_detached_workflow(... session_cwd)` → `run_from(... session_cwd)`, so resumed nodes re-run in the persisted directory.
- [ ] **Step 4: Run → green.** `cargo test -p bridge-a2a-inbound`, clippy, check.
- [ ] **Step 5: Commit** `git commit -m "feat(inbound): boot resume restores per-task session_cwd into run_from"`

---

## Phase F — Verification, live gate, ADR

### Task 8: full sweep + coverage
- [ ] `cargo fmt --all && cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` → all green.
- [ ] `cargo llvm-cov clean --workspace` then `--workspace --fail-under-lines 85`, `--package bridge-core --fail-under-lines 90`, `--package bridge-workflow --fail-under-lines 90`. Top up tests if any floor dips; commit additions.

### Task 9: gated live (real agent, distinct host vs session cwd — DoD-6)
- [ ] Rebuild the `a2a-claude-agent` image (or reuse). Create `/tmp/scwd-serve/a2a-bridge.toml`: a containerized claude agent with **`cwd` (host: e.g. `/tmp/scwd-serve`) ≠ `session_cwd` (in-container: `/work`)**, `-v <repo>:/work -w /work` (NO identical-path hack), `[store] path`. Submit a `dev-task`; verify the container edits the repo + task `Completed` — proving session_cwd works without identical paths. Then a second run with a **broad parent mount** (`-v ~/code:~/code`) + per-request `a2a-bridge.cwd=~/code/<subdir>` selecting a subdir; verify the agent operated in that subdir. Record in the ADR.

### Task 10: ADR-0014
- [ ] Write `docs/adr/0014-session-cwd.md`: the decision (decouple session cwd from spawn cwd; per-request cwd via `EffectiveConfig` riding the existing per-session stash; `allowed_cwd_root` guard; persist+resume), the dual-review provenance, the live-gate result (distinct cwd/session_cwd; per-request subdir). Commit with the controller trailer.

---

## DoD (spec §Definition of Done) → tasks
| DoD | Task |
|-----|------|
| 1 (session_cwd config; session/new uses it; back-compat) | 1, 2 |
| 2 (a2a-bridge.cwd extracted + validated; absolute; allowed_cwd_root) | 3 |
| 3 (single-agent dispatch honors per-request cwd) | 4 |
| 4 (workflow dispatch applies cwd to all nodes; executor pure) | 5 |
| 5 (tasks.session_cwd persisted + restored on resume; migration) | 6, 7 |
| 6 (identical-path hack no longer required — distinct cwd/session_cwd) | 9 (live) |
| 7 (fmt/clippy/coverage; ADR) | 8, 10 |

## Notes for the implementer
- **The whole increment hangs on one insight:** `cwd` rides the existing `EffectiveConfig` → `configure_session` (stash) → `ensure_session` (mint) path that model/mode already use. Phases B–D are "add `cwd` to `EffectiveConfig`/`AgentOverride`, read it at mint, layer it in at each dispatch." No new dispatch seam.
- **`ensure_session` (acp_backend.rs:851) is the single mint point** — the `(mode,model,effort)` stash-or-static block is the template; add `cwd` to it identically.
- **Additive `TaskRecord.session_cwd` ripples** to every literal (Task 6) — the W3b lesson; the build surfaces them, grep `TaskRecord {`.
- **Executor purity:** `session_cwd` is a forwarded opaque param (like `run_id`), never read by graph/scheduling logic.
- **`allowed_cwd_root` uses lexical normalization, not `canonicalize()`** — a containerized session cwd may not exist on the host, so requiring existence would wrongly reject valid containerized targets.
- **`run cargo check --workspace` after every task.** Firewall: design from bridge ports + ACP §11A + A2A metadata; a2a-local-bridge did not inform it. Controller docs (this plan, ADR-0014) carry the `Co-Authored-By: Claude Opus 4.8 (1M context)` trailer; task commits do NOT. Coverage after `cargo llvm-cov clean --workspace`.
